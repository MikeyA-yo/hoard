//! Installed apps from the Windows Uninstall registry keys (read-only).

use std::path::PathBuf;

use winreg::enums::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_READ};
use winreg::RegKey;

use super::InstalledApp;

const UNINSTALL_KEYS: [&str; 2] = [
    r"SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall",
    r"SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall",
];

pub(super) fn installed_apps() -> Vec<InstalledApp> {
    let mut apps: Vec<InstalledApp> = Vec::new();
    for hive in [HKEY_LOCAL_MACHINE, HKEY_CURRENT_USER] {
        let root = RegKey::predef(hive);
        for path in UNINSTALL_KEYS {
            let Ok(list) = root.open_subkey_with_flags(path, KEY_READ) else {
                continue;
            };
            for name in list.enum_keys().flatten() {
                let Ok(k) = list.open_subkey_with_flags(&name, KEY_READ) else {
                    continue;
                };
                if let Some(app) = read_app(&k) {
                    apps.push(app);
                }
            }
        }
    }
    // The same app can be registered in several places.
    apps.sort_by_key(|a| a.name.to_lowercase());
    apps.dedup_by(|a, b| {
        a.name.eq_ignore_ascii_case(&b.name) && a.install_location == b.install_location
    });
    apps
}

fn read_app(k: &RegKey) -> Option<InstalledApp> {
    let get = |n: &str| {
        k.get_value::<String, _>(n)
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
    };
    let name = get("DisplayName")?;
    // Skip OS components, updates and patches.
    if k.get_value::<u32, _>("SystemComponent").unwrap_or(0) == 1 || get("ParentKeyName").is_some()
    {
        return None;
    }
    if matches!(
        get("ReleaseType").as_deref(),
        Some("Update" | "Hotfix" | "Security Update")
    ) {
        return None;
    }
    let install_location = get("InstallLocation")
        .map(|s| PathBuf::from(s.trim_matches('"').trim_end_matches('\\').to_string()))
        .filter(|p| p.is_absolute());
    Some(InstalledApp {
        name,
        publisher: get("Publisher"),
        version: get("DisplayVersion"),
        install_location,
        // EstimatedSize is in KiB.
        estimated_size: k
            .get_value::<u32, _>("EstimatedSize")
            .ok()
            .map(|kb| u64::from(kb) * 1024),
    })
}

#[cfg(test)]
mod tests {
    #[test]
    fn reads_the_registry_without_failing() {
        // Read-only; contents depend on the machine.
        let apps = super::installed_apps();
        assert!(apps.iter().all(|a| !a.name.is_empty()));
    }
}
