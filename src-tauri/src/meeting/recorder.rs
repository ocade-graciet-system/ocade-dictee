//! Enregistreur de réunion. Un fil dédié possède le flux du micro (cpal) et
//! le fichier WAV : chaque bloc capté est rééchantillonné à 16 kHz et écrit
//! aussitôt sur le disque, si bien que la mémoire reste constante quelle que
//! soit la durée. L'en-tête du WAV est mis à jour chaque seconde : un arrêt
//! brutal ne perd au plus que la dernière seconde, que `MeetingStore`
//! récupère au démarrage suivant.
//!
//! Le flux est indépendant de celui de la dictée : dicter pendant une réunion
//! reste possible. La pause ferme le micro (son voyant s'éteint) ; la reprise
//! le rouvre et continue le même fichier.

use std::fs::File;
use std::io::BufWriter;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{Sample, SizedSample};
use tauri::{AppHandle, Manager};
use tauri_specta::Event;

use super::keep_awake::KeepAwake;
use super::store::MeetingStore;
use super::wav;
use super::{
    now_unix, FinishReason, MeetingError, MeetingRecorderStatus, MeetingRecording,
    MeetingRecordingFinished, RecorderPhase, RecordingState,
};
use crate::audio_toolkit::audio::FrameResampler;
use crate::audio_toolkit::{
    get_cpal_host, is_microphone_access_denied, is_no_input_device_error, AudioRecorder,
};
use crate::managers::audio::AudioRecordingManager;

/// Fréquence des mises à jour du chronomètre et du niveau.
const STATUS_INTERVAL: Duration = Duration::from_millis(250);
/// Mise à jour de l'en-tête WAV : au plus une seconde perdue sur un plantage.
const FLUSH_INTERVAL: Duration = Duration::from_secs(1);
/// Avancement mémorisé en base.
const PERSIST_INTERVAL: Duration = Duration::from_secs(15);
const DISK_CHECK_INTERVAL: Duration = Duration::from_secs(30);
/// Sans audio pendant ce délai, le micro est considéré comme perdu (certains
/// pilotes cessent d'envoyer des données sans signaler d'erreur).
const NO_DATA_TIMEOUT: Duration = Duration::from_secs(4);
const RECONNECT_INTERVAL: Duration = Duration::from_secs(2);
/// Espace libre exigé pour démarrer : ~4 h d'audio.
pub const MIN_FREE_BYTES_TO_START: u64 = 500 * 1024 * 1024;
/// En dessous, l'enregistrement s'arrête proprement plutôt que d'échouer en
/// pleine écriture.
pub const MIN_FREE_BYTES_WHILE_RECORDING: u64 = 100 * 1024 * 1024;

enum SessionCmd {
    Pause(mpsc::Sender<Result<(), MeetingError>>),
    Resume(mpsc::Sender<Result<(), MeetingError>>),
    Stop(
        FinishReason,
        mpsc::Sender<Result<MeetingRecording, MeetingError>>,
    ),
}

struct ActiveSession {
    cmd_tx: mpsc::Sender<SessionCmd>,
    worker: std::thread::JoinHandle<()>,
}

pub struct MeetingRecorder {
    app: AppHandle,
    store: Arc<MeetingStore>,
    active: Mutex<Option<ActiveSession>>,
    status: Arc<Mutex<MeetingRecorderStatus>>,
}

/// Nom donné à un nouvel enregistrement : « Réunion du 24/10/2026 à 14h05 ».
fn default_name() -> String {
    chrono::Local::now()
        .format("Réunion du %d/%m/%Y à %Hh%M")
        .to_string()
}

fn classify_open_error(message: String) -> MeetingError {
    if is_microphone_access_denied(&message) {
        MeetingError::MicrophonePermission
    } else if is_no_input_device_error(&message) {
        MeetingError::NoMicrophone
    } else {
        MeetingError::MicrophoneFailed { detail: message }
    }
}

impl MeetingRecorder {
    pub fn new(app: &AppHandle, store: Arc<MeetingStore>) -> Self {
        Self {
            app: app.clone(),
            store,
            active: Mutex::new(None),
            status: Arc::new(Mutex::new(MeetingRecorderStatus::idle())),
        }
    }

    pub fn store(&self) -> &Arc<MeetingStore> {
        &self.store
    }

    pub fn status(&self) -> MeetingRecorderStatus {
        self.status.lock().unwrap().clone()
    }

