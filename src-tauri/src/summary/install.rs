//! Installation du moteur (`llama-server` + bibliothèques) et du modèle GGUF
//! dans les données de l'app, avec reprise, SHA-256, extraction et contrôle
//! `llama-server --version`.

use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use flate2::read::GzDecoder;
use tokio_util::sync::CancellationToken;

use super::assets::{engine_asset, ArchiveKind, EngineAsset, ModelAsset, LLAMA_BUILD, MODEL};
use super::system::{check_disk_space, free_disk_bytes};
use super::SummaryError;
use crate::download::{
    download_to_part, download_with_resume, part_path, DownloadError, DownloadEvent,
};

/// Emplacements des fichiers du résumé dans les données de l'app.
#[derive(Debug, Clone)]
pub struct SummaryPaths {
    /// `<app_data>/bin` (partagé avec ffmpeg et yt-dlp).
    pub bin_dir: PathBuf,
    /// `<app_data>/models/summary`.
    pub models_dir: PathBuf,
}

impl SummaryPaths {
    pub fn new(app_data_dir: &Path) -> Self {
        Self {
            bin_dir: app_data_dir.join("bin"),
            models_dir: app_data_dir.join("models").join("summary"),
        }
    }

    /// `<app_data>/bin/llama-<build>/`.
    pub fn engine_dir(&self) -> PathBuf {
        self.bin_dir.join(format!("llama-{LLAMA_BUILD}"))
    }

    /// Chemin de `llama-server` (None sur une plateforme sans build).
    pub fn engine_exe(&self) -> Option<PathBuf> {
        engine_asset().map(|asset| self.engine_dir().join(asset.exe_name))
    }

    pub fn model_file(&self) -> PathBuf {
        self.models_dir.join(MODEL.file_name)
    }

    /// `<app_data>/bin/llama/server.pid`.
    pub fn pid_file(&self) -> PathBuf {
        self.bin_dir.join("llama").join("server.pid")
    }
}

/// Indicateur Windows : pas de fenêtre console pour les processus enfants.
#[cfg(windows)]
pub const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Échéance du contrôle `llama-server --version` : le binaire n'affiche que
/// deux lignes ; au-delà il est tenu pour bloqué (bibliothèque sur un volume
/// réseau injoignable, pilote qui ne rend pas la main…).
const VERSION_CHECK_TIMEOUT: Duration = Duration::from_secs(30);

/// Intervalle de scrutation du processus pendant l'attente du contrôle.
const VERSION_CHECK_POLL: Duration = Duration::from_millis(100);

impl From<DownloadError> for SummaryError {
    fn from(e: DownloadError) -> Self {
        match e {
            DownloadError::Connect(_) => SummaryError::Offline,
            DownloadError::Failed(detail) => SummaryError::DownloadFailed { detail },
            DownloadError::ChecksumMismatch { .. } => SummaryError::ChecksumMismatch,
            DownloadError::Cancelled => SummaryError::Cancelled,
            DownloadError::Io(e) => SummaryError::DownloadFailed {
                detail: e.to_string(),
            },
        }
    }
}

fn percent(downloaded: u64, total: u64) -> u32 {
    if total == 0 {
        0
    } else {
        ((downloaded as f64 / total as f64) * 100.0).clamp(0.0, 100.0) as u32
    }
}

/// Refuse une entrée d'archive qui sortirait du dossier cible (« zip slip » /
/// « tar slip ») : chemin absolu, préfixe de volume Windows, composant `..`.
fn check_archive_path(path: &Path) -> Result<(), String> {
    use std::path::Component;
    let escapes = path.components().any(|component| {
        matches!(
            component,
            Component::ParentDir | Component::RootDir | Component::Prefix(_)
        )
    });
    if escapes {
        return Err(format!("entrée d'archive refusée: {}", path.display()));
    }
    Ok(())
}

/// Extrait `archive` dans `final_dir` (remplacé s'il existe) via un dossier
/// temporaire `<final_dir>.extracting`. Un tar.gz dont l'unique entrée racine
/// est un dossier (`llama-b10930/`) est « aplati » : `final_dir` contient
/// directement les fichiers, comme pour le zip Windows à plat.
pub fn extract_engine_archive(
    archive: &Path,
    kind: ArchiveKind,
    final_dir: &Path,
) -> Result<(), String> {
    let temp_dir = final_dir.with_extension("extracting");
    if temp_dir.exists() {
        std::fs::remove_dir_all(&temp_dir)
            .map_err(|e| format!("nettoyage de {}: {e}", temp_dir.display()))?;
    }
    std::fs::create_dir_all(&temp_dir)
        .map_err(|e| format!("création de {}: {e}", temp_dir.display()))?;

    let result = (|| -> Result<(), String> {
        let file = File::open(archive).map_err(|e| format!("ouverture de l'archive: {e}"))?;
        match kind {
            ArchiveKind::TarGz => extract_tar_gz(file, &temp_dir),
            ArchiveKind::Zip => extract_zip(file, &temp_dir),
        }
    })();
    if let Err(e) = result {
        let _ = std::fs::remove_dir_all(&temp_dir);
        return Err(e);
    }

    let entries: Vec<PathBuf> = std::fs::read_dir(&temp_dir)
        .map_err(|e| e.to_string())?
        .filter_map(|entry| entry.ok().map(|e| e.path()))
        .collect();
    let source_dir = match entries.as_slice() {
        [single] if single.is_dir() => single.clone(),
        _ => temp_dir.clone(),
    };
    if final_dir.exists() {
        std::fs::remove_dir_all(final_dir)
            .map_err(|e| format!("remplacement de {}: {e}", final_dir.display()))?;
    }
    std::fs::rename(&source_dir, final_dir)
        .map_err(|e| format!("installation dans {}: {e}", final_dir.display()))?;
    let _ = std::fs::remove_dir_all(&temp_dir);
    Ok(())
}

