use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{collections::HashMap, sync::Arc};
use test_tool::{PingTool, TestTool};
use tokio::sync::{mpsc, RwLock};
use typed_builder::TypedBuilder;

pub mod calculator;
pub mod file_system;
pub mod test_tool;

use crate::error::McpError;
use crate::protocol::JsonRpcNotification;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolType {
    Calculator,
    TestTool,
    PingTool,
    FileSystem,
}

impl ToolType {
    pub fn to_tool_provider(&self) -> Arc<dyn ToolProvider> {
        match self {
            ToolType::Calculator => Arc::new(calculator::CalculatorTool::new()),
            ToolType::TestTool => Arc::new(TestTool::new()),
            ToolType::PingTool => Arc::new(PingTool::new()),
            ToolType::FileSystem => Arc::new(file_system::FileSystemTools::new()),
        }
    }
}

// Tool Types
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Tool {
    pub name: String,
    pub description: String,
    pub input_schema: ToolInputSchema,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolInputSchema {
    #[serde(rename = "type")]
    pub schema_type: String,
    pub properties: HashMap<String, Value>,
    pub required: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
#[serde(rename_all = "camelCase")]
pub enum ToolContent {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(rename = "image")]
    Image { data: String, mime_type: String },
    #[serde(rename = "resource")]
    Resource { resource: ResourceContent },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResourceContent {
    pub uri: String,
    pub mime_type: Option<String>,
    pub text: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolResult {
    pub content: Vec<ToolContent>,
    pub is_error: bool,

    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(default)]
    #[serde(rename = "_meta")]
    pub _meta: Option<HashMap<String, Value>>,
}

// Request/Response types
#[derive(Debug, Deserialize, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ListToolsRequest {
    pub cursor: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListToolsResponse {
    pub tools: Vec<Tool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CallToolRequest {
    pub name: String,
    pub arguments: Value,
    pub tool_id: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, TypedBuilder)]
pub struct CallToolArgs {
    pub arguments: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[builder(default, setter(into))]
    pub tool_id: Option<String>,
}

// Tool Provider trait
#[async_trait]
pub trait ToolProvider: Send + Sync {
    /// Get tool definition
    async fn get_tool(&self) -> Tool;

    /// Execute tool
    async fn execute(&self, arguments: CallToolArgs) -> Result<ToolResult, McpError>;
}

// Tool Manager
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolCapabilities {
    pub list_changed: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolUpdateNotification {
    pub tool_name: String,
    pub update_type: ToolUpdateType,
    pub details: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolUpdateType {
    Added,
    Updated,
    Removed,
}

pub struct ToolManager {
    pub tools: Arc<RwLock<HashMap<String, Arc<dyn ToolProvider>>>>,
    pub capabilities: ToolCapabilities,
    notification_tx: Option<mpsc::Sender<JsonRpcNotification>>,
}

impl ToolManager {
    pub fn new(capabilities: ToolCapabilities) -> Self {
        Self {
            tools: Arc::new(RwLock::new(HashMap::new())),
            notification_tx: None,
            capabilities,
        }
    }

    pub fn with_notification_sender(
        capabilities: ToolCapabilities,
        notification_tx: mpsc::Sender<JsonRpcNotification>,
    ) -> Self {
        Self {
            tools: Arc::new(RwLock::new(HashMap::new())),
            notification_tx: Some(notification_tx),
            capabilities,
        }
    }

    pub async fn register_tool(&self, provider: Arc<dyn ToolProvider>) {
        let tool = provider.get_tool().await;
        let mut tools = self.tools.write().await;
        tools.insert(tool.name.clone(), provider);

        // Send notification if tool updates are enabled
        if self.capabilities.list_changed {
            self.send_tool_update_notification(
                &tool.name,
                ToolUpdateType::Added,
                Some(format!("Tool '{}' registered", tool.name)),
            )
            .await;
        }
    }

    pub async fn unregister_tool(&self, name: &str) -> Result<(), McpError> {
        let mut tools = self.tools.write().await;
        if tools.remove(name).is_some() {
            // Send notification if tool updates are enabled
            if self.capabilities.list_changed {
                self.send_tool_update_notification(
                    name,
                    ToolUpdateType::Removed,
                    Some(format!("Tool '{}' unregistered", name)),
                )
                .await;
            }
            Ok(())
        } else {
            Err(McpError::InvalidRequest(format!(
                "Tool '{}' not found",
                name
            )))
        }
    }

    pub async fn update_tool(&self, provider: Arc<dyn ToolProvider>) -> Result<(), McpError> {
        let tool = provider.get_tool().await;
        let mut tools = self.tools.write().await;

        if tools.contains_key(&tool.name) {
            tools.insert(tool.name.clone(), provider);

            // Send notification if tool updates are enabled
            if self.capabilities.list_changed {
                self.send_tool_update_notification(
                    &tool.name,
                    ToolUpdateType::Updated,
                    Some(format!("Tool '{}' updated", tool.name)),
                )
                .await;
            }
            Ok(())
        } else {
            Err(McpError::InvalidRequest(format!(
                "Tool '{}' not found",
                tool.name
            )))
        }
    }

    async fn send_tool_update_notification(
        &self,
        tool_name: &str,
        update_type: ToolUpdateType,
        details: Option<String>,
    ) {
        if let Some(tx) = &self.notification_tx {
            let notification = ToolUpdateNotification {
                tool_name: tool_name.to_string(),
                update_type,
                details,
            };

            let json_notification = JsonRpcNotification {
                jsonrpc: "2.0".to_string(),
                method: "tools/update".to_string(),
                params: Some(serde_json::to_value(notification).unwrap_or_default()),
            };

            if let Err(e) = tx.send(json_notification).await {
                tracing::error!("Failed to send tool update notification: {}", e);
            }
        }
    }

    pub async fn list_tools(&self, _cursor: Option<String>) -> Result<ListToolsResponse, McpError> {
        let tools = self.tools.read().await;
        let mut tool_list = Vec::new();

        for provider in tools.values() {
            tool_list.push(provider.get_tool().await);
        }

        Ok(ListToolsResponse {
            tools: tool_list,
            next_cursor: None, // Implement pagination if needed
        })
    }

    pub async fn call_tool(
        &self,
        name: &str,
        arguments: Value,
        tool_id: Option<String>,
    ) -> Result<ToolResult, McpError> {
        let tools = self.tools.read().await;
        let provider = tools
            .get(name)
            .ok_or_else(|| McpError::InvalidRequest(format!("Unknown tool: {}", name)))?;

        provider
            .execute(
                CallToolArgs::builder()
                    .arguments(arguments)
                    .tool_id(tool_id)
                    .build(),
            )
            .await
    }
}
