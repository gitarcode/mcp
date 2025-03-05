use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{collections::HashMap, sync::Arc};
use test_tool::{PingTool, TestTool};
use tokio::sync::RwLock;

pub mod calculator;
pub mod file_system;
pub mod test_tool;

use crate::error::McpError;

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
}

// Tool Provider trait
#[async_trait]
pub trait ToolProvider: Send + Sync {
    /// Get tool definition
    async fn get_tool(&self) -> Tool;

    /// Execute tool
    async fn execute(&self, arguments: Value) -> Result<ToolResult, McpError>;
}

// Tool Manager
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolCapabilities {
    pub list_changed: bool,
}

pub struct ToolManager {
    pub tools: Arc<RwLock<HashMap<String, Arc<dyn ToolProvider>>>>,
    pub capabilities: ToolCapabilities,
}

impl ToolManager {
    pub fn new(capabilities: ToolCapabilities) -> Self {
        Self {
            tools: Arc::new(RwLock::new(HashMap::new())),
            capabilities,
        }
    }

    pub async fn register_tool(&self, provider: Arc<dyn ToolProvider>) {
        let tool = provider.get_tool().await;
        let mut tools = self.tools.write().await;
        tools.insert(tool.name, provider);
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

    pub async fn call_tool(&self, name: &str, arguments: Value) -> Result<ToolResult, McpError> {
        let tools = self.tools.read().await;
        let provider = tools
            .get(name)
            .ok_or_else(|| McpError::InvalidRequest(format!("Unknown tool: {}", name)))?;

        provider.execute(arguments).await
    }
}
