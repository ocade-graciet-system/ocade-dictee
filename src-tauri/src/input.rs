use enigo::{Enigo, Key, Keyboard, Mouse, Settings};
use std::sync::Mutex;
use tauri::{AppHandle, Manager};

/// Exécute `f` SUR LE MAIN THREAD et renvoie son résultat de façon synchrone.
///
/// Pourquoi : sur macOS, `Enigo::new()` et la frappe de caractères
/// (`enigo.text()` / `enigo.key(Key::Unicode(..))`) lisent la disposition
/// clavier via les API Text Input Sources (`TISCopyCurrentKeyboardInputSource`,
/// `kTISPropertyUnicodeKeyLayoutData`, …). Ces API HIToolbox DOIVENT tourner sur
/// le main thread ; macOS 15+ le fait respecter via `dispatch_assert_queue`, qui
/// **tue le process** en cas de violation. Ce helper garantit ce dispatch.
///
/// ⚠️ Ne JAMAIS appeler depuis le main thread : la closure est postée sur la file
/// principale et l'on bloque sur `recv()`, ce qui provoquerait un deadlock. Les
/// appelants actuels (commande Tauri `initialize_enigo`, threads d'action) sont
/// tous hors main thread.
pub fn run_on_main_thread_blocking<T, F>(app: &AppHandle, f: F) -> Result<T, String>
where
    T: Send + 'static,
    F: FnOnce() -> T + Send + 'static,
{
    let (tx, rx) = std::sync::mpsc::channel();
    app.run_on_main_thread(move || {
        // Si le récepteur a disparu (appelant abandonné), on ignore l'erreur.
        let _ = tx.send(f());
    })
    .map_err(|e| format!("run_on_main_thread a échoué: {e}"))?;
    rx.recv()
        .map_err(|e| format!("la closure main-thread n'a pas tourné: {e}"))
}

/// Wrapper for Enigo to store in Tauri's managed state.
/// Enigo is wrapped in a Mutex since it requires mutable access.
pub struct EnigoState(pub Mutex<Enigo>);

impl EnigoState {
    /// Construit l'instance Enigo **sur le main thread**. `Enigo::new()` lit la
    /// disposition clavier via les API TIS (voir [`run_on_main_thread_blocking`]),
    /// donc l'appeler depuis un thread secondaire tue le process sur macOS 15+.
    /// À appeler depuis un thread secondaire uniquement (jamais le main thread).
    pub fn new_on_main_thread(app: &AppHandle) -> Result<Self, String> {
        run_on_main_thread_blocking(app, || {
            Enigo::new(&Settings::default())
                .map(|enigo| Self(Mutex::new(enigo)))
                .map_err(|e| format!("Failed to initialize Enigo: {}", e))
        })?
    }
}

/// Get the current mouse cursor position using the managed Enigo instance.
/// Returns None if the state is not available or if getting the location fails.
pub fn get_cursor_position(app_handle: &AppHandle) -> Option<(i32, i32)> {
    let enigo_state = app_handle.try_state::<EnigoState>()?;
    let enigo = enigo_state.0.lock().ok()?;
    enigo.location().ok()
}

/// Sends a Ctrl+V or Cmd+V paste command using platform-specific virtual key codes.
/// This ensures the paste works regardless of keyboard layout (e.g., Russian, AZERTY, DVORAK).
/// Note: On Wayland, this may not work - callers should check for Wayland and use alternative methods.
pub fn send_paste_ctrl_v(enigo: &mut Enigo) -> Result<(), String> {
    // Platform-specific key definitions
    #[cfg(target_os = "macos")]
    let (modifier_key, v_key_code) = (Key::Meta, Key::Other(9));
    #[cfg(target_os = "windows")]
    let (modifier_key, v_key_code) = (Key::Control, Key::Other(0x56)); // VK_V
    #[cfg(target_os = "linux")]
    let (modifier_key, v_key_code) = (Key::Control, Key::Unicode('v'));

    // Press modifier + V
    enigo
        .key(modifier_key, enigo::Direction::Press)
        .map_err(|e| format!("Failed to press modifier key: {}", e))?;
    enigo
        .key(v_key_code, enigo::Direction::Click)
        .map_err(|e| format!("Failed to click V key: {}", e))?;

    std::thread::sleep(std::time::Duration::from_millis(100));

    enigo
        .key(modifier_key, enigo::Direction::Release)
        .map_err(|e| format!("Failed to release modifier key: {}", e))?;

    Ok(())
}

