#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

mod ipc;
mod platform;
mod splash;
mod update;
mod build;

use std::process::Stdio;
use std::sync::Arc;
use tokio::process::Command;
use tokio::sync::{broadcast, Mutex};
use log::{error, info, warn};

pub use update::UpdateManifest;

#[derive(Debug, Clone, serde::Serialize)]
#[serde(tag = "type", rename_all = "SCREAMING_SNAKE_CASE")]
pub enum OutboundMsg {
    UpdateAvailable { version: String },
    DownloadProgress { percent: f64, bytes_per_second: u64, transferred: u64, total: u64 },
    UpdateReady { version: String },
    UpdateError { message: String },
    NoUpdate,
}

#[derive(Debug, Clone, serde::Deserialize)]
#[serde(tag = "type", rename_all = "SCREAMING_SNAKE_CASE")]
pub enum InboundMsg {
    ApplyUpdate,
    CheckUpdate,
}

#[derive(Debug, Clone)]
pub enum SplashCmd {
    SetStatus(String),
    SetProgress(f64),
    HideProgress,
    Close,
}

fn main() {
    env_logger::init();

    if std::env::args().any(|a| a == "--splash-test") {
        let (tx, rx) = std::sync::mpsc::channel::<SplashCmd>();
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(500));
            let _ = tx.send(SplashCmd::SetStatus("Checking for updates...".into()));
            std::thread::sleep(std::time::Duration::from_secs(1));

            let _ = tx.send(SplashCmd::SetStatus("Downloading update 6.5.0...".into()));
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
    let (outbound_tx, _) = broadcast::channel::<OutboundMsg>(32);
    let outbound_tx = Arc::new(outbound_tx);

    let (inbound_tx, mut inbound_rx) = tokio::sync::mpsc::channel::<InboundMsg>(8);
    let pending_update: Arc<Mutex<Option<std::path::PathBuf>>> = Arc::new(Mutex::new(None));

    let skip_check = {
        let marker = platform::just_updated_marker();
        if let Ok(installed_version) = std::fs::read_to_string(&marker) {
            let installed = installed_version.trim();
            let current = env!("CARGO_PKG_VERSION");
            std::fs::remove_file(&marker).ok();

            if installed == current {
                info!("Just updated to {} — skipping update check this launch", current);
                true
            } else {
                info!("Stale marker (installed={}, current={}) — doing normal check", installed, current);
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

                match update::download_update(&manifest, move |percent, bps, _transferred, _total| {
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
                        } else {
                            return;
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

    if platform::is_app_already_running() {
        info!("App already running, skipping launch");
        let _ = splash_tx.send(SplashCmd::Close);
    } else {
        let electron_path = platform::electron_exe_path();
        info!("Launching Electron: {}", electron_path.display());

        let mut child = Command::new(&electron_path)
            .stdin(Stdio::null())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .spawn()
            .expect("Failed to launch Electron");

        tokio::time::sleep(std::time::Duration::from_millis(800)).await;
        let _ = splash_tx.send(SplashCmd::Close);

        let ipc_tx = Arc::clone(&outbound_tx);
        tokio::spawn(async move {
            if let Err(e) = ipc::serve(ipc_tx, inbound_tx).await {
                error!("IPC server error: {}", e);
            }
        });

        {
            let tx = Arc::clone(&outbound_tx);
            let pending = Arc::clone(&pending_update);
            tokio::spawn(async move {
                let mut interval =
                    tokio::time::interval(std::time::Duration::from_secs(3600));
                interval.tick().await;
                loop {
                    interval.tick().await;
                    run_background_check(Arc::clone(&tx), Arc::clone(&pending)).await;
                }
            });
        }

        while let Some(msg) = inbound_rx.recv().await {
            match msg {
                InboundMsg::ApplyUpdate => {
                    let path = pending_update.lock().await.clone();
                    if let Some(update_path) = path {
                        info!("Apply requested: {}", update_path.display());
                        tokio::time::sleep(std::time::Duration::from_millis(400)).await;
                        if let Err(e) = platform::apply_update(&update_path, "pending").await {
                            error!("Apply failed: {}", e);
                            let _ = outbound_tx.send(OutboundMsg::UpdateError {
                                message: e.to_string(),
                            });
                        }
                        std::process::exit(0);
                    } else {
                        warn!("ApplyUpdate received but no pending update");
                    }
                }
                InboundMsg::CheckUpdate => {
                    run_background_check(
                        Arc::clone(&outbound_tx),
                        Arc::clone(&pending_update),
                    ).await;
                }
            }
        }

        let _ = child.wait().await;
    }

    std::process::exit(0);
}

async fn run_background_check(
    tx: Arc<broadcast::Sender<OutboundMsg>>,
    pending: Arc<Mutex<Option<std::path::PathBuf>>>,
) {
    info!("Background update check...");

    match update::check_for_update().await {
        Ok(Some(manifest)) => {
            let version = manifest.version.clone();
            let _ = tx.send(OutboundMsg::UpdateAvailable { version: version.clone() });

            match update::download_update(&manifest, |percent, bps, transferred, total| {
                let _ = tx.send(OutboundMsg::DownloadProgress {
                    percent,
                    bytes_per_second: bps,
                    transferred,
                    total,
                });
            })
                .await
            {
                Ok(path) => {
                    *pending.lock().await = Some(path);
                    let _ = tx.send(OutboundMsg::UpdateReady { version });
                }
                Err(e) => {
                    let _ = tx.send(OutboundMsg::UpdateError { message: e.to_string() });
                }
            }
        }
        Ok(None) => {
            let _ = tx.send(OutboundMsg::NoUpdate);
        }
        Err(e) => {
            let _ = tx.send(OutboundMsg::UpdateError { message: e.to_string() });
        }
    }
}