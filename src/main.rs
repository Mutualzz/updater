#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

mod install;
mod layout;
mod platform;
mod splash;
mod update;

use log::{error, info, warn};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

#[derive(Debug, Clone)]
pub enum SplashCmd {
    SetStatus(String),
    SetProgress(f64),
    HideProgress,
    SetAllowSkip(bool),
    Close,
}

fn main() {
    let log_path = std::env::temp_dir().join("mutualzz-updater.log");
    let log_file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
        .unwrap_or_else(|_| panic!("Failed to open log file: {}", log_path.display()));

    env_logger::Builder::new()
        .filter_level(log::LevelFilter::Info)
        .target(env_logger::Target::Pipe(Box::new(log_file)))
        .init();

    #[cfg(windows)]
    platform::set_app_user_model_id();

    layout::ensure_data_dirs();
    if let Err(e) = layout::migrate_legacy_layout_if_needed() {
        warn!("Legacy layout migration failed: {}", e);
    }

    #[cfg(windows)]
    {
        if layout::layout_v2_marker().is_file() {
            if let Some(version) = layout::read_current_version() {
                let _ = install::register_windows_uninstall_entry(&version);
            }
        }
    }

    let args: Vec<String> = std::env::args().collect();
    let splash_fast = args.iter().any(|a| a == "--fast");

    let _lock = acquire_single_instance_lock();

    if args.iter().any(|a| a == "--uninstall") {
        #[cfg(windows)]
        {
            match install::run_uninstall() {
                Ok(()) => std::process::exit(0),
                Err(e) => {
                    error!("Uninstall failed: {}", e);
                    std::process::exit(1);
                }
            }
        }

        #[cfg(not(windows))]
        {
            error!("Uninstall is only supported on Windows");
            std::process::exit(1);
        }
    }

    if args.iter().any(|a| a == "--splash-test") {
        let (tx, rx) = std::sync::mpsc::channel::<SplashCmd>();
        let tick = if splash_fast { 8u64 } else { 30 };
        let pause = if splash_fast { 200u64 } else { 1000 };
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(pause / 2));
            let _ = tx.send(SplashCmd::SetStatus("Checking for updates...".into()));
            std::thread::sleep(std::time::Duration::from_millis(pause));
            for i in 0..=100 {
                let _ = tx.send(SplashCmd::SetProgress(i as f64));
                let _ = tx.send(SplashCmd::SetStatus(update::format_download_status(
                    i as f64,
                    8_000_000,
                    (i as u64) * 1_048_576 / 10,
                    10 * 1_048_576,
                )));
                std::thread::sleep(std::time::Duration::from_millis(tick));
            }
            std::thread::sleep(std::time::Duration::from_millis(pause / 2));
            let _ = tx.send(SplashCmd::SetStatus("Launching Mutualzz...".into()));
            let _ = tx.send(SplashCmd::HideProgress);
            std::thread::sleep(std::time::Duration::from_millis(pause));
            let _ = tx.send(SplashCmd::Close);
        });
        splash::run(rx, None);
        return;
    }

    if should_run_install(&args) {
        let install_args = if args.iter().any(|a| a == "--install") {
            args.clone()
        } else {
            vec![args[0].clone(), "--install".to_string()]
        };

        if let Some((zip_path, version)) = install::parse_install_args(&install_args) {
            info!(
                "--install mode: {} (version: {})",
                zip_path.display(),
                version
            );

            let (splash_tx, splash_rx) = std::sync::mpsc::channel::<SplashCmd>();
            let install_ok = Arc::new(AtomicBool::new(false));
            let install_ok_thread = install_ok.clone();

            std::thread::spawn(move || {
                let rt = tokio::runtime::Runtime::new().expect("Failed to create Tokio runtime");
                rt.block_on(async move {
                    match install::run_install(splash_tx.clone(), zip_path, version).await {
                        Ok(()) => {
                            install_ok_thread.store(true, Ordering::SeqCst);
                            let _ = splash_tx.send(SplashCmd::Close);
                        }
                        Err(e) => {
                            error!("Install failed: {}", e);
                            let _ = splash_tx
                                .send(SplashCmd::SetStatus(format!("Install failed: {}", e)));
                            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                            let _ = splash_tx.send(SplashCmd::Close);
                        }
                    }
                });
            });

            splash::run(splash_rx, None);

            if install_ok.load(Ordering::SeqCst) {
                #[cfg(windows)]
                {
                    use std::os::windows::process::CommandExt;
                    const CREATE_BREAKAWAY_FROM_JOB: u32 = 0x01000000;
                    const CREATE_NEW_PROCESS_GROUP: u32 = 0x00000200;

                    let update_exe = layout::bootstrapper_at_data_root();
                    std::process::Command::new(&update_exe)
                        .creation_flags(CREATE_BREAKAWAY_FROM_JOB | CREATE_NEW_PROCESS_GROUP)
                        .spawn()
                        .expect("Failed to launch Update.exe after install");
                    std::process::exit(0);
                }

                #[cfg(not(windows))]
                {
                    platform::exec_into_electron();
                }
            }
        } else {
            error!("Install mode requested but no install package was found");
            let (splash_tx, splash_rx) = std::sync::mpsc::channel::<SplashCmd>();
            let _ = splash_tx.send(SplashCmd::SetStatus(
                "Install failed: package not found".into(),
            ));
            std::thread::spawn(move || {
                std::thread::sleep(std::time::Duration::from_secs(2));
                let _ = splash_tx.send(SplashCmd::Close);
            });
            splash::run(splash_rx, None);
        }
        return;
    }

    info!("Mutualzz bootstrapper starting");
    update::seed_installed_versions_if_needed();

    let skip_launch = Arc::new(AtomicBool::new(false));
    let (splash_tx, splash_rx) = std::sync::mpsc::channel::<SplashCmd>();
    let splash_tx_async = splash_tx.clone();
    let skip_async = skip_launch.clone();

    std::thread::spawn(move || {
        let rt = tokio::runtime::Runtime::new().expect("Failed to create Tokio runtime");
        rt.block_on(async_main(splash_tx_async, skip_async));
    });

    splash::run(splash_rx, Some(skip_launch));
}

