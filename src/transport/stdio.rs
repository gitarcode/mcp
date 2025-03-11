use async_trait::async_trait;
use std::process::Stdio;
use std::sync::Arc;
use tokio::process::Command;
use tokio::{
    io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader},
    sync::mpsc,
};

use super::{JsonRpcMessage, Transport, TransportChannels, TransportCommand, TransportEvent};
use crate::error::McpError;

pub struct StdioTransport {
    buffer_size: usize,
    reader: Option<Box<dyn AsyncRead + Unpin + Send + Sync>>,
    writer: Option<Box<dyn AsyncWrite + Unpin + Send + Sync>>,
}

impl StdioTransport {
    pub fn new(buffer_size: Option<usize>) -> Self {
        Self {
            buffer_size: buffer_size.unwrap_or(4092),
            reader: None,
            writer: None,
        }
    }

    /// Create a new StdioTransport with custom streams
    ///
    /// This allows connecting the transport to arbitrary async streams
    /// instead of the default stdin/stdout, which is useful for connecting
    /// to child processes or other I/O sources.
    pub fn with_streams(
        reader: BufReader<impl AsyncRead + Unpin + Send + Sync + 'static>,
        writer: impl AsyncWrite + Unpin + Send + Sync + 'static,
        buffer_size: Option<usize>,
    ) -> Self {
        Self {
            reader: Some(Box::new(reader)),
            writer: Some(Box::new(writer)),
            buffer_size: buffer_size.unwrap_or(4092),
        }
    }

    /// Creates a new StdioTransport connected to a child process
    ///
    /// This spawns a new process with the provided command and arguments,
    /// and connects the transport to its stdin and stdout.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use tokio::runtime::Runtime;
    /// # use crate::transport::stdio::StdioTransport;
    ///
    /// # fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// # let rt = Runtime::new()?;
    /// # rt.block_on(async {
    /// let mut transport = StdioTransport::from_process("program", &["arg1", "arg2"]).await?;
    /// let channels = transport.start().await?;
    /// # Ok(())
    /// # })
    /// # }
    /// ```
    pub async fn from_process(
        program: &str,
        args: &[&str],
        buffer_size: Option<usize>,
    ) -> Result<Self, McpError> {
        let mut child = Command::new(program)
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .map_err(|_| McpError::IoError)?;

        let stdin = child.stdin.take().ok_or(McpError::IoError)?;
        let stdout = child.stdout.take().ok_or(McpError::IoError)?;

        // Spawn a task to manage the child process
        tokio::spawn(async move {
            let status = child.wait().await;
            if let Err(e) = status {
                tracing::error!("Child process error: {:?}", e);
            }
        });

        let reader = BufReader::new(stdout);
        Ok(Self::with_streams(reader, stdin, buffer_size))
    }

