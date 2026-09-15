use crate::audio_toolkit::{
    chunk_ranges, decode_to_samples, decode_to_samples_with_fallback_cancellable,
};
use crate::commands::video_download::ensure_ffmpeg;
use crate::managers::document::assemble_document;
use crate::managers::file_history::FileHistoryManager;
use crate::managers::transcription::TranscriptionManager;
use log::warn;
use serde::{Deserialize, Serialize};
use specta::Type;
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tauri::{AppHandle, State};
use tauri_plugin_shell::process::CommandEvent;
use tauri_plugin_shell::ShellExt;
use tauri_specta::Event;

/// Tronçon = 30 s à 16 kHz (fréquence attendue par le pipeline de transcription).
const CHUNK_LEN_SAMPLES: usize = 30 * 16_000;
/// Chevauchement entre tronçons consécutifs.
///
/// Volontairement à 0 (Phase A) : avec un chevauchement > 0, la zone commune
/// entre deux tronçons est transcrite deux fois et `assemble_document` ne
/// fait qu'un simple `join(" ")`, ce qui produisait du texte dupliqué toutes
/// les ~29 s. Un chevauchement nul évite cette duplication ; le risque résiduel
/// est qu'un mot soit coupé pile à la frontière des 30 s, un défaut mineur
/// que la structuration LLM de la Phase B pourra lisser.
const CHUNK_OVERLAP_SAMPLES: usize = 0;

/// Drapeau d'annulation partagé pour le pipeline de transcription de fichier.
/// L'app ne traite qu'un fichier à la fois (pas de file d'attente), donc un
/// simple static suffit — pas besoin de le faire transiter par le state Tauri
/// géré (`app.manage`) ni par un mutex.
static CANCEL_FILE_TRANSCRIPTION: AtomicBool = AtomicBool::new(false);

/// Drapeau de ré-entrance : empêche deux transcriptions de fichier de tourner
/// en parallèle. Sans lui, un 2e appel à `transcribe_audio_file` remettrait
/// `CANCEL_FILE_TRANSCRIPTION` à `false` et annulerait silencieusement la
/// demande d'annulation du 1er run.
static FILE_TRANSCRIPTION_RUNNING: AtomicBool = AtomicBool::new(false);

/// Garde RAII qui remet `FILE_TRANSCRIPTION_RUNNING` à `false` quel que soit
/// le chemin de sortie de `transcribe_audio_file` (succès, erreur via `?`,
/// panique) tant qu'elle reste en vie dans son scope.
struct RunningGuard;

impl Drop for RunningGuard {
    fn drop(&mut self) {
        FILE_TRANSCRIPTION_RUNNING.store(false, Ordering::Relaxed);
    }
}

/// Phase du pipeline de transcription de fichier, pour piloter l'UI de progression.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "lowercase")]
pub enum FileTranscriptionPhase {
    /// Téléchargement du média distant (transcription par URL uniquement) ;
    /// `current` transporte le pourcentage (0-100).
    Download,
    /// Préparation de l'outil de conversion (repli ffmpeg, issue #10) :
    /// téléchargement unique et non interruptible d'ffmpeg dans les données
    /// de l'app quand le décodage natif a échoué et qu'aucun ffmpeg n'est
    /// encore disponible (vague de correction n°2, item H3). Pas de suivi en
    /// pourcentage (hors périmètre, voir issue #22).
    PrepareTool,
    Decode,
    Transcribe,
    Assemble,
    Done,
}

/// Progression du pipeline de transcription de fichier, émise à chaque étape
/// (décodage, chaque tronçon transcrit, assemblage, puis fin).
#[derive(Clone, Debug, Serialize, Deserialize, Type, tauri_specta::Event)]
pub struct FileTranscriptionProgress {
    pub phase: FileTranscriptionPhase,
    pub current: u32,
    pub total: u32,
}

/// Résultat renvoyé par `transcribe_audio_file`.
///
/// Contient `content` en plus de `path` : le `.md` est écrit à côté du
/// fichier source (hors `$APPDATA`), donc le scope fs Tauri (limité à
/// `$APPDATA`) empêche le frontend de le relire via `readTextFile`. Renvoyer
/// le contenu directement évite cette relecture et garde l'aperçu
/// fonctionnel quel que soit l'emplacement du fichier source.
#[derive(Clone, Debug, Serialize, Deserialize, Type)]
pub struct FileTranscriptionResult {
    pub path: String,
    pub content: String,
    /// Id de l'entrée créée dans l'historique fichier, ou `None` si son
    /// enregistrement a échoué (la transcription reste utilisable).
    pub history_id: Option<i64>,
}

fn emit_progress(app: &AppHandle, phase: FileTranscriptionPhase, current: u32, total: u32) {
    let _ = FileTranscriptionProgress {
        phase,
        current,
        total,
    }
    .emit(app);
}

