use std::sync::Arc;
use tokio::{
    io::{self, AsyncRead, AsyncWrite, AsyncBufReadExt, AsyncWriteExt},
    sync::mpsc,
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

pub struct StdioTransport {
  buffer_size: usize,
}

impl StdioTransport {
  pub fn new(buffer_size: Option<usize>) -> Self {
      Self { buffer_size: buffer_size.unwrap_or(4092) }
  }

  async fn run(
      reader: tokio::io::BufReader<tokio::io::Stdin>,
      writer: tokio::io::Stdout,
      mut cmd_rx: mpsc::Receiver<TransportCommand>,
      event_tx: mpsc::Sender<TransportEvent>,
  ) {
      let (write_tx, mut write_rx) = mpsc::channel::<String>(32);

      // Writer task
      let writer_handle = {
          let mut writer = writer;
          tokio::spawn(async move {
              while let Some(msg) = write_rx.recv().await {
                  // Skip logging for certain types of messages
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
          })
      };

      // Reader task
      let reader_handle = tokio::spawn({
          let mut reader = reader;
          let event_tx = event_tx.clone();
          async move {
              let mut line = String::new();
              loop {
                  line.clear();
                  match reader.read_line(&mut line).await {
                      Ok(0) => break, // EOF
                      Ok(_) => {
                          let trimmed = line.trim();
                          if (!trimmed.contains("notifications/message") && !trimmed.contains("list_changed")) {
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
}

#[async_trait]
impl Transport for StdioTransport {
  async fn start(&mut self) -> Result<TransportChannels, McpError> {
      let (cmd_tx, cmd_rx) = mpsc::channel(self.buffer_size);
      let (event_tx, event_rx) = mpsc::channel(self.buffer_size);

      // Set up buffered stdin/stdout
      let stdin = tokio::io::stdin();
      let stdout = tokio::io::stdout();
      let reader = tokio::io::BufReader::with_capacity(4096, stdin);

      // Spawn the transport actor
      tokio::spawn(Self::run(reader, stdout, cmd_rx, event_tx));

      let event_rx = Arc::new(tokio::sync::Mutex::new(event_rx));
      Ok(TransportChannels { cmd_tx, event_rx })
  }
}