/// Entrées du tar.gz, chacune validée avant d'être écrite. Les dossiers sont
/// extraits en dernier (comme `tar::Archive::unpack`) pour que leurs droits ne
/// gênent pas l'écriture de leur contenu.
fn extract_tar_gz(file: File, temp_dir: &Path) -> Result<(), String> {
    let fail = |e: std::io::Error| format!("extraction tar.gz: {e}");
    let mut tar = tar::Archive::new(GzDecoder::new(file));
    let mut directories = Vec::new();
    for entry in tar.entries().map_err(fail)? {
        let mut entry = entry.map_err(fail)?;
        let path = entry.path().map_err(fail)?.into_owned();
        check_archive_path(&path)?;
        // Une cible de lien absolue ne peut jamais être légitime dans une
        // archive relogeable ; les cibles relatives sont vérifiées par `tar`
        // lui-même (canonicalisation sous le dossier de destination).
        if let Some(link) = entry.link_name().map_err(fail)? {
            if link.is_absolute() {
                return Err(format!("entrée d'archive refusée: {}", link.display()));
            }
        }
        if entry.header().entry_type().is_dir() {
            directories.push(entry);
        } else {
            entry.unpack_in(temp_dir).map_err(fail)?;
        }
    }
    // Ordre décroissant des chemins, comme `tar::Archive::unpack` : un parent
    // aux droits restrictifs (`0400`) doit être servi après son contenu, sinon
    // celui-ci n'est plus atteignable pour y poser quoi que ce soit.
    directories.sort_by(|a, b| b.path_bytes().cmp(&a.path_bytes()));
    for mut dir in directories {
        dir.unpack_in(temp_dir).map_err(fail)?;
    }
    Ok(())
}

/// Refuse un nom brut d'entrée zip qui serait absolu ou porterait un préfixe
/// de volume Windows : `enclosed_name()` ne les rejette pas, il les reloge
/// silencieusement sous la cible (`/etc/x` → `etc/x`, `C:\x` → `x`), alors que
/// la règle d'extraction est de refuser une telle archive.
fn check_zip_entry_name(name: &str) -> Result<(), String> {
    let bytes = name.as_bytes();
    let rooted = matches!(bytes.first(), Some(b'/' | b'\\'));
    let volume = bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':';
    if rooted || volume {
        return Err(format!("entrée d'archive refusée: {name}"));
    }
    Ok(())
}

/// Entrées du zip, toutes validées avant la moindre écriture.
fn extract_zip(file: File, temp_dir: &Path) -> Result<(), String> {
    let mut zip = zip::ZipArchive::new(file).map_err(|e| format!("ouverture zip: {e}"))?;
    for index in 0..zip.len() {
        let entry = zip
            .by_index(index)
            .map_err(|e| format!("extraction zip: {e}"))?;
        check_zip_entry_name(entry.name())?;
        match entry.enclosed_name() {
            Some(name) => check_archive_path(&name)?,
            None => return Err(format!("entrée d'archive refusée: {}", entry.name())),
        }
    }
    zip.extract(temp_dir)
        .map_err(|e| format!("extraction zip: {e}"))
}

/// Bit exécutable (macOS/Linux) ; sans effet sur Windows.
pub fn set_executable(path: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))?;
    }
    #[cfg(not(unix))]
    {
        let _ = path;
    }
    Ok(())
}

/// Pourquoi un contrôle `--version` n'a pas abouti.
#[derive(Debug)]
enum VersionCheckFailure {
    /// Lancement impossible, ou code de sortie non nul.
    Detail(String),
    /// Échéance dépassée : le processus a été tué.
    TimedOut,
    /// Annulation demandée pendant l'attente : le processus a été tué.
    Cancelled,
}

impl VersionCheckFailure {
    fn detail(self) -> String {
        match self {
            Self::Detail(detail) => detail,
            Self::TimedOut => "llama-server --version : délai dépassé".to_string(),
            Self::Cancelled => "llama-server --version : annulé".to_string(),
        }
    }
}

/// Contrôle final : `llama-server --version` doit renvoyer 0 (bibliothèques
/// trouvées, binaire exécutable, pas de quarantaine). Borné à
/// `VERSION_CHECK_TIMEOUT` : un binaire qui ne rend pas la main est tué et le
/// contrôle échoue, il ne bloque pas l'appelant.
pub fn run_version_check(exe: &Path) -> Result<(), String> {
    run_version_check_with_timeout(exe, VERSION_CHECK_TIMEOUT, None)
        .map_err(VersionCheckFailure::detail)
}

/// Corps du contrôle : l'échéance est un paramètre (tests) et `cancel`, s'il
/// est fourni, interrompt l'attente. Dans les deux cas le processus est tué,
/// jamais laissé derrière. Bloquant : à lancer dans `spawn_blocking` depuis un
/// contexte async (voir `check_engine_version`).
fn run_version_check_with_timeout(
    exe: &Path,
    timeout: Duration,
    cancel: Option<&CancellationToken>,
) -> Result<(), VersionCheckFailure> {
    let mut command = std::process::Command::new(exe);
    // `--version` écrit sur stderr ; stdout part au néant plutôt que dans un
    // tube que personne ne draine (un tube plein bloquerait le processus).
    // stderr est lu après la fin du processus : un binaire qui noierait son
    // tube serait de toute façon arrêté par l'échéance.
    command
        .arg("--version")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped());
    // Le dossier du binaire est le dossier de travail (DLL voisines sur
    // Windows) ; un parent vide (`exe` sans dossier) n'en est pas un.
    if let Some(dir) = exe.parent().filter(|dir| !dir.as_os_str().is_empty()) {
        command.current_dir(dir);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    let mut child = command
        .spawn()
        .map_err(|e| VersionCheckFailure::Detail(format!("lancement de {}: {e}", exe.display())))?;

    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {}
            Err(e) => {
                kill_child(&mut child);
                let detail = format!("attente de {}: {e}", exe.display());
                warn_version_check(&detail, &read_stderr(&mut child));
                return Err(VersionCheckFailure::Detail(detail));
            }
        }
        if cancel.is_some_and(|cancel| cancel.is_cancelled()) {
            kill_child(&mut child);
            return Err(VersionCheckFailure::Cancelled);
        }
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            kill_child(&mut child);
            warn_version_check(
                &VersionCheckFailure::TimedOut.detail(),
                &read_stderr(&mut child),
            );
            return Err(VersionCheckFailure::TimedOut);
        }
        std::thread::sleep(left.min(VERSION_CHECK_POLL));
    };
    if status.success() {
        return Ok(());
    }
    let stderr = read_stderr(&mut child);
    let detail = format!(
        "`llama-server --version` a renvoyé {:?}: {stderr}",
        status.code()
    );
    warn_version_check(&detail, &stderr);
    Err(VersionCheckFailure::Detail(detail))
}

