//! Installation du moteur (`llama-server` + bibliothèques) et du modèle GGUF
//! dans les données de l'app, avec reprise, SHA-256, extraction et contrôle
//! `llama-server --version`.

use std::fs::File;
use std::path::{Path, PathBuf};

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
    for mut dir in directories {
        dir.unpack_in(temp_dir).map_err(fail)?;
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

/// Contrôle final : `llama-server --version` doit renvoyer 0 (bibliothèques
/// trouvées, binaire exécutable, pas de quarantaine).
pub fn run_version_check(exe: &Path) -> Result<(), String> {
    let mut command = std::process::Command::new(exe);
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
    let output = command
        .output()
        .map_err(|e| format!("lancement de {}: {e}", exe.display()))?;
    if output.status.success() {
        Ok(())
    } else {
        Err(format!(
            "`llama-server --version` a renvoyé {:?}: {}",
            output.status.code(),
            String::from_utf8_lossy(&output.stderr).trim()
        ))
    }
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

fn ensure_disk_space(dir: &Path) -> Result<(), SummaryError> {
    if let Some(free) = free_disk_bytes(dir) {
        check_disk_space(free)?;
    }
    Ok(())
}

/// Installe le moteur si besoin et renvoie le chemin de `llama-server`.
pub async fn ensure_engine(
    paths: &SummaryPaths,
    client: &reqwest::Client,
    cancel: &CancellationToken,
    on_progress: impl FnMut(u32),
) -> Result<PathBuf, SummaryError> {
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
    let exe = paths.engine_dir().join(asset.exe_name);
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
        match extract_engine_archive(&part, asset.kind, &paths.engine_dir()) {
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
    let _ = std::fs::remove_file(&part);
    if !exe.is_file() {
        return Err(SummaryError::EngineStartFailed {
            detail: format!("{} absent de l'archive", asset.exe_name),
        });
    }
    set_executable(&exe).map_err(|e| SummaryError::EngineStartFailed {
        detail: e.to_string(),
    })?;
    run_version_check(&exe).map_err(|detail| SummaryError::EngineStartFailed { detail })?;
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

    /// Serveur HTTP local qui sert `body` en entier (une requête par connexion).
    fn serve_once(body: Vec<u8>) -> String {
        use std::io::{BufRead, BufReader, Write};
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
                let _ = write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = stream.write_all(&body);
            }
        });
        url
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
        let asset = EngineAsset {
            file_name: "llama-b10930-bin-test.tar.gz",
            url: leak(serve_once(b"corrupt".to_vec())),
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

    /// Avec la vraie archive de la plateforme (OCADE_LLAMA_ARCHIVE=<chemin>),
    /// extrait et exécute réellement `llama-server --version`. Silencieux sinon.
    #[test]
    fn real_archive_extracts_and_runs_when_provided() {
        let Ok(archive) = std::env::var("OCADE_LLAMA_ARCHIVE") else {
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
