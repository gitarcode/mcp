use std::{
    net::IpAddr,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
};
use tokio::sync::mpsc;
use warp::{ws::Message as WarpMessage, Filter};

use super::{JsonRpcMessage, Transport, TransportChannels, TransportCommand, TransportEvent};
use crate::error::McpError;

use async_trait::async_trait;
use futures::{SinkExt, StreamExt};
use tokio_tungstenite::{connect_async, tungstenite::Message as TungsteniteMessage};
use tracing;

pub struct WebSocketTransport {
    host: String,
    port: u16,
    client_mode: bool,
    buffer_size: usize,
}

impl WebSocketTransport {
    pub fn new_server(host: String, port: u16, buffer_size: usize) -> Self {
        Self {
            host,
            port,
            client_mode: false,
            buffer_size,
        }
    }

    pub fn new_client(host: String, port: u16, buffer_size: usize) -> Self {
        Self {
            host,
            port,
            client_mode: true,
            buffer_size,
        }
    }

    async fn run_server(
        host: String,
        port: u16,
        mut cmd_rx: mpsc::Receiver<TransportCommand>,
        event_tx: mpsc::Sender<TransportEvent>,
    ) {
        // Client counter for unique IDs
        let client_counter = Arc::new(AtomicU64::new(0));

        // Create a channel for broadcasting messages to all connected clients
        let (broadcast_tx, _) = tokio::sync::broadcast::channel::<JsonRpcMessage>(100);
        let broadcast_tx = Arc::new(broadcast_tx);
        let broadcast_tx_clone = Arc::clone(&broadcast_tx);

        // WebSocket connection handler
        let ws_route = warp::path("ws")
            .and(warp::ws())
            .map(move |ws: warp::ws::Ws| {
                let client_id = client_counter.fetch_add(1, Ordering::SeqCst);
                let event_tx = event_tx.clone();
                let broadcast_tx = Arc::clone(&broadcast_tx);

                tracing::debug!("New WebSocket connection: client_id={}", client_id);

                ws.on_upgrade(move |websocket| async move {
                    let (mut ws_tx, mut ws_rx) = websocket.split();
                    let mut broadcast_rx = broadcast_tx.subscribe();

                    // Task to forward messages from broadcast to this client
                    let forward_task = tokio::spawn(async move {
                        while let Ok(msg) = broadcast_rx.recv().await {
                            let json = match serde_json::to_string(&msg) {
                                Ok(json) => json,
                                Err(e) => {
                                    tracing::error!("Failed to serialize message: {:?}", e);
                                    continue;
                                }
                            };

                            if let Err(e) = ws_tx.send(WarpMessage::text(json)).await {
                                tracing::error!("Failed to send WebSocket message: {:?}", e);
                                break;
                            }
                        }
                    });

                    // Process incoming messages from this client
                    while let Some(result) = ws_rx.next().await {
                        match result {
                            Ok(msg) => {
                                if msg.is_close() {
                                    tracing::debug!(
                                        "WebSocket connection closed: client_id={}",
                                        client_id
                                    );
                                    break;
                                }

                                if msg.is_text() {
                                    let text = msg.to_str().unwrap_or_default();
                                    match serde_json::from_str::<JsonRpcMessage>(text) {
                                        Ok(json_rpc_msg) => {
                                            if let Err(e) = event_tx
                                                .send(TransportEvent::Message(json_rpc_msg))
                                                .await
                                            {
                                                tracing::error!(
                                                    "Failed to forward message: {:?}",
                                                    e
                                                );
                                                break;
                                            }
                                        }
                                        Err(e) => {
                                            tracing::error!(
                                                "Failed to parse WebSocket message: {:?}",
                                                e
                                            );
                                        }
                                    }
                                }
                            }
                            Err(e) => {
                                tracing::error!("WebSocket error: {:?}", e);
                                break;
                            }
                        }
                    }

                    // Cancel the forward task when the connection is closed
                    forward_task.abort();
                    tracing::debug!(
                        "WebSocket connection handler completed: client_id={}",
                        client_id
                    );
                })
            });

        // Message forwarding task
        tokio::spawn(async move {
            while let Some(cmd) = cmd_rx.recv().await {
                match cmd {
                    TransportCommand::SendMessage(msg) => {
                        // Skip broadcasting debug log messages about WebSocket and internal operations
                        let should_skip = match &msg {
                            JsonRpcMessage::Notification(n)
                                if n.method == "notifications/message" =>
                            {
                                if let Some(params) = &n.params {
                                    // Check the log message and logger
                                    let is_debug = params.get("level").and_then(|l| l.as_str())
                                        == Some("debug");

                                    let logger =
                                        params.get("logger").and_then(|l| l.as_str()).unwrap_or("");

                                    let message = params
                                        .get("data")
                                        .and_then(|d| d.get("message"))
                                        .and_then(|m| m.as_str())
                                        .unwrap_or("");

                                    is_debug
                                        && (logger.starts_with("hyper::")
                                            || logger.starts_with("mcp_rs::transport")
                                            || message.contains("Broadcasting WebSocket message")
                                            || message.contains("Failed to broadcast message"))
                                } else {
                                    false
                                }
                            }
                            _ => false,
                        };

                        if !should_skip {
                            tracing::debug!("Broadcasting WebSocket message: {:?}", msg);
                            if let Err(e) = broadcast_tx_clone.send(msg) {
                                tracing::error!("Failed to broadcast message: {:?}", e);
                            }
                        }
                    }
                    TransportCommand::Close => break,
                }
            }
        });

        // Start the server
        let host_addr = match host.parse::<IpAddr>() {
            Ok(addr) => addr,
            Err(e) => {
                tracing::error!("Failed to parse host address: {:?}", e);
                return;
            }
        };

        tracing::info!("Starting WebSocket server at ws://{}:{}/ws", host, port);
        warp::serve(ws_route).run((host_addr, port)).await;
    }