/// Orchestre la transcription d'un fichier audio/vidéo déposé par l'utilisateur :
/// décodage -> découpage en tronçons -> transcription tronçon par tronçon ->
/// assemblage -> écriture d'un `.md` à côté du fichier source. Renvoie le
/// chemin du `.md` produit.
///
/// Choix résultat-via-await vs event : le pipeline lourd (décodage + inférence)
/// tourne entièrement dans `spawn_blocking` pour ne jamais geler la boucle
/// async de la commande ; la commande `await` cette tâche et renvoie
/// directement le chemin obtenu via son `Result<String, String>`. C'est le
/// choix le plus simple et le plus proche des conventions du dépôt
/// (`commands::history::retry_history_entry_transcription` fait de même pour
/// `transcribe()`). L'event `file-transcription-progress` sert uniquement à
/// piloter une barre de progression côté UI pendant l'attente ; il n'est pas
/// nécessaire pour transporter le résultat final (pas de second event "done"
/// séparé pour le chemin — la phase `done` de l'event ne fait que signaler la
/// fin, le chemin voyage par la valeur de retour de la commande).
#[tauri::command]
#[specta::specta]
pub async fn transcribe_audio_file(
    app: AppHandle,
    transcription_manager: State<'_, Arc<TranscriptionManager>>,
    file_history: State<'_, Arc<FileHistoryManager>>,
    path: String,
) -> Result<FileTranscriptionResult, String> {
    // Protection anti-ré-entrance : un seul run de transcription de fichier à
    // la fois. Sans elle, un 2e appel remettrait `CANCEL_FILE_TRANSCRIPTION`
    // à `false` et annulerait silencieusement l'annulation du run en cours.
    if FILE_TRANSCRIPTION_RUNNING
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Relaxed)
        .is_err()
    {
        return Err("Une transcription est déjà en cours.".to_string());
    }
    // Remet le drapeau à `false` en sortie de fonction, quel que soit le
    // chemin de sortie (succès, `?`, panique).
    let _running_guard = RunningGuard;

    // Repart d'un drapeau propre à chaque nouvelle transcription.
    CANCEL_FILE_TRANSCRIPTION.store(false, Ordering::Relaxed);

    // Démarre le chargement du modèle si nécessaire (no-op si déjà chargé) ;
    // `transcribe()` attend la fin du chargement avant de tourner, comme dans
    // `retry_history_entry_transcription`.
    transcription_manager.initiate_model_load();

    let tm = Arc::clone(&transcription_manager);
    let fh = Arc::clone(&file_history);
    let app_for_task = app.clone();
    let source_ref = path.clone();

    tauri::async_runtime::spawn_blocking(move || {
        run_pipeline(&app_for_task, &tm, &fh, &path, "local", &source_ref)
    })
    .await
    .map_err(|e| format!("Tâche de transcription interrompue: {e}"))?
}

/// Transcrit l'audio d'une vidéo en ligne (YouTube ou tout site géré par
/// yt-dlp) : téléchargement de la piste audio via yt-dlp (sidecar embarqué,
/// ou binaire téléchargé au premier usage sur macOS), puis
/// pipeline de transcription habituel. Le média téléchargé est supprimé après
/// coup (re-téléchargeable) — seuls le `.md` et l'entrée d'historique restent.
#[tauri::command]
#[specta::specta]
pub async fn transcribe_url(
    app: AppHandle,
    transcription_manager: State<'_, Arc<TranscriptionManager>>,
    file_history: State<'_, Arc<FileHistoryManager>>,
    url: String,
) -> Result<FileTranscriptionResult, String> {
    validate_media_url(&url)?;

    // Même slot d'exécution unique que `transcribe_audio_file` : le
    // téléchargement fait partie du job (le drapeau couvre download +
    // transcription, et l'annulation tue aussi le téléchargement).
    if FILE_TRANSCRIPTION_RUNNING
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Relaxed)
        .is_err()
    {
        return Err("Une transcription est déjà en cours.".to_string());
    }
    let _running_guard = RunningGuard;
    CANCEL_FILE_TRANSCRIPTION.store(false, Ordering::Relaxed);

    // Chargement du modèle lancé en parallèle du téléchargement.
    transcription_manager.initiate_model_load();

    let media_path = download_media(&app, &url).await?;

    let tm = Arc::clone(&transcription_manager);
    let fh = Arc::clone(&file_history);
    let app_for_task = app.clone();
    let media_path_str = media_path.to_string_lossy().to_string();
    let source_ref = url.clone();

    let result = tauri::async_runtime::spawn_blocking(move || {
        run_pipeline(&app_for_task, &tm, &fh, &media_path_str, "url", &source_ref)
    })
    .await
    .map_err(|e| format!("Tâche de transcription interrompue: {e}"))?;

    // Ne pas accumuler des centaines de Mo d'audio re-téléchargeable ; le
    // texte est dans l'historique et le `.md` reste à côté. En cas d'ÉCHEC en
    // revanche, le média est conservé : yt-dlp voit alors le fichier final
    // déjà présent au prochain essai et saute le re-téléchargement — précieux
    // pour une vidéo de plusieurs heures.
    if result.is_ok() {
        let _ = std::fs::remove_file(&media_path);
    }

    result
}

/// L'URL doit être http(s) avec un hôte non vide ; le reste (site supporté,
/// vidéo existante...) est laissé au verdict de yt-dlp.
fn validate_media_url(url: &str) -> Result<(), String> {
    let rest = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"));
    match rest {
        Some(host) if !host.is_empty() && !host.starts_with('/') => Ok(()),
        _ => Err("invalid_url".to_string()),
    }
}

