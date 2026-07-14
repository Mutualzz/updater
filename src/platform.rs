use log::info;
use std::path::PathBuf;

pub fn electron_exe_path() -> PathBuf {
    if let Ok(path) = std::env::var("UPDATER_ELECTRON_PATH") {
        return PathBuf::from(path);
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
    update_path: &std::path::Path,
    version: &str,
    electron_version: Option<&str>,
    updater_version: Option<&str>,
) -> anyhow::Result<()> {
    let install = install_dir();
    info!("Applying {} → {}", update_path.display(), install.display());

    let format = update_format(update_path);

    match format.as_str() {
        #[cfg(target_os = "macos")]
        "dmg" => {
            apply_dmg(update_path, &install).await?;
            crate::update::set_installed_version(version);
            if let Some(ev) = electron_version {
                crate::update::set_installed_electron_version(ev);
            }
            if let Some(uv) = updater_version {
                crate::update::set_installed_updater_version(uv);
            }
            std::fs::write(just_updated_marker(), version.as_bytes()).ok();
            crate::update::cleanup_update_temp();
            relaunch_bootstrapper();
        }

        #[cfg(target_os = "windows")]
        "exe" => {
            let _ = (version, electron_version, updater_version);
            apply_nsis(update_path).await?;
            Ok(())
        }

        #[cfg(target_os = "linux")]
        "AppImage" => {
            apply_appimage(update_path, &install).await?;
            crate::update::set_installed_version(version);
            if let Some(ev) = electron_version {
                crate::update::set_installed_electron_version(ev);
            }
            if let Some(uv) = updater_version {
                crate::update::set_installed_updater_version(uv);
            }
            std::fs::write(just_updated_marker(), version.as_bytes()).ok();
            crate::update::cleanup_update_temp();
            relaunch_bootstrapper();
        }

        #[cfg(target_os = "linux")]
        "deb" => {
            apply_deb(update_path).await?;
            crate::update::set_installed_version(version);
            if let Some(ev) = electron_version {
                crate::update::set_installed_electron_version(ev);
            }
            if let Some(uv) = updater_version {
                crate::update::set_installed_updater_version(uv);
            }
            std::fs::write(just_updated_marker(), version.as_bytes()).ok();
            crate::update::cleanup_update_temp();
            relaunch_bootstrapper();
        }

        #[cfg(target_os = "linux")]
        "rpm" => {
            apply_rpm(update_path).await?;
            crate::update::set_installed_version(version);
            if let Some(ev) = electron_version {
                crate::update::set_installed_electron_version(ev);
            }
            if let Some(uv) = updater_version {
                crate::update::set_installed_updater_version(uv);
            }
            std::fs::write(just_updated_marker(), version.as_bytes()).ok();
            crate::update::cleanup_update_temp();
            relaunch_bootstrapper();
        }

        #[cfg(target_os = "linux")]
        "pacman" => {
            apply_pacman(update_path).await?;
            crate::update::set_installed_version(version);
            if let Some(ev) = electron_version {
                crate::update::set_installed_electron_version(ev);
            }
            if let Some(uv) = updater_version {
                crate::update::set_installed_updater_version(uv);
            }
            std::fs::write(just_updated_marker(), version.as_bytes()).ok();
            crate::update::cleanup_update_temp();
            relaunch_bootstrapper();
        }

        other => return Err(anyhow::anyhow!("Unknown update format: {}", other)),
    }
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
    if name.ends_with(".dmg") {
        return "dmg".to_string();
    }
    if name.ends_with(".exe") {
        return "exe".to_string();
    }

    path.extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_string()
}

#[cfg(unix)]
fn relaunch_bootstrapper() -> ! {
    #[cfg(target_os = "linux")]
    let exe = std::env::var("APPIMAGE")
        .map(PathBuf::from)
        .unwrap_or_else(|_| std::env::current_exe().expect("Cannot resolve bootstrapper path"));

    #[cfg(not(target_os = "linux"))]
    let exe = std::env::current_exe().expect("Cannot resolve bootstrapper path");

    info!("Relaunching bootstrapper: {}", exe.display());
    std::thread::sleep(std::time::Duration::from_secs(2));
    use std::os::unix::process::CommandExt;
    let err = std::process::Command::new(&exe).exec();
    panic!("Failed to re-exec: {}", err);
}

#[cfg(target_os = "windows")]
async fn apply_nsis(installer_path: &std::path::Path) -> anyhow::Result<()> {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x08000000;
    const CREATE_NEW_PROCESS_GROUP: u32 = 0x00000200;
    const DETACHED_PROCESS: u32 = 0x00000008;

    std::process::Command::new(installer_path)
        .arg("/S")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .creation_flags(CREATE_NO_WINDOW | CREATE_NEW_PROCESS_GROUP | DETACHED_PROCESS)
        .spawn()?;

    info!("NSIS launched detached, exiting");
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

#[cfg(target_os = "macos")]
async fn apply_dmg(
    dmg_path: &std::path::Path,
    install_dir: &std::path::Path,
) -> anyhow::Result<()> {
    use tokio::process::Command;

    let out = Command::new("hdiutil")
        .args(["attach", "-nobrowse", "-noverify", "-plist"])
        .arg(dmg_path)
        .output()
        .await?;

    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);

    if !out.status.success() {
        return Err(anyhow::anyhow!(
            "hdiutil attach failed:\nstdout: {}\nstderr: {}",
            stdout,
            stderr
        ));
    }

    let mount_point = stdout
        .lines()
        .skip_while(|l| !l.contains("<key>mount-point</key>"))
        .nth(1)
        .and_then(|l| {
            let s = l.trim();
            let s = s.strip_prefix("<string>")?;
            let s = s.strip_suffix("</string>")?;
            Some(s.to_string())
        })
        .ok_or_else(|| anyhow::anyhow!("No mount-point found in hdiutil output:\n{}", stdout))?;

    info!("DMG mounted at: {}", mount_point);

    let app_in_dmg = std::fs::read_dir(&mount_point)?
        .filter_map(|e| e.ok())
        .find(|e| e.path().extension().and_then(|x| x.to_str()) == Some("app"))
        .map(|e| e.path())
        .ok_or_else(|| anyhow::anyhow!("No .app in DMG at: {}", mount_point))?;

    info!("Found app: {}", app_in_dmg.display());

    let apps_dir = install_dir
        .parent()
        .ok_or_else(|| anyhow::anyhow!("No parent dir for install_dir"))?;

    let rsync = Command::new("rsync")
        .args(["-a", "--delete", "--no-times"])
        .arg(&app_in_dmg)
        .arg(apps_dir)
        .status()
        .await?;

    if !rsync.success() {
        let _ = Command::new("hdiutil")
            .args(["detach", "-quiet", &mount_point])
            .status()
            .await;
        return Err(anyhow::anyhow!("rsync failed: {:?}", rsync.code()));
    }

    let _ = Command::new("hdiutil")
        .args(["detach", "-quiet", &mount_point])
        .status()
        .await;

    tokio::fs::remove_file(dmg_path).await.ok();
    info!("macOS update applied");
    Ok(())
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

#[cfg(target_os = "linux")]
async fn apply_deb(deb_path: &std::path::Path) -> anyhow::Result<()> {
    run_privileged_install(&["dpkg", "-i"], deb_path).await?;
    tokio::fs::remove_file(deb_path).await.ok();
    info!("Linux deb applied");
    Ok(())
}

#[cfg(target_os = "linux")]
async fn apply_rpm(rpm_path: &std::path::Path) -> anyhow::Result<()> {
    if which_exists("dnf") {
        run_privileged_install(&["dnf", "install", "-y"], rpm_path).await?;
    } else if which_exists("zypper") {
        run_privileged_install(&["zypper", "--non-interactive", "install"], rpm_path).await?;
    } else {
        run_privileged_install(&["rpm", "-Uvh"], rpm_path).await?;
    }

    tokio::fs::remove_file(rpm_path).await.ok();
    info!("Linux rpm applied");
    Ok(())
}

#[cfg(target_os = "linux")]
async fn apply_pacman(pacman_path: &std::path::Path) -> anyhow::Result<()> {
    run_privileged_install(&["pacman", "-U", "--noconfirm"], pacman_path).await?;
    tokio::fs::remove_file(pacman_path).await.ok();
    info!("Linux pacman applied");
    Ok(())
}

#[cfg(target_os = "linux")]
fn which_exists(bin: &str) -> bool {
    std::process::Command::new("which")
        .arg(bin)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

#[cfg(target_os = "linux")]
async fn run_privileged_install(
    command: &[&str],
    package_path: &std::path::Path,
) -> anyhow::Result<()> {
    use tokio::process::Command;

    let mut pkexec_args: Vec<&str> = command.to_vec();
    let package = package_path
        .to_str()
        .ok_or_else(|| anyhow::anyhow!("Invalid package path"))?;
    pkexec_args.push(package);

    let pkexec_status = Command::new("pkexec").args(&pkexec_args).status().await;

    if let Ok(status) = pkexec_status {
        if status.success() {
            return Ok(());
        }
    }

    let status = Command::new(command[0])
        .args(&command[1..])
        .arg(package_path)
        .status()
        .await?;

    if !status.success() {
        return Err(anyhow::anyhow!(
            "{} install failed for {}",
            command[0],
            package_path.display()
        ));
    }

    Ok(())
}