    async fn run_client(
        host: String,
        port: u16,
        mut cmd_rx: mpsc::Receiver<TransportCommand>,
        event_tx: mpsc::Sender<TransportEvent>,
    ) {
        let ws_url = format!("ws://{}:{}/ws", host, port);
        tracing::debug!("Connecting to WebSocket endpoint: {}", ws_url);

        // Connect to the WebSocket server
        let ws_stream = match connect_async(&ws_url).await {
            Ok((stream, response)) => {
                tracing::debug!("Connected to WebSocket server: {:?}", response);
                stream
            }
            Err(e) => {
                tracing::error!("Failed to connect to WebSocket server: {:?}", e);
                let _ = event_tx
                    .send(TransportEvent::Error(McpError::ConnectionClosed))
                    .await;
                return;
            }
        };

        let (mut write, mut read) = ws_stream.split();

        // Task to receive messages from the WebSocket server
        let event_tx_clone = event_tx.clone();
        tokio::spawn(async move {
            while let Some(msg) = read.next().await {
                match msg {
                    Ok(msg) => {
                        if msg.is_text() {
                            let text = msg.to_text().unwrap_or_default();
                            match serde_json::from_str::<JsonRpcMessage>(text) {
                                Ok(json_rpc_msg) => {
                                    if let Err(e) = event_tx_clone
                                        .send(TransportEvent::Message(json_rpc_msg))
                                        .await
                                    {
                                        tracing::error!(
                                            "Failed to forward WebSocket message: {:?}",
                                            e
                                        );
                                        break;
                                    }
                                }
                                Err(e) => {
                                    tracing::error!("Failed to parse WebSocket message: {:?}", e);
                                }
                            }
                        }
                    }
                    Err(e) => {
                        tracing::error!("WebSocket error: {:?}", e);
                        let _ = event_tx_clone
                            .send(TransportEvent::Error(McpError::ConnectionClosed))
                            .await;
                        break;
                    }
                }
            }
        });

        // Process commands and send messages to the WebSocket server
        while let Some(cmd) = cmd_rx.recv().await {
            match cmd {
                TransportCommand::SendMessage(msg) => {
                    tracing::debug!("Sending WebSocket message: {:?}", msg);
                    match serde_json::to_string(&msg) {
                        Ok(json) => {
                            if let Err(e) = write.send(TungsteniteMessage::Text(json.into())).await
                            {
                                tracing::error!("Failed to send WebSocket message: {:?}", e);
                                break;
                            }
                        }
                        Err(e) => {
                            tracing::error!("Failed to serialize message: {:?}", e);
                        }
                    }
                }
                TransportCommand::Close => break,
            }
        }

        let _ = event_tx.send(TransportEvent::Closed).await;
    }
}

