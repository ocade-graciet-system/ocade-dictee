//! Téléchargement HTTP avec reprise (`Range`), vérification de taille et de
//! SHA-256. Partagé par le modèle de dictée (`managers::model`), le moteur de
//! résumé et le modèle de résumé (`summary::install`).
//!
//! Le fichier en cours s'écrit dans `part` ; une fois complet et vérifié il
//! reste à cet emplacement (`download_to_part`) ou est renommé vers sa
//! destination finale (`download_with_resume`). Un serveur qui ignore `Range`
//! (200 au lieu de 206) ou refuse la reprise (416) fait repartir de zéro.

use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use futures_util::StreamExt;
use reqwest::header::RANGE;
use reqwest::StatusCode;
use sha2::{Digest, Sha256};
use tokio_util::sync::CancellationToken;

/// Suffixe du fichier temporaire d'un téléchargement en cours.
pub const PART_SUFFIX: &str = ".part";

/// Étapes remontées à l'appelant pendant un téléchargement.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DownloadEvent {
    /// Octets reçus (cumulés, reprise comprise) et total attendu (0 si inconnu).
    Progress { downloaded: u64, total: u64 },
    /// Le fichier est complet, le SHA-256 est en cours de calcul.
    Verifying,
}

#[derive(Debug)]
pub enum DownloadError {
    /// La requête n'a même pas pu partir (pas de réseau, DNS, connexion
    /// refusée, délai de connexion) : rien n'a été reçu.
    Connect(String),
    /// Réponse HTTP inattendue, flux interrompu ou taille incohérente. Le
    /// fichier partiel est conservé pour une reprise ultérieure.
    Failed(String),
    /// Le fichier complet ne correspond pas au SHA-256 attendu ; il a été
    /// supprimé pour que la prochaine tentative reparte de zéro.
    ChecksumMismatch {
        expected: String,
        actual: String,
    },
    /// Annulé via le jeton ; le fichier partiel est conservé.
    Cancelled,
    Io(std::io::Error),
}

impl std::fmt::Display for DownloadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DownloadError::Connect(e) => write!(f, "connexion impossible: {e}"),
            DownloadError::Failed(e) => write!(f, "téléchargement échoué: {e}"),
            DownloadError::ChecksumMismatch { expected, actual } => {
                write!(f, "SHA-256 inattendu (attendu {expected}, obtenu {actual})")
            }
            DownloadError::Cancelled => write!(f, "téléchargement annulé"),
            DownloadError::Io(e) => write!(f, "erreur disque: {e}"),
        }
    }
}

impl std::error::Error for DownloadError {}

impl From<std::io::Error> for DownloadError {
    fn from(e: std::io::Error) -> Self {
        DownloadError::Io(e)
    }
}

/// Chemin du fichier temporaire associé à `dest` (`<dest>.part`).
pub fn part_path(dest: &Path) -> PathBuf {
    let mut name = dest.as_os_str().to_os_string();
    name.push(PART_SUFFIX);
    PathBuf::from(name)
}

/// Client HTTP adapté aux gros fichiers : délai de connexion, délai
/// d'inactivité *par lecture* (un débit faible mais continu passe), aucun
/// délai global (un fichier de 2 Go légitimement lent ne doit pas être coupé).
pub fn build_client() -> Result<reqwest::Client, reqwest::Error> {
    reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(30))
        .read_timeout(Duration::from_secs(60))
        .build()
}