    /// Un enregistrement est en cours ou en pause : la mise à jour automatique
    /// attend (elle redémarrerait l'application).
    pub fn is_active(&self) -> bool {
        self.status().phase != RecorderPhase::Idle
    }

    /// Oublie une session terminée d'elle-même (disque plein, écriture
    /// impossible) : son fil a déjà tout finalisé.
    fn reap(active: &mut Option<ActiveSession>) {
        if active
            .as_ref()
            .is_some_and(|session| session.worker.is_finished())
        {
            if let Some(session) = active.take() {
                let _ = session.worker.join();
            }
        }
    }

    pub fn start(&self) -> Result<MeetingRecorderStatus, MeetingError> {
        let mut active = self.active.lock().unwrap();
        Self::reap(&mut active);
        if active.is_some() {
            return Err(MeetingError::AlreadyRecording);
        }
        if let Some(free_bytes) = crate::summary::system::free_disk_bytes(self.store.audio_dir()) {
            if free_bytes < MIN_FREE_BYTES_TO_START {
                return Err(MeetingError::DiskSpace { free_bytes });
            }
        }

        let id = self.store.insert_recording(&default_name(), now_unix())?;
        let (cmd_tx, cmd_rx) = mpsc::channel();
        let (init_tx, init_rx) = mpsc::sync_channel(1);
        let context = SessionContext {
            app: self.app.clone(),
            store: Arc::clone(&self.store),
            status: Arc::clone(&self.status),
            id,
        };
        let worker = std::thread::Builder::new()
            .name("meeting-recorder".into())
            .spawn(move || run_session(context, cmd_rx, init_tx))
            .map_err(MeetingError::storage)?;

        let init = init_rx
            .recv()
            .unwrap_or_else(|e| Err(MeetingError::storage(e)));
        match init {
            Ok(()) => {
                *active = Some(ActiveSession { cmd_tx, worker });
                log::info!("Réunion {id} : enregistrement démarré");
                Ok(self.status())
            }
            Err(error) => {
                let _ = worker.join();
                if let Err(e) = self.store.delete(id) {
                    log::warn!("Réunion {id} : nettoyage après échec impossible ({e})");
                }
                log::warn!("Réunion : démarrage impossible ({error})");
                Err(error)
            }
        }
    }

    fn send<T>(
        &self,
        make: impl FnOnce(mpsc::Sender<Result<T, MeetingError>>) -> SessionCmd,
    ) -> Result<T, MeetingError> {
        let mut active = self.active.lock().unwrap();
        Self::reap(&mut active);
        let session = active.as_ref().ok_or(MeetingError::NotRecording)?;
        let (reply_tx, reply_rx) = mpsc::channel();
        session
            .cmd_tx
            .send(make(reply_tx))
            .map_err(|_| MeetingError::NotRecording)?;
        reply_rx.recv().map_err(|_| MeetingError::NotRecording)?
    }

    pub fn pause(&self) -> Result<MeetingRecorderStatus, MeetingError> {
        self.send(SessionCmd::Pause)?;
        Ok(self.status())
    }

    pub fn resume(&self) -> Result<MeetingRecorderStatus, MeetingError> {
        self.send(SessionCmd::Resume)?;
        Ok(self.status())
    }

    pub fn stop(&self) -> Result<MeetingRecording, MeetingError> {
        self.stop_with(FinishReason::User)
    }

    fn stop_with(&self, reason: FinishReason) -> Result<MeetingRecording, MeetingError> {
        let recording = self.send(|reply| SessionCmd::Stop(reason, reply))?;
        let mut active = self.active.lock().unwrap();
        if let Some(session) = active.take() {
            let _ = session.worker.join();
        }
        Ok(recording)
    }

    /// Fermeture de l'application : l'enregistrement en cours est finalisé
    /// (fichier complet, liste à jour) avant la fin du processus.
    pub fn shutdown(&self) {
        if self.is_active() {
            match self.stop_with(FinishReason::AppExit) {
                Ok(recording) => log::info!(
                    "Réunion {} finalisée à la fermeture ({} ms)",
                    recording.id,
                    recording.duration_ms
                ),
                Err(e) => log::warn!("Réunion : finalisation à la fermeture impossible ({e})"),
            }
        }
    }
}

struct SessionContext {
    app: AppHandle,
    store: Arc<MeetingStore>,
    status: Arc<Mutex<MeetingRecorderStatus>>,
    id: i64,
}