/// Sortie d'erreur du processus (vide si le tube n'est plus disponible).
fn read_stderr(child: &mut std::process::Child) -> String {
    let mut stderr = String::new();
    if let Some(mut piped) = child.stderr.take() {
        let _ = piped.read_to_string(&mut stderr);
    }
    stderr.trim().to_string()
}

/// Journalise l'échec du contrôle `--version` avec la sortie d'erreur du
/// moteur : c'est le seul endroit où la cause réelle (bibliothèque manquante,
/// binaire refusé) reste lisible pour le support.
fn warn_version_check(detail: &str, stderr: &str) {
    if stderr.is_empty() {
        log::warn!("Contrôle du moteur de résumé échoué : {detail}");
    } else {
        log::warn!("Contrôle du moteur de résumé échoué : {detail}\n{stderr}");
    }
}

/// Tue le processus et le récolte : pas de zombie derrière une échéance ou une
/// annulation.
fn kill_child(child: &mut std::process::Child) {
    let _ = child.kill();
    let _ = child.wait();
}

/// Contrôle `--version` depuis un contexte async : hors de la boucle
/// d'exécution (`spawn_blocking`), borné par `VERSION_CHECK_TIMEOUT` et
/// interrompu par `cancel`.
async fn check_engine_version(exe: &Path, cancel: &CancellationToken) -> Result<(), SummaryError> {
    let exe = exe.to_path_buf();
    let cancel = cancel.clone();
    let checked = tokio::task::spawn_blocking(move || {
        run_version_check_with_timeout(&exe, VERSION_CHECK_TIMEOUT, Some(&cancel))
    })
    .await;
    match checked {
        Ok(Ok(())) => Ok(()),
        Ok(Err(VersionCheckFailure::Cancelled)) => Err(SummaryError::Cancelled),
        Ok(Err(failure)) => Err(SummaryError::EngineStartFailed {
            detail: failure.detail(),
        }),
        Err(e) => Err(SummaryError::EngineStartFailed {
            detail: format!("contrôle --version interrompu: {e}"),
        }),
    }
}

/// Pré-vol macOS, avant tout téléchargement : les binaires llama.cpp épinglés
/// exigent macOS 13.3 alors que l'application démarre dès 10.15. Sur une
/// version antérieure le moteur ne se lancerait pas — inutile de télécharger
/// 2,2 Go pour échouer ensuite. Version illisible ou absente : on continue.
#[cfg(target_os = "macos")]
fn check_macos_version() -> Result<(), SummaryError> {
    let Some(version) = sysinfo::System::os_version() else {
        return Ok(());
    };
    if super::assets::macos_version_supports_engine(&version) == Some(false) {
        let detail =
            "macOS 13.3 ou plus récent est requis pour le résumé (moteur llama.cpp)".to_string();
        log::error!("{detail} (macOS {version} détecté)");
        return Err(SummaryError::EngineStartFailed { detail });
    }
    Ok(())
}

fn engine_asset_or_unsupported() -> Result<&'static EngineAsset, SummaryError> {
    engine_asset().ok_or_else(|| SummaryError::EngineStartFailed {
        detail: format!(
            "aucun build llama.cpp pour {}/{}",
            std::env::consts::OS,
            std::env::consts::ARCH
        ),
    })
}

/// Supprime une installation inutilisable pour qu'un lancement suivant la
/// retélécharge : sans cela le court-circuit `exe.is_file()` la reprendrait
/// telle quelle, sans jamais repasser le contrôle `--version`. Un échec de
/// suppression est journalisé, jamais bloquant.
fn discard_engine_dir(engine_dir: &Path) {
    match std::fs::remove_dir_all(engine_dir) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => log::warn!(
            "Moteur inutilisable, suppression de {} impossible: {e}",
            engine_dir.display()
        ),
    }
}

fn ensure_disk_space(dir: &Path) -> Result<(), SummaryError> {
    if let Some(free) = free_disk_bytes(dir) {
        check_disk_space(free)?;
    }
    Ok(())
}

/// Installe le moteur si besoin et renvoie le chemin de `llama-server`.
/// Une installation qui ne passe pas le contrôle `--version` est supprimée :
/// le moteur présent sur le disque est donc toujours un moteur vérifié, et un
/// moteur cassé est retéléchargé au lancement suivant.
pub async fn ensure_engine(
    paths: &SummaryPaths,
    client: &reqwest::Client,
    cancel: &CancellationToken,
    on_progress: impl FnMut(u32),
) -> Result<PathBuf, SummaryError> {
    #[cfg(target_os = "macos")]
    check_macos_version()?;
    let asset = engine_asset_or_unsupported()?;
    ensure_engine_from(asset, paths, client, cancel, on_progress).await
}

