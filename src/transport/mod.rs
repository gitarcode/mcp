use async_trait::async_trait;
use futures::StreamExt;
use jsonrpc_core::request;
use reqwest::RequestBuilder;
use reqwest_eventsource::{Event, EventSource};
use serde::{Deserialize, Serialize};
use std::{
    net::IpAddr,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    time::Duration,
};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt},
    sync::{broadcast, mpsc},
};
use warp::Filter;

use crate::{
    error::McpError,
    protocol::{JsonRpcNotification, JsonRpcRequest, JsonRpcResponse},
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

// JSON-RPC Message types
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum JsonRpcMessage {
    Request(JsonRpcRequest),
    Response(JsonRpcResponse),
    Notification(JsonRpcNotification),
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

#[cfg(test)]
mod tests {
    use crate::{
        error::McpError,
        protocol::JsonRpcNotification,
        transport::{
            JsonRpcMessage, 
            Transport, 
            TransportChannels, 
            TransportCommand,
            TransportEvent,
            stdio::StdioTransport,
        },
    };

    #[tokio::test]
    async fn test_transport() -> Result<(), McpError> {
        // Create and start transport
        let mut transport = StdioTransport::new(None);
        let TransportChannels { cmd_tx, event_rx } = transport.start().await?;

        // Handle events
        tokio::spawn(async move {
            let event_rx = event_rx.clone();

            loop {
                let event = {
                    let mut guard = event_rx.lock().await;
                    guard.recv().await
                };

                match event {
                    Some(TransportEvent::Message(msg)) => println!("Received: {:?}", msg),
                    Some(TransportEvent::Error(err)) => println!("Error: {:?}", err),
                    Some(TransportEvent::Closed) => break,
                    None => break,
                }
            }
        });

        // Send a message
        cmd_tx
            .send(TransportCommand::SendMessage(JsonRpcMessage::Notification(
                JsonRpcNotification {
                    jsonrpc: "2.0".to_string(),
                    method: "test".to_string(),
                    params: None,
                },
            )))
            .await
            .unwrap();

        Ok(())
    }
}