/// Fichier(s) WAV de l'enregistrement en cours.
struct PartWriter {
    dir: PathBuf,
    id: i64,
    part: i64,
    writer: Option<hound::WavWriter<BufWriter<File>>>,
    part_bytes: u64,
    data_bytes: u64,
    /// Taille sur le disque des fichiers déjà terminés (en-têtes compris).
    closed_parts_bytes: u64,
    dirty: bool,
}

impl PartWriter {
    fn create(dir: PathBuf, id: i64) -> Result<Self, MeetingError> {
        let writer = hound::WavWriter::create(wav::part_path(&dir, id, 1), wav::spec())
            .map_err(MeetingError::storage)?;
        Ok(Self {
            dir,
            id,
            part: 1,
            writer: Some(writer),
            part_bytes: 0,
            data_bytes: 0,
            closed_parts_bytes: 0,
            dirty: false,
        })
    }

    /// Passe au fichier suivant avant la limite de 4 Go du format WAV.
    fn roll(&mut self) -> Result<(), MeetingError> {
        if let Some(writer) = self.writer.take() {
            writer.finalize().map_err(MeetingError::storage)?;
        }
        self.closed_parts_bytes += self.part_bytes + 44;
        self.part += 1;
        self.part_bytes = 0;
        let path = wav::part_path(&self.dir, self.id, self.part);
        self.writer =
            Some(hound::WavWriter::create(path, wav::spec()).map_err(MeetingError::storage)?);
        log::info!("Réunion {} : fichier {} commencé", self.id, self.part);
        Ok(())
    }

    fn write(&mut self, frame: &[f32]) -> Result<bool, MeetingError> {
        let needed = frame.len() as u64 * 2;
        let mut rolled = false;
        if self.part_bytes + needed > wav::MAX_PART_BYTES {
            self.roll()?;
            rolled = true;
        }
        let writer = self.writer.as_mut().ok_or(MeetingError::NotRecording)?;
        for &sample in frame {
            let value = (sample.clamp(-1.0, 1.0) * i16::MAX as f32) as i16;
            writer.write_sample(value).map_err(MeetingError::storage)?;
        }
        self.part_bytes += needed;
        self.data_bytes += needed;
        self.dirty = true;
        Ok(rolled)
    }

    fn flush(&mut self) -> Result<(), MeetingError> {
        if self.dirty {
            if let Some(writer) = self.writer.as_mut() {
                writer.flush().map_err(MeetingError::storage)?;
            }
            self.dirty = false;
        }
        Ok(())
    }

    fn finalize(&mut self) -> Result<(), MeetingError> {
        if let Some(writer) = self.writer.take() {
            writer.finalize().map_err(MeetingError::storage)?;
        }
        Ok(())
    }

    fn duration_ms(&self) -> i64 {
        wav::duration_ms_for(self.data_bytes)
    }

    fn size_bytes(&self) -> i64 {
        (self.closed_parts_bytes + self.part_bytes + 44) as i64
    }
}

/// Flux du micro ouvert : les blocs arrivent en mono, à la fréquence du
/// périphérique, et sont ramenés à 16 kHz par `resampler`.
struct Capture {
    stream: Option<cpal::Stream>,
    rx: mpsc::Receiver<Vec<f32>>,
    failed: Arc<AtomicBool>,
    resampler: FrameResampler,
    device_name: String,
}

impl Capture {
    /// Ouvre le micro des réglages ; s'il ne répond pas (débranché), le micro
    /// par défaut du système. Mieux vaut enregistrer avec le micro intégré que
    /// perdre la réunion.
    fn open(app: &AppHandle) -> Result<Self, MeetingError> {
        let manager = app.try_state::<Arc<AudioRecordingManager>>();
        let selected = manager
            .as_ref()
            .and_then(|m| m.effective_microphone_device());
        let had_selection = selected.is_some();
        match Self::open_device(selected) {
            Ok(capture) => Ok(capture),
            Err(first) if had_selection => {
                if let Some(m) = &manager {
                    m.invalidate_device_cache();
                }
                let fresh = manager
                    .as_ref()
                    .and_then(|m| m.effective_microphone_device());
                Self::open_device(fresh)
                    .or_else(|_| Self::open_device(None))
                    .map_err(|_| first)
            }
            Err(error) => Err(error),
        }
    }

