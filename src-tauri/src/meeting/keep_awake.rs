//! Empêche la mise en veille automatique pendant un enregistrement : une
//! réunion de plusieurs heures sans toucher au clavier endormirait sinon
//! l'ordinateur, et la capture s'arrêterait. L'écran peut s'éteindre ; seule
//! la veille du système est retenue. Refermer l'écran d'un portable met
//! toujours l'ordinateur en veille : aucune application ne peut l'empêcher.

use std::marker::PhantomData;

/// Garde RAII : la veille redevient possible quand elle est libérée. Sous
/// Windows le réglage est propre au fil d'exécution qui l'a posé : la garde
/// n'est donc pas `Send` et vit sur le fil de l'enregistrement.
pub struct KeepAwake {
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    child: Option<std::process::Child>,
    _not_send: PhantomData<*const ()>,
}

impl KeepAwake {
    pub fn acquire() -> Self {
        #[cfg(target_os = "windows")]
        {
            use windows::Win32::System::Power::{
                SetThreadExecutionState, ES_CONTINUOUS, ES_SYSTEM_REQUIRED,
            };
            // SAFETY: appel Win32 sans pointeur ; la valeur précédente
            // renvoyée (0 en cas d'échec) n'a pas besoin d'être conservée.
            let previous = unsafe { SetThreadExecutionState(ES_CONTINUOUS | ES_SYSTEM_REQUIRED) };
            if previous.0 == 0 {
                log::warn!("Réunion : impossible d'empêcher la mise en veille");
            }
            Self {
                _not_send: PhantomData,
            }
        }

        #[cfg(target_os = "macos")]
        {
            // `-w` : caffeinate s'arrête de lui-même si l'application disparaît.
            let child = std::process::Command::new("caffeinate")
                .args(["-i", "-w", &std::process::id().to_string()])
                .spawn()
                .map_err(|e| log::warn!("Réunion : caffeinate indisponible ({e})"))
                .ok();
            Self {
                child,
                _not_send: PhantomData,
            }
        }

        #[cfg(target_os = "linux")]
        {
            // `tail --pid` rend la main dès que l'application disparaît, ce qui
            // lève l'inhibition même après un plantage.
            let child = std::process::Command::new("systemd-inhibit")
                .args([
                    "--what=idle:sleep",
                    "--who=OCADE Dictée",
                    "--why=Enregistrement de réunion",
                    "--mode=block",
                    "tail",
                    &format!("--pid={}", std::process::id()),
                    "-f",
                    "/dev/null",
                ])
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn()
                .map_err(|e| log::warn!("Réunion : systemd-inhibit indisponible ({e})"))
                .ok();
            Self {
                child,
                _not_send: PhantomData,
            }
        }

        #[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
        {
            Self {
                _not_send: PhantomData,
            }
        }
    }
}

impl Drop for KeepAwake {
    fn drop(&mut self) {
        #[cfg(target_os = "windows")]
        {
            use windows::Win32::System::Power::{SetThreadExecutionState, ES_CONTINUOUS};
            // SAFETY: voir `acquire`.
            unsafe {
                SetThreadExecutionState(ES_CONTINUOUS);
            }
        }

        #[cfg(any(target_os = "macos", target_os = "linux"))]
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}
