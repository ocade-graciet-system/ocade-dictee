//! État partagé du résumé (`Arc<SummaryEngine>` géré par Tauri) : serveur
//! chaud, minuteur d'inactivité, jeton d'annulation, verrou « un résumé à la
//! fois ». Indépendant de Tauri : la commande fournit les callbacks.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tokio_util::sync::CancellationToken;

use super::assets::{engine_asset, CTX_SIZE, MODEL};
use super::install::{ensure_engine, ensure_model, SummaryPaths};
use super::server::{
    build_local_client, default_gpu_layers, kill_orphan_server, LlamaServer, LlamaServerConfig,
    START_TIMEOUT,
};
use super::summarize::{summarize_text, LlamaCompletion};
use super::system::{available_memory_bytes, check_memory, physical_cores};
use super::{SummaryError, SummaryPhase, SummaryStatus};

/// Le serveur est arrêté après ce délai sans requête.
pub const IDLE_TIMEOUT: Duration = Duration::from_secs(10 * 60);
/// Période du veilleur d'inactivité.
pub const WATCHDOG_PERIOD: Duration = Duration::from_secs(30);
/// Sondage de l'état de la dictée avant de démarrer un résumé.
pub const DICTATION_POLL: Duration = Duration::from_millis(500);

pub struct SummaryEngine {
    paths: SummaryPaths,
    client: reqwest::Client,
    local_client: reqwest::Client,
    server: Mutex<Option<Arc<LlamaServer>>>,
    last_used: Mutex<Instant>,
    running: AtomicBool,
    cancel: Mutex<Option<CancellationToken>>,
    available_memory: fn() -> u64,
}

/// Verrou tolérant à l'empoisonnement : ces mutex sont pris depuis un `Drop`
/// (`RunGuard`), où une panique avorterait tout le processus. Aucun d'eux ne
/// protège un invariant qu'une panique ailleurs pourrait casser.
fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Restaure l'état partagé quel que soit le chemin de sortie de `run` :
/// retour, `?`, panique, ou abandon du futur par l'appelant.
struct RunGuard<'a>(&'a SummaryEngine);

impl Drop for RunGuard<'_> {
    fn drop(&mut self) {
        *lock(&self.0.cancel) = None;
        *lock(&self.0.last_used) = Instant::now();
        // Relâché en dernier : tant que le verrou tient, le résumé suivant est
        // `Busy` et ne peut pas voir son jeton effacé par ce nettoyage.
        self.0.running.store(false, Ordering::Release);
    }
}

/// Arrêt du serveur hors de l'exécuteur async : `kill()` attend la fin du
/// processus, c'est-à-dire le déchargement d'un modèle de 2 Go. Le travail
/// bloquant se poursuit même si ce futur est abandonné.
async fn kill_blocking(server: Arc<LlamaServer>) {
    if let Err(e) = tokio::task::spawn_blocking(move || server.kill()).await {
        log::warn!("Arrêt de llama-server interrompu: {e}");
    }
}

impl SummaryEngine {
    pub fn new(paths: SummaryPaths) -> Result<Self, reqwest::Error> {
        Ok(Self {
            paths,
            // Téléchargements : client Internet (proxy système honoré).
            client: crate::download::build_client()?,
            // Serveur local : jamais de proxy (cf. `build_local_client`).
            local_client: build_local_client()?,
            server: Mutex::new(None),
            last_used: Mutex::new(Instant::now()),
            running: AtomicBool::new(false),
            cancel: Mutex::new(None),
            available_memory: available_memory_bytes,
        })
    }

    /// Sonde mémoire de remplacement (tests) : la garde doit être vérifiable
    /// sans dépendre de la charge de la machine.
    #[cfg(test)]
    fn with_memory_probe(mut self, probe: fn() -> u64) -> Self {
        self.available_memory = probe;
        self
    }

    pub fn paths(&self) -> &SummaryPaths {
        &self.paths
    }

    /// Au démarrage de l'app : tue un serveur survivant d'une fermeture brutale.
    pub fn cleanup_orphan(&self) {
        if let Some(exe) = self.paths.engine_exe() {
            kill_orphan_server(&self.paths.pid_file(), &exe);
        }
    }