    fn open_device(device: Option<cpal::Device>) -> Result<Self, MeetingError> {
        let device = match device {
            Some(device) => device,
            None => get_cpal_host()
                .default_input_device()
                .ok_or(MeetingError::NoMicrophone)?,
        };
        let device_name = device.name().unwrap_or_default();
        let config = AudioRecorder::get_preferred_config(&device)
            .map_err(|e| classify_open_error(format!("Failed to fetch preferred config: {e}")))?;
        let sample_rate = config.sample_rate().0;
        let channels = config.channels() as usize;
        let (tx, rx) = mpsc::channel();
        let failed = Arc::new(AtomicBool::new(false));

        let stream = match config.sample_format() {
            cpal::SampleFormat::U8 => build_stream::<u8>(&device, &config, tx, channels, &failed),
            cpal::SampleFormat::I8 => build_stream::<i8>(&device, &config, tx, channels, &failed),
            cpal::SampleFormat::I16 => build_stream::<i16>(&device, &config, tx, channels, &failed),
            cpal::SampleFormat::I32 => build_stream::<i32>(&device, &config, tx, channels, &failed),
            cpal::SampleFormat::F32 => build_stream::<f32>(&device, &config, tx, channels, &failed),
            format => {
                return Err(MeetingError::MicrophoneFailed {
                    detail: format!("format non pris en charge: {format:?}"),
                })
            }
        }
        .map_err(|e| classify_open_error(format!("Failed to build input stream: {e}")))?;
        stream
            .play()
            .map_err(|e| classify_open_error(format!("Failed to start microphone stream: {e}")))?;

        log::info!(
            "Réunion : micro « {device_name} » ouvert ({sample_rate} Hz, {channels} canal/canaux)"
        );
        Ok(Self {
            stream: Some(stream),
            rx,
            failed,
            resampler: FrameResampler::new(
                sample_rate as usize,
                wav::SAMPLE_RATE as usize,
                Duration::from_millis(30),
            ),
            device_name,
        })
    }
}

fn build_stream<T>(
    device: &cpal::Device,
    config: &cpal::SupportedStreamConfig,
    tx: mpsc::Sender<Vec<f32>>,
    channels: usize,
    failed: &Arc<AtomicBool>,
) -> Result<cpal::Stream, cpal::BuildStreamError>
where
    T: Sample + SizedSample + Send + 'static,
    f32: cpal::FromSample<T>,
{
    let failed = Arc::clone(failed);
    device.build_input_stream(
        &config.clone().into(),
        move |data: &[T], _: &cpal::InputCallbackInfo| {
            let mono: Vec<f32> = if channels <= 1 {
                data.iter().map(|&s| s.to_sample::<f32>()).collect()
            } else {
                data.chunks_exact(channels)
                    .map(|frame| {
                        frame.iter().map(|&s| s.to_sample::<f32>()).sum::<f32>() / channels as f32
                    })
                    .collect()
            };
            let _ = tx.send(mono);
        },
        move |err| {
            log::warn!("Réunion : erreur du flux micro ({err})");
            failed.store(true, Ordering::Relaxed);
        },
        None,
    )
}

/// Niveau affiché, de 0 à 1 : RMS du bloc sur une échelle de -60 à 0 dBFS.
fn level_of(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    let rms = (samples.iter().map(|s| s * s).sum::<f32>() / samples.len() as f32).sqrt();
    ((20.0 * rms.max(1e-9).log10() + 60.0) / 60.0).clamp(0.0, 1.0)
}

struct Session {
    context: SessionContext,
    writer: PartWriter,
    capture: Option<Capture>,
    phase: RecorderPhase,
    level: f32,
    last_data: Instant,
    last_reconnect: Instant,
    last_status: Instant,
    last_flush: Instant,
    last_persist: Instant,
    last_disk_check: Instant,
    /// Fichier courant déjà mémorisé en base : un passage au fichier suivant
    /// l'est aussitôt, pour qu'un plantage ne l'oublie pas.
    persisted_part: i64,
    microphone: Option<String>,
}

enum Flow {
    Continue,
    Exit,
}

