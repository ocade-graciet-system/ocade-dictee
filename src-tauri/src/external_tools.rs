//! Outils externes de l'onglet « Fichier » (yt-dlp, ffmpeg) : installation
//! partagée et pré-chargement en tâche de fond au démarrage.
//!
//! # Pourquoi un pré-chargement
//!
//! L'onglet « Fichier » s'appuie sur deux binaires qui ne sont pas dans
//! l'installeur sur toutes les plateformes :
//!
//! - **yt-dlp** est embarqué en sidecar Tauri sous Windows et Linux ; sous
//!   macOS il ne peut pas l'être (la re-signature ad-hoc du bundle casse le
//!   binaire PyInstaller, cf. [`crate::commands::file_transcription`]) et il
//!   est donc téléchargé (~37 Mo) ;
//! - **ffmpeg** est téléchargé sur toutes les plateformes (~80 Mo, cf.
//!   [`crate::commands::video_download::ensure_ffmpeg`]) ;
//! - **QuickJS** est téléchargé sur toutes les plateformes (~1 à 2,6 Mo, cf.
//!   [`crate::commands::file_transcription::ensure_quickjs`]) : c'est le
//!   moteur JavaScript sans lequel yt-dlp ne sait plus extraire YouTube.
//!
//! Sans pré-chargement, l'utilisateur qui vient d'installer l'application et
//! qui colle une URL attend ces dizaines de méga-octets au moment précis où il
//! demande un résultat. On les récupère donc en tâche de fond, peu après le
//! démarrage, pour que l'onglet soit déjà opérationnel au premier clic.
//!
//! # Règles de conception
//!
//! - **Jamais bloquant** : tout se passe dans une tâche async détachée
//!   ([`spawn_preload`]) ; ni la création de la fenêtre ni aucune interaction
//!   n'attendent quoi que ce soit.
//! - **Rien en mode headless** (`--transcribe-file`, `--list-devices`,
//!   `--list-models`) : le processus doit faire son travail et sortir, sans
//!   toucher au réseau. La règle est portée par [`preload_plan`] en plus de
//!   l'aiguillage de `lib.rs`, pour qu'elle survive à un déplacement de
//!   l'appel.
//! - **Pas de course avec le chemin à la demande** : le pré-chargement appelle
//!   exactement les mêmes fonctions `ensure_*` que l'interface, et chacune est
//!   sérialisée par un verrou async. Le second arrivant attend le premier puis
//!   repart de son résultat — jamais deux téléchargements du même outil.
//! - **Échec silencieux pour l'utilisateur**, explicite dans le journal : ni
//!   event, ni toast, ni fenêtre d'erreur. C'est du confort, pas une fonction
//!   demandée ; le chemin à la demande affichera, lui, une vraie erreur si
//!   l'utilisateur va au bout de sa démarche.
//! - **Une seule tentative par outil et par session**, sans minuterie de
//!   réessai : sur un réseau coupé, un portail captif ou un proxy d'entreprise
//!   fermé, une boucle martèlerait le réseau toute la session pour un service
//!   que l'utilisateur n'a pas demandé. Les vraies relances sont le prochain
//!   démarrage de l'application et le premier usage réel de l'onglet.
//! - **Priorité à la dictée** : au tout premier lancement, l'écran d'accueil
//!   télécharge le modèle français (~512 Mo) et l'utilisateur ne peut rien
//!   faire avant. On laisse le lien réseau à ce téléchargement et on n'ouvre
//!   le nôtre qu'une fois l'accueil terminé (voir [`start_decision`]) : la
//!   dictée est la fonction principale, les outils de l'onglet Fichier sont
//!   secondaires. Le pré-chargement démarre donc dans la foulée de l'accueil,
//!   pendant la même session — l'utilisateur qui enchaîne sur l'onglet
//!   Fichier n'a pas à relancer l'application pour en profiter.
//! - **`--start-hidden` / démarrage automatique en barre d'état** : le
//!   pré-chargement a lieu quand même. C'est justement le cas où il paie le
//!   plus (l'application est lancée à l'ouverture de session, les outils sont
//!   prêts bien avant que l'utilisateur n'ouvre la fenêtre), et le coût est
//!   ponctuel : une fois les binaires sur le disque, aucun démarrage ultérieur
//!   ne retélécharge quoi que ce soit. Pour une machine où ce trafic n'est pas
//!   souhaité (connexion facturée au volume, poste de démonstration), la
//!   variable d'environnement `OCADE_NO_TOOL_PRELOAD=1` le désactive.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use log::{debug, info, warn};
use tauri::{AppHandle, Manager};
use tokio_util::sync::CancellationToken;

