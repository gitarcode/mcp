use async_trait::async_trait;
use std::sync::Arc;
use tokio::sync::mpsc;

use crate::{
    error::McpError,
    protocol::types::*, // Import all JSON-RPC types from protocol
};

// Message types for the transport actor
#[derive(Debug)]
pub enum TransportCommand {
    SendMessage(JsonRpcMessage),
    Close,
}

#[derive(Debug)]
pub enum TransportEvent {
    Message(JsonRpcMessage),
    Error(McpError),
    Closed,
}

// Transport trait
#[async_trait]
pub trait Transport: Send + Sync + 'static {
    /// Start the transport and return channels for communication
    async fn start(&mut self) -> Result<TransportChannels, McpError>;
}

// Channels for communicating with the transport
#[derive(Debug, Clone)]
pub struct TransportChannels {
    /// Send commands to the transport
    pub cmd_tx: mpsc::Sender<TransportCommand>,
    /// Receive events from the transport
    pub event_rx: Arc<tokio::sync::Mutex<mpsc::Receiver<TransportEvent>>>,
}

// Stdio Transport Implementation
pub mod stdio;

// SSE Transport Implementation
pub mod sse;

// WebSocket Transport Implementation
pub mod ws;

// Unix Domain Socket Transport Implementation
#[cfg(unix)]
pub mod unix;
