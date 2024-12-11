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
    pub protocol_version: String,
    pub capabilities: ServerCapabilities,
    pub server_info: ServerInfo,
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
    state: Arc<RwLock<ServerState>>,
    supported_versions: Vec<String>,
    client_capabilities: Arc<RwLock<Option<ClientCapabilities>>>,
}

impl<H> McpServer<H>
where
    H: RequestHandler + Send + Sync + 'static
{
    pub fn new(config: ServerConfig, handler: H) -> Self {
        let (notification_tx, notification_rx) = mpsc::channel(100);
        
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
            state: Arc::new(RwLock::new(ServerState::Created)),
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
        let transport = UnixTransport::new_server(
            PathBuf::from(&self.config.server.host),
            Some(self.config.server.port as usize)
        );
        self.run_transport(transport).await
    }

    async fn run_transport<T: Transport>(&mut self, transport: T) -> Result<(), McpError> {
        // Transport handling implementation
        Ok(())
    }
}
