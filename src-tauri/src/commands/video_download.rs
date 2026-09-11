//! Téléchargement à la demande de la vidéo (MP4) d'une entrée d'historique
//! de type "url", conservée dans les données de l'app puis exportable où
//! l'utilisateur veut (typiquement son dossier Téléchargements).
//!
//! La transcription, elle, ne télécharge que l'audio (rapide) ; la vidéo n'est
//! récupérée que si l'utilisateur la demande, une seule fois par entrée.

use crate::commands::file_transcription::{parse_download_percent, resolve_yt_dlp_command};
use crate::managers::file_history::FileHistoryManager;
use serde::{Deserialize, Serialize};
use specta::Type;
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tauri::{AppHandle, State};
use tauri_plugin_shell::process::CommandEvent;
use tauri_specta::Event;

static CANCEL_VIDEO_DOWNLOAD: AtomicBool = AtomicBool::new(false);
static VIDEO_DOWNLOAD_RUNNING: AtomicBool = AtomicBool::new(false);

struct RunningGuard;

impl Drop for RunningGuard {
    fn drop(&mut self) {
        VIDEO_DOWNLOAD_RUNNING.store(false, Ordering::Relaxed);
    }
}

/// Progression du téléchargement de la vidéo d'une entrée d'historique.
#[derive(Clone, Debug, Serialize, Deserialize, Type, tauri_specta::Event)]
pub struct VideoDownloadProgress {
    pub history_id: i64,
    pub percent: u32,
}

/// Source des binaires ffmpeg statiques (merge vidéo+audio par yt-dlp, et
/// repli de décodage de l'onglet Fichier — voir [`ensure_ffmpeg`]).
/// Version épinglée : ni le merge/remux ni la conversion n'ont besoin d'un
/// ffmpeg dernier cri, et une URL stable garantit la reproductibilité.
const FFMPEG_BASE_URL: &str = "https://github.com/eugeneware/ffmpeg-static/releases/download/b6.0";

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
const FFMPEG_ASSET: &str = "ffmpeg-darwin-arm64";
#[cfg(all(target_os = "macos", target_arch = "x86_64"))]
const FFMPEG_ASSET: &str = "ffmpeg-darwin-x64";
#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
const FFMPEG_ASSET: &str = "ffmpeg-linux-x64";
#[cfg(all(target_os = "linux", target_arch = "aarch64"))]
const FFMPEG_ASSET: &str = "ffmpeg-linux-arm64";
// Windows ARM : pas de build dédié, le x64 tourne via l'émulation.
#[cfg(target_os = "windows")]
const FFMPEG_ASSET: &str = "ffmpeg-win32-x64";

