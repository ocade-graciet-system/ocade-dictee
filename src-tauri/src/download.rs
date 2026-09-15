//! Téléchargement HTTP avec reprise (`Range`), vérification de taille et de
//! SHA-256. Partagé par le modèle de dictée (`managers::model`), le moteur de
//! résumé et le modèle de résumé (`summary::install`).
//!
//! Le fichier en cours s'écrit dans `part` ; une fois complet et vérifié il
//! reste à cet emplacement (`download_to_part`) ou est renommé vers sa
//! destination finale (`download_with_resume`). Un serveur qui ignore `Range`
//! (200 au lieu de 206), refuse la reprise (416) ou répond 206 à partir d'un
//! autre décalage que celui demandé fait repartir de zéro.
//!
//! **Les appelants passent toujours un SHA-256 attendu** (`managers::model`,
//! `summary::install`) : les en-têtes du serveur ne sont que des indications
//! (le total peut être absent, le `Content-Range` mensonger), et seule cette
//! empreinte garantit au final l'intégrité du fichier livré. Un `.part` déjà
//! complet est d'ailleurs reconnu par son empreinte, sans aucune requête.

use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use futures_util::StreamExt;
use reqwest::header::{CONTENT_RANGE, RANGE};
use reqwest::StatusCode;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use specta::Type;
use tokio_util::sync::CancellationToken;

/// Suffixe du fichier temporaire d'un téléchargement en cours.
pub const PART_SUFFIX: &str = ".part";

/// Étapes remontées à l'appelant pendant un téléchargement.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DownloadEvent {
    /// Octets reçus (cumulés, reprise comprise) et total attendu. Quand le
    /// serveur n'annonce aucune taille, `total` vaut `downloaded` : la barre
    /// avance sans jamais se remplir, plutôt que d'afficher 0.
    Progress { downloaded: u64, total: u64 },
    /// Le fichier est complet, le SHA-256 est en cours de calcul.
    Verifying,
}

/// Nature d'une panne réseau, déduite de l'erreur `reqwest` et de sa chaîne de
/// causes. Tout mettre dans un seul panier « hors ligne » envoyait l'utilisateur
/// vérifier une connexion qui marchait très bien : chaque cas a sa conduite à
/// tenir, et le front lui associe son message
/// (`settings.file.summary.errors.network.*`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum NetworkFailure {
    /// Aucune route ne sort de la machine : interface coupée, Wi-Fi éteint,
    /// mode avion. C'est le seul cas où « connectez-vous à Internet » est le
    /// bon conseil.
    Offline,
    /// Le nom du serveur n'a pas pu être résolu. Sur Windows, une machine
    /// réellement hors ligne produit la même signature qu'un DNS filtré : le
    /// message couvre donc les deux pistes.
    DnsFailed,
    /// Connexion refusée, réinitialisée ou coupée avant la réponse : pare-feu,
    /// proxy d'entreprise ou antivirus filtrant, typiquement.
    Blocked,
    /// Poignée de main TLS rejetée (certificat non reconnu) : signature d'un
    /// antivirus ou d'un proxy qui inspecte le trafic chiffré.
    TlsRejected,
    /// Délai de connexion ou de lecture dépassé : lien très lent, ou paquets
    /// avalés sans réponse par un filtrage.
    Timeout,
    /// Échec réseau dont la chaîne de causes ne dit rien d'exploitable : seul
    /// le détail technique permet d'avancer.
    Unknown,
}