/// Variante paramétrée par l'asset (testable avec un serveur HTTP local).
pub async fn ensure_engine_from(
    asset: &EngineAsset,
    paths: &SummaryPaths,
    client: &reqwest::Client,
    cancel: &CancellationToken,
    mut on_progress: impl FnMut(u32),
) -> Result<PathBuf, SummaryError> {
    let engine_dir = paths.engine_dir();
    let exe = engine_dir.join(asset.exe_name);
    if exe.is_file() {
        return Ok(exe);
    }
    std::fs::create_dir_all(&paths.bin_dir).map_err(|e| SummaryError::DownloadFailed {
        detail: e.to_string(),
    })?;
    ensure_disk_space(&paths.bin_dir)?;

    let archive = paths.bin_dir.join(asset.file_name);
    let part = part_path(&archive);
    let mut attempts = 0;
    loop {
        attempts += 1;
        let download = download_to_part(
            client,
            asset.url,
            &part,
            Some(asset.sha256),
            cancel,
            |event| {
                if let DownloadEvent::Progress { downloaded, total } = event {
                    on_progress(percent(downloaded, total));
                }
            },
        )
        .await;
        match download {
            Ok(()) => {}
            Err(DownloadError::ChecksumMismatch { .. }) if attempts < 2 => {
                log::warn!("Archive du moteur corrompue, nouvelle tentative");
                continue;
            }
            Err(e) => return Err(e.into()),
        }
        // Le `.part` est conservé : une reprise le revalidera par son empreinte.
        if cancel.is_cancelled() {
            return Err(SummaryError::Cancelled);
        }
        match extract_engine_archive(&part, asset.kind, &engine_dir) {
            Ok(()) => break,
            Err(detail) if attempts < 2 => {
                log::warn!("Extraction du moteur impossible ({detail}), nouvelle tentative");
                let _ = std::fs::remove_file(&part);
                continue;
            }
            Err(detail) => {
                let _ = std::fs::remove_file(&part);
                return Err(SummaryError::EngineStartFailed { detail });
            }
        }
    }
    // Un dossier moteur présent doit avoir passé `--version` : une annulation
    // ici laisserait sinon une installation jamais contrôlée, que le
    // court-circuit `exe.is_file()` reprendrait au lancement suivant.
    if cancel.is_cancelled() {
        discard_engine_dir(&engine_dir);
        return Err(SummaryError::Cancelled);
    }
    let _ = std::fs::remove_file(&part);
    if !exe.is_file() {
        discard_engine_dir(&engine_dir);
        return Err(SummaryError::EngineStartFailed {
            detail: format!("{} absent de l'archive", asset.exe_name),
        });
    }
    if let Err(e) = set_executable(&exe) {
        discard_engine_dir(&engine_dir);
        return Err(SummaryError::EngineStartFailed {
            detail: e.to_string(),
        });
    }
    if let Err(e) = check_engine_version(&exe, cancel).await {
        discard_engine_dir(&engine_dir);
        return Err(e);
    }
    on_progress(100);
    Ok(exe)
}

/// Télécharge le modèle GGUF si besoin (reprise, SHA-256) et renvoie son chemin.
pub async fn ensure_model(
    paths: &SummaryPaths,
    client: &reqwest::Client,
    cancel: &CancellationToken,
    on_progress: impl FnMut(u32),
) -> Result<PathBuf, SummaryError> {
    ensure_model_from(&MODEL, paths, client, cancel, on_progress).await
}

/// Variante paramétrée par l'asset (testable avec un serveur HTTP local).
pub async fn ensure_model_from(
    model: &ModelAsset,
    paths: &SummaryPaths,
    client: &reqwest::Client,
    cancel: &CancellationToken,
    mut on_progress: impl FnMut(u32),
) -> Result<PathBuf, SummaryError> {
    let dest = paths.models_dir.join(model.file_name);
    if dest.is_file() {
        return Ok(dest);
    }
    std::fs::create_dir_all(&paths.models_dir).map_err(|e| SummaryError::DownloadFailed {
        detail: e.to_string(),
    })?;
    ensure_disk_space(&paths.models_dir)?;
    let mut attempts = 0;
    loop {
        attempts += 1;
        let download = download_with_resume(
            client,
            model.url,
            &dest,
            Some(model.sha256),
            cancel,
            |event| {
                if let DownloadEvent::Progress { downloaded, total } = event {
                    on_progress(percent(downloaded, total));
                }
            },
        )
        .await;
        match download {
            Ok(()) => break,
            Err(DownloadError::ChecksumMismatch { .. }) if attempts < 2 => {
                log::warn!("Modèle de résumé corrompu, nouvelle tentative");
                continue;
            }
            Err(e) => return Err(e.into()),
        }
    }
    on_progress(100);
    Ok(dest)
}

#[cfg(test)]
pub(crate) mod test_archives {
    //! Fabrique de petites archives au format des releases llama.cpp.
    use super::*;
    use std::io::Write;

    /// Script qui joue le rôle de `llama-server` : `--version` → code 0.
    pub const FAKE_SERVER: &[u8] =
        b"#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then echo 'version: fake'; exit 0; fi\nexit 1\n";

    pub fn write_tar_gz(path: &Path, root: &str, files: &[(&str, &[u8], u32)]) {
        let file = File::create(path).unwrap();
        let encoder = flate2::write::GzEncoder::new(file, flate2::Compression::default());
        let mut builder = tar::Builder::new(encoder);
        for (name, data, mode) in files {
            let mut header = tar::Header::new_gnu();
            header.set_size(data.len() as u64);
            header.set_mode(*mode);
            header.set_cksum();
            builder
                .append_data(&mut header, format!("{root}/{name}"), *data)
                .unwrap();
        }
        builder.into_inner().unwrap().finish().unwrap();
    }