/// Fournit ffmpeg depuis les données de l'app, téléchargé au premier usage.
/// Même stratégie que yt-dlp sur macOS : hors du bundle signé (pas de
/// re-signature ad-hoc qui casse), hors navigateur (pas de quarantaine), et
/// sans alourdir l'installeur de 45 à 80 Mo selon la plateforme pour une
/// fonction optionnelle.
///
/// `pub(crate)` : partagé avec le repli de décodage de l'onglet Fichier
/// (issue #10, voir `commands::file_transcription::resolve_ffmpeg`) — même
/// binaire, pas de second téléchargement ni de sidecar dédié.
pub(crate) async fn ensure_ffmpeg(app: &AppHandle) -> Result<PathBuf, String> {
    let bin_dir = crate::portable::app_data_dir(app)
        .map_err(|e| format!("Dossier de données inaccessible: {e}"))?
        .join("bin");
    let bin_name = if cfg!(target_os = "windows") {
        "ffmpeg.exe"
    } else {
        "ffmpeg"
    };
    let bin_path = bin_dir.join(bin_name);
    if bin_path.exists() {
        return Ok(bin_path);
    }

    std::fs::create_dir_all(&bin_dir)
        .map_err(|e| format!("Création du dossier bin impossible: {e}"))?;

    let url = format!("{FFMPEG_BASE_URL}/{FFMPEG_ASSET}");
    let response = reqwest::get(&url)
        .await
        .and_then(|r| r.error_for_status())
        .map_err(|e| format!("Téléchargement de ffmpeg impossible: {e}"))?;
    let bytes = response
        .bytes()
        .await
        .map_err(|e| format!("Téléchargement de ffmpeg interrompu: {e}"))?;

    // Nom de staging unique par appel (au lieu d'un nom fixe partagé,
    // `ffmpeg.download`) : le téléchargement vidéo et le repli de décodage de
    // l'onglet Fichier appellent tous deux `ensure_ffmpeg`, chacun gardé par
    // son propre drapeau de ré-entrance (`VIDEO_DOWNLOAD_RUNNING`,
    // `FILE_TRANSCRIPTION_RUNNING`) : rien n'empêche les deux de démarrer un
    // téléchargement en parallèle au tout premier lancement (aucun binaire
    // encore présent pour aucun des deux). Avec un nom de staging fixe,
    // leurs écritures s'entrelaceraient dans le même fichier et
    // corrompraient le binaire final de façon permanente (le fichier
    // existerait désormais, donc plus jamais retéléchargé ni réparé).
    let staging = tempfile::Builder::new()
        .prefix("ffmpeg-")
        .tempfile_in(&bin_dir)
        .map_err(|e| format!("Fichier temporaire ffmpeg impossible: {e}"))?;
    std::fs::write(staging.path(), &bytes)
        .map_err(|e| format!("Écriture de ffmpeg impossible: {e}"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(staging.path(), std::fs::Permissions::from_mode(0o755))
            .map_err(|e| format!("Permissions de ffmpeg impossibles: {e}"))?;
    }

    // Un appelant concurrent a pu terminer son propre téléchargement entre-
    // temps (même contenu attendu, même URL) : on ne l'écrase pas.
    if bin_path.exists() {
        return Ok(bin_path);
    }
    staging
        .persist(&bin_path)
        .map_err(|e| format!("Installation de ffmpeg impossible: {e}"))?;

    Ok(bin_path)
}

/// Télécharge (au premier appel) et conserve la vidéo MP4 d'une entrée
/// d'historique "url", puis renvoie son chemin dans les données de l'app.
/// Jusqu'à 1080p, H.264/mp4 pour une lecture universelle ; la fusion
/// vidéo+audio passe par le ffmpeg fourni par [`ensure_ffmpeg`].
#[tauri::command]
#[specta::specta]
pub async fn download_entry_video(
    app: AppHandle,
    file_history: State<'_, Arc<FileHistoryManager>>,
    history_id: i64,
) -> Result<String, String> {
    let entry = file_history
        .get(history_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "entry_not_found".to_string())?;
    if entry.source_kind != "url" {
        return Err("not_a_url_entry".to_string());
    }
    // Vidéo déjà conservée : pas de nouveau téléchargement.
    if let Some(path) = entry.video_path.as_deref() {
        if Path::new(path).exists() {
            return Ok(path.to_string());
        }
    }

    if VIDEO_DOWNLOAD_RUNNING
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Relaxed)
        .is_err()
    {
        return Err("Un téléchargement de vidéo est déjà en cours.".to_string());
    }
    let _running_guard = RunningGuard;
    CANCEL_VIDEO_DOWNLOAD.store(false, Ordering::Relaxed);

    let emit = |percent: u32| {
        let _ = VideoDownloadProgress {
            history_id,
            percent,
        }
        .emit(&app);
    };
    emit(0);

    let ffmpeg_path = ensure_ffmpeg(&app).await?;

    let videos_dir = crate::portable::app_data_dir(&app)
        .map_err(|e| format!("Dossier de données inaccessible: {e}"))?
        .join("videos");
    std::fs::create_dir_all(&videos_dir)
        .map_err(|e| format!("Création du dossier vidéos impossible: {e}"))?;
    let output_template = videos_dir.join("%(title).120B [%(id)s].%(ext)s");

    let command = resolve_yt_dlp_command(&app).await?.args([
        "--no-playlist",
        // mp4 H.264 + m4a de préférence (lecture universelle), 1080p max.
        "-f",
        "bv*[ext=mp4]+ba[ext=m4a]/b[ext=mp4]/b",
        "-S",
        "res:1080",
        "--merge-output-format",
        "mp4",
        "--ffmpeg-location",
        ffmpeg_path.to_string_lossy().as_ref(),
        "-q",
        "--progress",
        "--newline",
        "--progress-template",
        "download:HANDY_DL %(progress._percent_str)s",
        "--print",
        "after_move:filepath",
        "-o",
        output_template.to_string_lossy().as_ref(),
        &entry.source_ref,
    ]);

    let (mut rx, child) = command
        .spawn()
        .map_err(|e| format!("Lancement de yt-dlp impossible: {e}"))?;
    let mut child = Some(child);

    let mut exit_code: Option<i32> = None;
    let mut printed_path: Option<String> = None;
    let mut stderr_tail: VecDeque<String> = VecDeque::new();

    while let Some(event) = rx.recv().await {
        if CANCEL_VIDEO_DOWNLOAD.load(Ordering::Relaxed) {
            if let Some(c) = child.take() {
                let _ = c.kill();
            }
            return Err("Téléchargement de la vidéo annulé".to_string());
        }
        match event {
            CommandEvent::Stdout(bytes) => {
                let line = String::from_utf8_lossy(&bytes);
                let line = line.trim();
                if let Some(percent) = parse_download_percent(line) {
                    emit(percent);
                } else if !line.is_empty() {
                    // Deux fichiers transitent (flux vidéo + audio) ; le
                    // `--print after_move:filepath` du fichier FUSIONNÉ est la
                    // dernière ligne utile.
                    printed_path = Some(line.to_string());
                }
            }
            CommandEvent::Stderr(bytes) => {
                let line = String::from_utf8_lossy(&bytes).trim().to_string();
                if !line.is_empty() {
                    if stderr_tail.len() >= 8 {
                        stderr_tail.pop_front();
                    }
                    stderr_tail.push_back(line);
                }
            }
            CommandEvent::Terminated(payload) => {
                exit_code = payload.code;
            }
            _ => {}
        }
    }

    if exit_code != Some(0) {
        let details: Vec<String> = stderr_tail.into();
        return Err(format!(
            "Échec du téléchargement de la vidéo (yt-dlp, code {:?}): {}",
            exit_code,
            details.join(" | ")
        ));
    }

    let path = printed_path
        .map(PathBuf::from)
        .filter(|p| p.exists())
        .ok_or_else(|| "yt-dlp n'a pas indiqué de fichier vidéo exploitable".to_string())?;

    let path_str = path.to_string_lossy().to_string();
    file_history
        .set_video_path(history_id, &path_str)
        .map_err(|e| e.to_string())?;

    emit(100);
    Ok(path_str)
}

/// Copie la vidéo conservée d'une entrée vers la destination choisie par
/// l'utilisateur (boîte « Enregistrer sous »). La copie tourne hors de la
/// boucle async — un MP4 peut peser des centaines de Mo.
#[tauri::command]
#[specta::specta]
pub async fn export_entry_video(
    file_history: State<'_, Arc<FileHistoryManager>>,
    history_id: i64,
    dest_path: String,
) -> Result<(), String> {
    let entry = file_history
        .get(history_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "entry_not_found".to_string())?;
    let source = entry
        .video_path
        .filter(|p| Path::new(p).exists())
        .ok_or_else(|| "video_not_downloaded".to_string())?;

    tauri::async_runtime::spawn_blocking(move || {
        std::fs::copy(&source, &dest_path)
            .map(|_| ())
            .map_err(|e| format!("Copie de la vidéo impossible: {e}"))
    })
    .await
    .map_err(|e| format!("Tâche de copie interrompue: {e}"))?
}

/// Demande l'annulation du téléchargement de vidéo en cours.
#[tauri::command]
#[specta::specta]
pub fn cancel_video_download() {
    CANCEL_VIDEO_DOWNLOAD.store(true, Ordering::Relaxed);
}
