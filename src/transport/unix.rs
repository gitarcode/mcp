use std::path::PathBuf;
use std::sync::Arc;
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    net::{UnixListener, UnixStream},
    sync::mpsc,
    time::{timeout, Duration},
};
use async_trait::async_trait;

use crate::error::McpError;
use super::{
    Transport,
    TransportChannels,
    TransportCommand,
    TransportEvent,
    JsonRpcMessage,
};

pub struct UnixTransport {
    path: PathBuf,
    server_mode: bool,
    buffer_size: usize,
}

impl UnixTransport {
    pub fn new_server(path: PathBuf, buffer_size: Option<usize>) -> Self {
        Self {
            path,
            server_mode: true,
            buffer_size: buffer_size.unwrap_or(4092),
        }
    }

    pub fn new_client(path: PathBuf, buffer_size: Option<usize>) -> Self {
        Self {
            path,
            server_mode: false,
            buffer_size: buffer_size.unwrap_or(4092),
        }
    }

    async fn handle_connection(
        stream: UnixStream,
        cmd_rx: mpsc::Receiver<TransportCommand>,
        event_tx: mpsc::Sender<TransportEvent>,
    ) {
        let (reader, writer) = stream.into_split();
        let mut reader = BufReader::new(reader);
        let mut writer = writer;
        let (write_tx, mut write_rx) = mpsc::channel::<String>(32);

        // Writer task
        let writer_handle = tokio::spawn(async move {
            while let Some(msg) = write_rx.recv().await {
                if !msg.contains("notifications/message") && !msg.contains("list_changed") {
                    tracing::debug!("-> {}", msg);
                }

                if let Err(e) = async {
                    writer.write_all(msg.as_bytes()).await?;
                    writer.write_all(b"\n").await?;
                    writer.flush().await?;
                    Ok::<_, std::io::Error>(())
                }.await {
                    tracing::error!("Write error: {:?}", e);
                    break;
                }
            }
        });

        // Reader task
        let reader_handle = tokio::spawn({
            let event_tx = event_tx.clone();
            async move {
                let mut line = String::new();
                loop {
                    line.clear();
                    match reader.read_line(&mut line).await {
                        Ok(0) => break, // EOF
                        Ok(_) => {
                            let trimmed = line.trim();
                            if !trimmed.contains("notifications/message") && !trimmed.contains("list_changed") {
                                tracing::debug!("<- {}", trimmed);
                            }

                            if !trimmed.is_empty() {
                                match serde_json::from_str::<JsonRpcMessage>(trimmed) {
                                    Ok(msg) => {
                                        if event_tx.send(TransportEvent::Message(msg)).await.is_err() {
                                            break;
                                        }
                                    }
                                    Err(e) => {
                                        tracing::error!("Parse error: {}, input: {}", e, trimmed);
                                        if event_tx.send(TransportEvent::Error(McpError::ParseError)).await.is_err() {
                                            break;
                                        }
                                    }
                                }
                            }
                        }
                        Err(e) => {
                            tracing::error!("Read error: {:?}", e);
                            let _ = event_tx.send(TransportEvent::Error(McpError::IoError)).await;
                            break;
                        }
                    }
                }
            }
        });

        // Main message loop
        let mut cmd_rx = cmd_rx;
        while let Some(cmd) = cmd_rx.recv().await {
            match cmd {
                TransportCommand::SendMessage(msg) => {
                    match serde_json::to_string(&msg) {
                        Ok(s) => {
                            if write_tx.send(s).await.is_err() {
                                break;
                            }
                        }
                        Err(e) => tracing::error!("Failed to serialize message: {:?}", e),
                    }
                }
                TransportCommand::Close => break,
            }
        }

        // Cleanup
        drop(write_tx);
        let _ = reader_handle.await;
        let _ = writer_handle.await;
        let _ = event_tx.send(TransportEvent::Closed).await;
    }