#[async_trait]
impl Transport for WebSocketTransport {
    async fn start(&mut self) -> Result<TransportChannels, McpError> {
        let (cmd_tx, cmd_rx) = mpsc::channel(self.buffer_size);
        let (event_tx, event_rx) = mpsc::channel(self.buffer_size);

        if self.client_mode {
            tokio::spawn(Self::run_client(
                self.host.clone(),
                self.port,
                cmd_rx,
                event_tx,
            ));
        } else {
            tokio::spawn(Self::run_server(
                self.host.clone(),
                self.port,
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
    use crate::protocol::{JsonRpcRequest, JsonRpcResponse};
    use futures::{SinkExt, StreamExt};
    use std::sync::atomic::{AtomicU16, Ordering as AtomicOrdering};
    use std::time::Duration;
    use tokio::time::sleep;
    use tokio_tungstenite::{connect_async, tungstenite::Message};

    // Static counter for generating unique port numbers
    static PORT_COUNTER: AtomicU16 = AtomicU16::new(3001);

    #[tokio::test]
    async fn test_websocket_transport_server() -> Result<(), McpError> {
        // Initialize tracing for tests
        let _ = tracing_subscriber::fmt()
            .with_env_filter("debug")
            .try_init();

        // Start a WebSocket server on a unique port
        let host = "127.0.0.1".to_string();
        let port = PORT_COUNTER.fetch_add(1, AtomicOrdering::SeqCst); // Unique port to avoid conflicts
        let mut transport = WebSocketTransport::new_server(host.clone(), port, 32);

        // Start the transport
        let TransportChannels { cmd_tx, event_rx } = transport.start().await?;

        // Give the server time to start
        sleep(Duration::from_millis(100)).await;

        // Connect a client to the server
        let ws_url = format!("ws://{}:{}/ws", host, port);
        let (ws_stream, _) = connect_async(&ws_url).await.expect("Failed to connect");
        let (mut write, mut read) = ws_stream.split();

        // Create a test message
        let request_id = 12345;
        let test_request = JsonRpcMessage::Request(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: request_id,
            method: "test.method".to_string(),
            params: None,
        });

        // Send the message from client to server
        let json_str = serde_json::to_string(&test_request).unwrap();
        write.send(Message::Text(json_str.into())).await.unwrap();

        // Wait for the server to process the message
        sleep(Duration::from_millis(100)).await;

        // Check if the server received the message
        let mut event_rx = event_rx.lock().await;
        if let Some(TransportEvent::Message(received)) = event_rx.recv().await {
            match received {
                JsonRpcMessage::Request(req) => {
                    assert_eq!(req.id, request_id);
                    assert_eq!(req.method, "test.method");
                }
                _ => panic!("Expected Request message"),
            }
        } else {
            panic!("No message received");
        }

        // Send a message from server to client
        let response = JsonRpcMessage::Response(JsonRpcResponse {
            jsonrpc: "2.0".to_string(),
            id: request_id,
            result: Some(serde_json::json!({"status": "success"})),
            error: None,
        });

        cmd_tx
            .send(TransportCommand::SendMessage(response.clone()))
            .await
            .unwrap();

        // Wait for the client to receive the message
        sleep(Duration::from_millis(100)).await;

        // Check if the client received the message
        if let Some(Ok(msg)) = read.next().await {
            if let Message::Text(text) = msg {
                let received: JsonRpcMessage = serde_json::from_str(&text).unwrap();
                match received {
                    JsonRpcMessage::Response(resp) => {
                        assert_eq!(resp.result, Some(serde_json::json!({"status": "success"})));
                    }
                    _ => panic!("Expected Response message"),
                }
            } else {
                panic!("Expected Text message");
            }
        } else {
            panic!("No message received by client");
        }

        // Close the transport
        cmd_tx.send(TransportCommand::Close).await.unwrap();

        // Wait for cleanup
        sleep(Duration::from_millis(100)).await;

        Ok(())
    }

    #[tokio::test]
    async fn test_websocket_transport_client() -> Result<(), McpError> {
        // Initialize tracing for tests
        let _ = tracing_subscriber::fmt()
            .with_env_filter("debug")
            .try_init();

        // Start a WebSocket server using warp for the client to connect to
        let host = "127.0.0.1".to_string();
        let port = PORT_COUNTER.fetch_add(1, AtomicOrdering::SeqCst); // Unique port to avoid conflicts

        // Create a channel to receive messages from the test server
        let (server_tx, mut server_rx) = mpsc::channel::<JsonRpcMessage>(32);

        // Create a simple WebSocket echo server
        let ws_route = warp::path("ws")
            .and(warp::ws())
            .map(move |ws: warp::ws::Ws| {
                let server_tx = server_tx.clone();
                ws.on_upgrade(move |websocket| async move {
                    let (mut tx, mut rx) = websocket.split();

                    // Forward incoming messages to the server_rx channel
                    while let Some(Ok(msg)) = rx.next().await {
                        if msg.is_text() {
                            if let Ok(text) = msg.to_str() {
                                if let Ok(json_msg) = serde_json::from_str::<JsonRpcMessage>(text) {
                                    // Echo the message back
                                    let echo_text = serde_json::to_string(&json_msg).unwrap();
                                    let _ = tx.send(WarpMessage::text(echo_text)).await;

                                    // Also send to our test channel
                                    let _ = server_tx.send(json_msg).await;
                                }
                            }
                        }
                    }
                })
            });

        // Start the test server
        let server_handle =
            tokio::spawn(warp::serve(ws_route).run((host.parse::<IpAddr>().unwrap(), port)));

        // Give the server time to start
        sleep(Duration::from_millis(100)).await;

        // Create and start the WebSocket client transport
        let mut transport = WebSocketTransport::new_client(host.clone(), port, 32);
        let TransportChannels { cmd_tx, event_rx } = transport.start().await?;

        // Give the client time to connect
        sleep(Duration::from_millis(100)).await;

        // Create a test message
        let request_id = 54321;
        let test_request = JsonRpcMessage::Request(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: request_id,
            method: "test.client.method".to_string(),
            params: None,
        });

        // Send the message from client to server
        cmd_tx
            .send(TransportCommand::SendMessage(test_request.clone()))
            .await
            .unwrap();

        // Wait for the server to process the message
        sleep(Duration::from_millis(100)).await;

        // Check if the server received the message
        if let Some(received) = server_rx.recv().await {
            match received {
                JsonRpcMessage::Request(req) => {
                    assert_eq!(req.id, request_id);
                    assert_eq!(req.method, "test.client.method");
                }
                _ => panic!("Expected Request message"),
            }
        } else {
            panic!("No message received by server");
        }

        // Check if the client received the echo response
        let mut event_rx = event_rx.lock().await;
        if let Some(TransportEvent::Message(received)) = event_rx.recv().await {
            match received {
                JsonRpcMessage::Request(req) => {
                    assert_eq!(req.id, request_id);
                    assert_eq!(req.method, "test.client.method");
                }
                _ => panic!("Expected Request message echo"),
            }
        } else {
            panic!("No echo message received by client");
        }

        // Close the transport
        cmd_tx.send(TransportCommand::Close).await.unwrap();

        // Wait for cleanup
        sleep(Duration::from_millis(100)).await;

        // Abort the server
        server_handle.abort();

        Ok(())
    }
}
