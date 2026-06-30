use std::path::PathBuf;
use log::info;


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
        const CREATE_NEW_PROCESS_GROUP:  u32 = 0x00000200;

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
) -> anyhow::Result<()> {
    let install = install_dir();
    info!("Applying {} → {}", update_path.display(), install.display());

    let ext = update_path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("");

    match ext {
        #[cfg(target_os = "macos")]
        "dmg" => {
            apply_dmg(update_path, &install).await?;
            crate::update::set_installed_version(version);
            std::fs::write(just_updated_marker(), version.as_bytes()).ok();
            relaunch_bootstrapper();
        }

        #[cfg(target_os = "windows")]
        "exe" => {
            let new_updater = install.join("updater.exe");
            crate::update::set_installed_version(version);
            std::fs::write(just_updated_marker(), version.as_bytes()).ok();
            apply_nsis(update_path, &new_updater, version).await?;
            Ok(())
        }

        #[cfg(target_os = "linux")]
        "AppImage" => {
            apply_appimage(update_path, &install).await?;
            crate::update::set_installed_version(version);
            std::fs::write(just_updated_marker(), version.as_bytes()).ok();
            relaunch_bootstrapper();
        }

        #[cfg(target_os = "linux")]
        "deb" => {
            apply_deb(update_path).await?;
            crate::update::set_installed_version(version);
            std::fs::write(just_updated_marker(), version.as_bytes()).ok();
            relaunch_bootstrapper();
        }

        other => return Err(anyhow::anyhow!("Unknown update format: {}", other)),
    }
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
async fn apply_nsis(
    installer_path: &std::path::Path,
    _new_updater_path: &std::path::Path,
    _version: &str,
) -> anyhow::Result<()> {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW:         u32 = 0x08000000;
    const CREATE_NEW_PROCESS_GROUP: u32 = 0x00000200;
    const DETACHED_PROCESS:         u32 = 0x00000008;

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
            stdout, stderr
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
        .ok_or_else(|| anyhow::anyhow!(
            "No mount-point found in hdiutil output:\n{}", stdout
        ))?;

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

    let ok = rsync.success() || rsync.code() == Some(23);
    if !ok {
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
    install_dir: &std::path::Path,
) -> anyhow::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    use tokio::fs;
    
    let dest = if let Ok(current_appimage) = std::env::var("APPIMAGE") {
        PathBuf::from(current_appimage)
    } else {
        install_dir.join("mutualzz-bin")
    };

    fs::copy(appimage_path, &dest).await?;

    let mut perms = fs::metadata(&dest).await?.permissions();
    perms.set_mode(0o755);
    fs::set_permissions(&dest, perms).await?;
    fs::remove_file(appimage_path).await.ok();

    info!("Linux AppImage applied to {}", dest.display());
    Ok(())
}


#[cfg(target_os = "linux")]
async fn apply_deb(deb_path: &std::path::Path) -> anyhow::Result<()> {
    use tokio::process::Command;

    let status = Command::new("dpkg").args(["-i"]).arg(deb_path).status().await?;
    if !status.success() {
        return Err(anyhow::anyhow!("dpkg -i failed"));
    }

    tokio::fs::remove_file(deb_path).await.ok();
    info!("Linux deb applied");
    Ok(())
}