/// Source du binaire yt-dlp pour macOS. Volontairement « latest » et non une
/// version épinglée : yt-dlp doit suivre les changements des sites qu'il
/// télécharge, une version figée cesserait de fonctionner en quelques mois.
#[cfg(target_os = "macos")]
const YT_DLP_MACOS_URL: &str =
    "https://github.com/yt-dlp/yt-dlp/releases/latest/download/yt-dlp_macos";

/// Verrou d'installation de yt-dlp : sérialise le pré-chargement de démarrage
/// ([`crate::external_tools`]) et l'appel à la demande. Le second arrivant
/// attend le premier et repart de son résultat, au lieu de lancer un second
/// téléchargement dans le même fichier de reprise.
#[cfg(target_os = "macos")]
static YT_DLP_INSTALL_LOCK: once_cell::sync::Lazy<tokio::sync::Mutex<()>> =
    once_cell::sync::Lazy::new(|| tokio::sync::Mutex::new(()));

/// macOS uniquement : emplacement de yt-dlp dans les données de l'app, qu'il y
/// soit ou non. Sert au pré-chargement de démarrage pour savoir s'il reste
/// quelque chose à télécharger, sans déclencher le téléchargement lui-même.
#[cfg(target_os = "macos")]
pub(crate) fn yt_dlp_bin_path(app: &AppHandle) -> Result<PathBuf, String> {
    Ok(crate::portable::app_data_dir(app)
        .map_err(|e| format!("Dossier de données inaccessible: {e}"))?
        .join("bin")
        .join("yt-dlp"))
}

/// macOS uniquement : fournit yt-dlp depuis les données de l'app, téléchargé
/// au premier usage (ou pré-chargé au démarrage, voir
/// [`crate::external_tools`]). Impossible de l'embarquer en sidecar sur cette
/// plateforme : Tauri re-signe le bundle en ad-hoc, or yt-dlp_macos
/// (PyInstaller) extrait au lancement une bibliothèque Python signée avec le
/// Team ID yt-dlp — dyld refuse alors le chargement (« mapping process and
/// mapped file have different Team IDs »). Téléchargé ici hors navigateur, le
/// binaire garde sa signature d'origine cohérente et ne porte pas d'attribut
/// de quarantaine.
#[cfg(target_os = "macos")]
pub(crate) async fn ensure_yt_dlp_macos(app: &AppHandle) -> Result<PathBuf, String> {
    let bin_path = yt_dlp_bin_path(app)?;
    if bin_path.exists() {
        return Ok(bin_path);
    }

    let _guard = YT_DLP_INSTALL_LOCK.lock().await;
    if bin_path.exists() {
        return Ok(bin_path);
    }

    crate::external_tools::install_executable(
        YT_DLP_MACOS_URL,
        &bin_path,
        // URL « latest » : compléter un fichier partiel laissé par une session
        // précédente collerait deux versions de yt-dlp bout à bout, sans
        // SHA-256 pour s'en apercevoir. On repart de zéro à chaque tentative.
        crate::external_tools::ResumePolicy::Forbidden,
    )
    .await
    .map_err(|e| format!("Téléchargement de l'outil yt-dlp impossible: {e}"))?;

    Ok(bin_path)
}

/// Version épinglée de QuickJS-NG, le moteur JavaScript que yt-dlp utilise
/// pour résoudre le défi YouTube (voir [`ensure_quickjs`]).
///
/// Épinglée, contrairement à yt-dlp qui doit rester en « latest » pour suivre
/// les changements des sites : un moteur JS n'a pas ce besoin, et une URL figée
/// autorise la reprise d'un téléchargement coupé
/// ([`crate::external_tools::ResumePolicy::Allowed`]).
///
/// La 0.12.0 est un plancher fonctionnel : en deçà, il manque des
/// optimisations sans lesquelles la résolution du défi peut prendre plusieurs
/// minutes au lieu de quelques secondes (cf. wiki EJS de yt-dlp).
const QUICKJS_VERSION: &str = "v0.16.2";

const QUICKJS_BASE_URL: &str = "https://github.com/quickjs-ng/quickjs/releases/download";

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
const QUICKJS_ASSET: &str = "qjs-darwin-arm64";
#[cfg(all(target_os = "macos", target_arch = "x86_64"))]
const QUICKJS_ASSET: &str = "qjs-darwin-x86_64";
#[cfg(all(target_os = "linux", target_arch = "aarch64"))]
const QUICKJS_ASSET: &str = "qjs-linux-aarch64";
#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
const QUICKJS_ASSET: &str = "qjs-linux-x86_64";
// Windows ARM : pas de build dédié, le x64 tourne via l'émulation — même
// stratégie que pour yt-dlp et ffmpeg.
#[cfg(target_os = "windows")]
const QUICKJS_ASSET: &str = "qjs-windows-x86_64.exe";

/// Verrou d'installation de QuickJS : sérialise le pré-chargement de démarrage
/// ([`crate::external_tools`]) et l'appel à la demande, comme pour yt-dlp et
/// ffmpeg. Sans lui, les deux écriraient dans le même fichier de reprise.
static QUICKJS_INSTALL_LOCK: once_cell::sync::Lazy<tokio::sync::Mutex<()>> =
    once_cell::sync::Lazy::new(|| tokio::sync::Mutex::new(()));

/// URL de l'asset QuickJS-NG de la plateforme courante.
fn quickjs_asset_url() -> String {
    format!("{QUICKJS_BASE_URL}/{QUICKJS_VERSION}/{QUICKJS_ASSET}")
}

