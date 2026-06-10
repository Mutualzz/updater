use std::sync::Arc;
use tokio::sync::{broadcast, mpsc};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use log::{error, info, warn};

use crate::{InboundMsg, OutboundMsg};

#[cfg(unix)]
pub const SOCKET_PATH: &str = "/tmp/mutualzz-updater.sock";

#[cfg(windows)]
pub const SOCKET_PATH: &str = r"\\.\pipe\mutualzz-updater";

pub async fn serve(
    tx: Arc<broadcast::Sender<OutboundMsg>>,
    inbound_tx: mpsc::Sender<InboundMsg>,
) -> anyhow::Result<()> {
    #[cfg(unix)]
    {
        use tokio::net::UnixListener;

        // Remove stale socket file before binding
        if std::path::Path::new(SOCKET_PATH).exists() {
            std::fs::remove_file(SOCKET_PATH)?;
            info!("Removed stale socket: {}", SOCKET_PATH);
        }

        let listener = UnixListener::bind(SOCKET_PATH)?;
        info!("IPC listening on: {}", SOCKET_PATH);

        loop {
            match listener.accept().await {
                Ok((stream, _)) => {
                    info!("Electron connected");
                    let tx_clone = Arc::clone(&tx);
                    let inbound = inbound_tx.clone();
                    tokio::spawn(handle_unix_connection(stream, tx_clone, inbound));
                }
                Err(e) => error!("IPC accept error: {}", e),
            }
        }
    }

    #[cfg(windows)]
    {
        use tokio::net::windows::named_pipe::ServerOptions;

        info!("IPC listening on: {}", SOCKET_PATH);

        loop {
            let server = ServerOptions::new()
                .first_pipe_instance(false)
                .create(SOCKET_PATH)?;

            server.connect().await?;
            info!("Electron connected");

            let tx_clone = Arc::clone(&tx);
            let inbound = inbound_tx.clone();
            tokio::spawn(handle_windows_connection(server, tx_clone, inbound));
        }
    }
}

#[cfg(unix)]
async fn handle_unix_connection(
    stream: tokio::net::UnixStream,
    tx: Arc<broadcast::Sender<OutboundMsg>>,
    inbound_tx: mpsc::Sender<InboundMsg>,
) {
    let (reader, mut writer) = tokio::io::split(stream);
    let mut lines = BufReader::new(reader).lines();
    let mut rx = tx.subscribe();

    let write_task = tokio::spawn(async move {
        loop {
            match rx.recv().await {
                Ok(msg) => {
                    if let Ok(mut json) = serde_json::to_string(&msg) {
                        json.push('\n');
                        if writer.write_all(json.as_bytes()).await.is_err() {
                            break;
                        }
                    }
                }
                Err(broadcast::error::RecvError::Lagged(n)) => warn!("Lagged {}", n),
                Err(_) => break,
            }
        }
    });

    while let Ok(Some(line)) = lines.next_line().await {
        match serde_json::from_str::<InboundMsg>(&line) {
            Ok(msg) => {
                if inbound_tx.send(msg).await.is_err() {
                    break;
                }
            }
            Err(e) => warn!("Bad inbound msg: {} — {}", e, line),
        }
    }

    info!("Electron disconnected");
    write_task.abort();

    // Clean up socket file when connection closes
    std::fs::remove_file(SOCKET_PATH).ok();
}

#[cfg(windows)]
async fn handle_windows_connection(
    pipe: tokio::net::windows::named_pipe::NamedPipeServer,
    tx: Arc<broadcast::Sender<OutboundMsg>>,
    inbound_tx: mpsc::Sender<InboundMsg>,
) {
    let (reader, mut writer) = tokio::io::split(pipe);
    let mut lines = BufReader::new(reader).lines();
    let mut rx = tx.subscribe();

    let write_task = tokio::spawn(async move {
        loop {
            match rx.recv().await {
                Ok(msg) => {
                    if let Ok(mut json) = serde_json::to_string(&msg) {
                        json.push('\n');
                        if writer.write_all(json.as_bytes()).await.is_err() {
                            break;
                        }
                    }
                }
                Err(broadcast::error::RecvError::Lagged(n)) => warn!("Lagged {}", n),
                Err(_) => break,
            }
        }
    });

    while let Ok(Some(line)) = lines.next_line().await {
        match serde_json::from_str::<InboundMsg>(&line) {
            Ok(msg) => {
                if inbound_tx.send(msg).await.is_err() {
                    break;
                }
            }
            Err(e) => warn!("Bad inbound msg: {} — {}", e, line),
        }
    }

    info!("Electron disconnected");
    write_task.abort();
}