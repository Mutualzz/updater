use crate::layout;
use crate::platform;
use crate::SplashCmd;
use log::{error, info};
use std::path::PathBuf;

pub async fn run_install(
    splash_tx: std::sync::mpsc::Sender<SplashCmd>,
    zip_path: PathBuf,
    version: String,
) -> anyhow::Result<()> {
    let _ = splash_tx.send(SplashCmd::SetStatus("Installing Mutualzz...".into()));

    layout::ensure_data_dirs();
    let dest = layout::app_version_dir(&version);

    if dest.exists() {
        let _ = tokio::fs::remove_dir_all(&dest).await;
    }

    let _ = splash_tx.send(SplashCmd::SetStatus("Extracting files...".into()));
    platform::extract_zip_package(&zip_path, &dest).await?;

    let version = layout::read_version_from_app_dir(&dest).unwrap_or(version);

    layout::set_current_version(&version);
    layout::hoist_windows_update_exe(&dest)?;

    if let Some(ev) = read_resource_version("electron-runtime-version.txt") {
        crate::update::set_installed_electron_version(&ev);
    } else if let Some(ev) = read_bundled_resource_from_dir(&dest, "electron-runtime-version.txt") {
        crate::update::set_installed_electron_version(&ev);
    }
    if let Some(uv) = read_resource_version("updater-runtime-version.txt") {
        crate::update::set_installed_updater_version(&uv);
    } else if let Some(uv) = read_bundled_resource_from_dir(&dest, "updater-runtime-version.txt") {
        crate::update::set_installed_updater_version(&uv);
    }
    crate::update::set_installed_version(&version);

    #[cfg(target_os = "windows")]
    {
        rewrite_windows_shortcuts()?;
        register_windows_uninstall_entry(&version)?;
    }

    std::fs::write(layout::layout_v2_marker(), b"2").ok();
    std::fs::write(platform::just_updated_marker(), version.as_bytes()).ok();

    let _ = splash_tx.send(SplashCmd::SetProgress(100.0));
    let _ = splash_tx.send(SplashCmd::SetStatus("Launching Mutualzz...".into()));

    info!("Install complete for version {}", version);
    Ok(())
}

fn read_resource_version(name: &str) -> Option<String> {
    let exe = std::env::current_exe().ok()?;
    let dir = exe.parent()?;
    for candidate in [dir.join("resources").join(name), dir.join(name)] {
        if candidate.is_file() {
            return std::fs::read_to_string(candidate)
                .ok()
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty());
        }
    }
    None
}

fn read_bundled_resource_from_dir(dir: &std::path::Path, name: &str) -> Option<String> {
    for candidate in [dir.join("resources").join(name), dir.join(name)] {
        if candidate.is_file() {
            return std::fs::read_to_string(candidate)
                .ok()
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty());
        }
    }
    None
}

#[cfg(target_os = "windows")]
pub fn rewrite_windows_shortcuts() -> anyhow::Result<()> {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x08000000;

    let update_exe = layout::data_root().join("Update.exe");
    let update = update_exe.to_string_lossy();
    let mutualzz = layout::resolve_active_app_dir()
        .map(|d| d.join("mutualzz.exe"))
        .unwrap_or_else(|| update_exe.clone());

    let desktop = dirs::desktop_dir().unwrap_or_else(|| layout::data_root());
    let desktop_lnk = desktop.join("Mutualzz.lnk");
    let start_menu = dirs::data_local_dir()
        .unwrap_or_else(|| layout::data_root())
        .join("Programs")
        .join("Mutualzz");
    std::fs::create_dir_all(&start_menu).ok();
    let start_lnk = start_menu.join("Mutualzz.lnk");

    let ps = format!(
        r#"
$WshShell = New-Object -ComObject WScript.Shell
$desktop = '{desktop}'
$start = '{start}'
$target = '{update}'
$icon = '{icon}'
$d = $WshShell.CreateShortcut($desktop)
$d.TargetPath = $target
$d.IconLocation = $icon
$d.Save()
$s = $WshShell.CreateShortcut($start)
$s.TargetPath = $target
$s.IconLocation = $icon
$s.Save()
"#,
        desktop = desktop_lnk.to_string_lossy().replace('\'', "''"),
        start = start_lnk.to_string_lossy().replace('\'', "''"),
        update = update.replace('\'', "''"),
        icon = mutualzz.to_string_lossy().replace('\'', "''"),
    );

    let status = std::process::Command::new("powershell")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-Command",
            &ps,
        ])
        .creation_flags(CREATE_NO_WINDOW)
        .status()?;

    if !status.success() {
        error!("Shortcut creation returned {:?}", status.code());
    }
    Ok(())
}