/// Sends a Ctrl+Shift+V paste command.
/// This is commonly used in terminal applications on Linux to paste without formatting.
/// Note: On Wayland, this may not work - callers should check for Wayland and use alternative methods.
pub fn send_paste_ctrl_shift_v(enigo: &mut Enigo) -> Result<(), String> {
    // Platform-specific key definitions
    #[cfg(target_os = "macos")]
    let (modifier_key, v_key_code) = (Key::Meta, Key::Other(9)); // Cmd+Shift+V on macOS
    #[cfg(target_os = "windows")]
    let (modifier_key, v_key_code) = (Key::Control, Key::Other(0x56)); // VK_V
    #[cfg(target_os = "linux")]
    let (modifier_key, v_key_code) = (Key::Control, Key::Unicode('v'));

    // Press Ctrl/Cmd + Shift + V
    enigo
        .key(modifier_key, enigo::Direction::Press)
        .map_err(|e| format!("Failed to press modifier key: {}", e))?;
    enigo
        .key(Key::Shift, enigo::Direction::Press)
        .map_err(|e| format!("Failed to press Shift key: {}", e))?;
    enigo
        .key(v_key_code, enigo::Direction::Click)
        .map_err(|e| format!("Failed to click V key: {}", e))?;

    std::thread::sleep(std::time::Duration::from_millis(100));

    enigo
        .key(Key::Shift, enigo::Direction::Release)
        .map_err(|e| format!("Failed to release Shift key: {}", e))?;
    enigo
        .key(modifier_key, enigo::Direction::Release)
        .map_err(|e| format!("Failed to release modifier key: {}", e))?;

    Ok(())
}

/// Sends a Shift+Insert paste command (Windows and Linux only).
/// This is more universal for terminal applications and legacy software.
/// Note: On Wayland, this may not work - callers should check for Wayland and use alternative methods.
pub fn send_paste_shift_insert(enigo: &mut Enigo) -> Result<(), String> {
    #[cfg(target_os = "windows")]
    let insert_key_code = Key::Other(0x2D); // VK_INSERT
    #[cfg(not(target_os = "windows"))]
    let insert_key_code = Key::Other(0x76); // XK_Insert (keycode 118 / 0x76, also used as fallback)

    // Press Shift + Insert
    enigo
        .key(Key::Shift, enigo::Direction::Press)
        .map_err(|e| format!("Failed to press Shift key: {}", e))?;
    enigo
        .key(insert_key_code, enigo::Direction::Click)
        .map_err(|e| format!("Failed to click Insert key: {}", e))?;

    std::thread::sleep(std::time::Duration::from_millis(100));

    enigo
        .key(Key::Shift, enigo::Direction::Release)
        .map_err(|e| format!("Failed to release Shift key: {}", e))?;

    Ok(())
}

/// Pastes text directly using the enigo text method.
/// This tries to use system input methods if possible, otherwise simulates keystrokes one by one.
pub fn paste_text_direct(enigo: &mut Enigo, text: &str) -> Result<(), String> {
    enigo
        .text(text)
        .map_err(|e| format!("Failed to send text directly: {}", e))?;

    Ok(())
}

