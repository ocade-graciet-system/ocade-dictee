//! Gardes système (mémoire, disque, cœurs) et recherche d'un processus
//! survivant, via `sysinfo` (features `system` + `disk`).

use std::ffi::OsString;
use std::path::Path;

use sysinfo::{Disks, Pid, ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};

use super::SummaryError;

/// 3 Gio d'espace libre requis avant tout téléchargement (moteur + modèle + marge).
pub const MIN_FREE_DISK_BYTES: u64 = 3 * 1024 * 1024 * 1024;
/// 3 Gio de mémoire disponible requis avant de charger le modèle (≈ 2,2 Go)
/// et son cache de contexte (≈ 0,5 Go) sans pousser la machine au swap.
pub const MIN_AVAILABLE_MEMORY_BYTES: u64 = 3 * 1024 * 1024 * 1024;

pub fn check_disk_space(free_bytes: u64) -> Result<(), SummaryError> {
    if free_bytes < MIN_FREE_DISK_BYTES {
        return Err(SummaryError::DiskSpace {
            needed_bytes: MIN_FREE_DISK_BYTES,
            free_bytes,
        });
    }
    Ok(())
}

pub fn check_memory(available_bytes: u64) -> Result<(), SummaryError> {
    if available_bytes < MIN_AVAILABLE_MEMORY_BYTES {
        return Err(SummaryError::Memory {
            needed_bytes: MIN_AVAILABLE_MEMORY_BYTES,
            free_bytes: available_bytes,
        });
    }
    Ok(())
}

/// Mémoire disponible (octets), au sens de l'OS (réutilisable sans swap).
pub fn available_memory_bytes() -> u64 {
    let mut system = System::new();
    system.refresh_memory();
    system.available_memory()
}

/// Espace libre (octets) sur le volume qui contient `path` ; `None` si le
/// volume n'est pas identifiable (la garde est alors ignorée, pas bloquante).
pub fn free_disk_bytes(path: &Path) -> Option<u64> {
    let disks = Disks::new_with_refreshed_list();
    disks
        .list()
        .iter()
        .filter(|disk| path.starts_with(disk.mount_point()))
        .max_by_key(|disk| disk.mount_point().as_os_str().len())
        .map(|disk| disk.available_space())
}

/// Cœurs physiques (repli : cœurs logiques / 2, au moins 2).
pub fn physical_cores() -> usize {
    System::physical_core_count()
        .or_else(|| {
            std::thread::available_parallelism()
                .ok()
                .map(|n| (n.get() / 2).max(1))
        })
        .unwrap_or(2)
        .max(2)
}

/// Un processus est « notre » serveur s'il exécute exactement `expected_exe`
/// ou si sa ligne de commande commence par ce chemin.
pub fn is_our_server(exe: Option<&Path>, cmd: &[OsString], expected_exe: &Path) -> bool {
    if exe.is_some_and(|e| e == expected_exe) {
        return true;
    }
    cmd.first()
        .is_some_and(|argv0| Path::new(argv0) == expected_exe)
}

/// Tue le processus `pid` s'il s'agit d'un `llama-server` lancé par nous
/// (survivant d'une fermeture brutale). Renvoie `true` s'il a été tué.
pub fn kill_if_our_server(pid: u32, expected_exe: &Path) -> bool {
    let pid = Pid::from_u32(pid);
    let mut system = System::new();
    system.refresh_processes_specifics(
        ProcessesToUpdate::Some(&[pid]),
        true,
        ProcessRefreshKind::nothing()
            .with_exe(UpdateKind::Always)
            .with_cmd(UpdateKind::Always),
    );
    match system.process(pid) {
        Some(process) if is_our_server(process.exe(), process.cmd(), expected_exe) => {
            process.kill()
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn guards_compare_against_three_gib() {
        assert_eq!(check_disk_space(MIN_FREE_DISK_BYTES), Ok(()));
        assert_eq!(
            check_disk_space(MIN_FREE_DISK_BYTES - 1),
            Err(SummaryError::DiskSpace {
                needed_bytes: MIN_FREE_DISK_BYTES,
                free_bytes: MIN_FREE_DISK_BYTES - 1
            })
        );
        assert_eq!(check_memory(4 * 1024 * 1024 * 1024), Ok(()));
        assert!(matches!(
            check_memory(1024),
            Err(SummaryError::Memory { .. })
        ));
    }

    #[test]
    fn system_probes_return_plausible_values() {
        assert!(available_memory_bytes() > 0);
        assert!(physical_cores() >= 2);
        let free = free_disk_bytes(&std::env::temp_dir());
        assert!(free.is_some_and(|f| f > 0), "{free:?}");
    }

    #[test]
    fn server_identity_is_matched_on_exe_or_argv0() {
        let exe = Path::new("/data/bin/llama-b10930/llama-server");
        assert!(is_our_server(Some(exe), &[], exe));
        assert!(is_our_server(
            None,
            &[
                OsString::from("/data/bin/llama-b10930/llama-server"),
                OsString::from("-m")
            ],
            exe
        ));
        assert!(!is_our_server(
            Some(Path::new("/usr/bin/python3")),
            &[OsString::from("python3")],
            exe
        ));
        assert!(!is_our_server(None, &[], exe));
    }

    #[test]
    fn unknown_pid_is_not_killed() {
        assert!(!kill_if_our_server(
            u32::MAX - 7,
            Path::new("/nowhere/llama-server")
        ));
        // Notre propre processus n'est pas un llama-server : jamais tué.
        assert!(!kill_if_our_server(
            std::process::id(),
            Path::new("/nowhere/llama-server")
        ));
    }
}
