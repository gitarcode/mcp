use crate::{
    error::McpError,
    logging::LoggingCapabilities,
    prompts::{
        GetPromptRequest, ListPromptsRequest, ListPromptsResponse, PromptCapabilities, PromptResult,
    },
    protocol::{Protocol, ProtocolHandle, ProtocolOptions},
    resource::{
        ListResourcesRequest, ListResourcesResponse, ReadResourceRequest, ReadResourceResponse,
        ResourceCapabilities,
    },
    tools::{CallToolRequest, ListToolsRequest, ListToolsResponse, ToolCapabilities, ToolResult},
    transport::{Transport, TransportCommand},
};
use serde::{Deserialize, Serialize};
use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::Duration,
};
use tokio::sync::RwLock;

// Client capabilities and info structs
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
pub struct ClientCapabilities {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub roots: Option<RootsCapabilities>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sampling: Option<SamplingCapabilities>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClientInfo {
    pub name: String,
    pub version: String,
}

// Server response structs
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerCapabilities {
    pub logging: Option<LoggingCapabilities>,
    pub prompts: Option<PromptCapabilities>,
    pub resources: Option<ResourceCapabilities>,
    pub tools: Option<ToolCapabilities>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerInfo {
    pub name: String,
    pub version: String,
}

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
    pub server_info: ServerInfo,
}

pub struct Client {
    protocol: Protocol,
    initialized: Arc<RwLock<bool>>,
    server_capabilities: Arc<RwLock<Option<ServerCapabilities>>>,
}

impl Default for Client {
    fn default() -> Self {
        Self::new()
    }
}

impl Client {
    pub fn new() -> Self {
        Self {
            protocol: Protocol::builder(Some(ProtocolOptions {
                enforce_strict_capabilities: true,
            }))
            .build(),
            initialized: Arc::new(RwLock::new(false)),
            server_capabilities: Arc::new(RwLock::new(None)),
        }
    }

    pub async fn connect<T: Transport>(
        &mut self,
        transport: T,
    ) -> Result<ProtocolHandle, McpError> {
        let timeout = Duration::from_secs(30);
        match tokio::time::timeout(timeout, self.protocol.connect(transport)).await {
            Ok(result) => result,
            Err(_) => Err(McpError::ConnectionClosed),
        }
    }

    pub async fn initialize(
        &mut self,
        client_info: ClientInfo,
    ) -> Result<InitializeResult, McpError> {
        // Ensure we're not already initialized
        if *self.initialized.read().await {
            return Err(McpError::InvalidRequest(
                "Client already initialized".to_string(),
            ));
        }

        // Prepare initialization parameters
        let params = InitializeParams {
            protocol_version: "2024-11-05".to_string(),
            capabilities: ClientCapabilities {
                roots: Some(RootsCapabilities { list_changed: true }),
                sampling: Some(SamplingCapabilities {}),
            },
            client_info,
        };

        // Send initialize request
        let result: InitializeResult = self
            .protocol
            .request("initialize", Some(params), None)
            .await?;

        // Validate protocol version
        if result.protocol_version != "2024-11-05" {
            return Err(McpError::InvalidRequest(format!(
                "Unsupported protocol version: {}",
                result.protocol_version
            )));
        }

        // Store server capabilities
        *self.server_capabilities.write().await = Some(result.capabilities.clone());

        // Send initialized notification
        self.protocol
            .notification("initialized", Option::<()>::None)
            .await?;

        // Mark as initialized
        *self.initialized.write().await = true;

        Ok(result)
    }

    // Resource methods
    pub async fn list_resources(
        &self,
        cursor: Option<String>,
    ) -> Result<ListResourcesResponse, McpError> {
        self.assert_initialized().await?;
        self.assert_capability("resources").await?;

        self.protocol
            .request(
                "resources/list",
                Some(ListResourcesRequest { cursor }),
                None,
            )
            .await
    }

