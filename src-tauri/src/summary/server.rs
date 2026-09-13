//! Cycle de vie du processus `llama-server` : port libre, clé aléatoire,
//! arguments, attente de `/health`, journal stderr, arrêt.

use std::collections::VecDeque;
use std::io::{BufRead, BufReader};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use rand::RngCore;

use super::SummaryError;

/// Délai maximal de chargement du modèle avant `EngineStartFailed`.
pub const START_TIMEOUT: Duration = Duration::from_secs(120);
/// Nombre de lignes de stderr conservées pour le diagnostic.
pub const STDERR_TAIL_LINES: usize = 20;
/// Court répit laissé au lecteur de stderr pour vider le tube quand le
/// processus vient de mourir : sans lui, le diagnostic citerait un journal
/// encore vide.
const STDERR_DRAIN_DELAY: Duration = Duration::from_millis(200);

#[derive(Debug, Clone)]
pub struct LlamaServerConfig {
    pub exe: PathBuf,
    pub model: PathBuf,
    pub ctx_size: u32,
    pub threads: usize,
    pub gpu_layers: u32,
    pub extra_args: Vec<String>,
    /// Fichier où écrire le pid (nettoyage d'un survivant au prochain démarrage).
    pub pid_file: Option<PathBuf>,
}

/// Couches déchargées sur le GPU : tout sur macOS (Metal), rien ailleurs (CPU).
pub fn default_gpu_layers() -> u32 {
    if cfg!(target_os = "macos") {
        99
    } else {
        0
    }
}

/// Port TCP libre sur 127.0.0.1 (lié puis relâché).
pub fn free_port() -> std::io::Result<u16> {
    Ok(TcpListener::bind("127.0.0.1:0")?.local_addr()?.port())
}

/// Clé API aléatoire (32 octets, hexadécimal) : seule l'app peut parler au serveur.
pub fn random_api_key() -> String {
    let mut bytes = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut bytes);
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Ligne de commande de `llama-server` (§4 de la spec).
pub fn build_args(config: &LlamaServerConfig, port: u16, api_key: &str) -> Vec<String> {
    let mut args: Vec<String> = vec![
        "-m".into(),
        config.model.to_string_lossy().into_owned(),
        "--host".into(),
        "127.0.0.1".into(),
        "--port".into(),
        port.to_string(),
        "--api-key".into(),
        api_key.to_string(),
        "-c".into(),
        config.ctx_size.to_string(),
        "--threads".into(),
        config.threads.to_string(),
        "-ngl".into(),
        config.gpu_layers.to_string(),
        "--parallel".into(),
        "1".into(),
        "--no-webui".into(),
    ];
    args.extend(config.extra_args.iter().cloned());
    args
}

/// Verrou tolérant à l'empoisonnement : `kill()` est appelé depuis `Drop`, et
/// une panique dans un destructeur avorte tout le processus.
fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

pub struct LlamaServer {
    child: Mutex<Option<Child>>,
    pid: u32,
    port: u16,
    api_key: String,
    stderr_tail: Arc<Mutex<VecDeque<String>>>,
    pid_file: Option<PathBuf>,
}

impl LlamaServer {
    /// Lance le serveur et attend `/health` = 200 (au plus `START_TIMEOUT`).
    pub async fn start(
        config: LlamaServerConfig,
        client: &reqwest::Client,
    ) -> Result<Self, SummaryError> {
        let port = free_port().map_err(|e| SummaryError::EngineStartFailed {
            detail: format!("aucun port libre: {e}"),
        })?;
        let api_key = random_api_key();
        let args = build_args(&config, port, &api_key);

        let mut command = Command::new(&config.exe);
        command
            .args(&args)
            .stdin(Stdio::null())
            // `null` et non `piped` : un tube que personne ne draine finit
            // plein et bloque le serveur au premier message de trop.
            .stdout(Stdio::null())
            .stderr(Stdio::piped());
        // Dossier de l'exécutable comme dossier de travail (DLL et dylib
        // voisines) ; un parent vide (`exe` sans dossier) n'en est pas un.
        if let Some(dir) = config
            .exe
            .parent()
            .filter(|dir| !dir.as_os_str().is_empty())
        {
            command.current_dir(dir);
        }
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(super::install::CREATE_NO_WINDOW);
        }
        let mut child = command
            .spawn()
            .map_err(|e| SummaryError::EngineStartFailed {
                detail: format!("lancement de {}: {e}", config.exe.display()),
            })?;
        let pid = child.id();
        log::info!("llama-server lancé (pid {pid}, port {port})");

