use std::sync::Arc;
use tokio::sync::{broadcast, mpsc};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use log::{error, info, warn};
use interprocess::local_socket::{
    tokio::{prelude::*, Stream},
    GenericFilePath, ListenerOptions,
};

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
    if std::path::Path::new(SOCKET_PATH).exists() {
        std::fs::remove_file(SOCKET_PATH).ok();
        info!("Removed stale socket: {}", SOCKET_PATH);
    }

    let name = SOCKET_PATH.to_fs_name::<GenericFilePath>()?;
    let opts = ListenerOptions::new().name(name);
    let listener = opts.create_tokio()?;

    info!("IPC listening on: {}", SOCKET_PATH);

    loop {
        match listener.accept().await {
            Ok(conn) => {
                info!("Electron connected");
                let tx_clone = Arc::clone(&tx);
                let inbound = inbound_tx.clone();
                tokio::spawn(handle_connection(conn, tx_clone, inbound));
            }
            Err(e) => error!("IPC accept error: {}", e),
        }
    }
}

async fn handle_connection(
    conn: Stream,
    tx: Arc<broadcast::Sender<OutboundMsg>>,
    inbound_tx: mpsc::Sender<InboundMsg>,
) {
    let (reader, mut writer) = conn.split();
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