    pub fn status(&self) -> SummaryStatus {
        let engine_ready = self.paths.engine_exe().is_some_and(|exe| exe.is_file());
        let model_ready = self.paths.model_file().is_file();
        let mut download_size_bytes = 0;
        if !engine_ready {
            download_size_bytes += engine_asset().map(|a| a.size_bytes).unwrap_or(0);
        }
        if !model_ready {
            download_size_bytes += MODEL.size_bytes;
        }
        SummaryStatus {
            engine_ready,
            model_ready,
            download_size_bytes,
        }
    }

    pub fn is_running(&self) -> bool {
        self.running.load(Ordering::Acquire)
    }

    /// Pid du serveur chaud, `None` s'il n'y en a pas (diagnostic et tests).
    pub fn server_pid(&self) -> Option<u32> {
        lock(&self.server).as_ref().map(|server| server.pid())
    }

    /// Demande l'annulation du résumé en cours (téléchargement, démarrage ou
    /// génération) ; sans effet s'il n'y en a pas.
    pub fn cancel(&self) {
        if let Some(token) = lock(&self.cancel).as_ref() {
            token.cancel();
        }
    }

    /// Retire le serveur chaud ; l'arrêter revient à l'appelant.
    fn take_server(&self) -> Option<Arc<LlamaServer>> {
        lock(&self.server).take()
    }

    /// Arrête le serveur s'il tourne (kill synchrone). Pour un contexte déjà
    /// bloquant — `RunEvent::Exit` notamment ; depuis l'async, préférer
    /// `stop_server_async`.
    pub fn stop_server(&self) {
        if let Some(server) = self.take_server() {
            server.kill();
        }
    }

    /// Même chose depuis un contexte async : l'attente du processus part sur
    /// un fil bloquant.
    pub async fn stop_server_async(&self) {
        if let Some(server) = self.take_server() {
            kill_blocking(server).await;
        }
    }

    /// Retire le serveur s'il est inactif depuis `idle` et qu'aucun résumé ne
    /// tourne. Le verrou du serveur couvre le contrôle *et* le retrait : un
    /// résumé qui démarre au même instant se voit refuser le serveur chaud (il
    /// en démarre un neuf) au lieu de l'utiliser pendant qu'on le tue.
    fn take_if_idle(&self, idle: Duration) -> Option<Arc<LlamaServer>> {
        let mut guard = lock(&self.server);
        if guard.is_none() || self.is_running() {
            return None;
        }
        let elapsed = lock(&self.last_used).elapsed();
        if elapsed < idle {
            return None;
        }
        log::info!("llama-server inactif depuis {elapsed:?}, arrêt");
        guard.take()
    }

    /// Arrête le serveur inactif depuis `idle`. Seule variante : le veilleur
    /// d'inactivité est la seule chose qui arrête un serveur au repos, et il
    /// tourne dans l'exécuteur async.
    pub async fn stop_if_idle_async(&self, idle: Duration) -> bool {
        match self.take_if_idle(idle) {
            Some(server) => {
                kill_blocking(server).await;
                true
            }
            None => false,
        }
    }

    /// Exécute un résumé complet. `is_dictating` est sondé toutes les 500 ms
    /// tant qu'une dictée est en cours ; `on_progress` reçoit chaque étape.
    pub async fn run<F: Fn() -> bool + Send>(
        &self,
        text: &str,
        is_dictating: F,
        mut on_progress: impl FnMut(SummaryPhase, u32, u32) + Send,
    ) -> Result<String, SummaryError> {
        if self
            .running
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return Err(SummaryError::Busy);
        }
        // Dès ici, tout chemin de sortie relâche le verrou et retire le jeton.
        let _guard = RunGuard(self);
        let cancel = CancellationToken::new();
        *lock(&self.cancel) = Some(cancel.clone());
        *lock(&self.last_used) = Instant::now();

        let result = self
            .run_inner(text, &cancel, is_dictating, &mut on_progress)
            .await;