impl NetworkFailure {
    /// Étiquette française courte, pour le journal et `Display`.
    pub fn label(self) -> &'static str {
        match self {
            NetworkFailure::Offline => "aucun réseau",
            NetworkFailure::DnsFailed => "nom du serveur non résolu (DNS)",
            NetworkFailure::Blocked => "connexion refusée ou interrompue",
            NetworkFailure::TlsRejected => "connexion sécurisée rejetée (certificat)",
            NetworkFailure::Timeout => "délai dépassé",
            NetworkFailure::Unknown => "échec de connexion",
        }
    }

    /// Plus le signal est spécifique, plus il prime quand plusieurs maillons
    /// de la chaîne parlent (« aucune route » l'emporte sur « DNS », qui n'en
    /// est alors que la conséquence).
    fn precedence(self) -> u8 {
        match self {
            NetworkFailure::Offline => 5,
            NetworkFailure::DnsFailed => 4,
            NetworkFailure::TlsRejected => 3,
            NetworkFailure::Blocked => 2,
            NetworkFailure::Timeout => 1,
            NetworkFailure::Unknown => 0,
        }
    }
}

/// Signature système d'une cause, indépendante de la langue : sur un Windows
/// français les messages d'erreur sont traduits, seuls `ErrorKind` et le code
/// brut restent lisibles.
fn classify_io_error(error: &std::io::Error) -> Option<NetworkFailure> {
    use std::io::ErrorKind;
    // Codes Winsock de résolution de nom, auxquels `std` n'associe aucun
    // `ErrorKind` : 11001 WSAHOST_NOT_FOUND, 11002 WSATRY_AGAIN,
    // 11003 WSANO_RECOVERY, 11004 WSANO_DATA. Aucun errno Unix n'atteint ces
    // valeurs, la garde est donc sans risque sur les autres plateformes.
    if matches!(error.raw_os_error(), Some(11001..=11004)) {
        return Some(NetworkFailure::DnsFailed);
    }
    match error.kind() {
        ErrorKind::NetworkUnreachable | ErrorKind::NetworkDown | ErrorKind::HostUnreachable => {
            Some(NetworkFailure::Offline)
        }
        ErrorKind::ConnectionRefused
        | ErrorKind::ConnectionReset
        | ErrorKind::ConnectionAborted
        | ErrorKind::BrokenPipe => Some(NetworkFailure::Blocked),
        ErrorKind::TimedOut => Some(NetworkFailure::Timeout),
        _ => None,
    }
}

/// Signature textuelle, pour ce qu'aucun `ErrorKind` ne couvre : la résolution
/// de nom (message de `std`, en anglais sur toutes les plateformes) et le rejet
/// TLS (message de la bibliothèque TLS). Les marqueurs restent volontairement
/// spécifiques : un mot trop courant classerait à tort une erreur quelconque.
fn classify_message(message: &str) -> Option<NetworkFailure> {
    const OFFLINE: [&str; 3] = [
        "network is unreachable",
        "réseau est inaccessible",
        "no route to host",
    ];
    const DNS: [&str; 6] = [
        "failed to lookup address information",
        "dns error",
        "name or service not known",
        "nodename nor servname",
        "temporary failure in name resolution",
        "no such host",
    ];
    const TLS: [&str; 7] = [
        "certificate",
        "certificat",
        "handshake",
        "self-signed",
        "self signed",
        "untrusted",
        "ssl routines",
    ];
    let lower = message.to_lowercase();
    let matches_any = |markers: &[&str]| markers.iter().any(|marker| lower.contains(marker));
    // L'ordre compte : « failed to lookup address information: Network is
    // unreachable » est une absence de réseau, pas un DNS en panne.
    if matches_any(&OFFLINE) {
        Some(NetworkFailure::Offline)
    } else if matches_any(&DNS) {
        Some(NetworkFailure::DnsFailed)
    } else if matches_any(&TLS) {
        Some(NetworkFailure::TlsRejected)
    } else {
        None
    }
}