/// Transforme un raccourci textuel ("ctrl+alt+shift+m") en séquence de touches
/// enigo. Le dernier token est la touche principale, les précédents sont des
/// modificateurs. Utilisé pour ENVOYER un raccourci (pas pour l'écouter).
pub fn parse_shortcut_to_keys(shortcut: &str) -> Result<Vec<enigo::Key>, String> {
    use enigo::Key;
    let tokens: Vec<&str> = shortcut
        .split('+')
        .map(|t| t.trim())
        .filter(|t| !t.is_empty())
        .collect();
    if tokens.is_empty() {
        return Err("raccourci vide".into());
    }
    let (main, mods) = tokens.split_last().unwrap();
    let mut keys: Vec<Key> = Vec::new();
    for m in mods {
        let key = match m.to_ascii_lowercase().as_str() {
            "ctrl" | "control" => Key::Control,
            "alt" | "option" => Key::Alt,
            "shift" => Key::Shift,
            "cmd" | "meta" | "super" | "win" => Key::Meta,
            other => return Err(format!("modificateur inconnu: {other}")),
        };
        keys.push(key);
    }
    let main_lc = main.to_ascii_lowercase();
    let main_key = if main_lc.len() == 1 && main_lc.chars().next().unwrap().is_ascii_alphabetic() {
        Key::Unicode(main_lc.chars().next().unwrap())
    } else if let Some(n) = main_lc.strip_prefix('f').and_then(|n| n.parse::<u8>().ok()) {
        // F1..F20 (F21-F24 n'existent pas dans enigo 0.6.1 sur macOS).
        const F_KEYS: [Key; 20] = [
            Key::F1,
            Key::F2,
            Key::F3,
            Key::F4,
            Key::F5,
            Key::F6,
            Key::F7,
            Key::F8,
            Key::F9,
            Key::F10,
            Key::F11,
            Key::F12,
            Key::F13,
            Key::F14,
            Key::F15,
            Key::F16,
            Key::F17,
            Key::F18,
            Key::F19,
            Key::F20,
        ];
        match n
            .checked_sub(1)
            .and_then(|i| F_KEYS.get(i as usize))
            .copied()
        {
            Some(k) => k,
            None => return Err(format!("touche fonction non supportée: f{n}")),
        }
    } else {
        return Err(format!("touche principale non supportée: {main}"));
    };
    keys.push(main_key);
    Ok(keys)
}

/// Ordre logique press-puis-release complet, testable sans taper réellement.
/// `true` = press, `false` = release. Release en ordre inverse.
pub fn press_then_release_order(keys: &[enigo::Key]) -> Vec<(enigo::Key, bool)> {
    let mut out: Vec<(enigo::Key, bool)> = keys.iter().map(|k| (*k, true)).collect();
    out.extend(keys.iter().rev().map(|k| (*k, false)));
    out
}

/// Envoie une séquence de touches. `Press` = maintient (modifiers d'abord),
/// `Release` = relâche (ordre inverse), `Click` = press+release complet.
pub fn send_shortcut(
    enigo: &mut Enigo,
    keys: &[enigo::Key],
    direction: enigo::Direction,
) -> Result<(), String> {
    match direction {
        enigo::Direction::Press => {
            for k in keys {
                enigo
                    .key(*k, enigo::Direction::Press)
                    .map_err(|e| e.to_string())?;
            }
        }
        enigo::Direction::Release => {
            for k in keys.iter().rev() {
                enigo
                    .key(*k, enigo::Direction::Release)
                    .map_err(|e| e.to_string())?;
            }
        }
        enigo::Direction::Click => {
            for (k, press) in press_then_release_order(keys) {
                let d = if press {
                    enigo::Direction::Press
                } else {
                    enigo::Direction::Release
                };
                enigo.key(k, d).map_err(|e| e.to_string())?;
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use enigo::Key;

    #[test]
    fn parses_modifiers_and_letter() {
        let keys = parse_shortcut_to_keys("ctrl+alt+shift+m").unwrap();
        assert_eq!(
            keys,
            vec![Key::Control, Key::Alt, Key::Shift, Key::Unicode('m')]
        );
    }

    #[test]
    fn parses_function_key() {
        let keys = parse_shortcut_to_keys("cmd+f13").unwrap();
        assert_eq!(keys, vec![Key::Meta, Key::F13]);
    }

    #[test]
    fn rejects_empty_or_unknown() {
        assert!(parse_shortcut_to_keys("").is_err());
        assert!(parse_shortcut_to_keys("ctrl+++").is_err());
    }

    #[test]
    fn release_order_is_reversed() {
        // press: modifiers puis touche ; release: touche puis modifiers.
        let keys = parse_shortcut_to_keys("ctrl+shift+m").unwrap();
        let order = super::press_then_release_order(&keys);
        assert_eq!(
            order,
            vec![
                (Key::Control, true),
                (Key::Shift, true),
                (Key::Unicode('m'), true),
                (Key::Unicode('m'), false),
                (Key::Shift, false),
                (Key::Control, false),
            ]
        );
    }
}