use crate::managers::model::ModelManager;

/// Délai avant la première requête réseau. Laisse le démarrage se dérouler
/// (fenêtre, gestionnaires, chargement du modèle depuis le disque) avant
/// d'ajouter des entrées/sorties.
const STARTUP_GRACE: Duration = Duration::from_secs(5);

/// Période de scrutation de l'état « l'application a mieux à faire du réseau ».
const BUSY_POLL_INTERVAL: Duration = Duration::from_secs(5);

/// Au-delà de cette attente, on abandonne le pré-chargement pour la session
/// plutôt que de finir par concurrencer un accueil très lent (ou resté ouvert
/// sans réponse) : le prochain démarrage réessaiera, modèle déjà en place.
const MAX_BUSY_WAIT: Duration = Duration::from_secs(30 * 60);

/// Variable d'échappement : `OCADE_NO_TOOL_PRELOAD=1` coupe le pré-chargement
/// (le téléchargement à la demande, lui, continue de fonctionner).
const DISABLE_ENV: &str = "OCADE_NO_TOOL_PRELOAD";

/// Outil externe dont l'onglet « Fichier » a besoin.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ExternalTool {
    /// Téléchargement des médias en ligne (macOS uniquement ici : sidecar
    /// embarqué sur les autres plateformes).
    YtDlp,
    /// Fusion vidéo+audio et repli de décodage des formats non natifs.
    Ffmpeg,
    /// Moteur JavaScript que yt-dlp exige pour résoudre le défi YouTube
    /// (toutes plateformes).
    QuickJs,
}

impl ExternalTool {
    /// Nom de l'outil, tel qu'il apparaît dans le journal.
    pub(crate) fn label(self) -> &'static str {
        match self {
            ExternalTool::YtDlp => "yt-dlp",
            ExternalTool::Ffmpeg => "ffmpeg",
            ExternalTool::QuickJs => "QuickJS",
        }
    }

    /// Taille approximative du téléchargement, pour que le journal dise
    /// tout de suite ce qui passe sur le lien.
    fn approx_size_mb(self) -> u32 {
        match self {
            ExternalTool::YtDlp => 37,
            ExternalTool::Ffmpeg => 80,
            ExternalTool::QuickJs => 1,
        }
    }
}

/// Tout ce qui décide du plan de pré-chargement. Rien n'est lu ici de
/// l'environnement ni du disque : la décision reste une fonction pure,
/// testable sans application Tauri.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PreloadContext {
    /// Mode headless (`--transcribe-file`, `--list-devices`, `--list-models`).
    pub headless: bool,
    /// Pré-chargement désactivé par l'environnement.
    pub disabled: bool,
    /// yt-dlp est embarqué en sidecar (Windows, Linux) : rien à télécharger.
    pub yt_dlp_bundled: bool,
    /// yt-dlp est déjà dans les données de l'application (macOS).
    pub yt_dlp_installed: bool,
    /// ffmpeg est déjà dans les données de l'application.
    pub ffmpeg_installed: bool,
    /// Le moteur JavaScript est déjà dans les données de l'application.
    pub quickjs_installed: bool,
}