    pub fn write_zip(path: &Path, files: &[(&str, &[u8])]) {
        let file = File::create(path).unwrap();
        let mut writer = zip::ZipWriter::new(file);
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        for (name, data) in files {
            writer.start_file(*name, options).unwrap();
            writer.write_all(data).unwrap();
        }
        writer.finish().unwrap();
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use std::time::Instant;

    use super::test_archives::*;
    use super::*;

    #[test]
    fn tar_gz_with_root_folder_is_flattened_into_final_dir() {
        let dir = tempfile::tempdir().unwrap();
        let archive = dir.path().join("llama-b10930-bin-test.tar.gz");
        write_tar_gz(
            &archive,
            "llama-b10930",
            &[
                ("llama-server", FAKE_SERVER, 0o755),
                ("libllama.dylib", b"lib", 0o644),
            ],
        );
        let final_dir = dir.path().join("llama-b10930");
        extract_engine_archive(&archive, ArchiveKind::TarGz, &final_dir).unwrap();
        assert!(final_dir.join("llama-server").is_file());
        assert!(final_dir.join("libllama.dylib").is_file());
        assert!(!dir.path().join("llama-b10930.extracting").exists());
        // Réinstallation par-dessus une version existante : pas d'erreur.
        extract_engine_archive(&archive, ArchiveKind::TarGz, &final_dir).unwrap();
    }

    #[test]
    fn flat_zip_is_extracted_into_final_dir() {
        let dir = tempfile::tempdir().unwrap();
        let archive = dir.path().join("llama-b10930-bin-win-cpu-x64.zip");
        write_zip(
            &archive,
            &[("llama-server.exe", b"exe"), ("ggml.dll", b"dll")],
        );
        let final_dir = dir.path().join("llama-b10930");
        extract_engine_archive(&archive, ArchiveKind::Zip, &final_dir).unwrap();
        assert!(final_dir.join("llama-server.exe").is_file());
        assert!(final_dir.join("ggml.dll").is_file());
    }

    #[test]
    fn corrupt_archive_is_reported_and_leaves_no_temp_dir() {
        let dir = tempfile::tempdir().unwrap();
        let archive = dir.path().join("bad.tar.gz");
        std::fs::write(&archive, b"not an archive").unwrap();
        let final_dir = dir.path().join("llama-b10930");
        let err = extract_engine_archive(&archive, ArchiveKind::TarGz, &final_dir).unwrap_err();
        assert!(err.contains("tar.gz"), "{err}");
        assert!(!final_dir.exists());
        assert!(!dir.path().join("llama-b10930.extracting").exists());
    }

    /// tar.gz malveillant : `tar::Builder::append_data` refuse un chemin en
    /// `..`, l'en-tête est donc écrit brut pour reproduire l'attaque.
    fn write_evil_tar_gz(path: &Path, name: &str, data: &[u8]) {
        let file = File::create(path).unwrap();
        let encoder = flate2::write::GzEncoder::new(file, flate2::Compression::default());
        let mut builder = tar::Builder::new(encoder);
        let mut header = tar::Header::new_gnu();
        header.as_gnu_mut().unwrap().name[..name.len()].copy_from_slice(name.as_bytes());
        header.set_entry_type(tar::EntryType::Regular);
        header.set_size(data.len() as u64);
        header.set_mode(0o644);
        header.set_cksum();
        builder.append(&header, data).unwrap();
        builder.into_inner().unwrap().finish().unwrap();
    }

    /// Sécurité : une entrée qui remonte hors du dossier cible est refusée et
    /// rien n'est écrit à côté (tar.gz comme zip).
    #[test]
    fn archive_entries_escaping_the_target_dir_are_refused() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("target");
        std::fs::create_dir_all(&target).unwrap();
        let final_dir = target.join("llama-b10930");

        let tar_archive = dir.path().join("evil.tar.gz");
        write_evil_tar_gz(&tar_archive, "../evil.txt", b"pwned");
        let err = extract_engine_archive(&tar_archive, ArchiveKind::TarGz, &final_dir).unwrap_err();
        assert!(err.contains("refus"), "{err}");

        let zip_archive = dir.path().join("evil.zip");
        write_zip(&zip_archive, &[("../evil.txt", b"pwned")]);
        let err = extract_engine_archive(&zip_archive, ArchiveKind::Zip, &final_dir).unwrap_err();
        assert!(err.contains("refus"), "{err}");

        assert!(!target.join("evil.txt").exists());
        assert!(!dir.path().join("evil.txt").exists());
        assert!(!final_dir.exists());
        assert!(!target.join("llama-b10930.extracting").exists());
    }

    /// `zip` reloge silencieusement un nom absolu sous la cible (`/etc/x` →
    /// `etc/x`, `C:\x` → `x`) : le nom brut doit donc être refusé à part.
    #[test]
    fn zip_entries_with_absolute_names_are_refused() {
        let dir = tempfile::tempdir().unwrap();
        let final_dir = dir.path().join("llama-b10930");
        for name in ["/etc/x", r"C:\x", r"\\serveur\partage\x"] {
            let archive = dir.path().join("evil.zip");
            write_zip(&archive, &[(name, b"pwned")]);
            let err =
                extract_engine_archive(&archive, ArchiveKind::Zip, &final_dir).expect_err(name);
            // Le nom brut, pas sa version relogée, doit figurer dans le refus.
            assert!(err.contains("refus") && err.contains(name), "{name}: {err}");
        }
        assert!(!final_dir.exists());
        assert!(!dir.path().join("llama-b10930.extracting").exists());
    }

    /// tar.gz avec des dossiers explicites (le brief n'en fabrique que des
    /// fichiers) : de quoi reproduire un parent aux droits restrictifs.
    fn write_tar_gz_with_dirs(path: &Path, dirs: &[(&str, u32)], files: &[(&str, &[u8], u32)]) {
        let file = File::create(path).unwrap();
        let encoder = flate2::write::GzEncoder::new(file, flate2::Compression::default());
        let mut builder = tar::Builder::new(encoder);
        for (name, mode) in dirs {
            let mut header = tar::Header::new_gnu();
            header.set_entry_type(tar::EntryType::Directory);
            header.set_size(0);
            header.set_mode(*mode);
            builder
                .append_data(&mut header, format!("{name}/"), std::io::empty())
                .unwrap();
        }
        for (name, data, mode) in files {
            let mut header = tar::Header::new_gnu();
            header.set_size(data.len() as u64);
            header.set_mode(*mode);
            builder.append_data(&mut header, *name, *data).unwrap();
        }
        builder.into_inner().unwrap().finish().unwrap();
    }

