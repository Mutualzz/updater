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
    return dir.join("mutualzz-bin");
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

    #[cfg(not(target_os = "macos"))]
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

    #[cfg(target_os = "windows")]
    {
        crate::update::set_installed_version(version);
        std::fs::write(just_updated_marker(), version.as_bytes()).ok();
        info!("Wrote version and marker before NSIS");
    }

    match ext {
        #[cfg(target_os = "macos")]
        "dmg" => apply_dmg(update_path, &install).await?,

        #[cfg(target_os = "windows")]
        "exe" => apply_nsis(update_path).await?,

        #[cfg(target_os = "linux")]
        "AppImage" => apply_appimage(update_path, &install).await?,

        #[cfg(target_os = "linux")]
        "deb" => apply_deb(update_path).await?,

        other => return Err(anyhow::anyhow!("Unknown update format: {}", other)),
    }

    #[cfg(not(target_os = "windows"))]
    {
        crate::update::set_installed_version(version);
        std::fs::write(just_updated_marker(), version.as_bytes()).ok();
    }

    relaunch_bootstrapper();
}


fn relaunch_bootstrapper() -> ! {
    #[cfg(unix)]
    {
        let exe = std::env::current_exe().expect("Cannot resolve bootstrapper path");
        info!("Relaunching bootstrapper: {}", exe.display());
        std::thread::sleep(std::time::Duration::from_secs(2));
        use std::os::unix::process::CommandExt;
        let err = std::process::Command::new(&exe).exec();
        panic!("Failed to re-exec: {}", err);
    }

    #[cfg(windows)]
    {
        let exe = install_dir().join("updater.exe");
        info!("Relaunching bootstrapper: {}", exe.display());

        let timeout = std::time::Duration::from_secs(30);
        let start = std::time::Instant::now();
        while start.elapsed() < timeout {
            if let Ok(meta) = std::fs::metadata(&exe) {
                if meta.len() > 0 { break; }
            }
            std::thread::sleep(std::time::Duration::from_millis(300));
        }

        if !exe.exists() {
            log::error!("Bootstrapper not found after 30s: {}", exe.display());
            std::process::exit(1);
        }

        std::thread::sleep(std::time::Duration::from_secs(2));

        use std::os::windows::process::CommandExt;
        const CREATE_BREAKAWAY_FROM_JOB: u32 = 0x01000000;
        const CREATE_NEW_PROCESS_GROUP:  u32 = 0x00000200;

        info!("Spawning new bootstrapper");
        std::process::Command::new(&exe)
            .creation_flags(CREATE_BREAKAWAY_FROM_JOB | CREATE_NEW_PROCESS_GROUP)
            .spawn()
            .expect("Relaunch failed");

        std::process::exit(0);
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
            "No mount-point found in hdiutil plist output:\n{}",
            stdout
        ))?;

    info!("DMG mounted at: {}", mount_point);

    let app_in_dmg = std::fs::read_dir(&mount_point)?
        .filter_map(|e| e.ok())
        .find(|e| e.path().extension().and_then(|x| x.to_str()) == Some("app"))
        .map(|e| e.path())
        .ok_or_else(|| anyhow::anyhow!("No .app found in DMG at: {}", mount_point))?;

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

    let rsync_ok = rsync.success() || rsync.code() == Some(23);
    if !rsync_ok {
        return Err(anyhow::anyhow!("rsync failed with exit code: {:?}", rsync.code()));
    }

    let _ = Command::new("hdiutil")
        .args(["detach", "-quiet", &mount_point])
        .status()
        .await;

    tokio::fs::remove_file(dmg_path).await.ok();
    info!("macOS update applied successfully");
    Ok(())
}


#[cfg(target_os = "windows")]
async fn apply_nsis(installer_path: &std::path::Path) -> anyhow::Result<()> {
    use tokio::process::Command;

    let current = std::env::current_exe()?;
    let renamed = current.with_extension("exe.old");
    std::fs::rename(&current, &renamed).ok();
    info!("Renamed running exe to avoid NSIS lock: {}", renamed.display());

    let status = Command::new(installer_path)
        .arg("/S")
        .status()
        .await?;

    match status.code() {
        Some(0) => info!("NSIS installer succeeded"),
        Some(2) => log::warn!("NSIS exit code 2 — continuing"),
        other => return Err(anyhow::anyhow!("NSIS failed with exit code: {:?}", other)),
    }

    tokio::fs::remove_file(installer_path).await.ok();
    tokio::fs::remove_file(&renamed).await.ok();
    info!("Windows update applied");
    Ok(())
}


#[cfg(target_os = "linux")]
async fn apply_appimage(
    appimage_path: &std::path::Path,
    install_dir: &std::path::Path,
) -> anyhow::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    use tokio::fs;

    let dest = install_dir.join("mutualzz-bin");
    fs::copy(appimage_path, &dest).await?;

    let mut perms = fs::metadata(&dest).await?.permissions();
    perms.set_mode(0o755);
    fs::set_permissions(&dest, perms).await?;
    fs::remove_file(appimage_path).await.ok();

    info!("Linux AppImage applied");
    Ok(())
}


#[cfg(target_os = "linux")]
async fn apply_deb(deb_path: &std::path::Path) -> anyhow::Result<()> {
    use tokio::process::Command;

    let status = Command::new("dpkg").args(["-i"]).arg(deb_path).status().await?;
    if !status.success() {
        return Err(anyhow::anyhow!("dpkg failed"));
    }

    tokio::fs::remove_file(deb_path).await.ok();
    info!("Linux deb applied");
    Ok(())
}