/// SHA-256 hexadécimal (minuscules) d'un fichier, lu par blocs de 64 Ko.
pub fn compute_sha256(path: &Path) -> std::io::Result<String> {
    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 65536];
    loop {
        let n = file.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        hasher.update(&buffer[..n]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

async fn compute_sha256_blocking(path: PathBuf) -> Result<String, DownloadError> {
    tokio::task::spawn_blocking(move || compute_sha256(&path))
        .await
        .map_err(|e| DownloadError::Failed(format!("calcul du SHA-256 interrompu: {e}")))?
        .map_err(DownloadError::Io)
}

async fn send_request(
    client: &reqwest::Client,
    url: &str,
    resume_from: u64,
) -> Result<reqwest::Response, DownloadError> {
    let mut request = client.get(url);
    if resume_from > 0 {
        request = request.header(RANGE, format!("bytes={resume_from}-"));
    }
    request
        .send()
        .await
        .map_err(|e| DownloadError::Connect(e.to_string()))
}

/// Télécharge `url` dans `part` (reprise si `part` existe déjà), vérifie le
/// SHA-256 si fourni, et laisse le fichier complet à `part`. L'appelant
/// décide ensuite (renommage, extraction…).
pub async fn download_to_part(
    client: &reqwest::Client,
    url: &str,
    part: &Path,
    expected_sha256: Option<&str>,
    cancel: &CancellationToken,
    mut on_event: impl FnMut(DownloadEvent),
) -> Result<(), DownloadError> {
    if let Some(parent) = part.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let mut resume_from = std::fs::metadata(part).map(|m| m.len()).unwrap_or(0);
    let mut response = send_request(client, url, resume_from).await?;

    // Serveur sans reprise (200 au lieu de 206) ou fichier partiel plus grand
    // que la ressource (416) : on repart de zéro, une seule fois.
    if resume_from > 0
        && (response.status() == StatusCode::OK
            || response.status() == StatusCode::RANGE_NOT_SATISFIABLE)
    {
        drop(response);
        let _ = std::fs::remove_file(part);
        resume_from = 0;
        response = send_request(client, url, 0).await?;
    }

    let status = response.status();
    if !status.is_success() {
        return Err(DownloadError::Failed(format!("HTTP {status}")));
    }

    let total = resume_from + response.content_length().unwrap_or(0);
    let mut file = if resume_from > 0 {
        OpenOptions::new().append(true).open(part)?
    } else {
        File::create(part)?
    };
    let mut downloaded = resume_from;
    on_event(DownloadEvent::Progress { downloaded, total });

    let mut last_emit = Instant::now();
    let mut stream = response.bytes_stream();
    loop {
        let next = tokio::select! {
            _ = cancel.cancelled() => return Err(DownloadError::Cancelled),
            chunk = stream.next() => chunk,
        };
        let Some(chunk) = next else { break };
        let chunk = chunk.map_err(|e| DownloadError::Failed(format!("flux interrompu: {e}")))?;
        file.write_all(&chunk)?;
        downloaded += chunk.len() as u64;
        if last_emit.elapsed() >= Duration::from_millis(100) {
            on_event(DownloadEvent::Progress { downloaded, total });
            last_emit = Instant::now();
        }
    }
    file.flush()?;
    drop(file);
    on_event(DownloadEvent::Progress {
        downloaded,
        total: total.max(downloaded),
    });

    if total > 0 && downloaded != total {
        return Err(DownloadError::Failed(format!(
            "taille inattendue: {downloaded} octets reçus sur {total}"
        )));
    }

    if let Some(expected) = expected_sha256 {
        on_event(DownloadEvent::Verifying);
        let actual = compute_sha256_blocking(part.to_path_buf()).await?;
        if !actual.eq_ignore_ascii_case(expected) {
            let _ = std::fs::remove_file(part);
            return Err(DownloadError::ChecksumMismatch {
                expected: expected.to_lowercase(),
                actual,
            });
        }
    }
    Ok(())
}

/// Télécharge `url` vers `dest` via `<dest>.part`, puis renomme. Si `dest`
/// existe déjà et correspond au SHA-256 attendu, ne fait rien.
pub async fn download_with_resume(
    client: &reqwest::Client,
    url: &str,
    dest: &Path,
    expected_sha256: Option<&str>,
    cancel: &CancellationToken,
    on_event: impl FnMut(DownloadEvent),
) -> Result<(), DownloadError> {
    if dest.is_file() {
        match expected_sha256 {
            None => return Ok(()),
            Some(expected) => {
                let actual = compute_sha256_blocking(dest.to_path_buf()).await?;
                if actual.eq_ignore_ascii_case(expected) {
                    return Ok(());
                }
                let _ = std::fs::remove_file(dest);
            }
        }
    }
    let part = part_path(dest);
    download_to_part(client, url, &part, expected_sha256, cancel, on_event).await?;
    std::fs::rename(&part, dest)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader};
    use std::net::{TcpListener, TcpStream};
    use std::sync::{Arc, Mutex};

    /// Comportement scripté du serveur factice pour une requête donnée.
    #[derive(Clone, Copy)]
    enum Behaviour {
        /// Honore `Range` (206) ; sans `Range`, envoie tout (200).
        Normal,
        /// Ignore `Range` : renvoie toujours 200 et le fichier entier.
        IgnoreRange,
        /// Répond 416 à toute requête avec `Range`.
        RejectRange,
        /// Envoie `n` octets puis coupe la connexion.
        TruncateAfter(usize),
        /// Envoie le corps par blocs de 16 octets espacés de 50 ms.
        Slow,
    }

    struct TestServer {
        url: String,
        ranges_seen: Arc<Mutex<Vec<Option<String>>>>,
    }

    fn read_request(stream: &mut TcpStream) -> Option<String> {
        let mut reader = BufReader::new(stream.try_clone().ok()?);
        let mut range = None;
        let mut line = String::new();
        loop {
            line.clear();
            if reader.read_line(&mut line).ok()? == 0 || line == "\r\n" {
                break;
            }
            if let Some(value) = line.to_ascii_lowercase().strip_prefix("range:") {
                range = Some(value.trim().to_string());
            }
        }
        Some(range.unwrap_or_default())
    }

    fn parse_start(range: &str) -> Option<u64> {
        range
            .strip_prefix("bytes=")?
            .trim_end_matches('-')
            .parse()
            .ok()
    }

    fn start_server(body: Vec<u8>, behaviours: Vec<Behaviour>) -> TestServer {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/file.bin", listener.local_addr().unwrap());
        let ranges_seen = Arc::new(Mutex::new(Vec::new()));
        let seen = ranges_seen.clone();
        std::thread::spawn(move || {
            let mut behaviours = behaviours.into_iter();
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { break };
                let behaviour = behaviours.next().unwrap_or(Behaviour::Normal);
                let range = read_request(&mut stream).unwrap_or_default();
                let start = parse_start(&range);
                seen.lock()
                    .unwrap()
                    .push(if range.is_empty() { None } else { Some(range) });
                let total = body.len() as u64;
                let (status, from) = match (behaviour, start) {
                    (Behaviour::RejectRange, Some(_)) => {
                        let head = format!(
                            "HTTP/1.1 416 Range Not Satisfiable\r\nContent-Range: bytes */{total}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                        );
                        let _ = stream.write_all(head.as_bytes());
                        continue;
                    }
                    (Behaviour::IgnoreRange, _) | (_, None) => ("200 OK", 0u64),
                    (_, Some(s)) => ("206 Partial Content", s),
                };
                let payload = &body[from as usize..];
                let mut head = format!(
                    "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n",
                    payload.len()
                );
                if from > 0 {
                    head.push_str(&format!(
                        "Content-Range: bytes {from}-{}/{total}\r\n",
                        total - 1
                    ));
                }
                head.push_str("\r\n");
                let _ = stream.write_all(head.as_bytes());
                match behaviour {
                    Behaviour::TruncateAfter(n) => {
                        let _ = stream.write_all(&payload[..n.min(payload.len())]);
                        let _ = stream.shutdown(std::net::Shutdown::Both);
                    }
                    Behaviour::Slow => {
                        for block in payload.chunks(16) {
                            if stream.write_all(block).is_err() {
                                break;
                            }
                            let _ = stream.flush();
                            std::thread::sleep(Duration::from_millis(50));
                        }
                    }
                    _ => {
                        let _ = stream.write_all(payload);
                    }
                }
            }
        });
        TestServer { url, ranges_seen }
    }

    fn body() -> Vec<u8> {
        (0..4000u32).map(|i| (i % 251) as u8).collect()
    }

    fn sha_hex(data: &[u8]) -> String {
        format!("{:x}", Sha256::digest(data))
    }

    #[tokio::test]
    async fn downloads_fresh_file_and_verifies_sha256() {
        let data = body();
        let server = start_server(data.clone(), vec![Behaviour::Normal]);
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("file.bin");
        let client = build_client().unwrap();
        let mut events = Vec::new();
        download_with_resume(
            &client,
            &server.url,
            &dest,
            Some(&sha_hex(&data)),
            &CancellationToken::new(),
            |e| events.push(e),
        )
        .await
        .unwrap();
        assert_eq!(std::fs::read(&dest).unwrap(), data);
        assert!(!part_path(&dest).exists());
        assert!(events.contains(&DownloadEvent::Verifying));
        assert_eq!(events.last(), Some(&DownloadEvent::Verifying));
        assert!(matches!(
            events[0],
            DownloadEvent::Progress {
                downloaded: 0,
                total: 4000
            }
        ));
        assert_eq!(server.ranges_seen.lock().unwrap().as_slice(), &[None]);
    }

    #[tokio::test]
    async fn resumes_partial_file_with_range() {
        let data = body();
        let server = start_server(data.clone(), vec![Behaviour::Normal]);
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("file.bin");
        std::fs::write(part_path(&dest), &data[..1500]).unwrap();
        let client = build_client().unwrap();
        download_with_resume(
            &client,
            &server.url,
            &dest,
            Some(&sha_hex(&data)),
            &CancellationToken::new(),
            |_| {},
        )
        .await
        .unwrap();
        assert_eq!(std::fs::read(&dest).unwrap(), data);
        assert_eq!(
            server.ranges_seen.lock().unwrap().as_slice(),
            &[Some("bytes=1500-".to_string())]
        );
    }

    #[tokio::test]
    async fn restarts_when_server_ignores_range() {
        let data = body();
        let server = start_server(
            data.clone(),
            vec![Behaviour::IgnoreRange, Behaviour::IgnoreRange],
        );
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("file.bin");
        std::fs::write(part_path(&dest), b"garbage-garbage").unwrap();
        let client = build_client().unwrap();
        download_with_resume(
            &client,
            &server.url,
            &dest,
            Some(&sha_hex(&data)),
            &CancellationToken::new(),
            |_| {},
        )
        .await
        .unwrap();
        assert_eq!(std::fs::read(&dest).unwrap(), data);
        let seen = server.ranges_seen.lock().unwrap();
        assert_eq!(seen.len(), 2, "une requête Range puis une requête complète");
        assert!(seen[0].is_some() && seen[1].is_none());
    }

    #[tokio::test]
    async fn restarts_on_416() {
        let data = body();
        let server = start_server(
            data.clone(),
            vec![Behaviour::RejectRange, Behaviour::Normal],
        );
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("file.bin");
        std::fs::write(part_path(&dest), vec![0u8; 5000]).unwrap();
        let client = build_client().unwrap();
        download_with_resume(
            &client,
            &server.url,
            &dest,
            Some(&sha_hex(&data)),
            &CancellationToken::new(),
            |_| {},
        )
        .await
        .unwrap();
        assert_eq!(std::fs::read(&dest).unwrap(), data);
        assert_eq!(server.ranges_seen.lock().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn checksum_mismatch_removes_partial_file() {
        let data = body();
        let server = start_server(data.clone(), vec![Behaviour::Normal]);
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("file.bin");
        let client = build_client().unwrap();
        let err = download_with_resume(
            &client,
            &server.url,
            &dest,
            Some("00ff"),
            &CancellationToken::new(),
            |_| {},
        )
        .await
        .unwrap_err();
        assert!(
            matches!(err, DownloadError::ChecksumMismatch { .. }),
            "{err}"
        );
        assert!(!part_path(&dest).exists());
        assert!(!dest.exists());
    }

    #[tokio::test]
    async fn truncated_stream_keeps_partial_then_resumes() {
        let data = body();
        let server = start_server(
            data.clone(),
            vec![Behaviour::TruncateAfter(1000), Behaviour::Normal],
        );
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("file.bin");
        let client = build_client().unwrap();
        let err = download_with_resume(
            &client,
            &server.url,
            &dest,
            Some(&sha_hex(&data)),
            &CancellationToken::new(),
            |_| {},
        )
        .await
        .unwrap_err();
        assert!(matches!(err, DownloadError::Failed(_)), "{err}");
        let partial = std::fs::metadata(part_path(&dest)).unwrap().len();
        assert!(partial > 0 && partial < 4000, "partiel conservé: {partial}");
        download_with_resume(
            &client,
            &server.url,
            &dest,
            Some(&sha_hex(&data)),
            &CancellationToken::new(),
            |_| {},
        )
        .await
        .unwrap();
        assert_eq!(std::fs::read(&dest).unwrap(), data);
        assert_eq!(
            server.ranges_seen.lock().unwrap()[1],
            Some(format!("bytes={partial}-"))
        );
    }

    #[tokio::test]
    async fn cancel_keeps_partial_file() {
        let data = body();
        let server = start_server(data.clone(), vec![Behaviour::Slow]);
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("file.bin");
        let client = build_client().unwrap();
        let cancel = CancellationToken::new();
        let cancel_after = cancel.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(200)).await;
            cancel_after.cancel();
        });
        let err = download_with_resume(&client, &server.url, &dest, None, &cancel, |_| {})
            .await
            .unwrap_err();
        assert!(matches!(err, DownloadError::Cancelled), "{err}");
        assert!(part_path(&dest).exists());
        assert!(!dest.exists());
    }

    #[tokio::test]
    async fn unreachable_server_is_a_connect_error() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/file.bin", listener.local_addr().unwrap());
        drop(listener);
        let dir = tempfile::tempdir().unwrap();
        let client = build_client().unwrap();
        let err = download_with_resume(
            &client,
            &url,
            &dir.path().join("f"),
            None,
            &CancellationToken::new(),
            |_| {},
        )
        .await
        .unwrap_err();
        assert!(matches!(err, DownloadError::Connect(_)), "{err}");
    }
}
