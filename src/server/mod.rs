use config::ServerConfig;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{path::PathBuf, sync::Arc, time::Duration};
use tokio::sync::watch;
use tokio::sync::RwLock;
use tracing::info;

use crate::prompts::{GetPromptRequest, ListPromptsRequest, PromptCapabilities, PromptManager};
use crate::tools::{ToolCapabilities, ToolManager};
use crate::{
    client::ServerCapabilities,
    error::McpError,
    logging::{LoggingCapabilities, LoggingManager},
    protocol::{JsonRpcNotification, Protocol, ProtocolOptions, RequestHandler},
    resource::{ListResourcesRequest, ReadResourceRequest, ResourceCapabilities, ResourceManager},
    tools::{CallToolRequest, ListToolsRequest},
    transport::{sse::SseTransport, stdio::StdioTransport, ws::WebSocketTransport, Transport},
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
    notification_rx: Option<mpsc::Receiver<JsonRpcNotification>>,
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
                    .clone()
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

    // Initialize the server with notification support
    pub fn initialize(&mut self) {
        // Create a new tool manager with notification support
        if self
            .config
            .capabilities
            .as_ref()
            .is_some_and(|c| c.tools.as_ref().is_some_and(|t| t.list_changed))
        {
            let notification_tx = self.notification_tx.clone();
            let tool_capabilities = ToolCapabilities { list_changed: true };

            // Replace the tool manager with one that has notification support
            self.tool_manager = Arc::new(ToolManager::with_notification_sender(
                tool_capabilities,
                notification_tx,
            ));
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
        self.initialize();
        let transport = StdioTransport::new(Some(1024));
        self.run_transport(transport).await
    }

    pub async fn run_sse_transport(&mut self) -> Result<(), McpError> {
        self.initialize();
        
        // Check if port is None, if so, don't start the transport
        if self.config.server.port.is_none() {
            tracing::info!("SSE transport disabled (port is None)");
            return Ok(());
        }
        
        let transport = SseTransport::new_server(
            self.config.server.host.clone(),
            self.config.server.port.unwrap(), // Safe to unwrap since we checked it's not None
            1024, // Buffer size
        );
        self.run_transport(transport).await
    }

    pub async fn run_websocket_transport(&mut self) -> Result<(), McpError> {
        self.initialize();
        
        // Check if port is None, if so, don't start the transport
        if self.config.server.port.is_none() {
            tracing::info!("WebSocket transport disabled (port is None)");
            return Ok(());
        }
        
        let transport = WebSocketTransport::new_server(
            self.config.server.host.clone(),
            self.config.server.port,
            1024, // Buffer size
        );
        self.run_transport(transport).await
    }

    #[cfg(unix)]
    pub async fn run_unix_transport(&mut self) -> Result<(), McpError> {
        tracing::info!("Starting Unix transport");
        self.initialize();
        let transport = crate::transport::unix::UnixTransport::new_server(
            PathBuf::from(&self.config.server.host),
            Some(1024),
        );
        self.run_transport(transport).await
    }

    async fn run_transport<T: Transport>(&mut self, transport: T) -> Result<(), McpError> {
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

        // Build protocol

        let resource_manager = Arc::clone(&self.resource_manager);
        let resource_manager2 = Arc::clone(&self.resource_manager);
        let resource_manager3 = Arc::clone(&self.resource_manager);
        let tool_manager = Arc::clone(&self.tool_manager);
        let tool_manager2 = Arc::clone(&self.tool_manager);
        let prompt_manager = Arc::clone(&self.prompt_manager);
        let prompt_manager2 = Arc::clone(&self.prompt_manager);

        let mut protocol = Protocol::builder(Some(ProtocolOptions {
            enforce_strict_capabilities: false,
        }))
        .with_request_handler(
            "initialize",
            Box::new(|req, _extra| {
                Box::pin(async move {
                    let _params: InitializeParams =
                        serde_json::from_value(req.params.unwrap_or_default())
                            .map_err(|_| McpError::InvalidParams)?;

                    let result = InitializeResult {
                        protocol_version: "2024-11-05".to_string(),
                        capabilities: ServerCapabilities {
                            logging: Some(LoggingCapabilities {}),
                            prompts: Some(PromptCapabilities {
                                list_changed: false,
                            }),
                            resources: Some(ResourceCapabilities {
                                subscribe: false,
                                list_changed: false,
                            }),
                            tools: Some(ToolCapabilities {
                                list_changed: false,
                            }),
                        },
                        server_info: InitializeServerInfo {
                            name: "test-server".to_string(),
                            version: "1.0.0".to_string(),
                        },
                    };

                    Ok(serde_json::to_value(result).unwrap())
                })
            }),
        )
        .with_request_handler(
            "resources/list",
            Box::new(move |req, _extra| {
                let resource_manager = Arc::clone(&resource_manager);
                Box::pin(async move {
                    let params: ListResourcesRequest = req
                        .params
                        .map(serde_json::from_value)
                        .transpose()
                        .map_err(|_| McpError::InvalidParams)?
                        .unwrap_or_default();

                    let resources_list = resource_manager.list_resources(params.cursor).await?;
                    Ok(serde_json::to_value(resources_list).unwrap())
                })
            }),
        )
        .with_request_handler(
            "resources/read",
            Box::new(move |req, _extra| {
                let resource_manager = Arc::clone(&resource_manager2);
                Box::pin(async move {
                    let params: ReadResourceRequest =
                        serde_json::from_value(req.params.unwrap_or_default())
                            .map_err(|_| McpError::InvalidParams)?;
                    let resource = resource_manager.read_resource(&params.uri).await?;
                    Ok(serde_json::to_value(resource).unwrap())
                })
            }),
        )
        .with_request_handler(
            "resources/templates/list",
            Box::new(move |_req, _extra| {
                let resource_manager = Arc::clone(&resource_manager3);
                Box::pin(async move {
                    let templates_list = resource_manager.list_templates().await?;
                    Ok(serde_json::to_value(templates_list).unwrap())
                })
            }),
        )
        .with_request_handler(
            "tools/list",
            Box::new(move |req, _extra| {
                let tool_manager = Arc::clone(&tool_manager);
                Box::pin(async move {
                    let params: ListToolsRequest = req
                        .params
                        .map(serde_json::from_value)
                        .transpose()
                        .map_err(|_| McpError::InvalidParams)?
                        .unwrap_or_default();

                    let tools_list = tool_manager.list_tools(params.cursor).await?;
                    Ok(serde_json::to_value(tools_list).unwrap())
                })
            }),
        )
        .with_request_handler(
            "tools/call",
            Box::new(move |req, _extra| {
                let tool_manager = Arc::clone(&tool_manager2);
                Box::pin(async move {
                    let params: CallToolRequest =
                        serde_json::from_value(req.params.unwrap_or_default())
                            .map_err(|_| McpError::InvalidParams)?;
                    let result = tool_manager
                        .call_tool(&params.name, params.arguments)
                        .await?;
                    Ok(serde_json::to_value(result).unwrap())
                })
            }),
        )
        .with_request_handler(
            "prompts/list",
            Box::new(move |req, _extra| {
                let prompt_manager = Arc::clone(&prompt_manager);
                Box::pin(async move {
                    let params: ListPromptsRequest = req
                        .params
                        .map(serde_json::from_value)
                        .transpose()
                        .map_err(|_| McpError::InvalidParams)?
                        .unwrap_or_default();

                    let prompts_list = prompt_manager.list_prompts(params.cursor).await?;
                    Ok(serde_json::to_value(prompts_list).unwrap())
                })
            }),
        )
        .with_request_handler(
            "prompts/get",
            Box::new(move |req, _extra| {
                let prompt_manager = Arc::clone(&prompt_manager2);
                Box::pin(async move {
                    let params: GetPromptRequest =
                        serde_json::from_value(req.params.unwrap_or_default())
                            .map_err(|_| McpError::InvalidParams)?;
                    let prompt = prompt_manager
                        .get_prompt(&params.name, params.arguments)
                        .await?;
                    Ok(serde_json::to_value(prompt).unwrap())
                })
            }),
        )
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

                                event_tx
                                    .send(TransportEvent::Message(response))
                                    .await
                                    .unwrap();
                            }
                        }
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
        let mut config = ServerConfig::default();
        config.server.host = "localhost".to_string();
        config.server.port = Some(8080);

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
        let (state_tx, _): &(watch::Sender<ServerState>, watch::Receiver<ServerState>) = &state;

        // Trigger shutdown
        state_tx.send(ServerState::ShuttingDown).unwrap();

        // Wait for server to shut down
        match tokio::time::timeout(Duration::from_secs(1), server_handle).await {
            Ok(result) => {
                assert!(result.unwrap().is_ok(), "Server should shut down cleanly");
            }
            Err(_) => panic!("Server did not shut down within timeout period"),
        }
    }

    #[tokio::test]
    async fn test_protocol_messages() {
        let mut config = ServerConfig::default();
        config.server.host = "localhost".to_string();
        config.server.port = Some(8080);

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
        let (state_tx, _): &(watch::Sender<ServerState>, watch::Receiver<ServerState>) = &state;
        state_tx.send(ServerState::ShuttingDown).unwrap();

        // Verify clean shutdown
        match tokio::time::timeout(Duration::from_secs(1), server_handle).await {
            Ok(result) => {
                assert!(result.unwrap().is_ok(), "Server should shut down cleanly");
            }
            Err(_) => panic!("Server did not shut down within timeout period"),
        }
    }
}