fn run_session(
    context: SessionContext,
    cmd_rx: mpsc::Receiver<SessionCmd>,
    init_tx: mpsc::SyncSender<Result<(), MeetingError>>,
) {
    let _awake = KeepAwake::acquire();
    let writer = match PartWriter::create(context.store.audio_dir().to_path_buf(), context.id) {
        Ok(writer) => writer,
        Err(error) => {
            let _ = init_tx.send(Err(error));
            return;
        }
    };
    let capture = match Capture::open(&context.app) {
        Ok(capture) => capture,
        Err(error) => {
            drop(writer);
            let _ = init_tx.send(Err(error));
            return;
        }
    };

    let now = Instant::now();
    let mut session = Session {
        microphone: Some(capture.device_name.clone()),
        context,
        writer,
        capture: Some(capture),
        phase: RecorderPhase::Recording,
        level: 0.0,
        last_data: now,
        last_reconnect: now,
        last_status: now,
        last_flush: now,
        last_persist: now,
        last_disk_check: now,
        persisted_part: 1,
    };
    session.publish_status();
    let _ = init_tx.send(Ok(()));
    session.run(cmd_rx);
}

impl Session {
    fn run(&mut self, cmd_rx: mpsc::Receiver<SessionCmd>) {
        loop {
            loop {
                match cmd_rx.try_recv() {
                    Ok(cmd) => {
                        if let Flow::Exit = self.handle(cmd) {
                            return;
                        }
                    }
                    Err(mpsc::TryRecvError::Empty) => break,
                    // L'application se ferme sans avoir arrêté l'enregistrement.
                    Err(mpsc::TryRecvError::Disconnected) => {
                        let _ = self.finish(FinishReason::AppExit);
                        return;
                    }
                }
            }

            if self.phase == RecorderPhase::Recording {
                if let Flow::Exit = self.capture_step() {
                    return;
                }
            } else {
                match cmd_rx.recv_timeout(Duration::from_millis(200)) {
                    Ok(cmd) => {
                        if let Flow::Exit = self.handle(cmd) {
                            return;
                        }
                    }
                    Err(mpsc::RecvTimeoutError::Timeout) => {}
                    Err(mpsc::RecvTimeoutError::Disconnected) => {
                        let _ = self.finish(FinishReason::AppExit);
                        return;
                    }
                }
                if self.phase == RecorderPhase::Reconnecting
                    && self.last_reconnect.elapsed() >= RECONNECT_INTERVAL
                {
                    self.try_reconnect();
                }
            }

            if let Flow::Exit = self.periodic() {
                return;
            }
        }
    }

    fn handle(&mut self, cmd: SessionCmd) -> Flow {
        match cmd {
            SessionCmd::Pause(reply) => {
                if self.phase != RecorderPhase::Paused {
                    let result = self.close_capture().and_then(|()| self.writer.flush());
                    self.phase = RecorderPhase::Paused;
                    self.level = 0.0;
                    self.publish_status();
                    log::info!("Réunion {} : pause", self.context.id);
                    let _ = reply.send(result);
                } else {
                    let _ = reply.send(Ok(()));
                }
                Flow::Continue
            }
            SessionCmd::Resume(reply) => {
                if self.phase == RecorderPhase::Recording {
                    let _ = reply.send(Ok(()));
                    return Flow::Continue;
                }
                match Capture::open(&self.context.app) {
                    Ok(capture) => {
                        self.microphone = Some(capture.device_name.clone());
                        self.capture = Some(capture);
                        self.phase = RecorderPhase::Recording;
                        self.last_data = Instant::now();
                        self.publish_status();
                        log::info!("Réunion {} : reprise", self.context.id);
                        let _ = reply.send(Ok(()));
                    }
                    Err(error) => {
                        let _ = reply.send(Err(error));
                    }
                }
                Flow::Continue
            }
            SessionCmd::Stop(reason, reply) => {
                let _ = reply.send(self.finish(reason));
                Flow::Exit
            }
        }
    }

    /// Écrit tout l'audio disponible ; signale un micro perdu.
    fn capture_step(&mut self) -> Flow {
        let Some(capture) = self.capture.as_mut() else {
            self.lose_microphone();
            return Flow::Continue;
        };
        let received = capture.rx.recv_timeout(Duration::from_millis(100));
        let disconnected = matches!(received, Err(mpsc::RecvTimeoutError::Disconnected));
        if let Ok(first) = received {
            self.last_data = Instant::now();
            let mut chunks = vec![first];
            while let Ok(chunk) = capture.rx.try_recv() {
                chunks.push(chunk);
            }
            for chunk in &chunks {
                let level = level_of(chunk);
                self.level = level.max(self.level * 0.85);
                if let Err(error) = write_through(&mut capture.resampler, &mut self.writer, chunk) {
                    log::error!(
                        "Réunion {} : écriture impossible ({error})",
                        self.context.id
                    );
                    let _ = self.finish(FinishReason::WriteFailed);
                    return Flow::Exit;
                }
            }
        }
        let failed = self
            .capture
            .as_ref()
            .is_some_and(|c| c.failed.load(Ordering::Relaxed));
        if disconnected || failed || self.last_data.elapsed() > NO_DATA_TIMEOUT {
            self.lose_microphone();
        }
        Flow::Continue
    }