/// Liste des outils à récupérer, dans l'ordre où ils seront téléchargés.
///
/// Du plus petit au plus gros, ce qui est aussi l'ordre de ce qui débloque le
/// geste le plus courant de l'onglet — transcrire une URL YouTube :
/// QuickJS (~1 Mo) sans lequel yt-dlp ne sait plus extraire YouTube du tout,
/// puis yt-dlp lui-même (~37 Mo), puis ffmpeg (~80 Mo) qui ne sert qu'à la
/// fusion vidéo et au repli de décodage.
pub(crate) fn preload_plan(ctx: &PreloadContext) -> Vec<ExternalTool> {
    if ctx.headless || ctx.disabled {
        return Vec::new();
    }
    let mut plan = Vec::new();
    if !ctx.quickjs_installed {
        plan.push(ExternalTool::QuickJs);
    }
    if !ctx.yt_dlp_bundled && !ctx.yt_dlp_installed {
        plan.push(ExternalTool::YtDlp);
    }
    if !ctx.ffmpeg_installed {
        plan.push(ExternalTool::Ffmpeg);
    }
    plan
}

/// Conduite à tenir avant de lancer le premier téléchargement.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StartDecision {
    /// Rien ne nous gêne : on y va.
    Go,
    /// L'accueil ou un téléchargement de modèle occupe le lien : on repasse
    /// plus tard.
    Wait,
    /// Ça dure trop : on laisse tomber pour cette session.
    GiveUp,
}

/// Décide s'il faut céder le lien réseau à ce qui est, lui, sur le chemin
/// critique de l'utilisateur : l'accueil de premier lancement et le
/// téléchargement du modèle de dictée (voir [`startup_busy`]).
pub(crate) fn start_decision(startup_busy: bool, waited: Duration) -> StartDecision {
    if !startup_busy {
        StartDecision::Go
    } else if waited < MAX_BUSY_WAIT {
        StartDecision::Wait
    } else {
        StartDecision::GiveUp
    }
}

/// Résultat d'une tentative de pré-chargement, pour le compte rendu final.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PreloadOutcome {
    Installed(ExternalTool),
    Failed { tool: ExternalTool, error: String },
}

/// Exécute le plan, un outil après l'autre.
///
/// Séquentiel et non parallèle : deux téléchargements simultanés ne feraient
/// que se partager la même bande passante, en retardant les deux. Un échec
/// n'interrompt pas la suite (les outils sont indépendants) et aucun outil
/// n'est retenté — la politique de réessai est décrite en tête de module.
pub(crate) async fn run_plan<F, Fut>(plan: &[ExternalTool], mut install: F) -> Vec<PreloadOutcome>
where
    F: FnMut(ExternalTool) -> Fut,
    Fut: std::future::Future<Output = Result<PathBuf, String>>,
{
    let mut outcomes = Vec::with_capacity(plan.len());
    for &tool in plan {
        let started = Instant::now();
        info!(
            "Pré-chargement de {} (~{} Mo)…",
            tool.label(),
            tool.approx_size_mb()
        );
        match install(tool).await {
            Ok(path) => {
                info!(
                    "{} prêt en {} s ({})",
                    tool.label(),
                    started.elapsed().as_secs(),
                    path.display()
                );
                outcomes.push(PreloadOutcome::Installed(tool));
            }
            Err(error) => {
                // Silencieux pour l'utilisateur, explicite ici : l'onglet
                // Fichier retentera le téléchargement s'il en a besoin.
                warn!("Pré-chargement de {} impossible: {error}", tool.label());
                outcomes.push(PreloadOutcome::Failed { tool, error });
            }
        }
    }
    outcomes
}