#[cfg(target_os = "windows")]
const UNINSTALL_REG_KEY: &str =
    r"HKCU\Software\Microsoft\Windows\CurrentVersion\Uninstall\Mutualzz";

#[cfg(target_os = "windows")]
pub fn register_windows_uninstall_entry(version: &str) -> anyhow::Result<()> {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x08000000;

    let install_dir = layout::data_root();
    let update_exe = install_dir.join("Update.exe");
    let icon = layout::resolve_active_app_dir()
        .map(|d| d.join("mutualzz.exe"))
        .filter(|p| p.is_file())
        .unwrap_or_else(|| update_exe.clone());

    let uninstall = format!(
        "\"{}\" --uninstall",
        update_exe.to_string_lossy().replace('"', "\\\"")
    );

    let entries: [(&str, &str, &str); 8] = [
        ("DisplayName", "REG_SZ", "Mutualzz"),
        ("DisplayVersion", "REG_SZ", version),
        ("Publisher", "REG_SZ", "Mutualzz"),
        ("InstallLocation", "REG_SZ", &install_dir.to_string_lossy()),
        ("DisplayIcon", "REG_SZ", &icon.to_string_lossy()),
        ("UninstallString", "REG_SZ", &uninstall),
        ("NoModify", "REG_DWORD", "1"),
        ("NoRepair", "REG_DWORD", "1"),
    ];

    for (name, value_type, data) in entries {
        let status = std::process::Command::new("reg")
            .args([
                "add",
                UNINSTALL_REG_KEY,
                "/v",
                name,
                "/t",
                value_type,
                "/d",
                data,
                "/f",
            ])
            .creation_flags(CREATE_NO_WINDOW)
            .status()?;

        if !status.success() {
            error!("Uninstall registry write failed for {} ({:?})", name, status.code());
        }
    }

    Ok(())
}

#[cfg(target_os = "windows")]
pub fn run_uninstall() -> anyhow::Result<()> {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x08000000;

    let desktop = dirs::desktop_dir().unwrap_or_else(|| layout::data_root());
    let desktop_lnk = desktop.join("Mutualzz.lnk");
    let start_menu = dirs::data_local_dir()
        .unwrap_or_else(|| layout::data_root())
        .join("Programs")
        .join("Mutualzz");

    let ps = format!(
        r#"
Remove-Item -LiteralPath '{desktop}' -Force -ErrorAction SilentlyContinue
Remove-Item -LiteralPath '{start}' -Recurse -Force -ErrorAction SilentlyContinue
"#,
        desktop = desktop_lnk.to_string_lossy().replace('\'', "''"),
        start = start_menu.to_string_lossy().replace('\'', "''"),
    );

    let _ = std::process::Command::new("powershell")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-Command",
            &ps,
        ])
        .creation_flags(CREATE_NO_WINDOW)
        .status();

    let _ = std::process::Command::new("reg")
        .args([
            "delete",
            r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run",
            "/v",
            "Mutualzz",
            "/f",
        ])
        .creation_flags(CREATE_NO_WINDOW)
        .status();

    let _ = std::process::Command::new("reg")
        .args(["delete", UNINSTALL_REG_KEY, "/f"])
        .creation_flags(CREATE_NO_WINDOW)
        .status();

    let root = layout::data_root();
    if root.exists() {
        std::fs::remove_dir_all(&root)?;
    }

    info!("Uninstall complete");
    Ok(())
}

pub fn bundled_install_zip() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let dir = exe.parent()?;
    for name in ["Mutualzz-win.zip", "install.zip"] {
        let path = dir.join(name);
        if path.is_file() {
            return Some(path);
        }
    }
    None
}

pub fn parse_install_args(args: &[String]) -> Option<(PathBuf, String)> {
    let zip = args
        .iter()
        .position(|a| a == "--install")
        .and_then(|pos| args.get(pos + 1))
        .map(PathBuf::from)
        .or_else(bundled_install_zip)?;

    let version = args
        .iter()
        .position(|a| a == "--version")
        .and_then(|p| args.get(p + 1))
        .cloned()
        .or_else(|| read_resource_version("app-version.txt"))
        .filter(|v| v != "0.0.0")
        .unwrap_or_else(|| "0.0.0".to_string());

    Some((zip, version))
}
