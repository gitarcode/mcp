use axum::{
    extract::ws::{Message as AxumMessage, WebSocket, WebSocketUpgrade},
    response::IntoResponse,
    routing::get,
    Router,
};
use std::{
    net::{IpAddr, SocketAddr},
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
};
use tokio::sync::mpsc;
use tower_http::cors::CorsLayer;

use super::{JsonRpcMessage, Transport, TransportChannels, TransportCommand, TransportEvent};
use crate::error::McpError;

use async_trait::async_trait;
use futures::{SinkExt, StreamExt};
use hyper;
use tokio_tungstenite::{
    connect_async,
    tungstenite::{
        protocol::{frame::coding::CloseCode, CloseFrame},
        Message as TungsteniteMessage,
    },
};
use tracing;
use base64;
use base64::Engine;
use rand::random;

/// Generate a valid Sec-WebSocket-Key header value for the WebSocket handshake
fn generate_sec_websocket_key() -> String {
    // Generate 16 random bytes
    let mut key = [0u8; 16];
    for i in 0..16 {
        key[i] = random();
    }
    // Encode with base64
    base64::engine::general_purpose::STANDARD.encode(key)
}

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
        // Create a counter for client IDs
        let client_counter = Arc::new(AtomicU64::new(0));

        // Create a broadcast channel for sending messages to all clients
        let (broadcast_tx, _) = tokio::sync::broadcast::channel(1024);
        let broadcast_tx = Arc::new(broadcast_tx);

        // Spawn a task to handle incoming commands
        let broadcast_tx_clone = broadcast_tx.clone();
        let message_task = tokio::spawn(async move {
            while let Some(cmd) = cmd_rx.recv().await {
                match cmd {
                    TransportCommand::SendMessage(msg) => {
                        // Skip debug logging for certain messages to reduce noise
                        let should_skip = match &msg {
                            JsonRpcMessage::Notification(n) => {
                                if n.method == "$/log" {
                                    // Create an empty map to use as default
                                    let empty_map = serde_json::Map::new();
                                    
                                    // Get the params object or use the empty map
                                    let params = match &n.params {
                                        Some(value) => value.as_object().unwrap_or(&empty_map),
                                        None => &empty_map,
                                    };
                                    
                                    let is_debug = params
                                        .get("level")
                                        .and_then(|l| l.as_str())
                                        .map(|l| l == "debug")
                                        .unwrap_or(false);

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
                            let receiver_count = match broadcast_tx_clone.send(msg) {
                                Ok(count) => count,
                                Err(e) => {
                                    tracing::error!("Failed to broadcast message: {:?}", e);
                                    0
                                }
                            };

                            if receiver_count == 0 {
                                tracing::debug!(
                                    "No WebSocket clients connected to receive message"
                                );
                            }
                        }
                    }
                    TransportCommand::Close => {
                        tracing::info!("Closing WebSocket server message task");
                        break;
                    }
                }
            }
        });

        // Create the Axum app with WebSocket route
        let app = Router::new()
            .route("/ws", get(ws_handler))
            .layer(CorsLayer::permissive())
            .with_state((client_counter, broadcast_tx, event_tx));

        // Parse the host address
        let host_addr: IpAddr = host
            .parse()
            .unwrap_or_else(|_| "127.0.0.1".parse().unwrap());
        let socket_addr = SocketAddr::from((host_addr, port));

        tracing::info!("Starting WebSocket server at ws://{}:{}/ws", host, port);

        // Start the Axum server and keep it running
        let server_task = tokio::spawn(async move {
            let listener = tokio::net::TcpListener::bind(socket_addr).await.unwrap();
            axum::serve(listener, app).await.unwrap();
        });
        
        // Wait for the server task to complete (which should only happen on shutdown)
        if let Err(e) = server_task.await {
            tracing::error!("WebSocket server task error: {:?}", e);
        }

        // If we got here, the server has been shut down
        message_task.abort();
        tracing::info!("WebSocket server shut down");
    }

    async fn run_client(
        host: String,
        port: u16,
        mut cmd_rx: mpsc::Receiver<TransportCommand>,
        event_tx: mpsc::Sender<TransportEvent>,
    ) {
        let ws_url = format!("ws://{}:{}/ws", host, port);
        tracing::debug!("Connecting to WebSocket endpoint: {}", ws_url);

        // Create a modified WebSocket request with the "mcp" subprotocol
        let request = hyper::Request::builder()
            .uri(&ws_url)
            .header("Sec-WebSocket-Protocol", "mcp")
            .header("Sec-WebSocket-Key", generate_sec_websocket_key())
            .header("Sec-WebSocket-Version", "13")
            .header("Connection", "Upgrade")
            .header("Upgrade", "websocket")
            .body(())
            .unwrap();

        // Connect to the WebSocket server
        let ws_stream = match connect_async(request).await {
            Ok((stream, response)) => {
                tracing::debug!("Connected to WebSocket server: {:?}", response);
                
                // Log the negotiated subprotocol
                if let Some(protocol) = response.headers().get("Sec-WebSocket-Protocol") {
                    if let Ok(protocol_str) = protocol.to_str() {
                        tracing::debug!("Negotiated subprotocol: {}", protocol_str);
                        if protocol_str != "mcp" {
                            tracing::warn!("Server negotiated a different subprotocol than 'mcp': {}", protocol_str);
                        }
                    }
                } else {
                    tracing::warn!("Server did not negotiate any subprotocol");
                }
                
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

        let (ws_write, ws_read) = ws_stream.split();
        let ws_write = Arc::new(tokio::sync::Mutex::new(ws_write));
        let ws_write_for_recv = Arc::clone(&ws_write);
        let event_tx_for_recv = event_tx.clone();

        // Task for sending messages from client to server
        let mut send_task = tokio::spawn(async move {
            while let Some(cmd) = cmd_rx.recv().await {
                match cmd {
                    TransportCommand::SendMessage(msg) => {
                        let json_str = match serde_json::to_string(&msg) {
                            Ok(s) => s,
                            Err(e) => {
                                tracing::error!("Failed to serialize message: {:?}", e);
                                continue;
                            }
                        };

                        let mut ws_write = ws_write.lock().await;
                        if let Err(e) = ws_write
                            .send(TungsteniteMessage::Text(json_str.into()))
                            .await
                        {
                            tracing::error!("Failed to send message: {:?}", e);
                            break;
                        }
                    }
                    TransportCommand::Close => {
                        tracing::debug!("Closing WebSocket client connection");
                        let mut ws_write = ws_write.lock().await;
                        let _ = ws_write
                            .send(TungsteniteMessage::Close(Some(CloseFrame {
                                code: CloseCode::Normal,
                                reason: "Connection closed by client".into(),
                            })))
                            .await;
                        break;
                    }
                }
            }
        });

        // Task for receiving messages from server
        let mut recv_task = tokio::spawn(async move {
            let mut ws_read = ws_read;
            while let Some(msg_result) = ws_read.next().await {
                match msg_result {
                    Ok(msg) => match msg {
                        TungsteniteMessage::Text(text) => {
                            match serde_json::from_str::<JsonRpcMessage>(&text) {
                                Ok(json_rpc_msg) => {
                                    if let Err(e) = event_tx_for_recv
                                        .send(TransportEvent::Message(json_rpc_msg))
                                        .await
                                    {
                                        tracing::error!("Failed to forward message: {:?}", e);
                                        break;
                                    }
                                }
                                Err(e) => {
                                    tracing::error!("Failed to parse message: {:?}", e);
                                }
                            }
                        }
                        TungsteniteMessage::Binary(data) => {
                            tracing::debug!("Received binary message: {} bytes", data.len());
                        }
                        TungsteniteMessage::Ping(data) => {
                            // Automatically respond with pong
                            let mut ws_write = ws_write_for_recv.lock().await;
                            if let Err(e) = ws_write.send(TungsteniteMessage::Pong(data)).await {
                                tracing::error!("Failed to send pong: {:?}", e);
                                break;
                            }
                        }
                        TungsteniteMessage::Pong(_) => {
                            // Just acknowledge pong messages
                        }
                        TungsteniteMessage::Close(_) => {
                            tracing::debug!("Received close frame from server");
                            let _ = event_tx_for_recv.send(TransportEvent::Closed).await;
                            break;
                        }
                        TungsteniteMessage::Frame(_) => {
                            // Low-level frame, just ignore
                        }
                    },
                    Err(e) => {
                        tracing::error!("WebSocket error: {:?}", e);
                        let _ = event_tx_for_recv
                            .send(TransportEvent::Error(McpError::ConnectionClosed))
                            .await;
                        break;
                    }
                }
            }
        });

        // Wait for either task to complete
        tokio::select! {
            _ = &mut send_task => {
                tracing::debug!("Send task completed");
                recv_task.abort();
            }
            _ = &mut recv_task => {
                tracing::debug!("Receive task completed");
                send_task.abort();
            }
        }

        tracing::debug!("WebSocket client connection closed");
    }
}

/// Axum WebSocket handler function
async fn ws_handler(
    ws: WebSocketUpgrade,
    axum::extract::State((client_counter, broadcast_tx, event_tx)): axum::extract::State<(
        Arc<AtomicU64>,
        Arc<tokio::sync::broadcast::Sender<JsonRpcMessage>>,
        mpsc::Sender<TransportEvent>,
    )>,
) -> impl IntoResponse {
    // Accept the WebSocket connection with the "mcp" subprotocol
    ws.protocols(["mcp"]).on_upgrade(move |socket| async move {
        let client_id = client_counter.fetch_add(1, Ordering::SeqCst);
        tracing::debug!("WebSocket connection established: client_id={}", client_id);

        // Handle the socket
        handle_socket(socket, client_id, broadcast_tx, event_tx).await;
    })
}

/// Handle an individual WebSocket connection
async fn handle_socket(
    socket: WebSocket,
    client_id: u64,
    broadcast_tx: Arc<tokio::sync::broadcast::Sender<JsonRpcMessage>>,
    event_tx: mpsc::Sender<TransportEvent>,
) {
    // Log the protocol if one was negotiated
    if let Some(protocol) = socket.protocol() {
        tracing::debug!("WebSocket connection using protocol: {:?}", protocol);
    } else {
        tracing::debug!("WebSocket connection without protocol");
    }

    // Split the WebSocket
    let (sender, receiver) = socket.split();
    let sender = Arc::new(tokio::sync::Mutex::new(sender));

    // Subscribe to broadcast channel
    let mut broadcast_rx = broadcast_tx.subscribe();

    // Forward messages from broadcast to client
    let mut send_task = tokio::spawn(async move {
        while let Ok(msg) = broadcast_rx.recv().await {
            // Forward message from subscription to WebSocket client
            tokio::spawn({
                let sender = Arc::clone(&sender);
                let json = serde_json::to_string(&msg).unwrap();
                async move {
                    if let Err(e) = sender
                        .lock()
                        .await
                        .send(AxumMessage::Text(json.into()))
                        .await
                    {
                        tracing::error!("Failed to send message to WebSocket client: {:?}", e);
                    }
                }
            });
        }
    });

    // Clone event_tx for the receive task
    let event_tx_recv = event_tx.clone();

    // Task for receiving messages from server
    let mut recv_task = tokio::spawn(async move {
        let mut receiver = receiver;
        while let Some(msg) = receiver.next().await {
            match msg {
                Ok(msg) => {
                    match msg {
                        AxumMessage::Text(text) => {
                            match serde_json::from_str::<JsonRpcMessage>(&text) {
                                Ok(json_rpc_msg) => {
                                    if let Err(e) = event_tx_recv
                                        .send(TransportEvent::Message(json_rpc_msg))
                                        .await
                                    {
                                        tracing::error!("Failed to forward message: {:?}", e);
                                        break;
                                    }
                                }
                                Err(e) => {
                                    tracing::error!("Failed to parse WebSocket message: {:?}", e);
                                }
                            }
                        }
                        AxumMessage::Binary(_) => {
                            tracing::warn!("Received binary message, which is not supported");
                        }
                        AxumMessage::Close(_) => {
                            tracing::debug!(
                                "WebSocket connection closed by client: client_id={}",
                                client_id
                            );
                            break;
                        }
                        AxumMessage::Ping(_) => {
                            // Pings are handled automatically by Axum
                        }
                        AxumMessage::Pong(_) => {
                            // Just acknowledge pong messages
                        }
                    }
                }
                Err(e) => {
                    tracing::error!("WebSocket error from client {}: {:?}", client_id, e);
                    break;
                }
            }
        }
    });

    // Wait for either task to complete
    tokio::select! {
        _ = &mut send_task => {
            tracing::debug!("Send task completed");
            recv_task.abort();
        }
        _ = &mut recv_task => {
            tracing::debug!("Receive task completed");
            send_task.abort();
        }
    }

    tracing::debug!(
        "WebSocket connection handler completed: client_id={}",
        client_id
    );
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
    use std::sync::atomic::{AtomicU16, Ordering as AtomicOrdering};
    use std::time::Duration;
    use tokio::time::sleep;

    // Create a counter for unique port numbers in tests
    static PORT_COUNTER: AtomicU16 = AtomicU16::new(3000);

    #[tokio::test]
    async fn test_websocket_transport_basic() -> Result<(), McpError> {
        // Initialize tracing for tests
        let _ = tracing_subscriber::fmt()
            .with_env_filter("debug")
            .try_init();

        // This test just verifies that we can create and start a WebSocket transport
        // without actually connecting to it

        // Create a WebSocket transport
        let host = "127.0.0.1".to_string();
        let port = PORT_COUNTER.fetch_add(1, AtomicOrdering::SeqCst); // Unique port to avoid conflicts
        let mut transport = WebSocketTransport::new_server(host.clone(), port, 32);

        // Start the transport with a timeout
        let start_result = tokio::time::timeout(Duration::from_secs(2), transport.start()).await;

        // Verify that the transport started successfully
        assert!(
            start_result.is_ok(),
            "Transport should start within timeout"
        );
        let channels = start_result.unwrap()?;

        // Verify that we got valid channels
        assert!(
            channels.cmd_tx.capacity() > 0,
            "Command channel should have capacity"
        );

        // Close the transport
        let close_result = channels.cmd_tx.send(TransportCommand::Close).await;
        assert!(close_result.is_ok(), "Should be able to send close command");

        // Wait for cleanup
        sleep(Duration::from_millis(100)).await;

        Ok(())
    }

    #[tokio::test]
    async fn test_websocket_transport_client_basic() -> Result<(), McpError> {
        // Initialize tracing for tests
        let _ = tracing_subscriber::fmt()
            .with_env_filter("debug")
            .try_init();

        // This test just verifies that we can create a WebSocket client transport
        // without actually connecting to a server

        // Create a WebSocket client transport
        let host = "127.0.0.1".to_string();
        let port = PORT_COUNTER.fetch_add(1, AtomicOrdering::SeqCst); // Unique port to avoid conflicts
        let transport = WebSocketTransport::new_client(host.clone(), port, 32);

        // We don't actually start the transport since that would try to connect
        // to a non-existent server. Instead, we just verify that the transport
        // was created successfully.
        assert_eq!(transport.host, host, "Host should match");
        assert_eq!(transport.port, port, "Port should match");
        assert_eq!(transport.buffer_size, 32, "Buffer size should match");
        assert!(transport.client_mode, "Should be in client mode");

        Ok(())
    }
}
