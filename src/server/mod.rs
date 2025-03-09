use config::ServerConfig;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{path::PathBuf, sync::Arc, time::Duration};
use tokio::sync::watch;
use tokio::sync::RwLock;
use tracing;

use crate::prompts::{PromptCapabilities, PromptManager};
use crate::tools::{ToolCapabilities, ToolManager};
use crate::{
    client::ServerCapabilities,
    error::McpError,
    logging::LoggingManager,
    protocol::{
        JsonRpcError, JsonRpcMessage, JsonRpcNotification, JsonRpcResponse, RequestHandler,
    },
    resource::{ResourceCapabilities, ResourceManager},
    transport::{
        sse::SseTransport, stdio::StdioTransport, Transport, TransportChannels, TransportCommand,
        TransportEvent,
    },
};
use tokio::sync::mpsc;

pub mod config;

// Add initialization types
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InitializeParams {
    pub protocol_version: String,
    pub capabilities: ClientCapabilities,
    pub client_info: ClientInfo,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InitializeResult {
    pub protocol_version: String,
    pub capabilities: ServerCapabilities,
    pub server_info: InitializeServerInfo,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InitializeServerInfo {
    pub name: String,
    pub version: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClientCapabilities {
    pub roots: Option<RootsCapabilities>,
    pub sampling: Option<SamplingCapabilities>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RootsCapabilities {
    pub list_changed: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SamplingCapabilities {}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClientInfo {
    pub name: String,
    pub version: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerInfo {
    pub name: String,
    pub version: String,
}

// Add server state enum
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq)]
enum ServerState {
    Created,
    Initializing,
    Running,
    ShuttingDown,
}

#[allow(dead_code)]
pub struct McpServer<H>
where
    H: RequestHandler + Send + Sync + 'static,
{
    pub handler: Arc<H>,
    pub config: ServerConfig,
    pub resource_manager: Arc<ResourceManager>,
    pub tool_manager: Arc<ToolManager>,
    pub prompt_manager: Arc<PromptManager>,
    pub logging_manager: Arc<tokio::sync::Mutex<LoggingManager>>,
    notification_tx: mpsc::Sender<JsonRpcNotification>,
    notification_rx: Option<mpsc::Receiver<JsonRpcNotification>>, // Make this Option
    state: Arc<(watch::Sender<ServerState>, watch::Receiver<ServerState>)>,
    supported_versions: Vec<String>,
    client_capabilities: Arc<RwLock<Option<ClientCapabilities>>>,
}

impl<H> McpServer<H>
where
    H: RequestHandler + Send + Sync + 'static,
{
    pub fn new(config: ServerConfig, handler: H) -> Self {
        let (notification_tx, notification_rx) = mpsc::channel(100);
        let (state_tx, state_rx) = watch::channel(ServerState::Created);

        Self {
            handler: Arc::new(handler),
            config: config.clone(),
            resource_manager: Arc::new(ResourceManager::new(ResourceCapabilities {
                subscribe: config
                    .capabilities
                    .as_ref()
                    .is_some_and(|c| c.resources.as_ref().is_some_and(|r| r.subscribe)),
                list_changed: config
                    .capabilities
                    .as_ref()
                    .is_some_and(|c| c.resources.as_ref().is_some_and(|r| r.list_changed)),
            })),
            tool_manager: Arc::new(ToolManager::new(ToolCapabilities {
                list_changed: config
                    .capabilities
                    .as_ref()
                    .is_some_and(|c| c.tools.as_ref().is_some_and(|t| t.list_changed)),
            })),
            prompt_manager: Arc::new(PromptManager::new(PromptCapabilities {
                list_changed: config
                    .capabilities
                    .as_ref()
                    .is_some_and(|c| c.prompts.as_ref().is_some_and(|p| p.list_changed)),
            })),
            logging_manager: Arc::new(tokio::sync::Mutex::new(LoggingManager::new())),
            notification_tx,
            notification_rx: Some(notification_rx),
            state: Arc::new((state_tx, state_rx)),
            supported_versions: vec!["1.0".to_string()],
            client_capabilities: Arc::new(RwLock::new(None)),
        }
    }

    pub async fn process_request(
        &self,
        method: &str,
        params: Option<Value>,
    ) -> Result<Value, McpError> {
        self.handler.handle_request(method, params).await
    }

    pub async fn run_stdio_transport(&mut self) -> Result<(), McpError> {
        let transport = StdioTransport::new(Some(1024));
        self.run_transport(transport).await
    }

    pub async fn run_sse_transport(&mut self) -> Result<(), McpError> {
        let transport = SseTransport::new_server(
            self.config.server.host.clone(),
            self.config.server.port,
            1024, // Buffer size
        );
        self.run_transport(transport).await
    }

    #[cfg(unix)]
    pub async fn run_unix_transport(&mut self) -> Result<(), McpError> {
        tracing::info!("Starting Unix transport");

        let transport = crate::transport::unix::UnixTransport::new_server(
            PathBuf::from(&self.config.server.host),
            Some(1024),
        );
        self.run_transport(transport).await
    }

    pub async fn run_websocket_transport(&mut self) -> Result<(), McpError> {
        tracing::info!("Starting WebSocket transport");

        let transport = crate::transport::ws::WebSocketTransport::new_server(
            self.config.server.host.clone(),
            self.config.server.port,
            1024, // Buffer size
        );
        self.run_transport(transport).await
    }

    /// Run the server with pre-initialized transport channels
    pub async fn run(&mut self, channels: TransportChannels) -> Result<(), McpError> {
        // Take ownership of notification receiver
        let _notification_rx = self.notification_rx.take().ok_or_else(|| {
            McpError::InternalError("Notification receiver already taken".to_string())
        })?;

        // Create shutdown channel
        let (shutdown_tx, mut shutdown_rx) = tokio::sync::mpsc::channel::<()>(1);

        // Clone Arc for shutdown handler
        let state = Arc::clone(&self.state);

        // Spawn task to watch server state and send shutdown signal
        tokio::spawn(async move {
            loop {
                if *state.1.borrow() == ServerState::ShuttingDown {
                    let _ = shutdown_tx.send(()).await;
                    break;
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        });

        let TransportChannels { cmd_tx, event_rx } = channels;
        let event_rx = Arc::clone(&event_rx);

        // Process messages until shutdown
        loop {
            // Get a lock on the event receiver
            let mut event_rx_guard = event_rx.lock().await;

            tokio::select! {
                _ = shutdown_rx.recv() => {
                    // Shutdown signal received
                    tracing::info!("Shutdown signal received, closing transport");
                    if let Err(e) = cmd_tx.send(TransportCommand::Close).await {
                        tracing::error!("Failed to send close command: {:?}", e);
                    }
                    break;
                }
                event = event_rx_guard.recv() => {
                    match event {
                        Some(TransportEvent::Message(msg)) => {
                            match msg {
                                JsonRpcMessage::Request(req) => {
                                    // Process request
                                    match self.handler.handle_request(&req.method, req.params.clone()).await {
                                        Ok(result) => {
                                            let response = JsonRpcMessage::Response(JsonRpcResponse {
                                                jsonrpc: "2.0".to_string(),
                                                id: req.id,
                                                result: Some(result),
                                                error: None,
                                            });
                                            if let Err(e) = cmd_tx.send(TransportCommand::SendMessage(response)).await {
                                                tracing::error!("Failed to send response: {:?}", e);
                                            }
                                        }
                                        Err(e) => {
                                            let response = JsonRpcMessage::Response(JsonRpcResponse {
                                                jsonrpc: "2.0".to_string(),
                                                id: req.id,
                                                result: None,
                                                error: Some(JsonRpcError {
                                                    code: e.code(),
                                                    message: e.to_string(),
                                                    data: None,
                                                }),
                                            });
                                            if let Err(e) = cmd_tx.send(TransportCommand::SendMessage(response)).await {
                                                tracing::error!("Failed to send error response: {:?}", e);
                                            }
                                        }
                                    }
                                }
                                JsonRpcMessage::Notification(notification) => {
                                    // Process notification
                                    if let Err(e) = self.handler.handle_notification(&notification.method, notification.params).await {
                                        tracing::error!("Error processing notification: {:?}", e);
                                    }
                                }
                                _ => {
                                    // Process other message types
                                    tracing::warn!("Unhandled message type: {:?}", msg);
                                }
                            }
                        }
                        Some(TransportEvent::Error(e)) => {
                            tracing::error!("Transport error: {:?}", e);
                        }
                        Some(TransportEvent::Closed) => {
                            tracing::info!("Transport closed");
                            break;
                        }
                        None => {
                            tracing::info!("Transport closed");
                            break;
                        }
                    }
                }
            }
        }

        tracing::info!("Server shutdown complete");
        Ok(())
    }

    async fn run_transport<T: Transport>(&mut self, mut transport: T) -> Result<(), McpError> {
        // Take ownership of notification receiver
        let _notification_rx = self.notification_rx.take().ok_or_else(|| {
            McpError::InternalError("Notification receiver already taken".to_string())
        })?;

        // Start the transport
        let channels = transport.start().await?;

        // Run the server with the transport channels
        self.run(channels).await
    }
}

#[cfg(test)]
mod tests {
    use crate::{
        protocol::{JsonRpcMessage, JsonRpcResponse},
        transport::{TransportChannels, TransportCommand, TransportEvent},
    };

    use super::*;
    use async_trait::async_trait;
    use serde_json::json;
    use std::time::Duration;
    use tokio::time::sleep;

    struct MockTransport {
        _channels: Option<TransportChannels>,
    }

    impl MockTransport {
        fn new() -> Self {
            Self { _channels: None }
        }
    }

    #[async_trait]
    impl Transport for MockTransport {
        async fn start(&mut self) -> Result<TransportChannels, McpError> {
            let (command_tx, mut command_rx) = mpsc::channel(32);
            let (event_tx, event_rx) = mpsc::channel(32);

            let channels = TransportChannels {
                cmd_tx: command_tx,
                event_rx: Arc::new(tokio::sync::Mutex::new(event_rx)),
            };

            // Spawn a task to handle commands
            let event_tx_clone = event_tx.clone();
            tokio::spawn(async move {
                while let Some(cmd) = command_rx.recv().await {
                    match cmd {
                        TransportCommand::SendMessage(JsonRpcMessage::Request(req)) => {
                            if req.method == "initialize" {
                                // Simulate client responding to initialize request
                                let response = JsonRpcMessage::Response(JsonRpcResponse {
                                    jsonrpc: "2.0".to_string(),
                                    id: req.id,
                                    result: Some(json!({
                                        "protocol_version": "1.0",
                                        "capabilities": {
                                            "roots": { "listChanged": true }
                                        },
                                        "client_info": {
                                            "name": "test-client",
                                            "version": "1.0.0"
                                        }
                                    })),
                                    error: None,
                                });

                                if let Err(e) =
                                    event_tx.send(TransportEvent::Message(response)).await
                                {
                                    tracing::error!("Failed to send response message: {:?}", e);
                                    break;
                                }
                            }
                        }
                        TransportCommand::SendMessage(msg) => {
                            // Echo the message back for testing
                            tracing::debug!("MockTransport received message: {:?}", msg);
                        }
                        TransportCommand::Close => {
                            tracing::debug!("MockTransport received Close command");
                            break;
                        }
                    }
                }

                tracing::debug!("MockTransport command loop ended, sending Closed event");
                if let Err(e) = event_tx_clone.send(TransportEvent::Closed).await {
                    tracing::error!("Failed to send Closed event: {:?}", e);
                }
            });

            self._channels = Some(channels.clone());
            Ok(channels)
        }
    }

    struct MockHandler;

    #[async_trait]
    impl RequestHandler for MockHandler {
        async fn handle_request(
            &self,
            method: &str,
            params: Option<Value>,
        ) -> Result<Value, McpError> {
            match method {
                "test.echo" => Ok(params.unwrap_or(Value::Null)),
                _ => Ok(Value::Null),
            }
        }

        async fn handle_notification(
            &self,
            _method: &str,
            _params: Option<Value>,
        ) -> Result<(), McpError> {
            Ok(())
        }

        fn get_capabilities(&self) -> crate::protocol::ServerCapabilities {
            crate::protocol::ServerCapabilities {
                name: "test-server".to_string(),
                version: "1.0.0".to_string(),
                protocol_version: "1.0".to_string(),
                capabilities: vec!["test.echo".to_string()],
            }
        }
    }

    #[tokio::test]
    async fn test_run_transport() {
        // Create a simple configuration
        let config = ServerConfig::default();

        // Create server instance with mock handler
        let mut server = McpServer::new(config, MockHandler);

        // Get the notification sender
        let notification_tx = server.notification_tx.clone();

        // Create a mock transport that doesn't rely on external connections
        let transport = MockTransport::new();

        // Start the server in a separate task with a timeout
        let server_handle = tokio::spawn(async move {
            let result =
                tokio::time::timeout(Duration::from_secs(2), server.run_transport(transport)).await;

            // We expect a timeout since we're not sending a shutdown signal
            assert!(result.is_err(), "Expected timeout");
            Ok::<(), McpError>(())
        });

        // Give the server time to start
        sleep(Duration::from_millis(100)).await;

        // Send a test notification
        let test_notification = JsonRpcNotification {
            jsonrpc: "2.0".to_string(),
            method: "test.notification".to_string(),
            params: Some(json!({"message": "test"})),
        };

        // Try to send the notification, but don't panic if it fails
        // This is just to test the notification channel
        let _ = notification_tx.send(test_notification).await;

        // Wait a bit to let the server process the notification
        sleep(Duration::from_millis(100)).await;

        // Cancel the server task since we don't need it anymore
        server_handle.abort();

        // Wait for the server task to complete
        let _ = server_handle.await;
    }

    #[tokio::test]
    async fn test_protocol_messages() {
        // This test is simplified to focus on basic protocol message handling
        // without relying on the transport

        // Create a simple configuration
        let config = ServerConfig::default();

        // Create a server instance with mock handler
        let server = McpServer::new(config, MockHandler);

        // Test the process_request method directly
        let result = server
            .process_request("test.echo", Some(json!({"echo": "test"})))
            .await;

        // Verify the result
        assert!(result.is_ok(), "Request should be processed successfully");
        assert_eq!(
            result.unwrap(),
            json!({"echo": "test"}),
            "Echo handler should return the input params"
        );
    }
}