fn should_run_install(args: &[String]) -> bool {
    if args.iter().any(|a| a == "--install") {
        return install::parse_install_args(args).is_some();
    }

    #[cfg(target_os = "windows")]
    {
        let is_setup = std::env::current_exe()
            .ok()
            .and_then(|p| {
                p.file_stem()
                    .map(|s| s.to_string_lossy().to_ascii_lowercase())
            })
            .map(|name| name.contains("setup"))
            .unwrap_or(false);

        if is_setup && install::bundled_install_zip().is_some() {
            return true;
        }
    }

    false
}

fn acquire_single_instance_lock() -> Option<std::fs::File> {
    let lock_path = std::env::temp_dir().join("mutualzz-updater.lock");

    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        match std::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .share_mode(0)
            .open(&lock_path)
        {
            Ok(f) => Some(f),
            Err(_) => {
                info!("Another updater instance is already running, exiting");
                std::process::exit(0);
            }
        }
    }

    #[cfg(unix)]
    {
        use std::os::unix::io::AsRawFd;
        match std::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .open(&lock_path)
        {
            Ok(f) => {
                let rc = unsafe { libc::flock(f.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
                if rc != 0 {
                    info!("Another updater instance is already running, exiting");
                    std::process::exit(0);
                }
                Some(f)
            }
            Err(e) => {
                warn!("Could not create lock file: {}", e);
                None
            }
        }
    }
}

async fn async_main(
    splash_tx: std::sync::mpsc::Sender<SplashCmd>,
    skip_launch: Arc<AtomicBool>,
) {
    let _ = splash_tx.send(SplashCmd::SetStatus("Checking for updates...".into()));

    let applied_in_process = match update::apply_staged_or_pending().await {
        Ok(update::StagedApplyOutcome::AppliedInProcess) => true,
        Ok(update::StagedApplyOutcome::None) => false,
        Err(e) => {
            warn!("Staged apply failed: {}", e);
            let _ = splash_tx.send(SplashCmd::SetStatus(format!("Update failed: {}", e)));
            tokio::time::sleep(std::time::Duration::from_millis(1500)).await;
            false
        }
    };

    let skip_check = if applied_in_process {
        true
    } else {
        let marker = platform::just_updated_marker();
        if let Ok(marker_version) = std::fs::read_to_string(&marker) {
            let marker_ver = marker_version.trim();
            let installed = update::get_installed_version();
            std::fs::remove_file(&marker).ok();

            if marker_ver == installed {
                info!("Just updated to {} — skipping check", installed);
            } else {
                warn!(
                    "Stale marker (marker={}, installed={}) — checking anyway",
                    marker_ver, installed
                );
            }
            marker_ver == installed
        } else {
            false
        }
    };

    if skip_check {
        update::cleanup_update_temp();
        let _ = splash_tx.send(SplashCmd::SetStatus("Up to date!".into()));
    } else {
        let _ = splash_tx.send(SplashCmd::SetStatus("Checking for updates...".into()));

        match update::check_for_update().await {
            Ok(Some(manifest)) => {
                info!("Update available: {}", manifest.version);
                let version = manifest.version.clone();

                if let Some(asar) = manifest.asar_update() {
                    info!("Electron runtime unchanged — using fast asar update");
                    let tx = splash_tx.clone();

                    match update::download_asar_update(&asar, move |percent, bps, dl, total| {
                        let _ = tx.send(SplashCmd::SetProgress(percent));
                        let _ = tx.send(SplashCmd::SetStatus(update::format_download_status(
                            percent, bps, dl, total,
                        )));
                    })
                    .await
                    {
                        Ok(path) => {
                            info!("Asar update downloaded: {}", path.display());
                            let _ = splash_tx.send(SplashCmd::SetProgress(100.0));
                            let _ =
                                splash_tx.send(SplashCmd::SetStatus("Applying update...".into()));

                            match platform::apply_asar_update(&path, &version).await {
                                Ok(()) => {
                                    update::cleanup_update_temp();
                                }
                                Err(e) => {
                                    error!("Failed to apply asar update: {}", e);
                                    let _ = splash_tx.send(SplashCmd::SetStatus(format!(
                                        "Update failed: {}",
                                        e
                                    )));
                                    tokio::time::sleep(std::time::Duration::from_millis(1500))
                                        .await;
                                }
                            }
                        }
                        Err(e) => {
                            error!("Asar download failed: {}", e);
                            let _ = splash_tx
                                .send(SplashCmd::SetStatus(format!("Download failed: {}", e)));
                            tokio::time::sleep(std::time::Duration::from_millis(1500)).await;
                        }
                    }
                } else if manifest.package_download_for_current_platform().is_some() {
                    let electron_version = manifest.electron_version_for_current_platform();
                    let updater_version = manifest.updater_version_for_current_platform();
                    let tx = splash_tx.clone();

                    match update::download_update(&manifest, move |percent, bps, dl, total| {
                        let _ = tx.send(SplashCmd::SetProgress(percent));
                        let _ = tx.send(SplashCmd::SetStatus(update::format_download_status(
                            percent, bps, dl, total,
                        )));
                    })
                    .await
                    {
                        Ok(path) => {
                            info!("Update downloaded: {}", path.display());
                            let _ = splash_tx.send(SplashCmd::SetProgress(100.0));
                            let _ =
                                splash_tx.send(SplashCmd::SetStatus("Applying update...".into()));

                            match platform::apply_update(
                                &path,
                                &version,
                                electron_version.as_deref(),
                                updater_version.as_deref(),
                            )
                            .await
                            {
                                Ok(()) => {
                                    update::cleanup_update_temp();
                                }
                                Err(e) => {
                                    error!("Failed to apply update: {}", e);
                                    let _ = splash_tx.send(SplashCmd::SetStatus(format!(
                                        "Update failed: {}",
                                        e
                                    )));
                                    tokio::time::sleep(std::time::Duration::from_millis(1500))
                                        .await;
                                }
                            }
                        }
                        Err(e) => {
                            error!("Download failed: {}", e);
                            let _ = splash_tx
                                .send(SplashCmd::SetStatus(format!("Download failed: {}", e)));
                            tokio::time::sleep(std::time::Duration::from_millis(1500)).await;
                        }
                    }
                } else {
                    warn!("No applicable update path for current platform");
                    let _ = splash_tx.send(SplashCmd::SetStatus("Up to date!".into()));
                }
            }
            Ok(None) => {
                info!("No update available");
                let _ = splash_tx.send(SplashCmd::SetStatus("Up to date!".into()));
            }
            Err(e) => {
                warn!("Update check failed: {}", e);
                skip_launch.store(false, Ordering::SeqCst);
                let _ = splash_tx.send(SplashCmd::SetAllowSkip(true));
                let _ = splash_tx.send(SplashCmd::SetStatus(
                    "Offline — hold Space to launch".into(),
                ));

                let deadline =
                    tokio::time::Instant::now() + std::time::Duration::from_millis(1800);
                while tokio::time::Instant::now() < deadline {
                    if skip_launch.load(Ordering::SeqCst) {
                        info!("User skipped offline update check");
                        break;
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(40)).await;
                }

                let _ = splash_tx.send(SplashCmd::SetAllowSkip(false));
            }
        }
    }

    tokio::time::sleep(std::time::Duration::from_millis(80)).await;
    let _ = splash_tx.send(SplashCmd::SetStatus("Launching Mutualzz...".into()));
    let _ = splash_tx.send(SplashCmd::HideProgress);
    tokio::time::sleep(std::time::Duration::from_millis(80)).await;

    #[cfg(windows)]
    {
        platform::exec_into_electron();
    }

    #[cfg(not(windows))]
    {
        let _ = splash_tx.send(SplashCmd::Close);
        platform::exec_into_electron();
    }
}