    /// Ferme le micro après avoir écrit tout ce qu'il avait déjà capté.
    fn close_capture(&mut self) -> Result<(), MeetingError> {
        let Some(mut capture) = self.capture.take() else {
            return Ok(());
        };
        // Arrêter le flux d'abord : plus aucun bloc n'arrive ensuite.
        drop(capture.stream.take());
        let mut result = Ok(());
        while let Ok(chunk) = capture.rx.try_recv() {
            if result.is_ok() {
                result = write_through(&mut capture.resampler, &mut self.writer, &chunk);
            }
        }
        let mut tail = Vec::new();
        capture
            .resampler
            .finish(|frame| tail.extend_from_slice(frame));
        if result.is_ok() && !tail.is_empty() {
            result = self.writer.write(&tail).map(|_| ());
        }
        result
    }

    fn lose_microphone(&mut self) {
        log::warn!(
            "Réunion {} : micro perdu, nouvelle tentative toutes les {:?}",
            self.context.id,
            RECONNECT_INTERVAL
        );
        if let Err(error) = self.close_capture() {
            log::warn!("Réunion {} : fin de bloc perdue ({error})", self.context.id);
        }
        self.phase = RecorderPhase::Reconnecting;
        self.level = 0.0;
        self.last_reconnect = Instant::now();
        self.publish_status();
    }

    fn try_reconnect(&mut self) {
        self.last_reconnect = Instant::now();
        match Capture::open(&self.context.app) {
            Ok(capture) => {
                log::info!(
                    "Réunion {} : micro retrouvé (« {} »)",
                    self.context.id,
                    capture.device_name
                );
                self.microphone = Some(capture.device_name.clone());
                self.capture = Some(capture);
                self.phase = RecorderPhase::Recording;
                self.last_data = Instant::now();
                self.publish_status();
            }
            Err(error) => log::debug!("Réunion : micro toujours indisponible ({error})"),
        }
    }

    fn periodic(&mut self) -> Flow {
        if self.last_flush.elapsed() >= FLUSH_INTERVAL {
            self.last_flush = Instant::now();
            if let Err(error) = self.writer.flush() {
                log::error!(
                    "Réunion {} : écriture impossible ({error})",
                    self.context.id
                );
                let _ = self.finish(FinishReason::WriteFailed);
                return Flow::Exit;
            }
        }
        if self.last_persist.elapsed() >= PERSIST_INTERVAL
            || self.writer.part != self.persisted_part
        {
            self.persist_progress();
        }
        if self.last_disk_check.elapsed() >= DISK_CHECK_INTERVAL {
            self.last_disk_check = Instant::now();
            if let Some(free) =
                crate::summary::system::free_disk_bytes(self.context.store.audio_dir())
            {
                if free < MIN_FREE_BYTES_WHILE_RECORDING {
                    log::warn!(
                        "Réunion {} : disque presque plein ({free} octets), arrêt",
                        self.context.id
                    );
                    let _ = self.finish(FinishReason::DiskFull);
                    return Flow::Exit;
                }
            }
        }
        if self.phase == RecorderPhase::Recording && self.last_status.elapsed() >= STATUS_INTERVAL {
            self.publish_status();
            // Le niveau retombe entre deux blocs plutôt que de rester figé.
            self.level *= 0.85;
        }
        Flow::Continue
    }

    fn persist_progress(&mut self) {
        self.last_persist = Instant::now();
        self.persisted_part = self.writer.part;
        if let Err(error) = self.context.store.update_progress(
            self.context.id,
            self.writer.duration_ms(),
            self.writer.size_bytes(),
            self.writer.part,
        ) {
            log::warn!(
                "Réunion {} : avancement non mémorisé ({error})",
                self.context.id
            );
        }
    }

