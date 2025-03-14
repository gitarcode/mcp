use http;
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
use tokio_tungstenite::{
    connect_async,
    tungstenite::{
        error::Error as TungsteniteError, protocol::CloseFrame, Message as TungsteniteMessage,
    },
};
use tracing;
use url;

/// Capture config to server/connect
enum WsTransportConfig {
    Server { host: String, port: u16 },
    Client { url: String },
}

pub struct WebSocketTransport {
    config: WsTransportConfig,
    buffer_size: usize,
    auth_header: Option<String>,
}

impl WebSocketTransport {
    pub fn new_server(host: String, port: u16, buffer_size: usize) -> Self {
        Self {
            config: WsTransportConfig::Server { host, port },
            buffer_size,
            auth_header: None,
        }
    }

    pub fn new_client(host: String, port: u16, buffer_size: usize) -> Self {
        Self {
            config: WsTransportConfig::Client {
                url: format!("ws://{}:{}/ws", host, port),
            },
            buffer_size,
            auth_header: None,
        }
    }

    pub fn new_wss_client(host: String, port: u16, buffer_size: usize) -> Self {
        Self {
            config: WsTransportConfig::Client {
                url: format!("wss://{}:{}/ws", host, port),
            },
            buffer_size,
            auth_header: None,
        }
    }

    pub fn new_client_with_url(url: String, buffer_size: usize) -> Self {
        Self {
            config: WsTransportConfig::Client { url },
            buffer_size,
            auth_header: None,
        }
    }

    pub fn with_auth_header(mut self, auth_header: String) -> Self {
        self.auth_header = Some(auth_header);
        self
    }