/// Une reprise de téléchargement est-elle acceptable pour cette ressource ?
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ResumePolicy {
    /// URL figée sur une version : le fichier distant ne changera pas, un
    /// `.part` d'une session précédente peut être complété sans risque.
    Allowed,
    /// URL de type « dernière version » : le fichier distant change à chaque
    /// publication. Compléter un `.part` téléchargé avant une mise à jour
    /// collerait deux versions bout à bout, et faute de SHA-256 de référence
    /// rien ne le détecterait : le binaire serait cassé pour de bon (il
    /// existe, donc plus personne ne le retélécharge). On repart de zéro.
    Forbidden,
}

/// Prépare le fichier de reprise et renvoie le nombre d'octets déjà présents
/// (0 si le `.part` a été supprimé ou n'existait pas).
pub(crate) fn prepare_part(part: &Path, resume: ResumePolicy) -> u64 {
    if resume == ResumePolicy::Forbidden {
        discard_unresumable_part(part, resume);
        return 0;
    }
    std::fs::metadata(part).map(|m| m.len()).unwrap_or(0)
}

/// Efface le fichier de reprise laissé par un téléchargement raté quand la
/// politique interdit de le reprendre : il ne servira jamais et n'est plus
/// que des dizaines de méga-octets perdus sur le disque de l'utilisateur.
pub(crate) fn discard_unresumable_part(part: &Path, resume: ResumePolicy) {
    if resume == ResumePolicy::Forbidden {
        let _ = std::fs::remove_file(part);
    }
}

/// Rend exécutable le fichier complet et le met en place sous son nom final.
///
/// Le bit exécutable est posé **avant** le renommage : le chemin final
/// n'apparaît qu'une fois le binaire complet et exécutable, jamais dans un
/// état intermédiaire qu'un autre appelant prendrait pour un outil prêt.
pub(crate) fn install_part(part: &Path, dest: &Path) -> Result<(), String> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(part, std::fs::Permissions::from_mode(0o755))
            .map_err(|e| format!("permissions impossibles: {e}"))?;
    }
    // Un binaire déjà en place ne doit jamais être écrasé : il peut être en
    // cours d'exécution (yt-dlp qui télécharge, ffmpeg qui convertit), et le
    // remplacer sous les pieds de l'outil échouerait sous Windows.
    if dest.exists() {
        let _ = std::fs::remove_file(part);
        return Ok(());
    }
    std::fs::rename(part, dest).map_err(|e| format!("installation impossible: {e}"))?;
    Ok(())
}

/// Télécharge un exécutable et l'installe à `dest`.
///
/// Passe par [`crate::download`] — le même téléchargeur que le modèle de
/// dictée et le moteur de résumé — pour trois raisons qui comptent sur 80 Mo :
/// des délais de connexion et d'inactivité (un proxy d'entreprise qui ne
/// répond jamais ne fige plus le téléchargement indéfiniment), l'écriture au
/// fil de l'eau dans un `.part` au lieu de garder tout le fichier en mémoire,
/// et une classification des pannes réseau lisible dans le journal. Faute de
/// SHA-256 publié pour ces binaires, l'intégrité repose sur le contrôle
/// de taille de `download_to_part` (`Content-Length`) — d'où l'interdiction de
/// reprise sur les URL mouvantes, voir [`ResumePolicy`].
pub(crate) async fn install_executable(
    url: &str,
    dest: &Path,
    resume: ResumePolicy,
) -> Result<(), String> {
    let part = crate::download::part_path(dest);
    let resumed_from = prepare_part(&part, resume);
    if resumed_from > 0 {
        debug!(
            "Reprise du téléchargement de {} à {} octets",
            dest.display(),
            resumed_from
        );
    }
    let client =
        crate::download::build_client().map_err(|e| format!("client HTTP indisponible: {e}"))?;
    // Jeton jamais annulé : ces téléchargements n'ont pas de bouton
    // d'annulation. Une fermeture de l'application coupe le processus, le
    // `.part` reste sur le disque et sert de point de reprise.
    let cancel = CancellationToken::new();
    if let Err(e) = crate::download::download_to_part(&client, url, &part, None, &cancel, |_| {})
        .await
        .map_err(|e| e.to_string())
    {
        discard_unresumable_part(&part, resume);
        return Err(e);
    }
    install_part(&part, dest)
}

