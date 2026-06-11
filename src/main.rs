#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

mod platform;
mod splash;
mod update;

use log::{error, info, warn};

#[derive(Debug, Clone)]
pub enum SplashCmd {
    SetStatus(String),
    SetProgress(f64),
    HideProgress,
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

    if std::env::args().any(|a| a == "--splash-test") {
        let (tx, rx) = std::sync::mpsc::channel::<SplashCmd>();
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(500));
            let _ = tx.send(SplashCmd::SetStatus("Checking for updates...".into()));
            std::thread::sleep(std::time::Duration::from_secs(1));
            for i in 0..=100 {
                let _ = tx.send(SplashCmd::SetProgress(i as f64));
                let _ = tx.send(SplashCmd::SetStatus(format!("Downloading... {}%", i)));
                std::thread::sleep(std::time::Duration::from_millis(30));
            }
            std::thread::sleep(std::time::Duration::from_secs(1));
            let _ = tx.send(SplashCmd::SetStatus("Launching Mutualzz...".into()));
            let _ = tx.send(SplashCmd::HideProgress);
            std::thread::sleep(std::time::Duration::from_secs(2));
            let _ = tx.send(SplashCmd::Close);
        });
        splash::run(rx);
        return;
    }

    // --apply <path> [--version <version>] mode
    let args: Vec<String> = std::env::args().collect();
    if let Some(pos) = args.iter().position(|a| a == "--apply") {
        if let Some(path) = args.get(pos + 1) {
            let path = std::path::PathBuf::from(path);
            let version = args.iter()
                .position(|a| a == "--version")
                .and_then(|p| args.get(p + 1))
                .map(|s| s.clone())
                .unwrap_or_else(|| "pending".to_string());

            let (splash_tx, splash_rx) = std::sync::mpsc::channel::<SplashCmd>();

            info!("Apply mode: {} (version: {})", path.display(), version);

            std::thread::spawn(move || {
                let rt = tokio::runtime::Runtime::new().expect("Failed to create Tokio runtime");
                rt.block_on(async move {
                    let _ = splash_tx.send(SplashCmd::SetStatus("Applying update...".into()));
                    let _ = splash_tx.send(SplashCmd::SetProgress(100.0));

                    match platform::apply_update(&path, &version).await {
                        Ok(_) => {
                            // apply_update calls relaunch — never reached
                        }
                        Err(e) => {
                            error!("Apply failed: {}", e);
                            let _ = splash_tx.send(SplashCmd::SetStatus(
                                format!("Update failed: {}", e)
                            ));
                            tokio::time::sleep(std::time::Duration::from_secs(3)).await;
                            let _ = splash_tx.send(SplashCmd::Close);
                        }
                    }
                });
            });

            splash::run(splash_rx);
            return;
        }
    }

    info!("Mutualzz bootstrapper starting");

    let (splash_tx, splash_rx) = std::sync::mpsc::channel::<SplashCmd>();
    let splash_tx_async = splash_tx.clone();

    std::thread::spawn(move || {
        let rt = tokio::runtime::Runtime::new().expect("Failed to create Tokio runtime");
        rt.block_on(async_main(splash_tx_async));
    });

    splash::run(splash_rx);
}

async fn async_main(splash_tx: std::sync::mpsc::Sender<SplashCmd>) {
    let skip_check = {
        let marker = platform::just_updated_marker();
        if let Ok(marker_version) = std::fs::read_to_string(&marker) {
            let marker_ver = marker_version.trim();
            let installed = update::get_installed_version();
            std::fs::remove_file(&marker).ok();

            if marker_ver == installed {
                info!("Just updated to {} — skipping check", installed);
                true
            } else {
                info!("Stale marker (marker={}, installed={}) — checking", marker_ver, installed);
                false
            }
        } else {
            false
        }
    };

    if skip_check {
        let _ = splash_tx.send(SplashCmd::SetStatus("Up to date!".into()));
    } else {
        let _ = splash_tx.send(SplashCmd::SetStatus("Checking for updates...".into()));

        match update::check_for_update().await {
            Ok(Some(manifest)) => {
                info!("Update available: {}", manifest.version);
                let version = manifest.version.clone();
                let tx = splash_tx.clone();

                match update::download_update(&manifest, move |percent, bps, _t, _total| {
                    let _ = tx.send(SplashCmd::SetProgress(percent));
                    let _ = tx.send(SplashCmd::SetStatus(format!(
                        "Downloading... {:.0}%  ({:.1} MB/s)",
                        percent,
                        bps as f64 / 1_048_576.0
                    )));
                })
                    .await
                {
                    Ok(path) => {
                        info!("Update downloaded: {}", path.display());
                        let _ = splash_tx.send(SplashCmd::SetProgress(100.0));
                        let _ = splash_tx.send(SplashCmd::SetStatus("Applying update...".into()));

                        if let Err(e) = platform::apply_update(&path, &version).await {
                            error!("Failed to apply update: {}", e);
                            let _ = splash_tx.send(SplashCmd::SetStatus(
                                format!("Update failed: {}", e)
                            ));
                            tokio::time::sleep(std::time::Duration::from_secs(3)).await;
                        }
                    }
                    Err(e) => {
                        error!("Download failed: {}", e);
                        let _ = splash_tx.send(SplashCmd::SetStatus(
                            format!("Download failed: {}", e)
                        ));
                        tokio::time::sleep(std::time::Duration::from_secs(3)).await;
                    }
                }
            }
            Ok(None) => {
                info!("No update available");
                let _ = splash_tx.send(SplashCmd::SetStatus("Up to date!".into()));
            }
            Err(e) => {
                warn!("Update check failed: {}", e);
                let _ = splash_tx.send(SplashCmd::SetStatus("Could not check for updates".into()));
            }
        }
    }

    tokio::time::sleep(std::time::Duration::from_millis(600)).await;
    let _ = splash_tx.send(SplashCmd::SetStatus("Launching Mutualzz...".into()));
    let _ = splash_tx.send(SplashCmd::HideProgress);
    tokio::time::sleep(std::time::Duration::from_millis(400)).await;
    let _ = splash_tx.send(SplashCmd::Close);

    platform::exec_into_electron();
}