/// Classe une erreur et toute sa chaîne de causes en retenant le signal le plus
/// spécifique. `reqwest` empile plusieurs couches (sa propre erreur, celle de
/// `hyper`, enfin l'erreur système) : la cause utile n'est jamais la première.
pub fn classify_error_chain(error: &(dyn std::error::Error + 'static)) -> NetworkFailure {
    let mut found = NetworkFailure::Unknown;
    let mut link = Some(error);
    while let Some(current) = link {
        let candidate = current
            .downcast_ref::<std::io::Error>()
            .and_then(classify_io_error)
            .or_else(|| classify_message(&current.to_string()));
        if let Some(candidate) = candidate {
            if candidate.precedence() > found.precedence() {
                found = candidate;
            }
        }
        link = current.source();
    }
    found
}

/// Classe une erreur `reqwest`. La chaîne de causes dit *pourquoi* ; à défaut,
/// `is_timeout` et `is_connect` disent au moins *quand* la requête a échoué.
/// Une erreur de requête sans cause lisible (`is_request` seul) reste
/// `Unknown` : mieux vaut afficher le détail technique qu'inventer un diagnostic.
pub fn classify_request_error(error: &reqwest::Error) -> NetworkFailure {
    match classify_error_chain(error) {
        NetworkFailure::Unknown if error.is_timeout() => NetworkFailure::Timeout,
        NetworkFailure::Unknown if error.is_connect() => NetworkFailure::Blocked,
        failure => failure,
    }
}

/// Masque les identifiants qu'une URL citée dans un message d'erreur pourrait
/// porter (`http://utilisateur:motdepasse@proxy:8080`) : c'est la forme sous
/// laquelle un proxy d'entreprise est souvent configuré, et ce détail-là est
/// fait pour être affiché, copié et envoyé au support.
fn redact_credentials(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(position) = rest.find("://") {
        let (before, after) = rest.split_at(position + 3);
        out.push_str(before);
        let authority_end = after
            .find(|c: char| c.is_whitespace() || matches!(c, '/' | ')' | '"' | '\'' | ','))
            .unwrap_or(after.len());
        let (authority, tail) = after.split_at(authority_end);
        match authority.rfind('@') {
            Some(at) => {
                out.push_str("***");
                out.push_str(&authority[at..]);
            }
            None => out.push_str(authority),
        }
        rest = tail;
    }
    out.push_str(rest);
    out
}

/// Message technique complet d'une erreur et de ses causes. `reqwest` n'affiche
/// que sa propre couche (« error sending request for url … ») : sans ce
/// parcours, la cause réelle — refus, certificat, DNS — est perdue. Les
/// maillons qui répètent ce qui précède sont sautés et l'ensemble est borné :
/// ce texte finit dans le journal et, replié, dans l'interface.
pub fn describe_error_chain(error: &(dyn std::error::Error + 'static)) -> String {
    const MAX_CHARS: usize = 400;
    let mut parts: Vec<String> = Vec::new();
    let mut link = Some(error);
    while let Some(current) = link {
        let text = current.to_string().trim().to_string();
        if !text.is_empty() && !parts.iter().any(|previous| previous.contains(&text)) {
            parts.push(text);
        }
        link = current.source();
    }
    let mut joined = redact_credentials(&parts.join(" → "));
    if joined.chars().count() > MAX_CHARS {
        let cut = joined
            .char_indices()
            .nth(MAX_CHARS - 1)
            .map(|(index, _)| index)
            .unwrap_or(joined.len());
        joined.truncate(cut);
        joined.push('…');
    }
    joined
}

#[derive(Debug)]
pub enum DownloadError {
    /// La requête n'a pas abouti : rien n'a été reçu. `failure` porte la
    /// classification (elle décide du message affiché), `detail` la chaîne de
    /// causes complète, qui ne doit jamais se perdre en route.
    Network {
        failure: NetworkFailure,
        detail: String,
    },
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
            DownloadError::Network { failure, detail } => {
                write!(f, "{} ({detail})", failure.label())
            }
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

/// `Content-Range: bytes <début>-<fin>/<total>` → `(début, Some(total))`.
/// Le total vaut `None` s'il est inconnu (`/*`). Renvoie `None` si l'en-tête
/// n'est pas une plage d'octets exploitable (`bytes */4000` d'un 416, unité
/// inconnue…).
fn parse_content_range(value: &str) -> Option<(u64, Option<u64>)> {
    let value = value.trim();
    let unit_end = value.find(' ')?;
    if !value[..unit_end].eq_ignore_ascii_case("bytes") {
        return None;
    }
    let (range, total) = value[unit_end + 1..].split_once('/')?;
    let start = range.split('-').next()?.trim().parse().ok()?;
    Some((start, total.trim().parse().ok()))
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
    request.send().await.map_err(|e| DownloadError::Network {
        failure: classify_request_error(&e),
        detail: describe_error_chain(&e),
    })
}

/// Coupure en cours de réception. La variante reste `Failed` quelle que soit la
/// cause : les octets déjà reçus sont conservés et la conduite à tenir est
/// toujours la même — réessayer, la reprise repart du dernier octet. Un
/// diagnostic « pare-feu » à cet endroit ferait croire à l'utilisateur qu'il a
/// tout perdu. La cause reconnue passe en tête du détail, pour le journal.
fn stream_error(error: reqwest::Error) -> DownloadError {
    let detail = describe_error_chain(&error);
    DownloadError::Failed(match classify_request_error(&error) {
        NetworkFailure::Unknown => format!("flux interrompu: {detail}"),
        failure => format!("flux interrompu ({}): {detail}", failure.label()),
    })
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

    // Un `.part` déjà complet — tentative précédente interrompue juste avant la
    // vérification ou le renommage — se reconnaît à son empreinte, sans la
    // moindre requête. Sinon le serveur répondrait 416 (plage hors fichier) et
    // on détruirait un fichier pourtant intact pour le retélécharger.
    if resume_from > 0 {
        if let Some(expected) = expected_sha256 {
            on_event(DownloadEvent::Verifying);
            let actual = compute_sha256_blocking(part.to_path_buf()).await?;
            if actual.eq_ignore_ascii_case(expected) {
                return Ok(());
            }
        }
    }

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

    // Une réponse 206 doit dire d'où elle repart : un serveur qui renvoie un
    // autre décalage (le fichier entier, par exemple) produirait des octets en
    // double si on se contentait de compléter le `.part`. On la traite alors
    // comme un 200 : troncature et réécriture depuis zéro.
    let content_range = response
        .headers()
        .get(CONTENT_RANGE)
        .and_then(|value| value.to_str().ok())
        .and_then(parse_content_range);
    let mut total = None;
    if status == StatusCode::PARTIAL_CONTENT {
        if let Some((start, announced_total)) = content_range {
            if start != resume_from {
                resume_from = 0;
            }
            total = announced_total;
        }
    }
    // Sans total annoncé, il reste inconnu : ne surtout pas le confondre avec
    // le décalage de reprise, sinon la garde de taille refuserait un fichier
    // pourtant complet.
    let total = total.or_else(|| {
        response
            .content_length()
            .map(|length| resume_from.saturating_add(length))
    });

    let mut file = if resume_from > 0 {
        OpenOptions::new().append(true).open(part)?
    } else {
        File::create(part)?
    };
    let mut downloaded = resume_from;
    let progress_total = |downloaded: u64| total.unwrap_or(downloaded).max(downloaded);
    on_event(DownloadEvent::Progress {
        downloaded,
        total: progress_total(downloaded),
    });

    let mut last_emit = Instant::now();
    let mut stream = response.bytes_stream();
    loop {
        let next = tokio::select! {
            _ = cancel.cancelled() => return Err(DownloadError::Cancelled),
            chunk = stream.next() => chunk,
        };
        let Some(chunk) = next else { break };
        let chunk = chunk.map_err(stream_error)?;
        file.write_all(&chunk)?;
        downloaded += chunk.len() as u64;
        if last_emit.elapsed() >= Duration::from_millis(100) {
            on_event(DownloadEvent::Progress {
                downloaded,
                total: progress_total(downloaded),
            });
            last_emit = Instant::now();
        }
    }
    file.flush()?;
    // Les octets doivent être sur le disque avant le renommage que l'appelant
    // va faire : sinon une coupure de courant laisserait un fichier au nom
    // définitif mais au contenu tronqué, que plus rien ne reprendrait.
    file.sync_all()?;
    drop(file);
    on_event(DownloadEvent::Progress {
        downloaded,
        total: progress_total(downloaded),
    });

    if let Some(total) = total {
        if downloaded != total {
            return Err(DownloadError::Failed(format!(
                "taille inattendue: {downloaded} octets reçus sur {total}"
            )));
        }
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
    mut on_event: impl FnMut(DownloadEvent),
) -> Result<(), DownloadError> {
    if dest.is_file() {
        match expected_sha256 {
            None => return Ok(()),
            Some(expected) => {
                on_event(DownloadEvent::Verifying);
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
        /// Répond 206 mais en repartant de zéro (`Content-Range: bytes 0-…`)
        /// alors qu'une reprise plus loin était demandée.
        ResumeFromZero,
        /// N'annonce aucune taille : ni `Content-Length` (le corps se termine
        /// à la fermeture) ni total dans `Content-Range` (`/*`).
        NoContentLength,
        /// Envoie `n` octets puis coupe la connexion.
        TruncateAfter(usize),
        /// Envoie le corps par blocs de 16 octets espacés de 50 ms.
        Slow,
        /// Répond 404 : le serveur est joignable, la ressource non.
        NotFound,
        /// Envoie l'en-tête et `n` octets, puis se tait sans fermer : le
        /// téléchargement reste suspendu jusqu'à l'échéance de lecture.
        StallAfter(usize),
        /// Accepte la connexion, lit la requête, puis ne répond jamais : le
        /// client doit abandonner sur son échéance (filtrage silencieux).
        Stall,
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
                let announces_size = !matches!(behaviour, Behaviour::NoContentLength);
                let (status, from) = match (behaviour, start) {
                    (Behaviour::Stall, _) => {
                        std::thread::sleep(Duration::from_secs(2));
                        continue;
                    }
                    (Behaviour::NotFound, _) => {
                        let _ = stream.write_all(
                            b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                        );
                        continue;
                    }
                    (Behaviour::RejectRange, Some(_)) => {
                        let head = format!(
                            "HTTP/1.1 416 Range Not Satisfiable\r\nContent-Range: bytes */{total}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                        );
                        let _ = stream.write_all(head.as_bytes());
                        continue;
                    }
                    (Behaviour::ResumeFromZero, Some(_)) => ("206 Partial Content", 0u64),
                    (Behaviour::IgnoreRange, _) | (_, None) => ("200 OK", 0u64),
                    (_, Some(s)) => ("206 Partial Content", s),
                };
                let payload = &body[from as usize..];
                let mut head = format!("HTTP/1.1 {status}\r\nConnection: close\r\n");
                if announces_size {
                    head.push_str(&format!("Content-Length: {}\r\n", payload.len()));
                }
                if status.starts_with("206") {
                    let announced = if announces_size {
                        total.to_string()
                    } else {
                        "*".to_string()
                    };
                    head.push_str(&format!(
                        "Content-Range: bytes {from}-{}/{announced}\r\n",
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
                    Behaviour::StallAfter(n) => {
                        let _ = stream.write_all(&payload[..n.min(payload.len())]);
                        let _ = stream.flush();
                        std::thread::sleep(Duration::from_secs(2));
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
    async fn existing_valid_file_skips_the_network() {
        let data = body();
        let server = start_server(data.clone(), vec![Behaviour::Normal]);
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("file.bin");
        std::fs::write(&dest, &data).unwrap();
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
        // Sans SHA-256 attendu, la seule présence du fichier suffit.
        download_with_resume(
            &client,
            &server.url,
            &dest,
            None,
            &CancellationToken::new(),
            |e| events.push(e),
        )
        .await
        .unwrap();
        assert_eq!(std::fs::read(&dest).unwrap(), data);
        assert_eq!(
            events,
            vec![DownloadEvent::Verifying],
            "seule la vérification du fichier déjà présent est annoncée"
        );
        assert!(
            server.ranges_seen.lock().unwrap().is_empty(),
            "aucune requête ne doit partir"
        );
    }

    #[tokio::test]
    async fn existing_file_with_wrong_sha256_is_replaced() {
        let data = body();
        let server = start_server(data.clone(), vec![Behaviour::Normal]);
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("file.bin");
        std::fs::write(&dest, b"version corrompue").unwrap();
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

    /// Un `.part` déjà complet (interruption juste avant le renommage) est
    /// reconnu par son SHA-256 sans qu'un seul octet reparte sur le réseau.
    #[tokio::test]
    async fn complete_part_with_valid_sha256_is_not_redownloaded() {
        let data = body();
        // Le serveur refuserait la reprise (416) : s'il voit passer quoi que ce
        // soit, c'est que le raccourci n'a pas fonctionné.
        let server = start_server(data.clone(), vec![Behaviour::RejectRange]);
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("file.bin");
        std::fs::write(part_path(&dest), &data).unwrap();
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
        assert_eq!(events, vec![DownloadEvent::Verifying]);
        assert!(
            server.ranges_seen.lock().unwrap().is_empty(),
            "aucun octet ne doit être retéléchargé"
        );
    }

    /// `.part` de la bonne taille mais corrompu : le SHA-256 ne correspond pas,
    /// le serveur refuse la reprise (416), on repart de zéro et ça aboutit.
    #[tokio::test]
    async fn complete_part_with_wrong_sha256_restarts_from_scratch() {
        let data = body();
        let server = start_server(
            data.clone(),
            vec![Behaviour::RejectRange, Behaviour::Normal],
        );
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("file.bin");
        std::fs::write(part_path(&dest), vec![0xAAu8; data.len()]).unwrap();
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
        let seen = server.ranges_seen.lock().unwrap();
        assert_eq!(
            seen.len(),
            2,
            "une reprise refusée puis une requête complète"
        );
        assert!(seen[0].is_some() && seen[1].is_none());
        assert_eq!(
            events.first(),
            Some(&DownloadEvent::Verifying),
            "le .part complet est haché avant toute requête"
        );
    }

    /// Reprise demandée mais réponse 206 repartant de zéro : le `.part` doit
    /// être tronqué et réécrit, jamais complété (sinon octets en double).
    #[tokio::test]
    async fn resume_response_starting_at_zero_is_not_appended() {
        let data = body();
        let server = start_server(data.clone(), vec![Behaviour::ResumeFromZero]);
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
            &[Some("bytes=1500-".to_string())],
            "une seule requête : la réponse reçue est réutilisée telle quelle"
        );
    }

    /// Réponse sans `Content-Length` (corps terminé par la fermeture) : le
    /// total est inconnu, ce n'est pas une erreur de taille.
    #[tokio::test]
    async fn missing_content_length_is_tolerated() {
        let data = body();
        let server = start_server(data.clone(), vec![Behaviour::NoContentLength]);
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
        let last_progress = events
            .iter()
            .rev()
            .find_map(|e| match e {
                DownloadEvent::Progress { downloaded, total } => Some((*downloaded, *total)),
                _ => None,
            })
            .unwrap();
        assert_eq!(
            last_progress,
            (4000, 4000),
            "total inconnu : la progression suit les octets reçus"
        );
    }

    /// Même chose sur une reprise : `Content-Range: bytes 1500-3999/*` sans
    /// `Content-Length` ne doit pas être pris pour un total de 1500 octets.
    #[tokio::test]
    async fn missing_content_length_on_resume_is_tolerated() {
        let data = body();
        let server = start_server(data.clone(), vec![Behaviour::NoContentLength]);
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

    #[test]
    fn parses_content_range() {
        assert_eq!(
            parse_content_range("bytes 1500-3999/4000"),
            Some((1500, Some(4000)))
        );
        assert_eq!(parse_content_range("bytes 0-9/*"), Some((0, None)));
        assert_eq!(parse_content_range("bytes */4000"), None);
        assert_eq!(parse_content_range("pages 1-2/10"), None);
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

    /// Connexion refusée (port fermé) : c'est un filtrage/refus, pas une
    /// absence de réseau — et le détail technique doit accompagner l'erreur.
    #[tokio::test]
    async fn refused_connection_is_classified_as_blocked_with_a_detail() {
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
        let DownloadError::Network { failure, detail } = &err else {
            panic!("{err}")
        };
        assert_eq!(*failure, NetworkFailure::Blocked);
        assert!(
            detail.contains(&url),
            "l'URL visée doit rester dans le détail: {detail}"
        );
        assert!(
            detail.len() > url.len() + 10,
            "la cause système doit accompagner le message reqwest: {detail}"
        );
    }

    /// Serveur qui accepte puis ne répond jamais : délai dépassé, pas « hors
    /// ligne » (c'est la signature d'un filtrage silencieux).
    #[tokio::test]
    async fn a_server_that_never_answers_is_a_timeout() {
        let server = start_server(body(), vec![Behaviour::Stall]);
        let dir = tempfile::tempdir().unwrap();
        let client = reqwest::Client::builder()
            .timeout(Duration::from_millis(300))
            .build()
            .unwrap();
        let err = download_with_resume(
            &client,
            &server.url,
            &dir.path().join("f"),
            None,
            &CancellationToken::new(),
            |_| {},
        )
        .await
        .unwrap_err();
        assert!(
            matches!(
                err,
                DownloadError::Network {
                    failure: NetworkFailure::Timeout,
                    ..
                }
            ),
            "{err}"
        );
    }

    /// Flux qui s'arrête en cours de route : quelle que soit la cause reconnue
    /// (ici l'échéance de lecture), les octets reçus sont conservés et une
    /// reprise termine le travail. La cause est nommée dans le détail, pour le
    /// journal, mais l'utilisateur n'est pas envoyé chercher un pare-feu.
    #[tokio::test]
    async fn a_stalled_stream_keeps_its_bytes_and_names_the_cause() {
        let data = body();
        let server = start_server(data.clone(), vec![Behaviour::StallAfter(1000)]);
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("file.bin");
        let client = reqwest::Client::builder()
            .read_timeout(Duration::from_millis(300))
            .build()
            .unwrap();
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
        let DownloadError::Failed(detail) = &err else {
            panic!("une coupure de flux reste réparable par une reprise: {err}")
        };
        assert!(detail.contains("délai dépassé"), "{detail}");
        // Les octets reçus sont conservés : la reprise (couverte par
        // `truncated_stream_keeps_partial_then_resumes`) repartira de là.
        assert_eq!(std::fs::metadata(part_path(&dest)).unwrap().len(), 1000);
        assert!(!dest.exists());
    }

    /// Réponse HTTP inattendue : le serveur a répondu, ce n'est donc jamais
    /// une panne réseau.
    #[tokio::test]
    async fn unexpected_http_status_is_not_a_network_failure() {
        let server = start_server(body(), vec![Behaviour::NotFound]);
        let dir = tempfile::tempdir().unwrap();
        let client = build_client().unwrap();
        let err = download_with_resume(
            &client,
            &server.url,
            &dir.path().join("f"),
            None,
            &CancellationToken::new(),
            |_| {},
        )
        .await
        .unwrap_err();
        assert!(
            matches!(&err, DownloadError::Failed(detail) if detail.contains("404")),
            "{err}"
        );
    }

    /// Maillon de chaîne de causes factice : `reqwest::Error` n'a pas de
    /// constructeur public, mais la classification ne lit que `source()`.
    #[derive(Debug)]
    struct Chained {
        message: &'static str,
        source: std::io::Error,
    }

    impl std::fmt::Display for Chained {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str(self.message)
        }
    }

    impl std::error::Error for Chained {
        fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
            Some(&self.source)
        }
    }

    fn chained(source: std::io::Error) -> Chained {
        Chained {
            message: "error sending request for url (https://huggingface.co/x.gguf)",
            source,
        }
    }

    /// Chaque signature système connue tombe dans sa catégorie : c'est ce qui
    /// remplace le fourre-tout « hors ligne » d'origine.
    #[test]
    fn classifies_known_system_causes() {
        use std::io::{Error, ErrorKind};
        let cases: Vec<(Error, NetworkFailure)> = vec![
            (
                Error::from(ErrorKind::NetworkUnreachable),
                NetworkFailure::Offline,
            ),
            (Error::from(ErrorKind::NetworkDown), NetworkFailure::Offline),
            (
                Error::from(ErrorKind::HostUnreachable),
                NetworkFailure::Offline,
            ),
            // Résolution de nom : message de `std` (identique partout) sur
            // Unix, code Winsock sur Windows.
            (
                Error::other("failed to lookup address information: nodename nor servname provided, or not known"),
                NetworkFailure::DnsFailed,
            ),
            (Error::from_raw_os_error(11001), NetworkFailure::DnsFailed),
            (
                Error::from(ErrorKind::ConnectionRefused),
                NetworkFailure::Blocked,
            ),
            (
                Error::from(ErrorKind::ConnectionReset),
                NetworkFailure::Blocked,
            ),
            (
                Error::from(ErrorKind::ConnectionAborted),
                NetworkFailure::Blocked,
            ),
            (
                Error::other("invalid peer certificate: UnknownIssuer"),
                NetworkFailure::TlsRejected,
            ),
            (
                Error::other("the certificate chain was issued by an authority that is not trusted"),
                NetworkFailure::TlsRejected,
            ),
            (Error::from(ErrorKind::TimedOut), NetworkFailure::Timeout),
            (
                Error::other("quelque chose d'inattendu"),
                NetworkFailure::Unknown,
            ),
        ];
        for (source, expected) in cases {
            let described = source.to_string();
            let classified = classify_error_chain(&chained(source));
            assert_eq!(classified, expected, "cause: {described}");
        }
    }

    /// Le détail remonté doit porter toute la chaîne : `reqwest` n'affiche que
    /// sa propre couche, la cause système n'est que dans `source()`.
    #[test]
    fn describes_the_whole_cause_chain() {
        let detail = describe_error_chain(&chained(std::io::Error::from(
            std::io::ErrorKind::ConnectionRefused,
        )));
        assert!(detail.contains("huggingface.co"), "{detail}");
        assert!(detail.contains("refused"), "{detail}");
    }

    /// Le détail s'affiche et se copie : les identifiants qu'une URL de proxy
    /// peut porter ne doivent pas partir avec lui.
    #[test]
    fn hides_credentials_an_url_could_carry() {
        let detail = describe_error_chain(&Chained {
            message: "error sending request",
            source: std::io::Error::other(
                "invalid URL: http://utilisateur:motdepasse@proxy.local:8080/ rejected",
            ),
        });
        assert!(detail.contains("http://***@proxy.local:8080/"), "{detail}");
        assert!(!detail.contains("motdepasse"), "{detail}");
        assert!(!detail.contains("utilisateur"), "{detail}");
        // Une URL sans identifiant reste intacte.
        assert!(describe_error_chain(&chained(std::io::Error::other("x")))
            .contains("https://huggingface.co/x.gguf"),);
    }

    /// Un maillon qui répète le message de son parent n'est pas recopié, et le
    /// détail reste court : il part dans une bulle d'interface.
    #[test]
    fn the_detail_stays_short_and_free_of_repetitions() {
        let repeated = Chained {
            message: "error sending request for url (https://huggingface.co/x.gguf)",
            source: std::io::Error::other(
                "error sending request for url (https://huggingface.co/x.gguf)",
            ),
        };
        assert_eq!(
            describe_error_chain(&repeated),
            "error sending request for url (https://huggingface.co/x.gguf)"
        );
        let long = Chained {
            message: "erreur",
            source: std::io::Error::other("x".repeat(1000)),
        };
        assert!(describe_error_chain(&long).chars().count() <= 400);
    }
}