        // Écrit dès le `spawn` : une fermeture brutale de l'app juste après
        // laisserait sinon un serveur introuvable au prochain démarrage.
        if let Some(pid_file) = &config.pid_file {
            if let Some(parent) = pid_file.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            if let Err(e) = std::fs::write(pid_file, pid.to_string()) {
                log::warn!("Écriture du fichier pid impossible: {e}");
            }
        }

        let stderr_tail = Arc::new(Mutex::new(VecDeque::with_capacity(STDERR_TAIL_LINES)));
        if let Some(stderr) = child.stderr.take() {
            let tail = stderr_tail.clone();
            // Le thread se termine de lui-même à la fermeture du tube,
            // c'est-à-dire à la mort du processus.
            std::thread::spawn(move || {
                for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                    log::debug!("llama-server: {line}");
                    let mut tail = lock(&tail);
                    if tail.len() >= STDERR_TAIL_LINES {
                        tail.pop_front();
                    }
                    tail.push_back(line);
                }
            });
        }

        // À partir d'ici l'enfant appartient à `server` : tout chemin d'erreur
        // passe par `kill()`, et `Drop` rattrape ceux qu'on oublierait.
        let server = Self {
            child: Mutex::new(Some(child)),
            pid,
            port,
            api_key,
            stderr_tail,
            pid_file: config.pid_file.clone(),
        };
        let health = wait_healthy(
            client,
            port,
            &server.api_key,
            START_TIMEOUT,
            || server.is_alive(),
            || server.stderr_tail(),
        )
        .await;
        match health {
            Ok(()) => Ok(server),
            Err(error) => {
                server.kill();
                Err(error)
            }
        }
    }

    pub fn pid(&self) -> u32 {
        self.pid
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    pub fn api_key(&self) -> &str {
        &self.api_key
    }

    /// `http://127.0.0.1:<port>/v1`, à passer en `base_url` du client LLM.
    pub fn base_url(&self) -> String {
        format!("http://127.0.0.1:{}/v1", self.port)
    }

    /// Dernières lignes de stderr (diagnostic d'un démarrage raté).
    pub fn stderr_tail(&self) -> String {
        lock(&self.stderr_tail)
            .iter()
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// `true` tant que le processus n'a pas terminé.
    pub fn is_alive(&self) -> bool {
        let mut guard = lock(&self.child);
        match guard.as_mut() {
            Some(child) => matches!(child.try_wait(), Ok(None)),
            None => false,
        }
    }

    /// Arrêt immédiat (kill + wait) ; idempotent.
    pub fn kill(&self) {
        let mut guard = lock(&self.child);
        if let Some(mut child) = guard.take() {
            // `wait` après `kill` : sans lui le processus resterait zombie.
            let _ = child.kill();
            let _ = child.wait();
            log::info!("llama-server arrêté (pid {})", self.pid);
        }
        if let Some(pid_file) = &self.pid_file {
            let _ = std::fs::remove_file(pid_file);
        }
    }
}

impl std::fmt::Debug for LlamaServer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LlamaServer")
            .field("pid", &self.pid)
            .field("port", &self.port)
            .finish()
    }
}

impl Drop for LlamaServer {
    fn drop(&mut self) {
        self.kill();
    }
}