    /// Les dossiers sont appliqués par chemin décroissant (comme
    /// `tar::Archive::unpack`) : les droits d'un parent sans bit `x` ne
    /// doivent pas empêcher de poser ceux de son contenu.
    #[cfg(unix)]
    #[test]
    fn restrictive_parent_directories_are_applied_last() {
        let dir = tempfile::tempdir().unwrap();
        let archive = dir.path().join("perms.tar.gz");
        write_tar_gz_with_dirs(
            &archive,
            &[
                ("llama-b10930", 0o755),
                ("llama-b10930/sous", 0o400),
                ("llama-b10930/sous/profond", 0o755),
            ],
            &[("llama-b10930/sous/profond/llama-server", FAKE_SERVER, 0o755)],
        );
        let final_dir = dir.path().join("llama-b10930");
        extract_engine_archive(&archive, ArchiveKind::TarGz, &final_dir).unwrap();
        // Rendre le dossier traversable pour l'inspection puis le nettoyage.
        set_executable(&final_dir.join("sous")).unwrap();
        assert!(final_dir
            .join("sous")
            .join("profond")
            .join("llama-server")
            .is_file());
    }

    /// Une extraction qui échoue ne touche pas à l'installation en place :
    /// `final_dir` n'est remplacé qu'au renommage final.
    #[test]
    fn a_failed_extraction_keeps_the_previous_installation() {
        let dir = tempfile::tempdir().unwrap();
        let final_dir = dir.path().join("llama-b10930");
        std::fs::create_dir_all(&final_dir).unwrap();
        std::fs::write(final_dir.join("llama-server"), b"ancien").unwrap();
        let archive = dir.path().join("bad.tar.gz");
        std::fs::write(&archive, b"not an archive").unwrap();
        extract_engine_archive(&archive, ArchiveKind::TarGz, &final_dir).unwrap_err();
        assert_eq!(
            std::fs::read(final_dir.join("llama-server")).unwrap(),
            b"ancien"
        );
        assert!(!dir.path().join("llama-b10930.extracting").exists());
    }

    #[cfg(unix)]
    #[test]
    fn version_check_runs_the_executable() {
        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("llama-server");
        std::fs::write(&exe, FAKE_SERVER).unwrap();
        set_executable(&exe).unwrap();
        run_version_check(&exe).unwrap();
        let failing = dir.path().join("failing");
        std::fs::write(
            &failing,
            b"#!/bin/sh\necho 'dyld: missing lib' >&2\nexit 134\n",
        )
        .unwrap();
        set_executable(&failing).unwrap();
        let err = run_version_check(&failing).unwrap_err();
        assert!(err.contains("134") && err.contains("missing lib"), "{err}");
    }

    /// Un moteur qui ne rend jamais la main est abandonné à l'échéance, et le
    /// processus est tué (échéance injectée ici ; 30 s en production).
    #[cfg(unix)]
    #[test]
    fn version_check_gives_up_and_kills_a_hanging_process() {
        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("llama-server");
        // Le marqueur n'est écrit qu'au bout du sommeil : s'il apparaît, le
        // processus a survécu à l'échéance.
        std::fs::write(&exe, b"#!/bin/sh\nsleep 2\ntouch marqueur\n").unwrap();
        set_executable(&exe).unwrap();
        let started = Instant::now();
        let err =
            run_version_check_with_timeout(&exe, Duration::from_millis(200), None).unwrap_err();
        assert!(matches!(err, VersionCheckFailure::TimedOut), "{err:?}");
        let waited = started.elapsed();
        assert!(waited < Duration::from_millis(1500), "{waited:?}");
        assert!(err.detail().contains("délai dépassé"));
        std::thread::sleep(Duration::from_millis(2200));
        assert!(!dir.path().join("marqueur").exists(), "processus non tué");
    }