/// Nom du binaire sur disque. yt-dlp exige `qjs`/`qjs.exe` dès lors qu'on ne
/// lui passe qu'un dossier ; on lui passe le chemin complet du fichier, mais
/// garder le nom canonique évite toute ambiguïté.
fn quickjs_bin_name() -> &'static str {
    if cfg!(target_os = "windows") {
        "qjs.exe"
    } else {
        "qjs"
    }
}

/// Valeur de `--js-runtimes` désignant NOTRE binaire QuickJS.
/// yt-dlp découpe `RUNTIME[:PATH]` sur le premier `:` uniquement
/// (`arg.split(':', 1)`), donc un chemin Windows (`C:\...`) passe intact.
fn quickjs_runtime_arg(path: &Path) -> String {
    format!("quickjs:{}", path.display())
}

/// Emplacement de QuickJS dans les données de l'app, qu'il y soit ou non.
/// Sert au pré-chargement de démarrage pour savoir s'il reste quelque chose à
/// télécharger, sans déclencher le téléchargement lui-même.
pub(crate) fn quickjs_bin_path(app: &AppHandle) -> Result<PathBuf, String> {
    Ok(crate::portable::app_data_dir(app)
        .map_err(|e| format!("Dossier de données inaccessible: {e}"))?
        .join("bin")
        .join(quickjs_bin_name()))
}

/// Fournit le moteur JavaScript dont yt-dlp a besoin pour YouTube, téléchargé
/// au premier usage (ou pré-chargé au démarrage, voir
/// [`crate::external_tools`]).
///
/// # Pourquoi un moteur JavaScript
///
/// Depuis 2025, YouTube impose un défi JavaScript que yt-dlp délègue à un
/// runtime externe (mécanisme « EJS »). Sans runtime, l'extraction échoue :
/// « No supported JavaScript runtime could be found », puis « This video is
/// not available ». Le runtime doit venir de l'application : compter sur un
/// `deno` ou `node` installé par l'utilisateur ne marche pas, et une
/// application lancée depuis le Finder ou un lanceur de bureau n'hérite de
/// toute façon pas du `PATH` du shell — c'est précisément ce qui rendait le
/// défaut invisible en ligne de commande et systématique dans l'application.
///
/// # Pourquoi QuickJS-NG plutôt que Deno
///
/// Deno est le runtime « recommandé » par yt-dlp, mais il est distribué en
/// archive zip de ~40 Mo pour ~130 Mo sur le disque. QuickJS-NG est un binaire
/// statique unique de 1 à 2,6 Mo selon la plateforme, sans dépendance (le
/// build Linux est `static-pie` : aucune exigence de glibc, donc toutes les
/// distributions), pour un temps de résolution du défi équivalent — mesuré à
/// ~11 s contre ~10 s pour Deno. Les scripts EJS eux-mêmes sont déjà embarqués
/// dans les exécutables officiels yt-dlp qu'utilise l'application : il n'y a
/// rien d'autre à récupérer.
pub(crate) async fn ensure_quickjs(app: &AppHandle) -> Result<PathBuf, String> {
    let bin_path = quickjs_bin_path(app)?;
    if bin_path.exists() {
        return Ok(bin_path);
    }

    let _guard = QUICKJS_INSTALL_LOCK.lock().await;
    if bin_path.exists() {
        return Ok(bin_path);
    }

    crate::external_tools::install_executable(
        &quickjs_asset_url(),
        &bin_path,
        // URL épinglée sur v0.16.2 : l'asset ne changera pas, une reprise
        // après coupure complète bien le même fichier.
        crate::external_tools::ResumePolicy::Allowed,
    )
    .await
    .map_err(|e| format!("Téléchargement du moteur JavaScript impossible: {e}"))?;

    Ok(bin_path)
}

/// Résout l'exécutable ffmpeg pour le repli de décodage (issue #10) :
/// binaire téléchargé au premier usage dans les données de l'app, comme pour
/// le téléchargement vidéo (`commands::video_download::ensure_ffmpeg`, même
/// binaire partagé — pas de second téléchargement), sinon `ffmpeg` du PATH.
/// `None` si rien n'est disponible : le message d'erreur du décodeur l'explique.
async fn resolve_ffmpeg(app: &AppHandle) -> Option<PathBuf> {
    match ensure_ffmpeg(app).await {
        Ok(path) => Some(path),
        Err(e) => {
            warn!("ffmpeg indisponible via les données de l'app ({e}), essai du PATH");
            crate::audio_toolkit::ffmpeg::ffmpeg_on_path()
        }
    }
}