    pub async fn read_resource(&self, uri: String) -> Result<ReadResourceResponse, McpError> {
        self.assert_initialized().await?;
        self.assert_capability("resources").await?;

        self.protocol
            .request("resources/read", Some(ReadResourceRequest { uri }), None)
            .await
    }

    pub async fn subscribe_to_resource(&self, uri: String) -> Result<(), McpError> {
        self.assert_initialized().await?;
        self.assert_capability("resources").await?;

        self.protocol
            .request("resources/subscribe", Some(uri), None)
            .await
    }

    // Prompt methods
    pub async fn list_prompts(
        &self,
        cursor: Option<String>,
    ) -> Result<ListPromptsResponse, McpError> {
        self.assert_initialized().await?;
        self.assert_capability("prompts").await?;

        self.protocol
            .request("prompts/list", Some(ListPromptsRequest { cursor }), None)
            .await
    }

    pub async fn get_prompt(
        &self,
        name: String,
        arguments: Option<serde_json::Value>,
    ) -> Result<PromptResult, McpError> {
        self.assert_initialized().await?;
        self.assert_capability("prompts").await?;

        self.protocol
            .request(
                "prompts/get",
                Some(GetPromptRequest { name, arguments }),
                None,
            )
            .await
    }

    // Tool methods
    pub async fn list_tools(&self, cursor: Option<String>) -> Result<ListToolsResponse, McpError> {
        self.assert_initialized().await?;
        self.assert_capability("tools").await?;

        self.protocol
            .request("tools/list", Some(ListToolsRequest { cursor }), None)
            .await
    }

    pub async fn call_tool(
        &self,
        name: String,
        arguments: serde_json::Value,
        tool_id: Option<String>,
    ) -> Result<ToolResult, McpError> {
        self.assert_initialized().await?;
        self.assert_capability("tools").await?;

        self.protocol
            .request(
                "tools/call",
                Some(CallToolRequest {
                    name,
                    arguments,
                    tool_id,
                }),
                None,
            )
            .await
    }

    // Logging methods
    pub async fn set_log_level(&self, level: String) -> Result<(), McpError> {
        self.assert_initialized().await?;
        self.assert_capability("logging").await?;

        self.protocol
            .request(
                "logging/setLevel",
                Some(serde_json::json!({ "level": level })),
                None,
            )
            .await
    }