/// Sonde `GET /health` toutes les 500 ms jusqu'à 200, la mort du processus
/// (`alive() == false`) ou l'expiration de `timeout`.
pub async fn wait_healthy(
    client: &reqwest::Client,
    port: u16,
    api_key: &str,
    timeout: Duration,
    alive: impl Fn() -> bool,
    stderr_tail: impl Fn() -> String,
) -> Result<(), SummaryError> {
    // `--api-key` ne protège que `/v1/*` sur les builds llama.cpp récents
    // (`/health` répond 200 sans clé, constaté sur b10930) ; l'en-tête est
    // envoyé quand même, pour une version qui le protégerait.
    let url = format!("http://127.0.0.1:{port}/health");
    let deadline = Instant::now() + timeout;
    loop {
        if !alive() {
            // Laisser le lecteur de stderr finir de vider le tube, sinon le
            // diagnostic citerait un journal encore vide.
            tokio::time::sleep(STDERR_DRAIN_DELAY).await;
            return Err(SummaryError::EngineStartFailed {
                detail: format!(
                    "le processus s'est arrêté pendant le démarrage\n{}",
                    stderr_tail()
                ),
            });
        }
        let probe = client
            .get(&url)
            .bearer_auth(api_key)
            .timeout(Duration::from_secs(5))
            .send()
            .await;
        if let Ok(response) = probe {
            if response.status().is_success() {
                return Ok(());
            }
        }
        if Instant::now() >= deadline {
            return Err(SummaryError::EngineStartFailed {
                detail: format!(
                    "délai de démarrage dépassé ({} s)\n{}",
                    timeout.as_secs(),
                    stderr_tail()
                ),
            });
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
}

/// Au démarrage de l'app : tue un `llama-server` survivant d'une fermeture
/// brutale (pid lu dans `pid_file`, identité vérifiée sur l'exécutable).
pub fn kill_orphan_server(pid_file: &Path, expected_exe: &Path) {
    let Ok(text) = std::fs::read_to_string(pid_file) else {
        return;
    };
    // Consommé quoi qu'il arrive : un pid illisible ou recyclé ne doit pas
    // revenir hanter chaque démarrage.
    let _ = std::fs::remove_file(pid_file);
    let Ok(pid) = text.trim().parse::<u32>() else {
        return;
    };
    if pid == std::process::id() {
        return;
    }
    // Le pid a pu être recyclé par un autre programme : on ne tue que si
    // l'exécutable est bien le nôtre.
    if super::system::kill_if_our_server(pid, expected_exe) {
        log::warn!("llama-server survivant (pid {pid}) arrêté");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn config(exe: &Path) -> LlamaServerConfig {
        LlamaServerConfig {
            exe: exe.to_path_buf(),
            model: PathBuf::from("/models/summary/model.gguf"),
            ctx_size: 16_384,
            threads: 4,
            gpu_layers: default_gpu_layers(),
            extra_args: vec!["--reasoning-budget".into(), "0".into()],
            pid_file: None,
        }
    }

    #[test]
    fn args_follow_the_spec() {
        let args = build_args(&config(Path::new("/bin/llama-server")), 4242, "k3y");
        let joined = args.join(" ");
        assert!(joined.starts_with("-m /models/summary/model.gguf --host 127.0.0.1 --port 4242 --api-key k3y -c 16384 --threads 4 -ngl "));
        assert!(joined.contains("--parallel 1 --no-webui"));
        assert!(joined.ends_with("--reasoning-budget 0"));
        if cfg!(target_os = "macos") {
            assert!(joined.contains("-ngl 99"));
        } else {
            assert!(joined.contains("-ngl 0"));
        }
    }

    #[test]
    fn free_port_and_key_are_usable() {
        let port = free_port().unwrap();
        assert!(port > 0);
        let key = random_api_key();
        assert_eq!(key.len(), 64);
        assert!(key.chars().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(key, random_api_key());
    }

    /// Serveur HTTP factice : répond `replies` dans l'ordre (un code par
    /// connexion), puis 200.
    fn fake_health_server(replies: Vec<u16>) -> u16 {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            let mut replies = replies.into_iter();
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { break };
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut line = String::new();
                while reader.read_line(&mut line).unwrap_or(0) > 0 && line != "\r\n" {
                    line.clear();
                }
                let code = replies.next().unwrap_or(200);
                let body = if code == 200 {
                    "{\"status\":\"ok\"}"
                } else {
                    "{\"error\":{\"code\":503,\"message\":\"Loading model\"}}"
                };
                let _ = write!(stream, "HTTP/1.1 {code} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
            }
        });
        port
    }

    #[tokio::test]
    async fn health_wait_tolerates_503_while_loading() {
        let port = fake_health_server(vec![503, 503, 200]);
        let client = reqwest::Client::new();
        let started = Instant::now();
        wait_healthy(
            &client,
            port,
            "k",
            Duration::from_secs(10),
            || true,
            String::new,
        )
        .await
        .unwrap();
        assert!(
            started.elapsed() >= Duration::from_millis(900),
            "deux attentes de 500 ms"
        );
    }

    #[tokio::test]
    async fn health_wait_fails_fast_when_process_died() {
        let port = free_port().unwrap();
        let client = reqwest::Client::new();
        let err = wait_healthy(
            &client,
            port,
            "k",
            Duration::from_secs(10),
            || false,
            || "boom".into(),
        )
        .await
        .unwrap_err();
        match err {
            SummaryError::EngineStartFailed { detail } => {
                assert!(
                    detail.contains("boom") && detail.contains("arrêté"),
                    "{detail}"
                )
            }
            other => panic!("{other:?}"),
        }
    }

    #[tokio::test]
    async fn health_wait_times_out() {
        let port = free_port().unwrap();
        let client = reqwest::Client::new();
        let err = wait_healthy(
            &client,
            port,
            "k",
            Duration::from_millis(700),
            || true,
            String::new,
        )
        .await
        .unwrap_err();
        assert!(
            matches!(err, SummaryError::EngineStartFailed { ref detail } if detail.contains("délai")),
            "{err:?}"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn start_reports_stderr_when_the_server_exits() {
        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("llama-server");
        std::fs::write(
            &exe,
            b"#!/bin/sh\necho 'srv load_model: failed to load model' >&2\nexit 1\n",
        )
        .unwrap();
        super::super::install::set_executable(&exe).unwrap();
        let pid_file = dir.path().join("llama").join("server.pid");
        let mut cfg = config(&exe);
        cfg.pid_file = Some(pid_file.clone());
        let client = reqwest::Client::new();
        let err = LlamaServer::start(cfg, &client).await.unwrap_err();
        match err {
            SummaryError::EngineStartFailed { detail } => {
                assert!(detail.contains("failed to load model"), "{detail}")
            }
            other => panic!("{other:?}"),
        }
        assert!(!pid_file.exists(), "fichier pid retiré après l'échec");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn start_succeeds_against_a_fake_server_and_kill_stops_it() {
        // Le « serveur » est un script qui dort ; /health est servi par un
        // vrai socket tenu par le test sur le port choisi par `start`… ce qui
        // est impossible à connaître d'avance : on vérifie donc ici le reste
        // du contrat (accesseurs, processus vivant, arrêt propre et idempotent,
        // fichier pid retiré).
        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("llama-server");
        // `exec` : le script *devient* le `sleep`, sinon tuer le shell
        // laisserait un `sleep` orphelin derrière le test.
        std::fs::write(&exe, b"#!/bin/sh\nexec sleep 30\n").unwrap();
        super::super::install::set_executable(&exe).unwrap();
        let pid_file = dir.path().join("server.pid");
        let mut command = Command::new(&exe);
        command
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped());
        let child = command.spawn().unwrap();
        let pid = child.id();
        let server = LlamaServer {
            child: Mutex::new(Some(child)),
            pid,
            port: 4242,
            api_key: "k".into(),
            stderr_tail: Arc::new(Mutex::new(VecDeque::new())),
            pid_file: Some(pid_file.clone()),
        };
        std::fs::write(&pid_file, pid.to_string()).unwrap();
        assert_eq!(server.pid(), pid);
        assert_eq!(server.port(), 4242);
        assert_eq!(server.api_key(), "k");
        assert_eq!(server.base_url(), "http://127.0.0.1:4242/v1");
        assert!(server.is_alive());
        server.kill();
        assert!(!server.is_alive());
        assert!(!pid_file.exists());
        // Idempotent : un second arrêt ne panique pas et ne bloque pas.
        server.kill();
        assert!(!server.is_alive());
    }

    #[test]
    fn orphan_cleanup_ignores_missing_or_invalid_pid_file() {
        let dir = tempfile::tempdir().unwrap();
        let pid_file = dir.path().join("server.pid");
        kill_orphan_server(&pid_file, Path::new("/nowhere/llama-server"));
        std::fs::write(&pid_file, "not-a-pid").unwrap();
        kill_orphan_server(&pid_file, Path::new("/nowhere/llama-server"));
        assert!(!pid_file.exists(), "fichier pid consommé");
        std::fs::write(&pid_file, std::process::id().to_string()).unwrap();
        kill_orphan_server(&pid_file, Path::new("/nowhere/llama-server"));
        assert!(!pid_file.exists());
    }

    /// Démarrage réel, sauté tant que `OCADE_LLAMA_SERVER_EXE` et
    /// `OCADE_LLAMA_MODEL` ne désignent pas un vrai moteur et un vrai modèle
    /// (la suite ordinaire ne lance aucun binaire externe).
    #[tokio::test]
    async fn real_engine_starts_serves_health_and_stops() {
        let (Ok(exe), Ok(model)) = (
            std::env::var("OCADE_LLAMA_SERVER_EXE"),
            std::env::var("OCADE_LLAMA_MODEL"),
        ) else {
            eprintln!("sauté : OCADE_LLAMA_SERVER_EXE / OCADE_LLAMA_MODEL non définis");
            return;
        };
        let dir = tempfile::tempdir().unwrap();
        let pid_file = dir.path().join("server.pid");
        let cfg = LlamaServerConfig {
            exe: PathBuf::from(&exe),
            model: PathBuf::from(&model),
            ctx_size: 4096,
            threads: super::super::system::physical_cores(),
            gpu_layers: default_gpu_layers(),
            extra_args: vec![],
            pid_file: Some(pid_file.clone()),
        };
        let client = crate::download::build_client().unwrap();
        let started = Instant::now();
        let server = LlamaServer::start(cfg, &client).await.unwrap();
        eprintln!(
            "llama-server prêt en {:?} (pid {}, port {})",
            started.elapsed(),
            server.pid(),
            server.port()
        );
        assert!(server.is_alive());
        assert_eq!(
            std::fs::read_to_string(&pid_file).unwrap(),
            server.pid().to_string()
        );

        // /health est public (constaté sur le build b10930) ; /v1/* exige la clé.
        let health = client
            .get(format!("http://127.0.0.1:{}/health", server.port()))
            .send()
            .await
            .unwrap();
        eprintln!("/health sans clé : {}", health.status());
        assert!(health.status().is_success());
        let models = client
            .get(format!("{}/models", server.base_url()))
            .send()
            .await
            .unwrap();
        eprintln!("/v1/models sans clé : {}", models.status());
        assert_eq!(models.status().as_u16(), 401, "la clé API protège /v1/*");
        let models = client
            .get(format!("{}/models", server.base_url()))
            .bearer_auth(server.api_key())
            .send()
            .await
            .unwrap();
        assert!(models.status().is_success());

        let pid = server.pid();
        server.kill();
        assert!(!server.is_alive());
        assert!(!pid_file.exists());
        // Le processus a bien disparu de la table : plus rien à tuer.
        assert!(
            !super::super::system::kill_if_our_server(pid, Path::new(&exe)),
            "un llama-server a survécu au kill"
        );
    }
}
