use log::{info, warn};
use std::path::{Path, PathBuf};

pub fn data_root() -> PathBuf {
    dirs::data_local_dir()
        .unwrap_or_else(|| dirs::home_dir().unwrap_or_else(|| PathBuf::from(".")))
        .join("Mutualzz")
}

pub fn packages_dir() -> PathBuf {
    data_root().join("packages")
}

pub fn current_file_path() -> PathBuf {
    data_root().join("current")
}

pub fn pending_restart_path() -> PathBuf {
    data_root().join("pending-restart.json")
}

pub fn layout_v2_marker() -> PathBuf {
    data_root().join("layout-v2")
}

pub fn app_version_dir(version: &str) -> PathBuf {
    data_root().join(format!("app-{}", version.trim()))
}

pub fn read_current_version() -> Option<String> {
    std::fs::read_to_string(current_file_path())
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

pub fn set_current_version(version: &str) {
    let root = data_root();
    std::fs::create_dir_all(&root).ok();
    std::fs::write(current_file_path(), version.trim().as_bytes()).ok();
    info!("Set current version: {}", version.trim());
}

pub fn resolve_active_app_dir() -> Option<PathBuf> {
    if let Some(version) = read_current_version() {
        let dir = app_version_dir(&version);
        if dir.is_dir() {
            return Some(dir);
        }
    }
    None
}

pub fn bootstrapper_at_data_root() -> PathBuf {
    #[cfg(target_os = "windows")]
    return data_root().join("Update.exe");

    #[cfg(target_os = "macos")]
    {
        if let Some(dir) = resolve_active_app_dir() {
            let app = dir.join("Mutualzz.app").join("Contents").join("MacOS").join("Mutualzz");
            if app.is_file() {
                return app;
            }
        }
        return std::env::current_exe().unwrap_or_else(|_| PathBuf::from("Mutualzz"));
    }

    #[cfg(target_os = "linux")]
    {
        if let Some(dir) = resolve_active_app_dir() {
            let updater = dir.join("updater");
            if updater.is_file() {
                return updater;
            }
        }
        return std::env::current_exe().unwrap_or_else(|_| PathBuf::from("updater"));
    }
}

pub fn legacy_flat_install_dir() -> Option<PathBuf> {
    #[cfg(target_os = "windows")]
    {
        if let Some(local) = dirs::data_local_dir() {
            let legacy = local.join("Programs").join("Mutualzz");
            if legacy.is_dir() {
                return Some(legacy);
            }
        }
    }
    None
}

pub fn migrate_legacy_layout_if_needed() -> anyhow::Result<()> {
    if layout_v2_marker().is_file() {
        return Ok(());
    }

    if resolve_active_app_dir().is_some() {
        std::fs::write(layout_v2_marker(), b"2").ok();
        return Ok(());
    }

    #[cfg(target_os = "windows")]
    {
        if let Some(legacy) = legacy_flat_install_dir() {
            let version = crate::update::get_installed_version();
            if version.is_empty() || version == "0.0.0" {
                return Ok(());
            }

            let dest = app_version_dir(&version);
            if dest.exists() {
                set_current_version(&version);
                std::fs::write(layout_v2_marker(), b"2").ok();
                #[cfg(target_os = "windows")]
                {
                    if let Err(e) = crate::install::rewrite_windows_shortcuts() {
                        warn!("Failed to rewrite shortcuts after migration: {}", e);
                    }
                    let _ = crate::install::register_windows_uninstall_entry(&version);
                }
                return Ok(());
            }

            info!(
                "Migrating legacy install {} → {}",
                legacy.display(),
                dest.display()
            );
            std::fs::create_dir_all(data_root()).ok();
            copy_dir_recursive(&legacy, &dest)?;
            set_current_version(&version);

            let update_src = legacy.join("Update.exe");
            let update_dst = data_root().join("Update.exe");
            if update_src.is_file() && !update_dst.exists() {
                std::fs::copy(&update_src, &update_dst).ok();
            }
            let updater_src = legacy.join("updater.exe");
            if updater_src.is_file() && !update_dst.exists() {
                std::fs::copy(&updater_src, &update_dst).ok();
            }

            std::fs::write(layout_v2_marker(), b"2").ok();
            info!("Legacy layout migration complete");

            #[cfg(target_os = "windows")]
            {
                if let Err(e) = crate::install::rewrite_windows_shortcuts() {
                    warn!("Failed to rewrite shortcuts after migration: {}", e);
                }
                let _ = crate::install::register_windows_uninstall_entry(&version);
            }
        }
    }

    Ok(())
}

fn copy_dir_recursive(src: &Path, dst: &Path) -> anyhow::Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let ty = entry.file_type()?;
        let from = entry.path();
        let to = dst.join(entry.file_name());
        if ty.is_dir() {
            copy_dir_recursive(&from, &to)?;
        } else {
            std::fs::copy(&from, &to)?;
        }
    }
    Ok(())
}

#[derive(serde::Serialize, serde::Deserialize, Debug, Clone)]
pub struct PendingRestart {
    pub version: String,
    pub artifact: String,
    #[serde(default)]
    pub electron_version: Option<String>,
    #[serde(default)]
    pub updater_version: Option<String>,
}

pub fn read_pending_restart() -> Option<PendingRestart> {
    let raw = std::fs::read_to_string(pending_restart_path()).ok()?;
    serde_json::from_str(&raw).ok()
}

pub fn clear_pending_restart() {
    let _ = std::fs::remove_file(pending_restart_path());
}

pub fn read_version_from_app_dir(app_dir: &Path) -> Option<String> {
    for name in ["app-version.txt", "resources/app-version.txt"] {
        let path = app_dir.join(name);
        if path.is_file() {
            return std::fs::read_to_string(path)
                .ok()
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty() && s != "0.0.0");
        }
    }
    None
}

pub fn find_staged_package() -> Option<PathBuf> {
    let packages = packages_dir();
    if !packages.is_dir() {
        return None;
    }

    let mut candidates: Vec<(PathBuf, std::time::SystemTime)> = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&packages) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_file() {
                let name = path.file_name().and_then(|s| s.to_str()).unwrap_or("");
                if name.ends_with(".partial") {
                    continue;
                }
                if let Ok(modified) = entry.metadata().and_then(|m| m.modified()) {
                    candidates.push((path, modified));
                }
            }
        }
    }

    candidates
        .sort_by_key(|(_, modified)| *modified);
    candidates.pop().map(|(path, _)| path)
}

#[cfg(target_os = "linux")]
pub fn can_apply_full_zip() -> bool {
    if std::env::var_os("APPIMAGE").is_some() {
        return true;
    }
    if resolve_active_app_dir().is_some() {
        return true;
    }
    false
}

#[cfg(not(target_os = "linux"))]
pub fn can_apply_full_zip() -> bool {
    true
}

pub fn ensure_data_dirs() {
    let _ = std::fs::create_dir_all(packages_dir());
    let _ = std::fs::create_dir_all(data_root());
}

pub fn hoist_windows_update_exe(app_dir: &Path) -> anyhow::Result<()> {
    #[cfg(target_os = "windows")]
    {
        let candidates = [
            app_dir.join("Update.exe"),
            app_dir.join("updater.exe"),
        ];
        let dest = data_root().join("Update.exe");
        std::fs::create_dir_all(data_root())?;
        for src in candidates {
            if src.is_file() {
                std::fs::copy(&src, &dest)?;
                info!("Hoisted bootstrapper to {}", dest.display());
                return Ok(());
            }
        }
        warn!("No Update.exe found in {}", app_dir.display());
    }
    Ok(())
}