    /// Waits for the server to acknowledge shutdown request
    async fn wait_for_shutdown(&mut self) -> Result<(), McpError> {
        let shutdown_ack = Arc::new(AtomicBool::new(false));

        // Register shutdown handler
        {
            let mut handlers = self.protocol.notification_handlers.write().await;
            let ack = shutdown_ack.clone();
            handlers.insert(
                "shutdown/ack".to_string(),
                Box::new(move |_notification| {
                    let ack = ack.clone();
                    Box::pin(async move {
                        ack.store(true, Ordering::SeqCst);
                        Ok(())
                    })
                }),
            );
        };

        // Send shutdown notification
        self.protocol.notification("shutdown", None::<()>).await?;

        // Wait for acknowledgment
        let mut attempts = 0;
        while !shutdown_ack.load(Ordering::SeqCst) {
            if attempts >= 50 {
                // 5 seconds with 100ms sleep
                return Err(McpError::ShutdownError(
                    "No shutdown acknowledgment received".into(),
                ));
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
            attempts += 1;
        }

        // Clean up handler
        self.protocol
            .notification_handlers
            .write()
            .await
            .remove("shutdown/ack");

        // Clean up resources
        self.cleanup_resources().await?;

        Ok(())
    }

    /// Performs graceful shutdown of the client
    pub async fn shutdown(&mut self) -> Result<(), McpError> {
        // Only attempt shutdown if we're initialized
        if !*self.initialized.read().await {
            return Ok(());
        }

        tracing::debug!("Starting client shutdown sequence");

        // Set client state to shutting down
        *self.initialized.write().await = false;

        // Wait for shutdown with timeout
        match tokio::time::timeout(Duration::from_secs(5), self.wait_for_shutdown()).await {
            Ok(result) => {
                tracing::debug!("Client shutdown completed successfully");
                result
            }
            Err(_) => {
                tracing::warn!("Client shutdown timed out");
                // Force cleanup on timeout
                self.cleanup_resources().await?;
                Err(McpError::ShutdownTimeout)
            }
        }
    }

    /// Cleans up client resources
    async fn cleanup_resources(&mut self) -> Result<(), McpError> {
        tracing::debug!("Cleaning up client resources");

        // Close transport
        if let Some(cmd_tx) = &self.protocol.cmd_tx {
            let _ = cmd_tx.send(TransportCommand::Close).await;
            self.protocol.cmd_tx = None;
        }

        // Clear handlers
        self.protocol.notification_handlers.write().await.clear();
        self.protocol.request_handlers.write().await.clear();
        self.protocol.response_handlers.write().await.clear();
        self.protocol.progress_handlers.write().await.clear();

        // Clear capabilities
        *self.server_capabilities.write().await = None;

        Ok(())
    }

    pub async fn assert_initialized(&self) -> Result<(), McpError> {
        if !*self.initialized.read().await {
            return Err(McpError::InvalidRequest(
                "Client not initialized".to_string(),
            ));
        }
        Ok(())
    }

    async fn assert_capability(&self, capability: &str) -> Result<(), McpError> {
        let caps = self.server_capabilities.read().await;
        let caps = caps
            .as_ref()
            .ok_or_else(|| McpError::InvalidRequest("No server capabilities".to_string()))?;

        let has_capability = match capability {
            "logging" => caps.logging.is_some(),
            "prompts" => caps.prompts.is_some(),
            "resources" => caps.resources.is_some(),
            "tools" => caps.tools.is_some(),
            _ => false,
        };

        if !has_capability {
            return Err(McpError::CapabilityNotSupported(capability.to_string()));
        }

        Ok(())
    }

    pub async fn get_server_capabilities(&self) -> Option<ServerCapabilities> {
        self.server_capabilities.read().await.clone()
    }

    // Helper method to check if server supports a capability
    pub async fn has_capability(&self, capability: &str) -> bool {
        if let Some(caps) = self.get_server_capabilities().await {
            match capability {
                "logging" => caps.logging.is_some(),
                "prompts" => caps.prompts.is_some(),
                "resources" => caps.resources.is_some(),
                "tools" => caps.tools.is_some(),
                _ => false,
            }
        } else {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transport::stdio::StdioTransport;
    use std::time::Duration;
    use tokio::io::{self, AsyncBufReadExt, AsyncWriteExt, BufReader};

    #[tokio::test]
    async fn test_client_lifecycle() -> Result<(), McpError> {
        tokio::time::timeout(Duration::from_secs(5), async {
            // Create mock stdio streams
            let (input_rx, mut input_tx) = io::duplex(64);
            let (output_rx, output_tx) = io::duplex(64);

            // Spawn a task to simulate server responses first
            let server_task = tokio::spawn(async move {
                let mut buf = String::new();
                let mut reader = BufReader::new(output_rx);
                if (reader.read_line(&mut buf).await).is_ok() {
                    let ack = r#"{"jsonrpc":"2.0","method":"shutdown/ack","params":null}"#;
                    let _ = input_tx.write_all(ack.as_bytes()).await;
                    let _ = input_tx.write_all(b"\n").await;
                }
            });

            let mut client = Client::new();

            // Connect using stdio transport with mock streams
            let transport = StdioTransport::with_streams(BufReader::new(input_rx), output_tx, None);
            client.connect(transport).await?;

            // Send shutdown command
            client.shutdown().await?;

            // Wait for server task
            let _ = server_task.await;

            Ok(())
        })
        .await
        .map_err(|_| McpError::ShutdownTimeout)?
    }
}