        if matches!(result, Err(SummaryError::Cancelled)) {
            // Un serveur en pleine génération ne s'interrompt qu'en le tuant.
            self.stop_server_async().await;
        }
        result
    }

    /// Serveur chaud encore vivant, le cas échéant ; un serveur mort est oublié.
    fn warm_server(&self) -> Option<Arc<LlamaServer>> {
        let mut guard = lock(&self.server);
        match guard.as_ref() {
            Some(server) if server.is_alive() => Some(server.clone()),
            Some(_) => {
                log::warn!("llama-server chaud disparu, redémarrage");
                guard.take();
                None
            }
            None => None,
        }
    }

    /// Démarre le serveur, avec un unique réessai sur un nouveau port : entre
    /// le `free_port()` de `start` et le `bind` du processus, un autre
    /// programme peut prendre le port, et le serveur meurt aussitôt. Un
    /// dépassement du délai de démarrage n'a lui rien à voir avec le port :
    /// il n'est pas réessayé (le premier essai a déjà duré `START_TIMEOUT`).
    async fn start_server(
        &self,
        config: LlamaServerConfig,
        cancel: &CancellationToken,
    ) -> Result<LlamaServer, SummaryError> {
        let started = Instant::now();
        let first = tokio::select! {
            biased;
            _ = cancel.cancelled() => return Err(SummaryError::Cancelled),
            result = LlamaServer::start(config.clone(), &self.local_client) => result,
        };
        let error = match first {
            Ok(server) => return Ok(server),
            Err(error) => error,
        };
        if cancel.is_cancelled() || started.elapsed() >= START_TIMEOUT {
            return Err(error);
        }
        log::warn!("Démarrage de llama-server échoué ({error}), nouvel essai sur un autre port");
        tokio::select! {
            biased;
            _ = cancel.cancelled() => Err(SummaryError::Cancelled),
            result = LlamaServer::start(config, &self.local_client) => result,
        }
    }

    /// Attend la fin d'une dictée en cours : sondage toutes les 500 ms,
    /// annulation honorée à tout instant. Appelée avant *chaque* phase lourde
    /// (téléchargements, démarrage du serveur, génération) et pas seulement au
    /// début : un téléchargement dure plusieurs minutes, une dictée lancée
    /// entre-temps se disputerait sinon le processeur avec le résumé.
    async fn wait_for_dictation<F: Fn() -> bool + Send>(
        &self,
        is_dictating: &F,
        cancel: &CancellationToken,
    ) -> Result<(), SummaryError> {
        while is_dictating() {
            tokio::select! {
                biased;
                _ = cancel.cancelled() => return Err(SummaryError::Cancelled),
                _ = tokio::time::sleep(DICTATION_POLL) => {}
            }
        }
        if cancel.is_cancelled() {
            return Err(SummaryError::Cancelled);
        }
        Ok(())
    }

    async fn run_inner<F: Fn() -> bool + Send>(
        &self,
        text: &str,
        cancel: &CancellationToken,
        is_dictating: F,
        on_progress: &mut (impl FnMut(SummaryPhase, u32, u32) + Send),
    ) -> Result<String, SummaryError> {
        // Jamais en concurrence avec la dictée (CPU/GPU partagés).
        self.wait_for_dictation(&is_dictating, cancel).await?;

        let server = match self.warm_server() {
            Some(server) => server,
            None => {
                // Garde mémoire seulement à froid : un serveur déjà chaud a
                // son modèle en mémoire, la refuser ici n'en libérerait pas.
                check_memory((self.available_memory)())?;
                // `ensure_*` n'émet pas de 100 % quand le fichier est déjà là,
                // et la fin de phase n'est annoncée ici que si quelque chose a
                // effectivement été téléchargé : sinon l'interface afficherait
                // « Téléchargement du moteur… 100 % » à chaque résumé.
                let engine_missing = self.paths.engine_exe().is_none_or(|exe| !exe.is_file());
                let exe = ensure_engine(&self.paths, &self.client, cancel, |percent| {
                    on_progress(SummaryPhase::Engine, percent, 100)
                })
                .await?;
                if engine_missing {
                    on_progress(SummaryPhase::Engine, 100, 100);
                }
                let model_missing = !self.paths.model_file().is_file();
                let model = ensure_model(&self.paths, &self.client, cancel, |percent| {
                    on_progress(SummaryPhase::Model, percent, 100)
                })
                .await?;
                if model_missing {
                    on_progress(SummaryPhase::Model, 100, 100);
                }

                // Une dictée a pu commencer pendant les téléchargements.
                self.wait_for_dictation(&is_dictating, cancel).await?;
                on_progress(SummaryPhase::Starting, 0, 1);
                let config = LlamaServerConfig {
                    exe,
                    model,
                    ctx_size: CTX_SIZE,
                    threads: physical_cores(),
                    gpu_layers: default_gpu_layers(),
                    extra_args: MODEL.server_args.iter().map(|s| s.to_string()).collect(),
                    pid_file: Some(self.paths.pid_file()),
                };
                let server = Arc::new(self.start_server(config, cancel).await?);
                *lock(&self.server) = Some(server.clone());
                on_progress(SummaryPhase::Starting, 1, 1);
                server
            }
        };

        // Dernier contrôle avant la génération, la phase la plus gourmande.
        self.wait_for_dictation(&is_dictating, cancel).await?;
        let completion = LlamaCompletion {
            server: &server,
            model_name: MODEL.model_name,
        };
        summarize_text(text, &completion, cancel, &mut |current, total| {
            on_progress(SummaryPhase::Summarizing, current, total)
        })
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    use std::sync::atomic::AtomicUsize;

    use super::super::assets::LLAMA_BUILD;

    /// Compte-rendu conforme au gabarit (§6), rendu par le faux moteur.
    const GOOD_SUMMARY: &str = "# Titre\n\n## Résumé\nPhrase.\n\n## Points clés\n- point\n";

    fn plenty_of_memory() -> u64 {
        8 * 1024 * 1024 * 1024
    }

    fn no_memory() -> u64 {
        1024
    }

    fn engine(dir: &Path) -> SummaryEngine {
        SummaryEngine::new(SummaryPaths::new(dir)).unwrap()
    }

    /// Moteur dont la sonde mémoire est figée : une machine chargée ne doit
    /// pas faire échouer les tests, ni une machine confortable masquer la garde.
    fn engine_with_memory(dir: &Path, probe: fn() -> u64) -> SummaryEngine {
        engine(dir).with_memory_probe(probe)
    }

    type Events = Arc<Mutex<Vec<(SummaryPhase, u32, u32)>>>;

    fn recorder() -> (Events, impl FnMut(SummaryPhase, u32, u32) + Send) {
        let events: Events = Arc::new(Mutex::new(Vec::new()));
        let sink = events.clone();
        (events, move |phase, current, total| {
            lock(&sink).push((phase, current, total))
        })
    }

    fn recorded(events: &Events) -> Vec<(SummaryPhase, u32, u32)> {
        lock(events).clone()
    }

    #[test]
    fn status_reports_missing_files_and_download_size() {
        let dir = tempfile::tempdir().unwrap();
        let engine = engine(dir.path());
        let engine_size = engine_asset().map(|a| a.size_bytes).unwrap_or(0);
        let status = engine.status();
        assert!(!status.engine_ready && !status.model_ready);
        assert_eq!(status.download_size_bytes, engine_size + MODEL.size_bytes);
        std::fs::create_dir_all(&engine.paths().models_dir).unwrap();
        std::fs::write(engine.paths().model_file(), b"gguf").unwrap();
        let status = engine.status();
        assert!(status.model_ready);
        assert_eq!(status.download_size_bytes, engine_size);
    }

    #[tokio::test]
    async fn cancelled_while_waiting_for_dictation() {
        let dir = tempfile::tempdir().unwrap();
        let engine = Arc::new(engine(dir.path()));
        let e = engine.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(100)).await;
            e.cancel();
        });
        let err = engine
            .run("texte", || true, |_, _, _| {})
            .await
            .unwrap_err();
        assert_eq!(err, SummaryError::Cancelled);
        assert!(!engine.is_running());
        // Le jeton du résumé terminé est retiré : plus rien à annuler.
        engine.cancel();
    }

    #[tokio::test]
    async fn second_run_is_busy_while_the_first_waits() {
        let dir = tempfile::tempdir().unwrap();
        let engine = Arc::new(engine(dir.path()));
        let e = engine.clone();
        let first = tokio::spawn(async move { e.run("texte", || true, |_, _, _| {}).await });
        tokio::time::sleep(Duration::from_millis(50)).await;
        let err = engine
            .run("texte", || false, |_, _, _| {})
            .await
            .unwrap_err();
        assert_eq!(err, SummaryError::Busy);
        engine.cancel();
        assert_eq!(first.await.unwrap().unwrap_err(), SummaryError::Cancelled);
        assert!(!engine.is_running());
    }

    #[tokio::test]
    async fn idle_stop_is_a_no_op_without_server() {
        let dir = tempfile::tempdir().unwrap();
        let engine = engine(dir.path());
        assert!(!engine.stop_if_idle_async(Duration::ZERO).await);
        assert!(engine.server_pid().is_none());
        engine.stop_server();
        engine.stop_server_async().await;
        engine.cleanup_orphan();
    }

    #[tokio::test]
    async fn memory_guard_refuses_before_any_download() {
        let dir = tempfile::tempdir().unwrap();
        let engine = engine_with_memory(dir.path(), no_memory);
        // Une annulation sans résumé en cours ne doit pas empoisonner le suivant.
        engine.cancel();
        let (events, on_progress) = recorder();
        let err = engine
            .run("texte", || false, on_progress)
            .await
            .unwrap_err();
        assert_eq!(
            err,
            SummaryError::Memory {
                needed_bytes: super::super::system::MIN_AVAILABLE_MEMORY_BYTES,
                free_bytes: no_memory(),
            }
        );
        assert!(recorded(&events).is_empty(), "aucun téléchargement entamé");
        assert!(!engine.paths().engine_dir().exists());
        assert!(!engine.is_running());
    }

    // ----- Doubles unix : scripts shell tenant lieu de `llama-server` -----

    /// `llama-server` factice : compte ses lancements, publie le port reçu
    /// (le test y branche alors un `/health` factice) puis dort.
    #[cfg(unix)]
    fn sleeping_engine_script(spawns: &Path, port_file: &Path) -> String {
        format!(
            "#!/bin/sh\n\
             echo x >> '{spawns}'\n\
             while [ \"$#\" -gt 0 ]; do\n\
             if [ \"$1\" = \"--port\" ]; then printf '%s' \"$2\" > '{port}.tmp'; mv '{port}.tmp' '{port}'; fi\n\
             shift\n\
             done\n\
             exec sleep 60\n",
            spawns = spawns.display(),
            port = port_file.display(),
        )
    }

    /// `llama-server` factice qui meurt au démarrage, port déjà pris.
    #[cfg(unix)]
    fn dying_engine_script(spawns: &Path) -> String {
        format!(
            "#!/bin/sh\n\
             echo x >> '{spawns}'\n\
             echo 'bind: address already in use' >&2\n\
             exit 1\n",
            spawns = spawns.display(),
        )
    }

    /// Installe le faux moteur et un faux modèle : `ensure_engine` et
    /// `ensure_model` court-circuitent, aucun réseau n'est touché.
    #[cfg(unix)]
    fn install_fake_engine(paths: &SummaryPaths, script: &str) {
        let exe = paths.engine_exe().expect("plateforme sans moteur");
        std::fs::create_dir_all(paths.engine_dir()).unwrap();
        std::fs::write(&exe, script).unwrap();
        super::super::install::set_executable(&exe).unwrap();
        std::fs::create_dir_all(&paths.models_dir).unwrap();
        std::fs::write(paths.model_file(), b"gguf").unwrap();
    }

    #[cfg(unix)]
    fn spawn_count(spawns: &Path) -> usize {
        std::fs::read_to_string(spawns)
            .map(|text| text.lines().count())
            .unwrap_or(0)
    }

    #[cfg(unix)]
    fn completion_reply(content: &str) -> String {
        serde_json::json!({
            "choices": [{ "message": { "content": content }, "finish_reason": "stop" }]
        })
        .to_string()
    }

    /// Sert `/health` et `/v1/chat/completions` sur le port publié par le faux
    /// moteur. `hang` : la complétion ne répond jamais (génération en cours).
    #[cfg(unix)]
    fn serve_fake_llama(port_file: std::path::PathBuf, reply: String, hang: bool) {
        use std::net::TcpListener;
        std::thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(30);
            let port = loop {
                if Instant::now() >= deadline {
                    return;
                }
                match std::fs::read_to_string(&port_file).map(|t| t.trim().parse::<u16>()) {
                    Ok(Ok(port)) => break port,
                    _ => std::thread::sleep(Duration::from_millis(20)),
                }
            };
            let Ok(listener) = TcpListener::bind(("127.0.0.1", port)) else {
                return;
            };
            for stream in listener.incoming() {
                let Ok(stream) = stream else { break };
                let reply = reply.clone();
                std::thread::spawn(move || handle_fake_request(stream, &reply, hang));
            }
        });
    }

    #[cfg(unix)]
    fn handle_fake_request(mut stream: std::net::TcpStream, reply: &str, hang: bool) {
        use std::io::{BufRead, BufReader, Read, Write};
        let mut reader = BufReader::new(stream.try_clone().unwrap());
        let mut request_line = String::new();
        if reader.read_line(&mut request_line).unwrap_or(0) == 0 {
            return;
        }
        let mut length = 0usize;
        loop {
            let mut line = String::new();
            if reader.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
                break;
            }
            if let Some(value) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                length = value.trim().parse().unwrap_or(0);
            }
        }
        if length > 0 {
            let mut body = vec![0u8; length];
            let _ = reader.read_exact(&mut body);
        }
        let body = if request_line.contains("/health") {
            "{\"status\":\"ok\"}".to_string()
        } else if request_line.contains("/chat/completions") {
            if hang {
                // La requête reste sans réponse : seule une annulation en sort.
                std::thread::sleep(Duration::from_secs(20));
                return;
            }
            reply.to_string()
        } else {
            "{}".to_string()
        };
        let _ = write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
    }

    #[cfg(unix)]
    fn process_alive(pid: u32) -> bool {
        std::process::Command::new("kill")
            .args(["-0", &pid.to_string()])
            // Le message « No such process » est la réponse attendue.
            .stderr(std::process::Stdio::null())
            .status()
            .map(|status| status.success())
            .unwrap_or(false)
    }

    /// Attend qu'une condition devienne vraie (sondage), au plus `timeout`.
    #[cfg(unix)]
    async fn wait_until(timeout: Duration, mut condition: impl FnMut() -> bool) -> bool {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            if condition() {
                return true;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        condition()
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn server_starts_once_stays_warm_and_stops_when_idle() {
        if engine_asset().is_none() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let paths = SummaryPaths::new(dir.path());
        let spawns = dir.path().join("spawns");
        let port_file = dir.path().join("port");
        install_fake_engine(&paths, &sleeping_engine_script(&spawns, &port_file));
        serve_fake_llama(port_file, completion_reply(GOOD_SUMMARY), false);
        let engine = engine_with_memory(dir.path(), plenty_of_memory);

        let (events, on_progress) = recorder();
        let summary = engine
            .run("Texte de la dictée.", || false, on_progress)
            .await
            .unwrap();
        assert_eq!(summary, GOOD_SUMMARY.trim());
        let pid = engine.server_pid().expect("serveur chaud");
        let events = recorded(&events);
        // Moteur et modèle déjà installés : aucune phase de téléchargement
        // n'est annoncée, pas même un 100 % de complaisance.
        assert_eq!(
            events,
            vec![
                (SummaryPhase::Starting, 0, 1),
                (SummaryPhase::Starting, 1, 1),
                (SummaryPhase::Summarizing, 1, 1),
            ]
        );
        assert!(paths.pid_file().is_file(), "pid écrit pour l'orphelin");

        // Deuxième résumé : même processus, aucune phase de démarrage.
        let (events, on_progress) = recorder();
        engine
            .run("Autre dictée.", || false, on_progress)
            .await
            .unwrap();
        assert_eq!(engine.server_pid(), Some(pid));
        assert_eq!(spawn_count(&spawns), 1, "serveur conservé chaud");
        assert_eq!(recorded(&events), vec![(SummaryPhase::Summarizing, 1, 1)]);

        // Inactivité : rien avant l'échéance, arrêt ensuite.
        assert!(!engine.stop_if_idle_async(Duration::from_secs(600)).await);
        assert!(engine.server_pid().is_some());
        assert!(engine.stop_if_idle_async(Duration::ZERO).await);
        assert!(engine.server_pid().is_none());
        assert!(!process_alive(pid), "processus arrêté");
        assert!(!paths.pid_file().exists());
        assert!(!engine.stop_if_idle_async(Duration::ZERO).await);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn start_is_retried_once_on_a_fresh_port() {
        if engine_asset().is_none() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let paths = SummaryPaths::new(dir.path());
        let spawns = dir.path().join("spawns");
        install_fake_engine(&paths, &dying_engine_script(&spawns));
        let engine = engine_with_memory(dir.path(), plenty_of_memory);

        let err = engine
            .run("texte", || false, |_, _, _| {})
            .await
            .unwrap_err();
        assert!(
            matches!(err, SummaryError::EngineStartFailed { ref detail } if detail.contains("address already in use")),
            "{err:?}"
        );
        assert_eq!(
            spawn_count(&spawns),
            2,
            "un seul réessai, sur un autre port"
        );
        assert!(engine.server_pid().is_none());
        assert!(!paths.pid_file().exists());
        assert!(!engine.is_running());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn cancel_during_generation_kills_the_server() {
        if engine_asset().is_none() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let paths = SummaryPaths::new(dir.path());
        let spawns = dir.path().join("spawns");
        let port_file = dir.path().join("port");
        install_fake_engine(&paths, &sleeping_engine_script(&spawns, &port_file));
        serve_fake_llama(port_file, completion_reply(GOOD_SUMMARY), true);
        let engine = Arc::new(engine_with_memory(dir.path(), plenty_of_memory));

        let runner = engine.clone();
        let run = tokio::spawn(async move { runner.run("texte", || false, |_, _, _| {}).await });
        assert!(
            wait_until(Duration::from_secs(20), || engine.server_pid().is_some()).await,
            "le serveur aurait dû démarrer"
        );
        let pid = engine.server_pid().unwrap();
        engine.cancel();
        assert_eq!(run.await.unwrap().unwrap_err(), SummaryError::Cancelled);
        assert!(
            engine.server_pid().is_none(),
            "serveur tué par l'annulation"
        );
        assert!(!process_alive(pid));
        assert!(!paths.pid_file().exists());
        assert!(!engine.is_running());
    }

    /// Une dictée qui démarre *après* le début du résumé retarde la phase
    /// suivante (ici le démarrage du serveur) sans faire échouer le
    /// compte-rendu : l'attente est refaite avant chaque phase lourde, et pas
    /// seulement une fois au début.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_dictation_started_mid_run_delays_the_next_phase() {
        if engine_asset().is_none() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let paths = SummaryPaths::new(dir.path());
        let spawns = dir.path().join("spawns");
        let port_file = dir.path().join("port");
        install_fake_engine(&paths, &sleeping_engine_script(&spawns, &port_file));
        serve_fake_llama(port_file, completion_reply(GOOD_SUMMARY), false);
        let engine = engine_with_memory(dir.path(), plenty_of_memory);

        // Aucune dictée au premier sondage ; une dictée occupe la machine aux
        // deux suivants, puis se termine.
        let calls = Arc::new(AtomicUsize::new(0));
        let probe = calls.clone();
        let is_dictating = move || (1..=2).contains(&probe.fetch_add(1, Ordering::SeqCst));

        let started = Instant::now();
        let summary = engine
            .run("Texte de la dictée.", is_dictating, |_, _, _| {})
            .await
            .unwrap();
        assert_eq!(summary, GOOD_SUMMARY.trim());
        let waited = started.elapsed();
        assert!(
            waited >= 2 * DICTATION_POLL,
            "la phase suivante n'a pas attendu la fin de la dictée ({waited:?})"
        );
        assert!(
            calls.load(Ordering::SeqCst) >= 4,
            "l'attente doit être refaite avant chaque phase lourde"
        );
    }

    /// Annulation pendant l'attente d'une dictée commencée après le début du
    /// résumé : `Cancelled`, et aucun serveur n'a été lancé.
    #[cfg(unix)]
    #[tokio::test]
    async fn cancel_while_waiting_for_a_dictation_started_mid_run() {
        if engine_asset().is_none() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let paths = SummaryPaths::new(dir.path());
        let spawns = dir.path().join("spawns");
        let port_file = dir.path().join("port");
        install_fake_engine(&paths, &sleeping_engine_script(&spawns, &port_file));
        let engine = Arc::new(engine_with_memory(dir.path(), plenty_of_memory));

        // Dictée absente au premier sondage, puis interminable.
        let calls = Arc::new(AtomicUsize::new(0));
        let probe = calls.clone();
        let is_dictating = move || probe.fetch_add(1, Ordering::SeqCst) >= 1;

        let runner = engine.clone();
        let run =
            tokio::spawn(async move { runner.run("texte", is_dictating, |_, _, _| {}).await });
        assert!(
            wait_until(Duration::from_secs(10), || calls.load(Ordering::SeqCst)
                >= 2)
            .await,
            "l'attente de fin de dictée aurait dû s'installer"
        );
        engine.cancel();
        assert_eq!(run.await.unwrap().unwrap_err(), SummaryError::Cancelled);
        assert_eq!(spawn_count(&spawns), 0, "aucun serveur lancé");
        assert!(engine.server_pid().is_none());
        assert!(!engine.is_running());
    }

    /// Passe complète avec le vrai `llama-server`, le vrai modèle et une vraie
    /// transcription du spike (liens symboliques : rien n'est copié).
    /// Exécution manuelle :
    /// `cargo test summary::engine::real_engine -- --ignored --nocapture`.
    #[cfg(unix)]
    #[tokio::test]
    #[ignore = "moteur et modèle réels (≈2 Go) : exécution manuelle"]
    async fn real_engine_summarizes_a_dictation() {
        let spike = std::path::PathBuf::from(std::env::var("HOME").unwrap())
            .join("Downloads/Ocade_Dictée-cargo/llm-spike");
        if !spike.is_dir() {
            eprintln!(
                "sauté : dossier d'essai absent ({}) — moteur et modèle réels requis",
                spike.display()
            );
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let paths = SummaryPaths::new(dir.path());
        std::fs::create_dir_all(&paths.bin_dir).unwrap();
        std::fs::create_dir_all(&paths.models_dir).unwrap();
        std::os::unix::fs::symlink(
            spike.join(format!("llama-{LLAMA_BUILD}")),
            paths.engine_dir(),
        )
        .unwrap();
        std::os::unix::fs::symlink(
            spike.join("models").join(MODEL.file_name),
            paths.model_file(),
        )
        .unwrap();
        let engine = engine(dir.path());
        // Transcription réelle de l'essai comparatif (§9) : une tranche.
        let text = std::fs::read_to_string(spike.join("texts").join("t2.txt")).unwrap();

        let (events, on_progress) = recorder();
        let started = Instant::now();
        let result = engine.run(&text, || false, on_progress).await;
        println!("--- passe réelle ({:?}) ---", started.elapsed());
        match &result {
            Ok(summary) => {
                println!("{summary}");
                assert!(
                    super::super::prompt::check_template(summary).is_ok(),
                    "{summary}"
                );
            }
            // Ce module garantit la passe complète (démarrage, génération,
            // arrêt) ; le respect du gabarit appartient à `prompt`/`summarize`.
            // Ministral rend aujourd'hui des titres en gras (`## **Résumé**`)
            // et des lignes `---` que `check_template` refuse : le compte-rendu
            // revient alors `IncompleteOutput` après l'unique relance. Signalé
            // au contrôleur, hors périmètre de cette tâche.
            Err(SummaryError::IncompleteOutput) => {
                println!("gabarit refusé (titres en gras / séparateur)")
            }
            Err(other) => panic!("{other:?}"),
        }
        let events = recorded(&events);
        // Moteur et modèle déjà en place : aucune phase de téléchargement.
        assert_eq!(
            events,
            vec![
                (SummaryPhase::Starting, 0, 1),
                (SummaryPhase::Starting, 1, 1),
                (SummaryPhase::Summarizing, 1, 1),
            ]
        );
        let pid = engine.server_pid().expect("serveur chaud");
        assert!(engine.stop_if_idle_async(Duration::ZERO).await);
        assert!(!process_alive(pid), "aucun llama-server survivant");
        assert!(!paths.pid_file().exists());
    }
}