/// Résout la commande yt-dlp de la plateforme : binaire téléchargé au premier
/// usage sur macOS (voir [`ensure_yt_dlp_macos`]), sidecar embarqué ailleurs
/// (tauri.{windows,linux}.conf.json). Partagé avec le téléchargement de vidéo
/// (`commands::video_download`).
///
/// `--encoding UTF-8` est posé ici, une fois pour toutes les invocations.
/// Sans lui, yt-dlp écrit sa sortie avec l'encodage de `sys.stdout` — soit la
/// page de codes ANSI du système sous Windows, cp1252 en français — et avec
/// `errors='ignore'` : tout caractère absent de cette page de codes disparaît
/// purement et simplement. Le chemin imprimé par `--print after_move:filepath`
/// ne désignait alors plus le fichier réellement écrit sur le disque, qui
/// garde, lui, son nom Unicode — d'où « yt-dlp n'a pas indiqué de fichier …
/// exploitable » dès qu'un titre de vidéo sortait de l'ASCII. macOS et Linux,
/// en UTF-8 de bout en bout, n'ont jamais rien vu.
///
/// La commande est également armée du moteur JavaScript fourni par
/// l'application ([`ensure_quickjs`]), sans quoi YouTube est inexploitable.
/// `--no-js-runtimes` passe en premier : il vide la liste par défaut (`deno`)
/// pour que le comportement soit identique sur les trois plateformes et ne
/// dépende pas de ce que l'utilisateur a pu installer — un `deno` trop ancien
/// ou cassé ne peut pas prendre le pas sur le nôtre.
pub(crate) async fn resolve_yt_dlp_command(
    app: &AppHandle,
) -> Result<tauri_plugin_shell::process::Command, String> {
    #[cfg(target_os = "macos")]
    let command = {
        let bin_path = ensure_yt_dlp_macos(app).await?;
        app.shell().command(bin_path)
    };
    #[cfg(not(target_os = "macos"))]
    let command = app
        .shell()
        .sidecar("yt-dlp")
        .map_err(|e| format!("Sidecar yt-dlp introuvable: {e}"))?;

    let command = command.args(["--encoding", "UTF-8"]);

    // Échec non fatal : les sites sans défi JavaScript restent téléchargeables,
    // et yt-dlp retombe alors sur sa détection par défaut. Seul YouTube en
    // pâtira, avec son propre message d'erreur.
    match ensure_quickjs(app).await {
        Ok(qjs) => Ok(command.args([
            "--no-js-runtimes",
            "--js-runtimes",
            &quickjs_runtime_arg(&qjs),
        ])),
        Err(e) => {
            warn!("Moteur JavaScript indisponible, YouTube va échouer: {e}");
            Ok(command)
        }
    }
}

/// Ligne de progression émise par yt-dlp via `--progress-template`
/// ("HANDY_DL  12.3%") -> pourcentage arrondi. `None` pour toute autre ligne.
pub(crate) fn parse_download_percent(line: &str) -> Option<u32> {
    let rest = line.strip_prefix("HANDY_DL")?.trim();
    let value: f32 = rest.strip_suffix('%')?.trim().parse().ok()?;
    Some(value.clamp(0.0, 100.0).round() as u32)
}

/// Ce qu'une ligne de stdout de yt-dlp nous apprend.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum YtDlpOutput<'a> {
    /// Ligne du `--progress-template`, en pourcentage arrondi.
    Progress(u32),
    /// Chemin imprimé par `--print after_move:filepath`.
    Path(&'a str),
    /// Octets non-UTF-8 : yt-dlp a écrit dans la page de codes du système
    /// malgré `--encoding UTF-8` (cf. [`resolve_yt_dlp_command`]). Le chemin
    /// qu'on en tirerait ne désignerait aucun fichier.
    Undecodable,
    /// Ligne vide, sans information.
    Ignored,
}

/// Classe une ligne de stdout de yt-dlp. Le décodage est strict : un
/// `from_utf8_lossy` fabriquerait un chemin plausible mais faux, et l'échec
/// qui s'ensuivait ne disait rien de sa cause.
pub(crate) fn parse_yt_dlp_stdout(bytes: &[u8]) -> YtDlpOutput<'_> {
    let Ok(line) = std::str::from_utf8(bytes) else {
        return YtDlpOutput::Undecodable;
    };
    let line = line.trim();
    match parse_download_percent(line) {
        Some(percent) => YtDlpOutput::Progress(percent),
        None if line.is_empty() => YtDlpOutput::Ignored,
        None => YtDlpOutput::Path(line),
    }
}

