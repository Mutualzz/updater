use log::info;
use std::path::{Path, PathBuf};

pub fn electron_exe_path() -> PathBuf {
    if let Ok(path) = std::env::var("UPDATER_ELECTRON_PATH") {
        return PathBuf::from(path);
    }

    if let Some(app_dir) = crate::layout::resolve_active_app_dir() {
        #[cfg(target_os = "macos")]
        {
            let mac = app_dir
                .join("Mutualzz.app")
                .join("Contents")
                .join("MacOS")
                .join("MutualzzApp");
            if mac.is_file() {
                return mac;
            }
            let flat = app_dir.join("Contents").join("MacOS").join("MutualzzApp");
            if flat.is_file() {
                return flat;
            }
        }

        #[cfg(target_os = "windows")]
        {
            let exe = app_dir.join("mutualzz.exe");
            if exe.is_file() {
                return exe;
            }
        }

        #[cfg(target_os = "linux")]
        {
            let bin = app_dir.join("mutualzz-bin");
            if bin.is_file() {
                return bin;
            }
        }
    }

    let bootstrapper = std::env::current_exe().expect("Cannot resolve bootstrapper path");
    let dir = bootstrapper.parent().expect("No parent dir");

    #[cfg(target_os = "macos")]
    return dir.join("MutualzzApp");

    #[cfg(target_os = "windows")]
    return dir.join("mutualzz.exe");

    #[cfg(target_os = "linux")]
    return install_dir().join("mutualzz-bin");
}

pub fn install_dir() -> PathBuf {
    if let Some(app_dir) = crate::layout::resolve_active_app_dir() {
        #[cfg(target_os = "macos")]
        {
            let bundled = app_dir.join("Mutualzz.app");
            if bundled.is_dir() {
                return bundled;
            }
            return app_dir;
        }
        return app_dir;
    }

    let bootstrapper = std::env::current_exe().expect("Cannot resolve bootstrapper path");
    let dir = bootstrapper.parent().expect("No parent dir");

    #[cfg(target_os = "macos")]
    return dir
        .parent()
        .and_then(|p| p.parent())
        .expect("Cannot resolve .app root")
        .to_path_buf();

    #[cfg(target_os = "linux")]
    {
        if let Ok(appimage_path) = std::env::var("APPIMAGE") {
            let p = PathBuf::from(appimage_path);
            if let Some(parent) = p.parent() {
                return parent.to_path_buf();
            }
        }
        return dir.to_path_buf();
    }

    #[cfg(target_os = "windows")]
    return dir.to_path_buf();
}

pub fn just_updated_marker() -> PathBuf {
    dirs::data_local_dir()
        .unwrap_or_else(|| dirs::home_dir().unwrap_or_else(|| PathBuf::from(".")))
        .join("Mutualzz")
        .join("just-updated")
}

pub fn exec_into_electron() -> ! {
    let electron_path = electron_exe_path();
    info!("Launching Electron: {}", electron_path.display());

    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        let err = std::process::Command::new(&electron_path).exec();
        panic!("exec failed: {}", err);
    }

    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_BREAKAWAY_FROM_JOB: u32 = 0x01000000;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x00000200;

        let dir = electron_path.parent().expect("No parent dir for Electron");

        std::process::Command::new(&electron_path)
            .current_dir(dir)
            .creation_flags(CREATE_BREAKAWAY_FROM_JOB | CREATE_NEW_PROCESS_GROUP)
            .spawn()
            .expect("Failed to launch Electron");
        std::process::exit(0);
    }
}

pub async fn apply_update(
    update_path: &Path,
    version: &str,
    electron_version: Option<&str>,
    updater_version: Option<&str>,
) -> anyhow::Result<()> {
    info!("Applying {} for version {}", update_path.display(), version);

    let format = update_format(update_path);

    match format.as_str() {
        "asar" => {
            apply_asar_update(update_path, version).await?;
            finish_apply(version, electron_version, updater_version);
        }

        "zip" => {
            apply_zip_package(update_path, version, electron_version, updater_version).await?;
        }

        #[cfg(target_os = "linux")]
        "AppImage" if !crate::layout::can_apply_full_zip() => {
            return Err(anyhow::anyhow!(
                "Full AppImage updates require the AppImage build. Download from mutualzz.com."
            ));
        }

        #[cfg(target_os = "linux")]
        "AppImage" => {
            apply_appimage(update_path, &install_dir()).await?;
            finish_apply(version, electron_version, updater_version);
            relaunch_bootstrapper();
        }

        #[cfg(target_os = "linux")]
        "deb" | "rpm" | "pacman" => {
            return Err(anyhow::anyhow!(
                "Package manager updates are not supported in-app. Use the AppImage build."
            ));
        }

        other => return Err(anyhow::anyhow!("Unknown update format: {}", other)),
    }

    Ok(())
}

fn finish_apply(
    version: &str,
    electron_version: Option<&str>,
    updater_version: Option<&str>,
) {
    crate::update::set_installed_version(version);
    if let Some(ev) = electron_version {
        crate::update::set_installed_electron_version(ev);
    }
    if let Some(uv) = updater_version {
        crate::update::set_installed_updater_version(uv);
    }
    std::fs::write(just_updated_marker(), version.as_bytes()).ok();
    crate::update::cleanup_packages_dir();
}

pub async fn apply_zip_package(
    zip_path: &Path,
    version: &str,
    electron_version: Option<&str>,
    updater_version: Option<&str>,
) -> anyhow::Result<()> {
    let dest = crate::layout::app_version_dir(version);
    if dest.exists() {
        tokio::fs::remove_dir_all(&dest).await.ok();
    }

    extract_zip_package(zip_path, &dest).await?;
    crate::layout::set_current_version(version);
    crate::layout::hoist_windows_update_exe(&dest)?;

    finish_apply(version, electron_version, updater_version);

    tokio::fs::remove_file(zip_path).await.ok();
    relaunch_bootstrapper();
}

