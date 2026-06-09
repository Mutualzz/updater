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
    return dir.join("mutualzz");
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

pub async fn apply_update(update_path: &std::path::Path) -> anyhow::Result<()> {
    let install = install_dir();
    info!("Applying {} → {}", update_path.display(), install.display());

    let ext = update_path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("");

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

    relaunch_bootstrapper();
}

fn relaunch_bootstrapper() -> ! {
    let exe = std::env::current_exe().expect("Cannot resolve bootstrapper path");
    info!("Relaunching: {}", exe.display());

    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        let err = std::process::Command::new(&exe).exec();
        panic!("Failed to re-exec: {}", err);
    }

    #[cfg(windows)]
    {
        std::process::Command::new(&exe).spawn().expect("Relaunch failed");
        std::process::exit(0);
    }
}

#[cfg(target_os = "macos")]
async fn apply_dmg(dmg_path: &std::path::Path, install_dir: &std::path::Path) -> anyhow::Result<()> {
    use tokio::process::Command;

    let out = Command::new("hdiutil")
        .args(["attach", "-nobrowse", "-quiet", "-noverify"])
        .arg(dmg_path)
        .output()
        .await?;

    if !out.status.success() {
        return Err(anyhow::anyhow!(
            "hdiutil failed: {}",
            String::from_utf8_lossy(&out.stderr)
        ));
    }

    let stdout = String::from_utf8_lossy(&out.stdout);

    // hdiutil output format: "diskN  <type>  /Volumes/AppName"
    // Find the line with /Volumes/ which is the mount point
    let mount_point = stdout
        .lines()
        .filter_map(|line| {
            let parts: Vec<&str> = line.split('\t').collect();
            parts.last().map(|s| s.trim().to_string())
        })
        .find(|s| s.starts_with("/Volumes/"))
        .ok_or_else(|| anyhow::anyhow!(
            "No mount point found in hdiutil output:\n{}",
            stdout
        ))?;

    info!("DMG mounted at: {}", mount_point);

    let app_in_dmg = std::fs::read_dir(&mount_point)?
        .filter_map(|e| e.ok())
        .find(|e| e.path().extension().and_then(|x| x.to_str()) == Some("app"))
        .map(|e| e.path())
        .ok_or_else(|| anyhow::anyhow!("No .app in DMG"))?;

    let apps_dir = install_dir
        .parent()
        .ok_or_else(|| anyhow::anyhow!("No parent dir for .app"))?;

    let rsync = Command::new("rsync")
        .args(["-a", "--delete"])
        .arg(&app_in_dmg)
        .arg(apps_dir)
        .status()
        .await?;

    if !rsync.success() {
        return Err(anyhow::anyhow!("rsync failed"));
    }

    let _ = Command::new("hdiutil")
        .args(["detach", "-quiet", &mount_point])
        .status()
        .await;

    tokio::fs::remove_file(dmg_path).await.ok();
    info!("macOS update applied");
    Ok(())
}

#[cfg(target_os = "windows")]
async fn apply_nsis(installer_path: &std::path::Path) -> anyhow::Result<()> {
    use tokio::process::Command;

    let status = Command::new(installer_path).arg("/S").status().await?;
    if !status.success() {
        return Err(anyhow::anyhow!("NSIS installer failed"));
    }

    tokio::fs::remove_file(installer_path).await.ok();
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

    let dest = install_dir.join("mutualzz");
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