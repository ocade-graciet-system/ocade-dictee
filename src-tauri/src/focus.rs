//! Destination du texte dicté : l'application où l'on se trouvait au lancement
//! de la dictée.
//!
//! Le collage final simule un Cmd+V dans l'application qui a le focus *au
//! moment du collage*. Or une fenêtre peut s'interposer pendant la dictée —
//! demande d'accès à un fichier, notification, dialogue système — et c'est
//! alors elle qui reçoit le texte, perdu pour l'utilisateur. On mémorise donc
//! l'application active au lancement et on la ramène au premier plan juste
//! avant de coller, si elle l'a perdu entre-temps.
//!
//! Règle retenue : **toujours l'application d'origine**, y compris si
//! l'utilisateur a lui-même changé d'application pendant la dictée. Elle est
//! simple à prévoir, et c'est la seule qui couvre à coup sûr les fenêtres
//! système — qu'on ne saurait pas distinguer de façon fiable d'un changement
//! volontaire.
//!
//! macOS uniquement pour l'instant ; ailleurs, le comportement historique est
//! conservé (collage dans l'application active).

use std::sync::Mutex;

/// Application (pid) où la dictée en cours a été lancée.
static DICTATION_TARGET: Mutex<Option<i32>> = Mutex::new(None);

/// Application à ramener au premier plan avant de coller, s'il y en a une.
/// `target` est l'application mémorisée au lancement, `current` celle qui a
/// le focus au moment du collage (`None` si illisible), `own` le pid d'Ocade
/// Dictée — jamais une destination : une dictée lancée depuis sa propre
/// fenêtre garde le comportement habituel.
fn restore_decision(target: Option<i32>, current: Option<i32>, own: i32) -> Option<i32> {
    target.filter(|&pid| pid != own && current != Some(pid))
}

/// À appeler au lancement de la dictée.
pub fn remember_dictation_target() {
    let target = platform::frontmost_pid();
    *DICTATION_TARGET.lock().unwrap_or_else(|e| e.into_inner()) = target;
    log::debug!("Destination de la dictée mémorisée : pid {target:?}");
}

/// À appeler juste avant le collage, **hors du thread principal** : sur macOS,
/// c'est lui qui met à jour l'application au premier plan, et l'attente de
/// confirmation ne verrait jamais le changement s'il était bloqué.
pub fn restore_dictation_target() {
    let target = DICTATION_TARGET
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .take();
    let own = std::process::id() as i32;
    if let Some(pid) = restore_decision(target, platform::frontmost_pid(), own) {
        platform::bring_to_front(pid);
    }
}

#[cfg(target_os = "macos")]
mod platform {
    use log::{info, warn};
    use objc2::rc::autoreleasepool;
    use objc2_app_kit::{NSApplicationActivationOptions, NSRunningApplication, NSWorkspace};
    use std::time::{Duration, Instant};

    /// Délai maximal pour que l'application ramenée passe effectivement au
    /// premier plan.
    const ACTIVATION_TIMEOUT: Duration = Duration::from_millis(600);
    const POLL_INTERVAL: Duration = Duration::from_millis(20);

    /// Temps laissé à l'application réactivée pour rétablir sa fenêtre clé et
    /// le champ qui avait le curseur, avant que le Cmd+V n'arrive.
    const SETTLE: Duration = Duration::from_millis(80);

    pub(super) fn frontmost_pid() -> Option<i32> {
        autoreleasepool(|_| {
            NSWorkspace::sharedWorkspace()
                .frontmostApplication()
                .map(|app| app.processIdentifier())
        })
    }

    /// Ramène `pid` au premier plan et attend de le constater. En cas d'échec
    /// (application fermée, activation refusée par le système), on n'insiste
    /// pas : le collage part dans l'application active, comme avant ce
    /// correctif — aucune régression possible.
    pub(super) fn bring_to_front(pid: i32) {
        let activated = autoreleasepool(|_| {
            NSRunningApplication::runningApplicationWithProcessIdentifier(pid).is_some_and(|app| {
                !app.isTerminated()
                    && app.activateWithOptions(NSApplicationActivationOptions::empty())
            })
        });
        if !activated {
            warn!("Destination de la dictée (pid {pid}) introuvable ou non réactivable ; collage dans l'application active");
            return;
        }

        let deadline = Instant::now() + ACTIVATION_TIMEOUT;
        while Instant::now() < deadline {
            if frontmost_pid() == Some(pid) {
                std::thread::sleep(SETTLE);
                info!("Focus rendu à la destination de la dictée (pid {pid}) avant le collage");
                return;
            }
            std::thread::sleep(POLL_INTERVAL);
        }
        warn!("La destination de la dictée (pid {pid}) n'est pas revenue au premier plan en {ACTIVATION_TIMEOUT:?} ; collage dans l'application active");
    }
}

#[cfg(not(target_os = "macos"))]
mod platform {
    pub(super) fn frontmost_pid() -> Option<i32> {
        None
    }

    pub(super) fn bring_to_front(_pid: i32) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    const OWN: i32 = 100;
    const EDITOR: i32 = 200;
    const DIALOG: i32 = 300;

    /// Le cas signalé : une fenêtre système (demande d'accès à un fichier,
    /// notification) a pris le focus pendant la dictée.
    #[test]
    fn a_window_that_stole_the_focus_hands_it_back_to_the_dictation_target() {
        assert_eq!(
            restore_decision(Some(EDITOR), Some(DIALOG), OWN),
            Some(EDITOR)
        );
    }

    #[test]
    fn nothing_happens_when_the_target_still_has_the_focus() {
        assert_eq!(restore_decision(Some(EDITOR), Some(EDITOR), OWN), None);
    }

    /// Dictée lancée depuis la fenêtre d'Ocade Dictée elle-même : ce n'est pas
    /// une destination de collage, on laisse le comportement habituel.
    #[test]
    fn our_own_window_is_never_a_restore_target() {
        assert_eq!(restore_decision(Some(OWN), Some(DIALOG), OWN), None);
    }

    #[test]
    fn nothing_happens_without_a_remembered_target() {
        assert_eq!(restore_decision(None, Some(DIALOG), OWN), None);
    }

    /// Premier plan illisible au moment du collage : on tente quand même de
    /// ramener la cible, c'est la seule information fiable dont on dispose.
    #[test]
    fn an_unreadable_frontmost_app_still_restores_the_target() {
        assert_eq!(restore_decision(Some(EDITOR), None, OWN), Some(EDITOR));
    }
}