    async fn run_server(
        host: String,
        port: u16,
        mut cmd_rx: mpsc::Receiver<TransportCommand>,
        event_tx: mpsc::Sender<TransportEvent>,
    ) {
        // Client counter for unique IDs
        let client_counter = Arc::new(AtomicU64::new(0));

        // Create broadcast channel for WebSocket clients
        let (broadcast_tx, _) = tokio::sync::broadcast::channel::<JsonRpcMessage>(100);
        let broadcast_tx = Arc::new(broadcast_tx);
        let broadcast_tx_clone = Arc::clone(&broadcast_tx);

        // Message forwarding task
        let message_task = tokio::spawn(async move {
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

        // WebSocket connection handler
        let ws_route = warp::path("ws")
            .and(warp::ws())
            .map(move |ws: warp::ws::Ws| {
                let client_id = client_counter.fetch_add(1, Ordering::SeqCst);
                let event_tx = event_tx.clone();
                let broadcast_tx = Arc::clone(&broadcast_tx);

                tracing::debug!("New WebSocket connection: client_id={}", client_id);

                ws.on_upgrade(move |websocket| async move {
                    let (ws_tx, mut ws_rx) = websocket.split();

                    // Use Mutex for shared access to the sink to avoid clone() issues
                    let ws_tx = Arc::new(tokio::sync::Mutex::new(ws_tx));
                    let ws_tx_clone = Arc::clone(&ws_tx);

                    // Subscribe to broadcast channel
                    let mut broadcast_rx = broadcast_tx.subscribe();

                    // Message forwarding task (broadcast -> client)
                    let forward_task = tokio::spawn(async move {
                        while let Ok(msg) = broadcast_rx.recv().await {
                            let json = match serde_json::to_string(&msg) {
                                Ok(json) => json,
                                Err(e) => {
                                    tracing::error!("Failed to serialize message: {:?}", e);
                                    continue;
                                }
                            };

                            let mut tx = ws_tx.lock().await;
                            if let Err(e) = tx.send(WarpMessage::text(json)).await {
                                tracing::error!(
                                    "Failed to send WebSocket message to client {}: {:?}",
                                    client_id,
                                    e
                                );
                                break;
                            }
                        }
                    });

                    // Ping task to keep connection alive
                    let ping_task = tokio::spawn(async move {
                        let mut interval =
                            tokio::time::interval(tokio::time::Duration::from_secs(30));
                        loop {
                            interval.tick().await;
                            let mut tx = ws_tx_clone.lock().await;
                            if let Err(e) = tx.send(WarpMessage::ping("")).await {
                                tracing::debug!(
                                    "Failed to send ping to client {}: {:?}",
                                    client_id,
                                    e
                                );
                                break;
                            }
                        }
                    });

                    // Handle incoming messages from client
                    while let Some(result) = ws_rx.next().await {
                        match result {
                            Ok(msg) => {
                                if msg.is_close() {
                                    tracing::debug!(
                                        "WebSocket connection closed by client: client_id={}",
                                        client_id
                                    );
                                    break;
                                }

                                if msg.is_ping() {
                                    // Reply with pong - warp handles this automatically
                                    continue;
                                }

                                if msg.is_pong() {
                                    // Just acknowledge pong messages
                                    continue;
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
                                tracing::error!(
                                    "WebSocket error from client {}: {:?}",
                                    client_id,
                                    e
                                );
                                break;
                            }
                        }
                    }

                    // Cancel tasks when the connection is closed
                    forward_task.abort();
                    ping_task.abort();

                    tracing::debug!(
                        "WebSocket connection handler completed: client_id={}",
                        client_id
                    );
                })
            });

        // Start the server
        let host_addr = match host.parse::<IpAddr>() {
            Ok(addr) => addr,
            Err(e) => {
                tracing::error!("Failed to parse host address: {:?}", e);
                message_task.abort();
                return;
            }
        };

        tracing::info!("Starting WebSocket server at ws://{}:{}/ws", host, port);

        // This will block until the server is shut down
        warp::serve(ws_route).run((host_addr, port)).await;

        // If we got here, the server has been shut down
        message_task.abort();
        tracing::info!("WebSocket server shut down");
    }

    async fn run_client(
        url: String,
        auth_header: Option<String>,
        mut cmd_rx: mpsc::Receiver<TransportCommand>,
        event_tx: mpsc::Sender<TransportEvent>,
    ) {
        tracing::debug!("Connecting to WebSocket endpoint: {}", url);

        // Connect to the WebSocket server
        let ws_stream_result = if let Some(auth) = &auth_header {
            // Create a custom connector with auth header
            let request = http::Request::builder()
                .uri(&url)
                .header("User-Agent", "mcp-rs-client")
                .header("Authorization", auth)
                .header("Connection", "Upgrade")
                .header("Upgrade", "websocket")
                .header("Host", {
                    // Extract host:port from URL for the Host header
                    if let Ok(parsed_url) = url::Url::parse(&url) {
                        format!(
                            "{}:{}",
                            parsed_url.host_str().unwrap_or("localhost"),
                            parsed_url
                                .port()
                                .unwrap_or(if parsed_url.scheme() == "wss" {
                                    443
                                } else {
                                    80
                                })
                        )
                    } else {
                        "localhost:80".to_string()
                    }
                })
                .header("Sec-WebSocket-Version", "13")
                .header(
                    "Sec-WebSocket-Key",
                    tokio_tungstenite::tungstenite::handshake::client::generate_key(),
                )
                .body(())
                .map_err(|e| {
                    tracing::error!("Failed to build WebSocket request: {:?}", e);
                    // Create a properly formatted error response
                    let response = http::Response::builder()
                        .status(http::StatusCode::BAD_REQUEST)
                        .body(None)
                        .unwrap();
                    TungsteniteError::Http(response)
                });

            if let Ok(request) = request {
                tracing::debug!("Added Authorization header to WebSocket request");
                connect_async(request).await
            } else {
                // Create a properly formatted error response
                let response = http::Response::builder()
                    .status(http::StatusCode::BAD_REQUEST)
                    .body(None)
                    .unwrap();
                Err(TungsteniteError::Http(response))
            }
        } else {
            // Use standard connection without auth
            connect_async(&url).await
        };

        // Connect to the WebSocket server
        let ws_stream = match ws_stream_result {
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

        let (ws_write, ws_read) = ws_stream.split();
        let ws_write = Arc::new(tokio::sync::Mutex::new(ws_write));
        let ws_write_clone = Arc::clone(&ws_write);

        // Create separate sender instances for each task
        let event_tx_receive = event_tx.clone();
        let event_tx_send = event_tx.clone();

        // Receive task - handles messages from the server
        let receive_task = tokio::spawn(async move {
            let mut stream = ws_read;

            while let Some(msg_result) = stream.next().await {
                match msg_result {
                    Ok(msg) => {
                        // Handle control messages
                        if msg.is_ping() {
                            // Pong is automatically handled by the library
                            continue;
                        } else if msg.is_pong() {
                            // Just acknowledge pong messages
                            continue;
                        } else if msg.is_close() {
                            // Handle close message
                            tracing::debug!("Received close frame from server");
                            let _ = event_tx_receive.send(TransportEvent::Closed).await;
                            break;
                        }

                        if let Ok(text) = msg.to_text() {
                            match serde_json::from_str::<JsonRpcMessage>(text) {
                                Ok(json_msg) => {
                                    if let Err(e) = event_tx_receive
                                        .send(TransportEvent::Message(json_msg))
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
                    }
                    Err(e) => {
                        tracing::error!("WebSocket error: {:?}", e);

                        // Check if it's a connection closed error
                        let is_connection_closed = matches!(
                            e,
                            TungsteniteError::ConnectionClosed
                                | TungsteniteError::AlreadyClosed
                                | TungsteniteError::Protocol(_)
                        );

                        if is_connection_closed {
                            let _ = event_tx_receive.send(TransportEvent::Closed).await;
                        } else {
                            let _ = event_tx_receive
                                .send(TransportEvent::Error(McpError::ConnectionClosed))
                                .await;
                        }

                        break;
                    }
                }
            }
        });

        // Ping task - keeps the connection alive
        let ping_task = tokio::spawn(async move {
            let mut interval = tokio::time::interval(tokio::time::Duration::from_secs(30));

            loop {
                interval.tick().await;
                let mut writer = ws_write_clone.lock().await;

                // Using tokio-tungstenite compatible empty Vec for ping
                if let Err(e) = writer.send(TungsteniteMessage::Ping(vec![].into())).await {
                    tracing::debug!("Failed to send ping: {:?}", e);
                    break;
                }
            }
        });

        // Send task - handles messages to be sent to the server
        loop {
            match cmd_rx.recv().await {
                Some(TransportCommand::SendMessage(msg)) => {
                    let json = match serde_json::to_string(&msg) {
                        Ok(j) => j,
                        Err(e) => {
                            tracing::error!("Failed to serialize message: {:?}", e);
                            continue;
                        }
                    };

                    let mut writer = ws_write.lock().await;
                    if let Err(e) = writer.send(TungsteniteMessage::Text(json.into())).await {
                        tracing::error!("Failed to send message: {:?}", e);
                        let _ = event_tx_send
                            .send(TransportEvent::Error(McpError::ConnectionClosed))
                            .await;
                        break;
                    }
                }
                Some(TransportCommand::Close) => {
                    tracing::debug!("Closing WebSocket client connection");

                    // Send close frame
                    let mut writer = ws_write.lock().await;
                    let _ = writer
                        .send(TungsteniteMessage::Close(Some(CloseFrame {
                            code: 1000.into(),
                            reason: "Client initiated close".into(),
                        })))
                        .await;

                    break;
                }
                None => {
                    tracing::debug!("Command channel closed");
                    break;
                }
            }
        }

        // Clean up tasks
        ping_task.abort();
        receive_task.abort();

        let _ = event_tx_send.send(TransportEvent::Closed).await;
        tracing::debug!("WebSocket client connection closed");
    }
}

#[async_trait]
impl Transport for WebSocketTransport {
    async fn start(&mut self) -> Result<TransportChannels, McpError> {
        let (cmd_tx, cmd_rx) = mpsc::channel(self.buffer_size);
        let (event_tx, event_rx) = mpsc::channel(self.buffer_size);

        match &self.config {
            WsTransportConfig::Client { url } => {
                tokio::spawn(Self::run_client(
                    url.clone(),
                    self.auth_header.clone(),
                    cmd_rx,
                    event_tx,
                ));
            }
            WsTransportConfig::Server { host, port } => {
                tokio::spawn(Self::run_server(
                    host.clone(),
                    port.clone(),
                    cmd_rx,
                    event_tx,
                ));
            }
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
        let ws_url = format!("ws://{}:{}/ws", host, port);

        // Start the transport
        let TransportChannels { cmd_tx, event_rx } = transport.start().await?;

        // Give the server time to start
        sleep(Duration::from_millis(300)).await;

        // Connect a client to the server
        let (ws_stream, _) = connect_async(&ws_url).await.expect("Failed to connect");
        let (mut write, mut read) = ws_stream.split();

        // Give time for the connection to be fully established
        sleep(Duration::from_millis(200)).await;

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
        sleep(Duration::from_millis(300)).await;

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
        sleep(Duration::from_millis(500)).await;

        // Check if the client received the message - with a timeout and handling ping messages
        let timeout_duration = Duration::from_secs(5);
        let start_time = std::time::Instant::now();

        loop {
            if start_time.elapsed() > timeout_duration {
                panic!("Timeout waiting for text message");
            }

            match read.next().await {
                Some(Ok(msg)) => {
                    match msg {
                        Message::Text(text) => {
                            let received: JsonRpcMessage = serde_json::from_str(&text).unwrap();
                            match received {
                                JsonRpcMessage::Response(resp) => {
                                    assert_eq!(
                                        resp.result,
                                        Some(serde_json::json!({"status": "success"}))
                                    );
                                    break; // Success case - exit the loop
                                }
                                _ => panic!("Expected Response message"),
                            }
                        }
                        Message::Ping(_) => {
                            // Just skip ping messages and continue waiting
                            continue;
                        }
                        other => {
                            panic!("Expected Text message, got: {:?}", other);
                        }
                    }
                }
                Some(Err(e)) => panic!("Error receiving message: {:?}", e),
                None => panic!("Connection closed without receiving message"),
            }
        }

        // Close the transport
        cmd_tx.send(TransportCommand::Close).await.unwrap();

        // Wait for cleanup
        sleep(Duration::from_millis(300)).await;

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
        let ws_url = format!("ws://{}:{}/ws", host, port);

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
        let mut transport = WebSocketTransport::new_client_with_url(ws_url, 32);
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

    #[tokio::test]
    async fn test_websocket_transport_with_auth() -> Result<(), McpError> {
        // Initialize tracing for tests
        let _ = tracing_subscriber::fmt()
            .with_env_filter("debug")
            .try_init();

        // Start a WebSocket server using warp for the client to connect to
        let host = "127.0.0.1".to_string();
        let port = PORT_COUNTER.fetch_add(1, AtomicOrdering::SeqCst); // Unique port to avoid conflicts
        let ws_url = format!("ws://{}:{}/ws", host, port);

        // Create a channel to receive messages from the test server
        let (server_tx, mut server_rx) = mpsc::channel::<JsonRpcMessage>(32);
        let (auth_tx, mut auth_rx) = mpsc::channel::<String>(1);

        // Create a WebSocket server that checks for auth headers
        let ws_route = warp::path("ws")
            .and(warp::ws())
            .and(warp::header::optional::<String>("authorization"))
            .map(move |ws: warp::ws::Ws, auth_header: Option<String>| {
                let server_tx = server_tx.clone();
                let auth_tx = auth_tx.clone();

                // Send the auth header to our test channel
                if let Some(auth) = auth_header.clone() {
                    let auth_tx_clone = auth_tx.clone();
                    tokio::spawn(async move {
                        let _ = auth_tx_clone.send(auth).await;
                    });
                }

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

        // Create and start the WebSocket client transport with auth header
        let auth_header = "Bearer test-token-123".to_string();
        let mut transport = WebSocketTransport::new_client_with_url(ws_url, 32)
            .with_auth_header(auth_header.clone());

        let TransportChannels {
            cmd_tx,
            event_rx: _,
        } = transport.start().await?;

        // Give the client time to connect
        sleep(Duration::from_millis(100)).await;

        // Check if the server received the auth header
        if let Some(received_auth) = auth_rx.recv().await {
            assert_eq!(received_auth, auth_header, "Auth header mismatch");
        } else {
            panic!("No auth header received by server");
        }

        // Create a test message
        let request_id = 54321;
        let test_request = JsonRpcMessage::Request(JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: request_id,
            method: "test.auth.method".to_string(),
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
                    assert_eq!(req.method, "test.auth.method");
                }
                _ => panic!("Expected Request message"),
            }
        } else {
            panic!("No message received by server");
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