    async fn run_server(
        path: PathBuf,
        cmd_rx: mpsc::Receiver<TransportCommand>,
        event_tx: mpsc::Sender<TransportEvent>,
    ) {
        // Remove existing socket file if it exists
        if path.exists() {
            let _ = std::fs::remove_file(&path);
        }

        // Create and bind to the Unix socket
        let listener = match UnixListener::bind(&path) {
            Ok(l) => l,
            Err(e) => {
                tracing::error!("Failed to bind Unix socket: {:?}", e);
                let _ = event_tx.send(TransportEvent::Error(McpError::IoError)).await;
                return;
            }
        };

        // Accept one connection
        match listener.accept().await {
            Ok((stream, _addr)) => {
                Self::handle_connection(stream, cmd_rx, event_tx).await;
            }
            Err(e) => {
                tracing::error!("Failed to accept connection: {:?}", e);
                let _ = event_tx.send(TransportEvent::Error(McpError::IoError)).await;
            }
        }

        // Cleanup socket file
        let _ = std::fs::remove_file(path);
    }

    async fn run_client(
        path: PathBuf,
        cmd_rx: mpsc::Receiver<TransportCommand>,
        event_tx: mpsc::Sender<TransportEvent>,
    ) {
        // Connect to the Unix socket
        match UnixStream::connect(&path).await {
            Ok(stream) => {
                Self::handle_connection(stream, cmd_rx, event_tx).await;
            }
            Err(e) => {
                tracing::error!("Failed to connect to Unix socket: {:?}", e);
                let _ = event_tx.send(TransportEvent::Error(McpError::IoError)).await;
            }
        }
    }
}

#[async_trait]
impl Transport for UnixTransport {
    async fn start(&mut self) -> Result<TransportChannels, McpError> {
        let (cmd_tx, cmd_rx) = mpsc::channel(self.buffer_size);
        let (event_tx, event_rx) = mpsc::channel(self.buffer_size);

        if self.server_mode {
            tokio::spawn(Self::run_server(
                self.path.clone(),
                cmd_rx,
                event_tx,
            ));
        } else {
            tokio::spawn(Self::run_client(
                self.path.clone(),
                cmd_rx,
                event_tx,
            ));
        }

        let event_rx = Arc::new(tokio::sync::Mutex::new(event_rx));
        Ok(TransportChannels { cmd_tx, event_rx })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::JsonRpcNotification;
    use std::time::Duration;
    use tokio::time::sleep;

    #[tokio::test]
    async fn test_unix_transport() -> Result<(), McpError> {
        tokio::time::timeout(Duration::from_secs(5), async {
            let socket_path = PathBuf::from("/tmp/test_mcp_socket");
            
            // Create server and client transports
            let mut server = UnixTransport::new_server(socket_path.clone(), Some(4092));
            let mut client = UnixTransport::new_client(socket_path.clone(), Some(4092));

            // Start server and client
            let server_channels = server.start().await?;
            sleep(Duration::from_millis(100)).await;
            let client_channels = client.start().await?;

            // Send test messages
            let test_msg = JsonRpcMessage::Notification(JsonRpcNotification {
                jsonrpc: "2.0".to_string(),
                method: "test".to_string(),
                params: None,
            });

            client_channels.cmd_tx.send(TransportCommand::SendMessage(test_msg.clone())).await.unwrap();
            server_channels.cmd_tx.send(TransportCommand::SendMessage(test_msg)).await.unwrap();

            // Send close commands immediately
            client_channels.cmd_tx.send(TransportCommand::Close).await.unwrap();
            server_channels.cmd_tx.send(TransportCommand::Close).await.unwrap();

            // Wait for cleanup
            sleep(Duration::from_millis(100)).await;

            // Verify socket file is cleaned up
            assert!(!socket_path.exists());

            Ok(())
        })
        .await
        .map_err(|_| McpError::ShutdownTimeout)?
    }
} 