use config::ServerConfig;
use serde::{Deserialize, Serialize};
use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    time::Duration,
    path::PathBuf,
};
use tokio::sync::RwLock;
use tracing::info;
use serde_json::Value;
use tokio::sync::watch;

use crate::prompts::{GetPromptRequest, ListPromptsRequest, PromptCapabilities, PromptManager};
use crate::tools::{ToolCapabilities, ToolManager};
use crate::{
    client::ServerCapabilities,
    error::McpError,
    logging::{LoggingManager, SetLevelRequest, LoggingCapabilities},
    protocol::{JsonRpcNotification, Protocol, ProtocolBuilder, ProtocolOptions, RequestHandler, BasicRequestHandler},
    protocol::types::*,
    resource::{ListResourcesRequest, ReadResourceRequest, ResourceCapabilities, ResourceManager},
    tools::{CallToolRequest, ListToolsRequest},
    transport::{
        sse::SseTransport,
        stdio::StdioTransport,
        unix::UnixTransport,
        Transport, TransportChannels, TransportCommand, TransportEvent,
    },
    NotificationSender,
};
use tokio::sync::mpsc;

pub mod config;

// Add initialization types
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InitializeParams {
    pub protocol_version: String,
    pub capabilities: ClientCapabilities,
    pub client_info: ClientInfo,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InitializeResult {
    pub name: String,
    pub version: String,
    pub protocol_version: String,
    pub capabilities: ServerCapabilities,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClientCapabilities {
    pub roots: Option<RootsCapabilities>,
    pub sampling: Option<SamplingCapabilities>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RootsCapabilities {
    pub list_changed: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SamplingCapabilities {}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClientInfo {
    pub name: String,
    pub version: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerInfo {
    pub name: String,
    pub version: String,
}

// Add server state enum
#[derive(Debug, Clone, Copy, PartialEq)]
enum ServerState {
    Created,
    Initializing,
    Running,
    ShuttingDown,
}

pub struct McpServer<H>
where
    H: RequestHandler + Send + Sync + 'static
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
    H: RequestHandler + Send + Sync + 'static
{
    pub fn new(config: ServerConfig, handler: H) -> Self {
        let (notification_tx, notification_rx) = mpsc::channel(100);
        let (state_tx, state_rx) = watch::channel(ServerState::Created);
        
        Self {
            handler: Arc::new(handler),
            config: config.clone(),
            resource_manager: Arc::new(ResourceManager::new(ResourceCapabilities { 
                subscribe: false,
                list_changed: false,
            })),
            tool_manager: Arc::new(ToolManager::new(ToolCapabilities { 
                list_changed: false,
            })),
            prompt_manager: Arc::new(PromptManager::new(PromptCapabilities {
                list_changed: false,
            })),
            logging_manager: Arc::new(tokio::sync::Mutex::new(LoggingManager::new())),
            notification_tx,
            notification_rx: Some(notification_rx),
            state: Arc::new((state_tx, state_rx)),
            supported_versions: vec!["1.0".to_string()],
            client_capabilities: Arc::new(RwLock::new(None)),
        }
    }

    pub async fn process_request(&self, method: &str, params: Option<Value>) -> Result<Value, McpError> {
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
            1024 // Buffer size
        );
        self.run_transport(transport).await
    }
    pub async fn run_unix_transport(&mut self) -> Result<(), McpError> {
        tracing::info!("Starting Unix transport");

        let transport = UnixTransport::new_server(
            PathBuf::from(&self.config.server.host),
            Some(1024)
        );
        self.run_transport(transport).await
    }

    async fn run_transport<T: Transport>(&mut self, transport: T) -> Result<(), McpError> {
        // Take ownership of notification receiver
        let notification_rx = self.notification_rx.take()
            .ok_or_else(|| McpError::InternalError("Notification receiver already taken".to_string()))?;

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

        // Build protocol
        let mut protocol = Protocol::builder(Some(ProtocolOptions {
            enforce_strict_capabilities: false,
        }))
            .with_request_handler("initialize", Box::new(|req, _extra| {
                Box::pin(async move {
                    let params: InitializeParams = serde_json::from_value(req.params.unwrap_or_default())
                        .map_err(|_| McpError::InvalidParams)?;
                    
                    let result = InitializeResult {
                        name: "test-server".to_string(),
                        version: "1.0.0".to_string(),
                        protocol_version: "1.0".to_string(),
                        capabilities: ServerCapabilities {
                            logging: Some(LoggingCapabilities {}),
                            prompts: Some(PromptCapabilities { list_changed: false }),
                            resources: Some(ResourceCapabilities { subscribe: false, list_changed: false }),
                            tools: Some(ToolCapabilities { list_changed: false }),
                        },
                    };

                    Ok(serde_json::to_value(result).unwrap())
                })
            }))
            .build();

        // Connect transport
        let protocol_handle = protocol.connect(transport).await?;

        info!("Server running and ready to handle requests");

        // Wait for shutdown signal
        shutdown_rx.recv().await;

        // Clean shutdown
        protocol_handle.close().await?;
        info!("Server shutting down");
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    use tokio::time::sleep;
    use async_trait::async_trait;
    use serde_json::json;

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
                                            "roots": { "list_changed": true }
                                        },
                                        "client_info": {
                                            "name": "test-client",
                                            "version": "1.0.0"
                                        }
                                    })),
                                    error: None,
                                });
                                
                                event_tx.send(TransportEvent::Message(response)).await.unwrap();
                            }
                        },
                        TransportCommand::Close => break,
                        _ => {}
                    }
                }
                event_tx.send(TransportEvent::Closed).await.unwrap();
            });
            
            self._channels = Some(channels.clone());
            Ok(channels)
        }
    }

    struct MockHandler;

    #[async_trait]
    impl RequestHandler for MockHandler {
        async fn handle_request(&self, method: &str, params: Option<Value>) -> Result<Value, McpError> {
            match method {
                "test.echo" => Ok(params.unwrap_or(Value::Null)),
                _ => Ok(Value::Null)
            }
        }

        async fn handle_notification(&self, _method: &str, _params: Option<Value>) -> Result<(), McpError> {
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
        let mut config = ServerConfig::default();
        config.server.host = "localhost".to_string();
        config.server.port = 8080;

        // Create server instance
        let mut server = McpServer::new(config, MockHandler);
        
        // Get state and notification sender before moving server
        let notification_tx = server.notification_tx.clone();
        let state = Arc::clone(&server.state);

        // Spawn server task
        let server_handle = tokio::spawn(async move {
            let transport = MockTransport::new();
            server.run_transport(transport).await
        });

        // Give the server a moment to start
        sleep(Duration::from_millis(100)).await;

        // Test sending a notification
        let test_notification = JsonRpcNotification {
            jsonrpc: "2.0".to_string(),
            method: "test.notification".to_string(),
            params: Some(json!({"message": "test"})),
        };
        notification_tx.send(test_notification).await.unwrap();

        // Give time for notification processing
        sleep(Duration::from_millis(100)).await;

        // Use the state we cloned earlier instead of trying to get it from server_handle
        let (state_tx, _): &(watch::Sender<ServerState>, watch::Receiver<ServerState>) = &*state;
        
        // Trigger shutdown
        state_tx.send(ServerState::ShuttingDown).unwrap();

        // Wait for server to shut down
        match tokio::time::timeout(Duration::from_secs(1), server_handle).await {
            Ok(result) => {
                assert!(result.unwrap().is_ok(), "Server should shut down cleanly");
            },
            Err(_) => panic!("Server did not shut down within timeout period"),
        }
    }

    #[tokio::test]
    async fn test_protocol_messages() {
        let mut config = ServerConfig::default();
        config.server.host = "localhost".to_string();
        config.server.port = 8080;

        let mut server = McpServer::new(config, MockHandler);
        
        // Get notification sender and state before moving server
        let notification_tx = server.notification_tx.clone();
        let state = Arc::clone(&server.state);

        // Spawn server with mock transport
        let server_handle = tokio::spawn(async move {
            let transport = MockTransport::new();
            server.run_transport(transport).await
        });

        // Wait for server to start
        sleep(Duration::from_millis(100)).await;

        // Test sending different types of notifications
        let notifications = vec![
            JsonRpcNotification {
                jsonrpc: "2.0".to_string(),
                method: "resource.changed".to_string(),
                params: Some(json!({
                    "path": "/test/resource",
                    "type": "modified"
                })),
            },
            JsonRpcNotification {
                jsonrpc: "2.0".to_string(),
                method: "tool.executed".to_string(),
                params: Some(json!({
                    "tool": "test-tool",
                    "status": "success"
                })),
            },
        ];

        for notification in notifications {
            notification_tx.send(notification).await.unwrap();
            sleep(Duration::from_millis(50)).await;
        }

        // Use cloned state
        let (state_tx, _): &(watch::Sender<ServerState>, watch::Receiver<ServerState>) = &*state;
        state_tx.send(ServerState::ShuttingDown).unwrap();

        // Verify clean shutdown
        match tokio::time::timeout(Duration::from_secs(1), server_handle).await {
            Ok(result) => {
                assert!(result.unwrap().is_ok(), "Server should shut down cleanly");
            },
            Err(_) => panic!("Server did not shut down within timeout period"),
        }
    }
}