/// Valeur d'environnement considérée comme « activée » (jumelle de celle de
/// `overlay.rs`, qui est réservée à Linux).
fn flag_enabled(value: Option<&str>) -> bool {
    match value {
        Some(v) => !matches!(
            v.trim().to_ascii_lowercase().as_str(),
            "" | "0" | "false" | "no" | "off"
        ),
        None => false,
    }
}

/// L'application a-t-elle mieux à faire du réseau que nos outils ?
///
/// Deux situations, toutes deux propres au premier lancement :
///
/// - l'accueil n'est pas terminé : l'utilisateur est sur l'écran de
///   téléchargement du modèle français, il ne peut rien faire tant qu'il n'est
///   pas arrivé (`onboarding_completed` passe à vrai à la sélection du modèle,
///   juste après son téléchargement) ;
/// - un téléchargement de modèle est en cours (reprise après un échec,
///   retéléchargement manuel).
///
/// Pour le second point, on lit un instantané du registre des modèles. Un
/// rescan concurrent peut brièvement remettre `is_downloading` à faux pendant
/// un téléchargement bien réel ; le pire cas est que le pré-chargement démarre
/// un peu trop tôt, sans conséquence fonctionnelle.
fn startup_busy(app: &AppHandle) -> bool {
    if !crate::settings::get_settings(app).onboarding_completed {
        return true;
    }
    match app.try_state::<Arc<ModelManager>>() {
        Some(manager) => manager
            .get_available_models()
            .iter()
            .any(|model| model.is_downloading),
        None => false,
    }
}

/// Récupère l'outil demandé par le **même** chemin que l'interface, pour que
/// le verrou d'installation de chaque outil sérialise les deux.
async fn install_tool(app: &AppHandle, tool: ExternalTool) -> Result<PathBuf, String> {
    match tool {
        ExternalTool::Ffmpeg => crate::commands::video_download::ensure_ffmpeg(app).await,
        ExternalTool::QuickJs => crate::commands::file_transcription::ensure_quickjs(app).await,
        ExternalTool::YtDlp => {
            #[cfg(target_os = "macos")]
            {
                crate::commands::file_transcription::ensure_yt_dlp_macos(app).await
            }
            // Hors macOS, yt-dlp est embarqué : `preload_plan` ne le met jamais
            // au plan, ce bras n'existe que pour la complétude du `match`.
            #[cfg(not(target_os = "macos"))]
            {
                let _ = app;
                Err("yt-dlp est embarqué en sidecar sur cette plateforme".to_string())
            }
        }
    }
}

/// Lance le pré-chargement en tâche de fond et rend la main immédiatement.
///
/// `headless` est transmis bien que l'appel se fasse déjà après l'aiguillage
/// headless de `lib.rs` : la règle « aucun réseau en mode headless » reste
/// ainsi vraie même si l'appel venait à être déplacé.
pub(crate) fn spawn_preload(app: &AppHandle, headless: bool) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move { preload_task(app, headless).await });
}