    /// L'annulation interrompt l'attente sans attendre la fin du processus.
    #[cfg(unix)]
    #[test]
    fn version_check_stops_on_cancellation() {
        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("llama-server");
        std::fs::write(&exe, b"#!/bin/sh\nsleep 5\n").unwrap();
        set_executable(&exe).unwrap();
        let cancel = CancellationToken::new();
        let trigger = cancel.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(150));
            trigger.cancel();
        });
        let started = Instant::now();
        let err =
            run_version_check_with_timeout(&exe, VERSION_CHECK_TIMEOUT, Some(&cancel)).unwrap_err();
        assert!(matches!(err, VersionCheckFailure::Cancelled), "{err:?}");
        let waited = started.elapsed();
        assert!(waited < Duration::from_millis(1500), "{waited:?}");
    }

    /// Serveur HTTP local qui sert `body` en entier (une requête par connexion).
    fn serve_once(body: Vec<u8>) -> String {
        serve_counting(body).0
    }

    /// Même serveur, avec le compteur de requêtes servies : de quoi vérifier
    /// qu'une archive corrompue n'est retéléchargée qu'une seule fois.
    fn serve_counting(body: Vec<u8>) -> (String, Arc<AtomicUsize>) {
        use std::io::{BufRead, BufReader, Write};
        let requests = Arc::new(AtomicUsize::new(0));
        let served = Arc::clone(&requests);
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/asset", listener.local_addr().unwrap());
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { break };
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut line = String::new();
                while reader.read_line(&mut line).unwrap_or(0) > 0 && line != "\r\n" {
                    line.clear();
                }
                served.fetch_add(1, Ordering::SeqCst);
                let _ = write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = stream.write_all(&body);
            }
        });
        (url, requests)
    }

    fn leak(s: String) -> &'static str {
        Box::leak(s.into_boxed_str())
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn ensure_engine_downloads_extracts_and_checks_version() {
        let dir = tempfile::tempdir().unwrap();
        let archive = dir.path().join("src.tar.gz");
        write_tar_gz(
            &archive,
            "llama-b10930",
            &[
                ("llama-server", FAKE_SERVER, 0o755),
                ("libllama.dylib", b"lib", 0o644),
            ],
        );
        let bytes = std::fs::read(&archive).unwrap();
        let sha = crate::download::compute_sha256(&archive).unwrap();
        let asset = EngineAsset {
            file_name: "llama-b10930-bin-test.tar.gz",
            url: leak(serve_once(bytes)),
            sha256: leak(sha),
            size_bytes: 0,
            kind: ArchiveKind::TarGz,
            exe_name: "llama-server",
        };
        let paths = SummaryPaths::new(&dir.path().join("data"));
        let client = crate::download::build_client().unwrap();
        let mut progress = Vec::new();
        let exe = ensure_engine_from(&asset, &paths, &client, &CancellationToken::new(), |p| {
            progress.push(p)
        })
        .await
        .unwrap();
        assert_eq!(exe, paths.engine_dir().join("llama-server"));
        assert!(exe.is_file());
        assert_eq!(progress.last(), Some(&100));
        assert!(
            !paths
                .bin_dir
                .join("llama-b10930-bin-test.tar.gz.part")
                .exists(),
            "archive supprimée après extraction"
        );
        // Second appel : déjà installé, aucun téléchargement (le serveur ne
        // répondrait plus qu'une fois de toute façon).
        let again = ensure_engine_from(&asset, &paths, &client, &CancellationToken::new(), |_| {
            panic!("pas de téléchargement")
        })
        .await
        .unwrap();
        assert_eq!(again, exe);
    }

    #[tokio::test]
    async fn ensure_engine_reports_checksum_mismatch_after_one_retry() {
        let dir = tempfile::tempdir().unwrap();
        let (url, requests) = serve_counting(b"corrupt".to_vec());
        let asset = EngineAsset {
            file_name: "llama-b10930-bin-test.tar.gz",
            url: leak(url),
            sha256: "0000000000000000000000000000000000000000000000000000000000000000",
            size_bytes: 0,
            kind: ArchiveKind::TarGz,
            exe_name: "llama-server",
        };
        let paths = SummaryPaths::new(&dir.path().join("data"));
        let client = crate::download::build_client().unwrap();
        let err = ensure_engine_from(&asset, &paths, &client, &CancellationToken::new(), |_| {})
            .await
            .unwrap_err();
        assert_eq!(err, SummaryError::ChecksumMismatch);
        assert_eq!(
            requests.load(Ordering::SeqCst),
            2,
            "un seul retéléchargement"
        );
    }

    /// Archive intacte au SHA-256 mais illisible : supprimée, retéléchargée une
    /// seule fois, puis abandon avec le détail de l'extraction.
    #[tokio::test]
    async fn ensure_engine_redownloads_once_when_extraction_fails() {
        let dir = tempfile::tempdir().unwrap();
        let body = b"ce n'est pas une archive".to_vec();
        let sha = format!("{:x}", <sha2::Sha256 as sha2::Digest>::digest(&body));
        let (url, requests) = serve_counting(body);
        let asset = EngineAsset {
            file_name: "llama-b10930-bin-test.tar.gz",
            url: leak(url),
            sha256: leak(sha),
            size_bytes: 0,
            kind: ArchiveKind::TarGz,
            exe_name: "llama-server",
        };
        let paths = SummaryPaths::new(&dir.path().join("data"));
        let client = crate::download::build_client().unwrap();
        let err = ensure_engine_from(&asset, &paths, &client, &CancellationToken::new(), |_| {})
            .await
            .unwrap_err();
        match err {
            SummaryError::EngineStartFailed { detail } => {
                assert!(detail.contains("tar.gz"), "{detail}")
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(
            requests.load(Ordering::SeqCst),
            2,
            "un seul retéléchargement"
        );
        assert!(!paths
            .bin_dir
            .join("llama-b10930-bin-test.tar.gz.part")
            .exists());
        assert!(!paths.engine_dir().exists());
    }

    /// `--version` qui échoue : l'erreur porte le code de sortie et stderr, et
    /// l'archive n'est pas retéléchargée (elle est intacte).
    #[cfg(unix)]
    #[tokio::test]
    async fn ensure_engine_reports_a_failing_version_check() {
        let dir = tempfile::tempdir().unwrap();
        let archive = dir.path().join("src.tar.gz");
        write_tar_gz(
            &archive,
            "llama-b10930",
            &[(
                "llama-server",
                b"#!/bin/sh\necho 'dyld: libggml introuvable' >&2\nexit 3\n",
                0o755,
            )],
        );
        let bytes = std::fs::read(&archive).unwrap();
        let sha = crate::download::compute_sha256(&archive).unwrap();
        let (url, requests) = serve_counting(bytes);
        let asset = EngineAsset {
            file_name: "llama-b10930-bin-test.tar.gz",
            url: leak(url),
            sha256: leak(sha),
            size_bytes: 0,
            kind: ArchiveKind::TarGz,
            exe_name: "llama-server",
        };
        let paths = SummaryPaths::new(&dir.path().join("data"));
        let client = crate::download::build_client().unwrap();
        let err = ensure_engine_from(&asset, &paths, &client, &CancellationToken::new(), |_| {})
            .await
            .unwrap_err();
        match err {
            SummaryError::EngineStartFailed { detail } => assert!(
                detail.contains("--version") && detail.contains("3") && detail.contains("libggml"),
                "{detail}"
            ),
            other => panic!("{other:?}"),
        }
        assert_eq!(requests.load(Ordering::SeqCst), 1);
    }

    /// Un moteur qui échoue au contrôle `--version` est désinstallé : sans
    /// cela le court-circuit `exe.is_file()` le reprendrait tel quel à chaque
    /// lancement, sans jamais le retélécharger.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_failing_version_check_removes_the_engine_and_reinstalls_it() {
        let dir = tempfile::tempdir().unwrap();
        let archive = dir.path().join("src.tar.gz");
        write_tar_gz(
            &archive,
            "llama-b10930",
            &[("llama-server", b"#!/bin/sh\nexit 1\n", 0o755)],
        );
        let bytes = std::fs::read(&archive).unwrap();
        let sha = crate::download::compute_sha256(&archive).unwrap();
        let (url, requests) = serve_counting(bytes);
        let asset = EngineAsset {
            file_name: "llama-b10930-bin-test.tar.gz",
            url: leak(url),
            sha256: leak(sha),
            size_bytes: 0,
            kind: ArchiveKind::TarGz,
            exe_name: "llama-server",
        };
        let paths = SummaryPaths::new(&dir.path().join("data"));
        let client = crate::download::build_client().unwrap();
        let err = ensure_engine_from(&asset, &paths, &client, &CancellationToken::new(), |_| {})
            .await
            .unwrap_err();
        assert!(
            matches!(err, SummaryError::EngineStartFailed { .. }),
            "{err:?}"
        );
        assert!(
            !paths.engine_dir().exists(),
            "moteur inutilisable laissé en place"
        );
        // Second lancement : plus rien sur le disque, le moteur est retéléchargé.
        let err = ensure_engine_from(&asset, &paths, &client, &CancellationToken::new(), |_| {})
            .await
            .unwrap_err();
        assert!(
            matches!(err, SummaryError::EngineStartFailed { .. }),
            "{err:?}"
        );
        assert_eq!(
            requests.load(Ordering::SeqCst),
            2,
            "le moteur n'a pas été retéléchargé"
        );
    }

    /// Annulation entre deux phases : l'archive déjà sur le disque est validée
    /// par son empreinte (sans la moindre requête), mais l'installation
    /// s'arrête avant l'extraction, sans jeter le téléchargement.
    #[tokio::test]
    async fn ensure_engine_stops_when_cancelled_after_the_download() {
        let dir = tempfile::tempdir().unwrap();
        let archive = dir.path().join("src.tar.gz");
        write_tar_gz(
            &archive,
            "llama-b10930",
            &[("llama-server", FAKE_SERVER, 0o755)],
        );
        let sha = crate::download::compute_sha256(&archive).unwrap();
        let asset = EngineAsset {
            file_name: "llama-b10930-bin-test.tar.gz",
            url: "http://127.0.0.1:1/asset",
            sha256: leak(sha),
            size_bytes: 0,
            kind: ArchiveKind::TarGz,
            exe_name: "llama-server",
        };
        let paths = SummaryPaths::new(&dir.path().join("data"));
        std::fs::create_dir_all(&paths.bin_dir).unwrap();
        let part = paths.bin_dir.join("llama-b10930-bin-test.tar.gz.part");
        std::fs::copy(&archive, &part).unwrap();
        let cancel = CancellationToken::new();
        cancel.cancel();
        let client = crate::download::build_client().unwrap();
        let err = ensure_engine_from(&asset, &paths, &client, &cancel, |_| {})
            .await
            .unwrap_err();
        assert_eq!(err, SummaryError::Cancelled);
        assert!(!paths.engine_dir().exists());
        assert!(part.is_file(), "téléchargement conservé pour la reprise");
    }

    #[tokio::test]
    async fn ensure_model_offline_when_server_unreachable() {
        let dir = tempfile::tempdir().unwrap();
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/model.gguf", listener.local_addr().unwrap());
        drop(listener);
        let model = ModelAsset {
            file_name: "m.gguf",
            url: leak(url),
            sha256: "00",
            size_bytes: 1,
            model_name: "m",
            server_args: &[],
        };
        let paths = SummaryPaths::new(&dir.path().join("data"));
        let client = crate::download::build_client().unwrap();
        let err = ensure_model_from(&model, &paths, &client, &CancellationToken::new(), |_| {})
            .await
            .unwrap_err();
        assert_eq!(err, SummaryError::Offline);
    }

    #[tokio::test]
    async fn ensure_model_downloads_and_verifies() {
        let dir = tempfile::tempdir().unwrap();
        let body = b"GGUF-fake-model".to_vec();
        let sha = format!("{:x}", <sha2::Sha256 as sha2::Digest>::digest(&body));
        let model = ModelAsset {
            file_name: "m.gguf",
            url: leak(serve_once(body.clone())),
            sha256: leak(sha),
            size_bytes: 15,
            model_name: "m",
            server_args: &[],
        };
        let paths = SummaryPaths::new(&dir.path().join("data"));
        let client = crate::download::build_client().unwrap();
        let path = ensure_model_from(&model, &paths, &client, &CancellationToken::new(), |_| {})
            .await
            .unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), body);
        assert_eq!(path, paths.models_dir.join("m.gguf"));
    }

    /// Avec la vraie archive de la plateforme, extrait et exécute réellement
    /// `llama-server --version`. Exécution manuelle :
    /// `OCADE_LLAMA_ARCHIVE=<chemin> cargo test --lib summary::install::tests::real_archive -- --ignored --nocapture`.
    #[test]
    #[ignore = "archive réelle du moteur (OCADE_LLAMA_ARCHIVE) : exécution manuelle"]
    fn real_archive_extracts_and_runs_when_provided() {
        let Ok(archive) = std::env::var("OCADE_LLAMA_ARCHIVE") else {
            eprintln!("sauté : OCADE_LLAMA_ARCHIVE non défini");
            return;
        };
        let asset = engine_asset().expect("plateforme prise en charge");
        let dir = tempfile::tempdir().unwrap();
        let final_dir = dir.path().join("llama-b10930");
        extract_engine_archive(Path::new(&archive), asset.kind, &final_dir).unwrap();
        let exe = final_dir.join(asset.exe_name);
        set_executable(&exe).unwrap();
        run_version_check(&exe).unwrap();
    }

    #[test]
    fn paths_follow_the_layout() {
        let paths = SummaryPaths::new(Path::new("/data"));
        assert_eq!(paths.engine_dir(), Path::new("/data/bin/llama-b10930"));
        assert_eq!(
            paths.model_file(),
            Path::new("/data/models/summary/Ministral-3-3B-Instruct-2512-Q4_K_M.gguf")
        );
        assert_eq!(paths.pid_file(), Path::new("/data/bin/llama/server.pid"));
    }
}