    fn publish_status(&mut self) {
        self.last_status = Instant::now();
        let status = MeetingRecorderStatus {
            phase: self.phase,
            recording_id: Some(self.context.id),
            duration_ms: self.writer.duration_ms(),
            size_bytes: self.writer.size_bytes(),
            level: self.level,
            microphone: self.microphone.clone(),
        };
        *self.context.status.lock().unwrap() = status.clone();
        if let Err(e) = status.emit(&self.context.app) {
            log::debug!("Réunion : état non émis ({e})");
        }
    }

    /// Termine l'enregistrement : micro fermé, fichier finalisé, liste à jour.
    fn finish(&mut self, reason: FinishReason) -> Result<MeetingRecording, MeetingError> {
        if let Err(error) = self.close_capture() {
            log::warn!("Réunion {} : fin de bloc perdue ({error})", self.context.id);
        }
        if let Err(error) = self.writer.finalize() {
            // L'en-tête sera réparé au prochain démarrage si besoin ; la
            // durée enregistrée en base reste juste.
            log::error!(
                "Réunion {} : finalisation du fichier ({error})",
                self.context.id
            );
        }
        let id = self.context.id;
        self.context.store.finish(
            id,
            self.writer.duration_ms(),
            self.writer.size_bytes(),
            self.writer.part,
            RecordingState::Done,
        )?;
        log::info!(
            "Réunion {id} : terminée ({:?}, {} ms, {} octets)",
            reason,
            self.writer.duration_ms(),
            self.writer.size_bytes()
        );

        let idle = MeetingRecorderStatus::idle();
        *self.context.status.lock().unwrap() = idle.clone();
        let _ = idle.emit(&self.context.app);

        let recording = self.context.store.get(id)?;
        let _ = MeetingRecordingFinished {
            recording: recording.clone(),
            reason,
        }
        .emit(&self.context.app);
        Ok(recording)
    }
}

/// Rééchantillonne un bloc brut et l'écrit.
fn write_through(
    resampler: &mut FrameResampler,
    writer: &mut PartWriter,
    chunk: &[f32],
) -> Result<(), MeetingError> {
    let mut result = Ok(());
    resampler.push(chunk, |frame| {
        if result.is_ok() {
            result = writer.write(frame).map(|_| ());
        }
    });
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn level_maps_silence_to_zero_and_full_scale_to_one() {
        assert_eq!(level_of(&[]), 0.0);
        assert_eq!(level_of(&[0.0; 480]), 0.0);
        assert!((level_of(&[1.0; 480]) - 1.0).abs() < 1e-6);
        // -30 dBFS ≈ mi-hauteur.
        let mid = level_of(&[0.0316; 480]);
        assert!((mid - 0.5).abs() < 0.02, "{mid}");
    }

    #[test]
    fn writer_counts_duration_and_size_across_parts() {
        let dir = tempfile::tempdir().unwrap();
        let mut writer = PartWriter::create(dir.path().to_path_buf(), 3).unwrap();
        writer.write(&[0.5; 16_000]).unwrap();
        writer.flush().unwrap();
        assert_eq!(writer.duration_ms(), 1000);
        assert_eq!(writer.size_bytes(), 32_044);
        writer.roll().unwrap();
        writer.write(&[-0.5; 8_000]).unwrap();
        writer.finalize().unwrap();
        assert_eq!(writer.duration_ms(), 1500);
        assert_eq!(writer.size_bytes(), 32_044 + 16_044);
        let first = hound::WavReader::open(wav::part_path(dir.path(), 3, 1)).unwrap();
        let second = hound::WavReader::open(wav::part_path(dir.path(), 3, 2)).unwrap();
        assert_eq!((first.len(), second.len()), (16_000, 8_000));
        assert_eq!(
            std::fs::metadata(wav::part_path(dir.path(), 3, 1))
                .unwrap()
                .len(),
            32_044
        );
    }

    #[test]
    fn samples_are_clamped_to_the_16_bit_range() {
        let dir = tempfile::tempdir().unwrap();
        let mut writer = PartWriter::create(dir.path().to_path_buf(), 1).unwrap();
        writer.write(&[2.0, -2.0, 0.0]).unwrap();
        writer.finalize().unwrap();
        let samples: Vec<i16> = hound::WavReader::open(wav::part_path(dir.path(), 1, 1))
            .unwrap()
            .into_samples::<i16>()
            .map(Result::unwrap)
            .collect();
        assert_eq!(samples, vec![i16::MAX, -i16::MAX, 0]);
    }
}