/// Télécharge la piste audio d'`url` dans `<app_data>/downloads/` via le
/// sidecar yt-dlp et renvoie le chemin du fichier produit.
///
/// `bestaudio[ext=m4a]` d'abord : format que le décodeur interne lit sans
/// conversion (pas de dépendance ffmpeg) ; fallback sur le meilleur format
/// disponible, également décodable (mp4/webm audio). En mode `-q --progress`,
/// stdout ne porte que nos lignes de progression et le chemin final imprimé
/// par `--print after_move:filepath`.
async fn download_media(app: &AppHandle, url: &str) -> Result<PathBuf, String> {
    let downloads_dir = crate::portable::app_data_dir(app)
        .map_err(|e| format!("Dossier de données inaccessible: {e}"))?
        .join("downloads");
    std::fs::create_dir_all(&downloads_dir)
        .map_err(|e| format!("Création du dossier de téléchargement impossible: {e}"))?;
    let output_template = downloads_dir.join("%(title).120B [%(id)s].%(ext)s");

    emit_progress(app, FileTranscriptionPhase::Download, 0, 100);

    let command = resolve_yt_dlp_command(app).await?.args([
        "--no-playlist",
        "-f",
        "bestaudio[ext=m4a]/bestaudio/best",
        "-q",
        "--progress",
        "--newline",
        "--progress-template",
        "download:HANDY_DL %(progress._percent_str)s",
        "--print",
        "after_move:filepath",
        "-o",
        output_template.to_string_lossy().as_ref(),
        url,
    ]);

    let (mut rx, child) = command
        .spawn()
        .map_err(|e| format!("Lancement de yt-dlp impossible: {e}"))?;
    let mut child = Some(child);

    let mut exit_code: Option<i32> = None;
    let mut printed_path: Option<String> = None;
    let mut undecodable_stdout = false;
    let mut stderr_tail: VecDeque<String> = VecDeque::new();

    while let Some(event) = rx.recv().await {
        if CANCEL_FILE_TRANSCRIPTION.load(Ordering::Relaxed) {
            if let Some(c) = child.take() {
                let _ = c.kill();
            }
            return Err("Transcription annulée par l'utilisateur".to_string());
        }
        match event {
            CommandEvent::Stdout(bytes) => match parse_yt_dlp_stdout(&bytes) {
                YtDlpOutput::Progress(pct) => {
                    emit_progress(app, FileTranscriptionPhase::Download, pct, 100)
                }
                // `--print after_move:filepath` : dernière ligne utile.
                YtDlpOutput::Path(path) => printed_path = Some(path.to_string()),
                YtDlpOutput::Undecodable => undecodable_stdout = true,
                YtDlpOutput::Ignored => {}
            },
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
        .ok_or_else(|| {
            if undecodable_stdout {
                "yt-dlp a imprimé le chemin du fichier téléchargé dans un encodage illisible"
                    .to_string()
            } else {
                "yt-dlp n'a pas indiqué de fichier téléchargé exploitable".to_string()
            }
        })?;

    emit_progress(app, FileTranscriptionPhase::Download, 100, 100);
    Ok(path)
}

/// Corps bloquant du pipeline (décodage + inférence + écriture disque),
/// exécuté hors du thread async par l'appelant.
fn run_pipeline(
    app: &AppHandle,
    tm: &TranscriptionManager,
    file_history: &FileHistoryManager,
    path: &str,
    source_kind: &str,
    source_ref: &str,
) -> Result<FileTranscriptionResult, String> {
    let source_path = PathBuf::from(path);

    emit_progress(app, FileTranscriptionPhase::Decode, 0, 1);
    // Décodage natif (symphonia) d'abord : rapide, couvre la majorité des
    // formats (mp3, wav, m4a, flac...) et échoue vite sur les autres (le
    // format n'est même pas reconnu à la sonde, avant toute lecture de
    // paquet — voir `audio_toolkit::decode::decode_to_samples`). ffmpeg
    // n'est résolu qu'en cas d'échec (repli, issue #10) pour ne pas
    // déclencher un téléchargement de 45 à 80 Mo selon la plateforme au
    // premier fichier venu alors que symphonia suffit déjà.
    //
    // Choix spawn_blocking/block_on : `run_pipeline` (cette fonction) n'est
    // pas async et tourne déjà entièrement dans un
    // `tauri::async_runtime::spawn_blocking` (voir `transcribe_audio_file` /
    // `transcribe_url` plus haut) — donc hors de la boucle async. La
    // conversion ffmpeg (`decode_to_samples_with_fallback_cancellable`, qui
    // lance ffmpeg via `Command::spawn()` et le sonde périodiquement) profite
    // directement de ce même mécanisme, sans rien ajouter. Seule
    // `resolve_ffmpeg` est `async` (elle peut télécharger ffmpeg via
    // `reqwest`) : comme elle n'a de sens qu'ici, on la fait tourner via
    // `tauri::async_runtime::block_on`, ce qui est sûr précisément parce
    // qu'on est déjà sur le pool de threads bloquants de Tokio
    // (spawn_blocking) et non sur un thread worker de la boucle async —
    // aucun risque de geler le runtime ni de paniquer ("cannot block the
    // current thread from within a runtime").
    //
    // Annulation : la conversion ffmpeg peut tourner un moment sur un gros
    // fichier ou un fichier corrompu ; elle sonde le même drapeau que le
    // téléchargement yt-dlp (`CANCEL_FILE_TRANSCRIPTION`, voir
    // `download_media` plus haut) pour rester interruptible. Si elle est
    // annulée, l'erreur remonte avec le même message que les autres points
    // d'annulation du pipeline ("Transcription annulée par l'utilisateur"),
    // plutôt que le message générique d'échec de décodage.
    //
    // Préparation de l'outil (vague de correction n°2, item H3) : avant cette
    // correction, `resolve_ffmpeg` pouvait déclencher un téléchargement
    // unique de plusieurs dizaines de Mo pendant lequel l'UI restait figée
    // sur le stade Décodage et Annuler n'avait aucun effet. Le stade dédié
    // `PrepareTool` rend ce temps d'attente visible ; le drapeau d'annulation
    // est vérifié juste avant et juste après l'appel, ce qui permet d'honorer
    // une annulation demandée pendant cette phase dès que possible. Le
    // téléchargement lui-même (dans `ensure_ffmpeg`) reste non interruptible
    // pendant son déroulement : il n'existe pas de point d'annulation à
    // mi-téléchargement (hors périmètre de cette correction, voir issue #22
    // pour un suivi en pourcentage qui permettrait d'y revenir). Cette phase
    // couvre aussi l'attente du verrou d'installation quand le
    // pré-chargement de démarrage (`crate::external_tools`) est déjà en train
    // de récupérer le même binaire : on attend le sien plutôt que d'en
    // télécharger un second en parallèle.
    let samples = match decode_to_samples(&source_path) {
        Ok(samples) => samples,
        Err(native_err) => {
            warn!(
                "Décodage natif impossible pour {} ({native_err}), tentative via ffmpeg",
                source_path.display()
            );
            if CANCEL_FILE_TRANSCRIPTION.load(Ordering::Relaxed) {
                return Err("Transcription annulée par l'utilisateur".to_string());
            }
            emit_progress(app, FileTranscriptionPhase::PrepareTool, 0, 1);
            let ffmpeg = tauri::async_runtime::block_on(resolve_ffmpeg(app));
            if CANCEL_FILE_TRANSCRIPTION.load(Ordering::Relaxed) {
                return Err("Transcription annulée par l'utilisateur".to_string());
            }
            // Retour au stade Décodage : la conversion ffmpeg qui suit peut
            // elle aussi prendre un moment sur un gros fichier.
            emit_progress(app, FileTranscriptionPhase::Decode, 0, 1);
            decode_to_samples_with_fallback_cancellable(&source_path, ffmpeg.as_deref(), &|| {
                CANCEL_FILE_TRANSCRIPTION.load(Ordering::Relaxed)
            })
            .map_err(|e| {
                if CANCEL_FILE_TRANSCRIPTION.load(Ordering::Relaxed) {
                    "Transcription annulée par l'utilisateur".to_string()
                } else {
                    format!(
                        "Format non supporté ou décodage impossible pour {}: {e}",
                        source_path.display()
                    )
                }
            })?
        }
    };
    if samples.is_empty() {
        return Err(format!(
            "Aucun contenu audio décodable dans {}",
            source_path.display()
        ));
    }
    emit_progress(app, FileTranscriptionPhase::Decode, 1, 1);

    let ranges = chunk_ranges(samples.len(), CHUNK_LEN_SAMPLES, CHUNK_OVERLAP_SAMPLES);
    let total = ranges.len() as u32;

    // Relance le chargement du modèle juste avant la transcription : celui
    // lancé à l'entrée de la commande a pu être déchargé entre-temps par le
    // timeout d'inactivité quand le téléchargement (vidéo de plusieurs
    // heures) ou le décodage a duré plus longtemps que ce délai —
    // `transcribe()` attend un chargement en cours mais ne relance jamais un
    // modèle déchargé (« Model is not loaded for transcription »).
    tm.initiate_model_load();

    let mut texts: Vec<String> = Vec::with_capacity(ranges.len());
    for (i, range) in ranges.into_iter().enumerate() {
        if CANCEL_FILE_TRANSCRIPTION.load(Ordering::Relaxed) {
            return Err("Transcription annulée par l'utilisateur".to_string());
        }

        let chunk_samples = samples[range].to_vec();
        let text = tm
            .transcribe(chunk_samples)
            .map_err(|e| format!("Échec de la transcription (tronçon {}/{total}): {e}", i + 1))?;
        texts.push(text);

        emit_progress(
            app,
            FileTranscriptionPhase::Transcribe,
            (i + 1) as u32,
            total,
        );
    }

    if CANCEL_FILE_TRANSCRIPTION.load(Ordering::Relaxed) {
        return Err("Transcription annulée par l'utilisateur".to_string());
    }

    emit_progress(app, FileTranscriptionPhase::Assemble, 0, 1);
    let document = assemble_document(&texts);
    let output_path = source_path.with_extension("md");
    std::fs::write(&output_path, document.as_bytes()).map_err(|e| {
        format!(
            "Écriture du document impossible ({}): {e}",
            output_path.display()
        )
    })?;
    emit_progress(app, FileTranscriptionPhase::Assemble, 1, 1);

    // Persistance dans l'historique fichier AVANT de signaler la fin : le
    // texte est sauvé même si l'UI est fermée pendant la transcription. Un
    // échec d'enregistrement ne fait pas échouer la transcription elle-même.
    let source_name = source_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("transcription")
        .to_string();
    let history_id = match file_history.insert(&source_name, source_kind, source_ref, &document) {
        Ok(id) => Some(id),
        Err(e) => {
            warn!("Échec d'enregistrement dans l'historique fichier: {e}");
            None
        }
    };

    let output_path_str = output_path.to_string_lossy().to_string();
    emit_progress(app, FileTranscriptionPhase::Done, total, total);

    Ok(FileTranscriptionResult {
        path: output_path_str,
        content: document,
        history_id,
    })
}

/// Demande l'annulation de la transcription de fichier en cours. Le pipeline
/// vérifie ce drapeau entre chaque tronçon (avant et après la boucle) et
/// s'arrête proprement dès que le tronçon en cours d'inférence se termine —
/// il n'interrompt pas un appel `transcribe()` déjà lancé sur le tronçon
/// courant.
#[tauri::command]
#[specta::specta]
pub fn cancel_file_transcription() {
    CANCEL_FILE_TRANSCRIPTION.store(true, Ordering::Relaxed);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Le nombre de tronçons annoncé dans les events de progression doit
    /// correspondre exactement à ce que produit `chunk_ranges` avec les
    /// constantes de découpage utilisées par le pipeline (30 s, sans
    /// chevauchement — cf. commentaire sur `CHUNK_OVERLAP_SAMPLES`).
    #[test]
    fn chunk_count_matches_pipeline_constants() {
        // ~65 s d'audio à 16 kHz -> tronçons de 30s jointifs (pas de 30s)
        // tronçons: [0..30s], [30..60s], [60..65s] => 3 tronçons
        let total_len = 65 * 16_000;
        let ranges = chunk_ranges(total_len, CHUNK_LEN_SAMPLES, CHUNK_OVERLAP_SAMPLES);
        assert_eq!(ranges.len(), 3);
        assert_eq!(ranges.first().unwrap().start, 0);
        assert_eq!(ranges.last().unwrap().end, total_len);
    }

    #[test]
    fn short_file_yields_single_chunk() {
        let total_len = 5 * 16_000; // 5 s, plus court qu'un tronçon
        let ranges = chunk_ranges(total_len, CHUNK_LEN_SAMPLES, CHUNK_OVERLAP_SAMPLES);
        assert_eq!(ranges, vec![0..total_len]);
    }

    #[test]
    fn media_url_validation() {
        assert!(validate_media_url("https://www.youtube.com/watch?v=abc").is_ok());
        assert!(validate_media_url("http://vimeo.com/12345").is_ok());
        assert!(validate_media_url("ftp://example.com/video").is_err());
        assert!(validate_media_url("youtube.com/watch?v=abc").is_err());
        assert!(validate_media_url("https://").is_err());
        assert!(validate_media_url("").is_err());
    }

    /// yt-dlp découpe `RUNTIME[:PATH]` sur le PREMIER `:` seulement
    /// (`arg.split(':', 1)`), donc un chemin Windows avec lettre de lecteur
    /// est transmis intact — c'est la plateforme où ça pouvait casser.
    #[test]
    fn quickjs_runtime_arg_keeps_full_path() {
        assert_eq!(
            quickjs_runtime_arg(Path::new("/Users/moi/Library/bin/qjs")),
            "quickjs:/Users/moi/Library/bin/qjs"
        );
        let arg = quickjs_runtime_arg(Path::new(r"C:\Users\moi\AppData\bin\qjs.exe"));
        let (runtime, path) = arg.split_once(':').unwrap();
        assert_eq!(runtime, "quickjs");
        assert_eq!(path, r"C:\Users\moi\AppData\bin\qjs.exe");
    }

    /// Le nom de fichier doit rester `qjs`/`qjs.exe` : yt-dlp l'exige sauf à
    /// pointer le binaire exact — on fait les deux, ceinture et bretelles.
    #[test]
    fn quickjs_binary_keeps_its_canonical_name() {
        assert!(quickjs_bin_name() == "qjs" || quickjs_bin_name() == "qjs.exe");
    }

    /// L'URL doit viser la release ÉPINGLÉE (pas `latest`, sinon la reprise
    /// d'un `.part` collerait deux versions) et un asset de la plateforme.
    #[test]
    fn quickjs_asset_url_is_pinned_and_platform_specific() {
        let url = quickjs_asset_url();
        assert!(url.starts_with("https://github.com/quickjs-ng/quickjs/releases/download/v"));
        assert!(!url.contains("/latest/"));
        let asset = url.rsplit('/').next().unwrap();
        assert!(asset.starts_with("qjs-"));
        if cfg!(target_os = "windows") {
            assert!(asset.ends_with(".exe"));
        } else {
            let os = if cfg!(target_os = "macos") {
                "darwin"
            } else {
                "linux"
            };
            assert!(asset.contains(os), "asset {asset} ne cible pas {os}");
        }
    }

    #[test]
    fn download_percent_parsing() {
        // Sortie du template "download:HANDY_DL %(progress._percent_str)s".
        assert_eq!(parse_download_percent("HANDY_DL  12.3%"), Some(12));
        assert_eq!(parse_download_percent("HANDY_DL 100.0%"), Some(100));
        assert_eq!(parse_download_percent("HANDY_DL 0.0%"), Some(0));
        // Toute autre ligne stdout est le chemin imprimé par --print.
        assert_eq!(parse_download_percent("/tmp/Ma vidéo [abc].m4a"), None);
        assert_eq!(parse_download_percent(""), None);
        assert_eq!(parse_download_percent("HANDY_DL n/a"), None);
    }

    /// Sous Windows, yt-dlp écrit sa sortie dans la page de codes ANSI du
    /// système : « é » y tient sur un seul octet (0xE9), invalide en UTF-8, et
    /// les caractères absents de cette page de codes disparaissent purement et
    /// simplement (`errors='ignore'`). Un chemin reconstruit à coups de U+FFFD
    /// ne désigne plus aucun fichier : la ligne doit être signalée illisible,
    /// jamais « réparée ».
    #[test]
    fn stdout_parsing_separates_progress_path_and_undecodable() {
        assert_eq!(
            parse_yt_dlp_stdout(b"HANDY_DL  12.3%"),
            YtDlpOutput::Progress(12)
        );
        assert_eq!(
            parse_yt_dlp_stdout("/tmp/Ma vidéo [abc].m4a".as_bytes()),
            YtDlpOutput::Path("/tmp/Ma vidéo [abc].m4a")
        );
        assert_eq!(
            parse_yt_dlp_stdout(b"/tmp/Ma vid\xE9o [abc].m4a"),
            YtDlpOutput::Undecodable
        );
        assert_eq!(parse_yt_dlp_stdout(b"   "), YtDlpOutput::Ignored);
    }
}
