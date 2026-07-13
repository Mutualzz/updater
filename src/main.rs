#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

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

    let args: Vec<String> = std::env::args().collect();
    let is_apply = args.iter().any(|a| a == "--apply");
    let splash_fast = args.iter().any(|a| a == "--fast");

    let _lock = acquire_single_instance_lock(is_apply);

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

    if let Some(pos) = args.iter().position(|a| a == "--apply") {
        if let Some(path) = args.get(pos + 1) {
            let path = std::path::PathBuf::from(path);
            let version = args
                .iter()
                .position(|a| a == "--version")
                .and_then(|p| args.get(p + 1))
                .cloned()
                .unwrap_or_else(|| "pending".to_string());

            info!("--apply mode: {} (version: {})", path.display(), version);

            let (splash_tx, splash_rx) = std::sync::mpsc::channel::<SplashCmd>();
            let apply_ok = Arc::new(AtomicBool::new(false));
            let apply_ok_thread = apply_ok.clone();

            std::thread::spawn(move || {
                let rt = tokio::runtime::Runtime::new().expect("Failed to create Tokio runtime");
                rt.block_on(async move {
                    let _ = splash_tx.send(SplashCmd::SetStatus("Applying update...".into()));

                    match platform::apply_update(&path, &version, None).await {
                        Ok(_) => {
                            update::cleanup_update_temp();
                            apply_ok_thread.store(true, Ordering::SeqCst);
                        }
                        Err(e) => {
                            error!("Apply failed: {}", e);
                            let _ = splash_tx
                                .send(SplashCmd::SetStatus(format!("Update failed: {}", e)));
                            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                            let _ = splash_tx.send(SplashCmd::Close);
                        }
                    }
                });
            });

            splash::run(splash_rx, None);

            if !apply_ok.load(Ordering::SeqCst) {
                warn!("Apply failed — relaunching existing app");
                platform::exec_into_electron();
            }
            return;
        }
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

fn acquire_single_instance_lock(is_apply: bool) -> Option<std::fs::File> {
    if is_apply {
        return None;
    }

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
    let skip_check = {
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
                } else {
                    let electron_version = manifest.electron_version_for_current_platform();
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