async fn preload_task(app: AppHandle, headless: bool) {
    #[cfg(target_os = "macos")]
    let yt_dlp_installed = crate::commands::file_transcription::yt_dlp_bin_path(&app)
        .map(|path| path.exists())
        .unwrap_or(false);
    #[cfg(not(target_os = "macos"))]
    let yt_dlp_installed = false;

    let ctx = PreloadContext {
        headless,
        disabled: flag_enabled(std::env::var(DISABLE_ENV).ok().as_deref()),
        yt_dlp_bundled: !cfg!(target_os = "macos"),
        yt_dlp_installed,
        ffmpeg_installed: crate::commands::video_download::ffmpeg_bin_path(&app)
            .map(|path| path.exists())
            .unwrap_or(false),
        quickjs_installed: crate::commands::file_transcription::quickjs_bin_path(&app)
            .map(|path| path.exists())
            .unwrap_or(false),
    };

    let plan = preload_plan(&ctx);
    if plan.is_empty() {
        debug!("Pré-chargement des outils : rien à faire ({ctx:?})");
        return;
    }

    // Le plan est établi avant d'attendre : le cas courant (outils déjà là)
    // ne laisse même pas de minuterie derrière lui.
    tokio::time::sleep(STARTUP_GRACE).await;

    let waiting_since = Instant::now();
    loop {
        match start_decision(startup_busy(&app), waiting_since.elapsed()) {
            StartDecision::Go => break,
            StartDecision::Wait => tokio::time::sleep(BUSY_POLL_INTERVAL).await,
            StartDecision::GiveUp => {
                info!(
                    "Pré-chargement des outils abandonné pour cette session : l'accueil ou le téléchargement du modèle dure encore ; nouvelle tentative au prochain démarrage."
                );
                return;
            }
        }
    }

    let outcomes = run_plan(&plan, |tool| {
        let app = app.clone();
        async move { install_tool(&app, tool).await }
    })
    .await;

    let failed = outcomes
        .iter()
        .filter(|outcome| matches!(outcome, PreloadOutcome::Failed { .. }))
        .count();
    if failed == 0 {
        info!("Pré-chargement des outils de l'onglet Fichier terminé");
    } else {
        warn!(
            "Pré-chargement incomplet : {failed} outil(s) manquant(s) ; aucune nouvelle tentative avant le prochain démarrage ou le premier usage de l'onglet Fichier."
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// Installation neuve sur macOS : aucun des outils n'est là, et yt-dlp
    /// n'y est pas embarqué en sidecar.
    fn fresh_macos() -> PreloadContext {
        PreloadContext {
            headless: false,
            disabled: false,
            yt_dlp_bundled: false,
            yt_dlp_installed: false,
            ffmpeg_installed: false,
            quickjs_installed: false,
        }
    }

    #[test]
    fn headless_mode_preloads_nothing() {
        let ctx = PreloadContext {
            headless: true,
            ..fresh_macos()
        };
        assert_eq!(preload_plan(&ctx), Vec::new());
    }

    #[test]
    fn the_escape_hatch_disables_the_whole_preload() {
        let ctx = PreloadContext {
            disabled: true,
            ..fresh_macos()
        };
        assert_eq!(preload_plan(&ctx), Vec::new());
    }

    #[test]
    fn a_fresh_macos_install_preloads_quickjs_then_yt_dlp_then_ffmpeg() {
        assert_eq!(
            preload_plan(&fresh_macos()),
            vec![
                ExternalTool::QuickJs,
                ExternalTool::YtDlp,
                ExternalTool::Ffmpeg
            ]
        );
    }

    #[test]
    fn a_bundled_yt_dlp_leaves_quickjs_and_ffmpeg_to_preload() {
        let ctx = PreloadContext {
            yt_dlp_bundled: true,
            ..fresh_macos()
        };
        assert_eq!(
            preload_plan(&ctx),
            vec![ExternalTool::QuickJs, ExternalTool::Ffmpeg]
        );
    }

    /// QuickJS est nécessaire partout, y compris là où yt-dlp est embarqué en
    /// sidecar (Windows, Linux) : c'est justement le cas où l'absence de
    /// moteur JavaScript était la seule chose qui manquait pour YouTube.
    #[test]
    fn quickjs_is_preloaded_even_where_yt_dlp_is_bundled() {
        let ctx = PreloadContext {
            yt_dlp_bundled: true,
            ffmpeg_installed: true,
            ..fresh_macos()
        };
        assert_eq!(preload_plan(&ctx), vec![ExternalTool::QuickJs]);
    }

    #[test]
    fn tools_already_on_disk_are_not_downloaded_again() {
        let ctx = PreloadContext {
            yt_dlp_installed: true,
            ffmpeg_installed: true,
            quickjs_installed: true,
            ..fresh_macos()
        };
        assert_eq!(preload_plan(&ctx), Vec::new());
        let ctx = PreloadContext {
            ffmpeg_installed: true,
            quickjs_installed: true,
            ..fresh_macos()
        };
        assert_eq!(preload_plan(&ctx), vec![ExternalTool::YtDlp]);
    }

    #[test]
    fn an_idle_startup_begins_the_preload_right_away() {
        assert_eq!(
            start_decision(false, Duration::from_secs(0)),
            StartDecision::Go
        );
        assert_eq!(
            start_decision(false, MAX_BUSY_WAIT + Duration::from_secs(1)),
            StartDecision::Go
        );
    }

    #[test]
    fn an_onboarding_or_model_download_defers_the_preload() {
        assert_eq!(
            start_decision(true, Duration::from_secs(0)),
            StartDecision::Wait
        );
        assert_eq!(
            start_decision(true, MAX_BUSY_WAIT - Duration::from_secs(1)),
            StartDecision::Wait
        );
    }

    #[test]
    fn the_preload_is_abandoned_after_the_maximum_wait() {
        assert_eq!(start_decision(true, MAX_BUSY_WAIT), StartDecision::GiveUp);
    }

    #[test]
    fn only_a_truthy_environment_value_disables_the_preload() {
        assert!(!flag_enabled(None));
        assert!(!flag_enabled(Some("")));
        assert!(!flag_enabled(Some("0")));
        assert!(!flag_enabled(Some("false")));
        assert!(!flag_enabled(Some("OFF")));
        assert!(flag_enabled(Some("1")));
        assert!(flag_enabled(Some("true")));
        assert!(flag_enabled(Some(" yes ")));
    }

    type BoxedInstall =
        std::pin::Pin<Box<dyn std::future::Future<Output = Result<PathBuf, String>> + Send>>;

    /// Faux installateur : journalise le début et la fin de chaque outil, et
    /// échoue pour ceux qu'on lui désigne.
    fn recording_installer(
        trace: Arc<Mutex<Vec<String>>>,
        failures: Vec<ExternalTool>,
    ) -> impl FnMut(ExternalTool) -> BoxedInstall {
        move |tool| {
            let trace = trace.clone();
            let fails = failures.contains(&tool);
            Box::pin(async move {
                trace
                    .lock()
                    .unwrap()
                    .push(format!("start {}", tool.label()));
                tokio::task::yield_now().await;
                trace.lock().unwrap().push(format!("end {}", tool.label()));
                if fails {
                    Err("réseau injoignable".to_string())
                } else {
                    Ok(PathBuf::from("/tmp").join(tool.label()))
                }
            })
        }
    }

    #[tokio::test]
    async fn the_plan_runs_one_tool_after_another() {
        let trace = Arc::new(Mutex::new(Vec::new()));
        let outcomes = run_plan(
            &[ExternalTool::YtDlp, ExternalTool::Ffmpeg],
            recording_installer(trace.clone(), Vec::new()),
        )
        .await;

        // Aucun chevauchement : le second outil ne démarre qu'une fois le
        // premier terminé (on ne sature pas le lien réseau de l'utilisateur).
        assert_eq!(
            *trace.lock().unwrap(),
            vec!["start yt-dlp", "end yt-dlp", "start ffmpeg", "end ffmpeg"]
        );
        assert_eq!(
            outcomes,
            vec![
                PreloadOutcome::Installed(ExternalTool::YtDlp),
                PreloadOutcome::Installed(ExternalTool::Ffmpeg),
            ]
        );
    }

    #[tokio::test]
    async fn a_failure_does_not_stop_the_next_tool() {
        let trace = Arc::new(Mutex::new(Vec::new()));
        let outcomes = run_plan(
            &[ExternalTool::YtDlp, ExternalTool::Ffmpeg],
            recording_installer(trace.clone(), vec![ExternalTool::YtDlp]),
        )
        .await;

        assert_eq!(
            outcomes,
            vec![
                PreloadOutcome::Failed {
                    tool: ExternalTool::YtDlp,
                    error: "réseau injoignable".to_string(),
                },
                PreloadOutcome::Installed(ExternalTool::Ffmpeg),
            ]
        );
    }

    #[tokio::test]
    async fn a_failing_tool_is_attempted_only_once_per_session() {
        let trace = Arc::new(Mutex::new(Vec::new()));
        let outcomes = run_plan(
            &[ExternalTool::Ffmpeg],
            recording_installer(
                trace.clone(),
                vec![ExternalTool::YtDlp, ExternalTool::Ffmpeg],
            ),
        )
        .await;

        // Pas de boucle de réessai : un seul passage, quoi qu'il arrive.
        assert_eq!(*trace.lock().unwrap(), vec!["start ffmpeg", "end ffmpeg"]);
        assert_eq!(outcomes.len(), 1);
    }

    #[test]
    fn a_forbidden_resume_wipes_the_stale_partial_file() {
        let dir = tempfile::tempdir().unwrap();
        let part = dir.path().join("yt-dlp.part");
        std::fs::write(&part, "ancienne version").unwrap();

        assert_eq!(prepare_part(&part, ResumePolicy::Forbidden), 0);
        assert!(!part.exists());
    }

    #[test]
    fn an_allowed_resume_keeps_the_partial_file() {
        let dir = tempfile::tempdir().unwrap();
        let part = dir.path().join("ffmpeg.part");
        std::fs::write(&part, "0123456789").unwrap();

        assert_eq!(prepare_part(&part, ResumePolicy::Allowed), 10);
        assert!(part.exists());
    }

    #[test]
    fn a_failed_download_keeps_only_a_partial_file_it_can_resume() {
        let dir = tempfile::tempdir().unwrap();
        let resumable = dir.path().join("ffmpeg.part");
        let doomed = dir.path().join("yt-dlp.part");
        std::fs::write(&resumable, "octets reçus").unwrap();
        std::fs::write(&doomed, "octets reçus").unwrap();

        discard_unresumable_part(&resumable, ResumePolicy::Allowed);
        discard_unresumable_part(&doomed, ResumePolicy::Forbidden);

        assert!(resumable.exists(), "la reprise doit rester possible");
        assert!(
            !doomed.exists(),
            "un fichier jamais repris est de la place perdue"
        );
    }

    #[test]
    fn installing_renames_the_completed_file_and_makes_it_executable() {
        let dir = tempfile::tempdir().unwrap();
        let part = dir.path().join("ffmpeg.part");
        let dest = dir.path().join("ffmpeg");
        std::fs::write(&part, "binaire").unwrap();

        install_part(&part, &dest).unwrap();

        assert!(!part.exists());
        assert_eq!(std::fs::read(&dest).unwrap(), "binaire".as_bytes());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&dest).unwrap().permissions().mode();
            assert_eq!(mode & 0o111, 0o111, "le bit exécutable doit être posé");
        }
    }

    #[test]
    fn installing_gives_up_when_the_binary_is_already_there() {
        let dir = tempfile::tempdir().unwrap();
        let part = dir.path().join("ffmpeg.part");
        let dest = dir.path().join("ffmpeg");
        std::fs::write(&part, "notre copie").unwrap();
        std::fs::write(&dest, "copie déjà installée").unwrap();

        install_part(&part, &dest).unwrap();

        // Le binaire en place n'est jamais écrasé (un autre appelant a pu
        // l'installer pendant notre téléchargement), et le `.part` est nettoyé.
        assert_eq!(
            std::fs::read(&dest).unwrap(),
            "copie déjà installée".as_bytes()
        );
        assert!(!part.exists());
    }
}