    /// Creates a new StdioTransport from raw stdin and stdout handles
    ///
    /// This is useful when you already have handles to stdin and stdout,
    /// such as when you've spawned a process using the standard library.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use std::process::{Command, Stdio};
    /// # use tokio::runtime::Runtime;
    /// # use crate::transport::stdio::StdioTransport;
    ///
    /// # fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// # let rt = Runtime::new()?;
    /// # rt.block_on(async {
    /// let mut child = Command::new("program")
    ///     .args(&["arg1", "arg2"])
    ///     .stdin(Stdio::piped())
    ///     .stdout(Stdio::piped())
    ///     .spawn()?;
    ///
    /// let stdin = child.stdin.take().unwrap();
    /// let stdout = child.stdout.take().unwrap();
    ///
    /// let mut transport = StdioTransport::from_raw_pipes(stdin, stdout, None).await?;
    /// let channels = transport.start().await?;
    /// # Ok(())
    /// # })
    /// # }
    /// ```
    pub async fn from_raw_pipes<R, W>(
        stdin: W,
        stdout: R,
        buffer_size: Option<usize>,
    ) -> Result<Self, McpError>
    where
        R: AsyncRead + Unpin + Send + Sync + 'static,
        W: AsyncWrite + Unpin + Send + Sync + 'static,
    {
        let reader = BufReader::new(stdout);
        Ok(Self::with_streams(reader, stdin, buffer_size))
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
                    }
                    .await
                    {
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
                            if !trimmed.contains("notifications/message")
                                && !trimmed.contains("list_changed")
                            {
                                tracing::debug!("<- {}", trimmed);
                            }

                            if !trimmed.is_empty() {
                                match serde_json::from_str::<JsonRpcMessage>(trimmed) {
                                    Ok(msg) => {
                                        if event_tx
                                            .send(TransportEvent::Message(msg))
                                            .await
                                            .is_err()
                                        {
                                            break;
                                        }
                                    }
                                    Err(e) => {
                                        tracing::error!("Parse error: {}, input: {}", e, trimmed);
                                        if event_tx
                                            .send(TransportEvent::Error(McpError::ParseError))
                                            .await
                                            .is_err()
                                        {
                                            break;
                                        }
                                    }
                                }
                            }
                        }
                        Err(e) => {
                            tracing::error!("Read error: {:?}", e);
                            let _ = event_tx
                                .send(TransportEvent::Error(McpError::IoError))
                                .await;
                            break;
                        }
                    }
                }
            }
        });

        // Main message loop
        while let Some(cmd) = cmd_rx.recv().await {
            match cmd {
                TransportCommand::SendMessage(msg) => match serde_json::to_string(&msg) {
                    Ok(s) => {
                        if write_tx.send(s).await.is_err() {
                            break;
                        }
                    }
                    Err(e) => tracing::error!("Failed to serialize message: {:?}", e),
                },
                TransportCommand::Close => break,
            }
        }

        // Cleanup
        drop(write_tx);
        let _ = reader_handle.await;
        let _ = writer_handle.await;
        let _ = event_tx.send(TransportEvent::Closed).await;
    }

    async fn run_with_custom_streams(
        reader: Box<dyn AsyncRead + Unpin + Send + Sync>,
        mut writer: Box<dyn AsyncWrite + Unpin + Send + Sync>,
        mut cmd_rx: mpsc::Receiver<TransportCommand>,
        event_tx: mpsc::Sender<TransportEvent>,
    ) {
        let (write_tx, mut write_rx) = mpsc::channel::<String>(32);

        // Writer task
        let writer_handle = tokio::spawn(async move {
            while let Some(msg) = write_rx.recv().await {
                if !msg.contains("notifications/message") && !msg.contains("list_changed") {
                    tracing::debug!("-> {}", msg);
                }

                if (writer.write_all(msg.as_bytes()).await).is_err() {
                    break;
                }
                if (writer.write_all(b"\n").await).is_err() {
                    break;
                }
                if (writer.flush().await).is_err() {
                    break;
                }
            }
        });

        // Reader task
        let reader_handle = tokio::spawn({
            let event_tx = event_tx.clone();
            async move {
                let mut reader = BufReader::new(reader);
                let mut line = String::new();
                while let Ok(n) = reader.read_line(&mut line).await {
                    if n == 0 {
                        break;
                    }
                    let trimmed = line.trim();
                    if !trimmed.contains("notifications/message")
                        && !trimmed.contains("list_changed")
                    {
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
                                if event_tx
                                    .send(TransportEvent::Error(McpError::ParseError))
                                    .await
                                    .is_err()
                                {
                                    break;
                                }
                            }
                        }
                    }
                    line.clear();
                }
            }
        });

        // Main message loop
        while let Some(cmd) = cmd_rx.recv().await {
            match cmd {
                TransportCommand::SendMessage(msg) => {
                    if let Ok(s) = serde_json::to_string(&msg) {
                        if write_tx.send(s).await.is_err() {
                            break;
                        }
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

        if let (Some(reader), Some(writer)) = (self.reader.take(), self.writer.take()) {
            // Use custom streams
            tokio::spawn(Self::run_with_custom_streams(
                reader, writer, cmd_rx, event_tx,
            ));
        } else {
            // Set up buffered stdin/stdout
            let stdin = tokio::io::stdin();
            let stdout = tokio::io::stdout();
            let reader = tokio::io::BufReader::with_capacity(4096, stdin);
            tokio::spawn(Self::run(reader, stdout, cmd_rx, event_tx));
        }

        let event_rx = Arc::new(tokio::sync::Mutex::new(event_rx));
        Ok(TransportChannels { cmd_tx, event_rx })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transport::TransportCommand;
    use std::time::Duration;
    use tokio::io::{self};

    #[tokio::test]
    async fn test_stdio_transport() -> Result<(), McpError> {
        let (input_rx, input_tx) = io::duplex(64);
        let (output_rx, output_tx) = io::duplex(64);

        let mut transport = StdioTransport::with_streams(BufReader::new(input_rx), output_tx, None);

        let TransportChannels { cmd_tx, event_rx } = transport.start().await?;

        // Send close command immediately
        cmd_tx.send(TransportCommand::Close).await.unwrap();

        // Wait for cleanup
        tokio::time::sleep(Duration::from_millis(100)).await;

        // Drop all channels to ensure cleanup
        drop(cmd_tx);
        drop(event_rx);
        drop(input_tx);
        drop(output_rx);

        Ok(())
    }
}