pub async fn extract_zip_package(zip_path: &Path, dest: &Path) -> anyhow::Result<()> {
    use std::io::Read;
    use zip::ZipArchive;

    tokio::fs::create_dir_all(dest).await?;
    let file = std::fs::File::open(zip_path)?;
    let mut archive = ZipArchive::new(file)?;

    for i in 0..archive.len() {
        let mut entry = archive.by_index(i)?;
        let name = entry.name().to_string();
        let out_path = dest.join(&name);

        if name.ends_with('/') {
            tokio::fs::create_dir_all(&out_path).await.ok();
            continue;
        }

        if let Some(parent) = out_path.parent() {
            tokio::fs::create_dir_all(parent).await.ok();
        }

        let mut buffer = Vec::new();
        entry.read_to_end(&mut buffer)?;
        tokio::fs::write(&out_path, buffer).await?;
    }

    info!("Extracted {} → {}", zip_path.display(), dest.display());
    Ok(())
}

fn update_format(path: &std::path::Path) -> String {
    let name = path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();

    if name.ends_with(".appimage") {
        return "AppImage".to_string();
    }
    if name.ends_with(".deb") {
        return "deb".to_string();
    }
    if name.ends_with(".rpm") {
        return "rpm".to_string();
    }
    if name.ends_with(".pacman")
        || name.ends_with(".pkg.tar.zst")
        || name.ends_with(".pkg.tar.xz")
        || name.ends_with(".pkg.tar.gz")
    {
        return "pacman".to_string();
    }
    if name.ends_with(".zip") {
        return "zip".to_string();
    }
    if name.ends_with(".asar") {
        return "asar".to_string();
    }
    if name.ends_with(".dmg") {
        return "dmg".to_string();
    }

    path.extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_string()
}

#[cfg(unix)]
fn relaunch_bootstrapper() -> ! {
    let exe = crate::layout::bootstrapper_at_data_root();

    info!("Relaunching bootstrapper: {}", exe.display());
    std::thread::sleep(std::time::Duration::from_secs(1));
    use std::os::unix::process::CommandExt;
    let err = std::process::Command::new(&exe).exec();
    panic!("Failed to re-exec: {}", err);
}

#[cfg(windows)]
fn relaunch_bootstrapper() -> ! {
    let exe = crate::layout::bootstrapper_at_data_root();
    use std::os::windows::process::CommandExt;
    const CREATE_BREAKAWAY_FROM_JOB: u32 = 0x01000000;
    const CREATE_NEW_PROCESS_GROUP: u32 = 0x00000200;

    info!("Relaunching bootstrapper: {}", exe.display());
    std::thread::sleep(std::time::Duration::from_secs(1));
    std::process::Command::new(&exe)
        .creation_flags(CREATE_BREAKAWAY_FROM_JOB | CREATE_NEW_PROCESS_GROUP)
        .spawn()
        .expect("Failed to relaunch bootstrapper");
    std::process::exit(0);
}

pub fn asar_dest_path() -> PathBuf {
    #[cfg(target_os = "macos")]
    {
        return install_dir()
            .join("Contents")
            .join("Resources")
            .join("app.asar");
    }

    #[cfg(not(target_os = "macos"))]
    {
        install_dir().join("resources").join("app.asar")
    }
}

pub async fn apply_asar_update(asar_path: &std::path::Path, version: &str) -> anyhow::Result<()> {
    let dest = asar_dest_path();

    info!(
        "Hot-swapping asar {} → {}",
        asar_path.display(),
        dest.display()
    );

    if let Some(parent) = dest.parent() {
        tokio::fs::create_dir_all(parent).await.ok();
    }

    let tmp = dest.with_extension("asar.new");
    tokio::fs::copy(asar_path, &tmp).await?;
    tokio::fs::rename(&tmp, &dest).await?;
    tokio::fs::remove_file(asar_path).await.ok();

    crate::update::set_installed_version(version);
    std::fs::write(just_updated_marker(), version.as_bytes()).ok();
    info!("Asar hot-swap applied");
    Ok(())
}

#[cfg(windows)]
pub fn set_app_user_model_id() {
    use std::os::windows::ffi::OsStrExt;

    let wide: Vec<u16> = std::ffi::OsStr::new("com.mutualzz.app")
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();

    #[link(name = "shell32")]
    extern "system" {
        fn SetCurrentProcessExplicitAppUserModelID(app_id: *const u16) -> i32;
    }

    unsafe {
        SetCurrentProcessExplicitAppUserModelID(wide.as_ptr());
    }
}

#[cfg(target_os = "linux")]
async fn apply_appimage(
    appimage_path: &std::path::Path,
    _install_dir: &std::path::Path,
) -> anyhow::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    use tokio::fs;

    let dest = match std::env::var("APPIMAGE") {
        Ok(current_appimage) => PathBuf::from(current_appimage),
        Err(_) => {
            return Err(anyhow::anyhow!(
                "AppImage update requires running from an AppImage"
            ));
        }
    };

    let tmp = dest.with_extension("update.AppImage");
    fs::copy(appimage_path, &tmp).await?;

    let mut perms = fs::metadata(&tmp).await?.permissions();
    perms.set_mode(0o755);
    fs::set_permissions(&tmp, perms).await?;

    fs::rename(&tmp, &dest).await?;

    fs::remove_file(appimage_path).await.ok();
    info!("Linux AppImage applied to {}", dest.display());
    Ok(())
}
