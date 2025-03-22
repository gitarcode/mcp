use async_trait::async_trait;

use crate::error::McpError;

use super::{CallToolArgs, Tool, ToolContent, ToolProvider, ToolResult};

pub struct TestTool;

impl Default for TestTool {
    fn default() -> Self {
        Self::new()
    }
}

impl TestTool {
    pub fn new() -> Self {
        TestTool
    }
}

#[async_trait]
impl ToolProvider for TestTool {
    async fn get_tool(&self) -> Tool {
        Tool {
            name: "test_tool".to_string(),
            description: "Test Tool".to_string(),
            input_schema: serde_json::from_str(
                r#"{
                "type": "object",
                "properties": {
                    "test": {
                        "type": "string",
                        "description": "Test property"
                    }
                },
                "required": ["test"]
            }"#,
            )
            .unwrap(),
        }
    }

    async fn execute(&self, _arguments: CallToolArgs) -> Result<ToolResult, McpError> {
        Ok(ToolResult {
            content: vec![],
            is_error: false,
            _meta: None,
        })
    }
}

pub struct PingTool;

impl Default for PingTool {
    fn default() -> Self {
        Self::new()
    }
}

impl PingTool {
    pub fn new() -> Self {
        PingTool
    }
}

#[async_trait]
impl ToolProvider for PingTool {
    async fn get_tool(&self) -> Tool {
        Tool {
            name: "ping_tool".to_string(),
            description: "Ping Tool".to_string(),
            input_schema: serde_json::from_str(
                r#"{
                "type": "object",
                "properties": {
                    "server": {
                        "type": "string",
                        "description": "Server to ping"
                    }
                },
                "required": ["server"]
            }"#,
            )
            .unwrap(),
        }
    }

    async fn execute(&self, arguments: CallToolArgs) -> Result<ToolResult, McpError> {
        let server = arguments
            .arguments
            .get("server")
            .and_then(|s| s.as_str())
            .unwrap_or("localhost");
        let res = reqwest::get(format!("http://{}", server))
            .await
            .map_err(|e| McpError::ToolExecutionError(e.to_string()))?;
        let body = res
            .text()
            .await
            .map_err(|e| McpError::ToolExecutionError(e.to_string()))?;
        Ok(ToolResult {
            content: vec![ToolContent::Text { text: body }],
            is_error: false,
            _meta: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        protocol::{RequestHandler, ServerCapabilities},
        server::{config::ServerConfig, McpServer},
    };
    use serde_json::{json, Value};

    struct TestHandler;

    #[async_trait]
    impl RequestHandler for TestHandler {
        async fn handle_request(
            &self,
            _method: &str,
            _params: Option<Value>,
        ) -> Result<Value, McpError> {
            Ok(json!({"success": true}))
        }

        async fn handle_notification(
            &self,
            _method: &str,
            _params: Option<Value>,
        ) -> Result<(), McpError> {
            Ok(())
        }

        fn get_capabilities(&self) -> ServerCapabilities {
            ServerCapabilities {
                name: "Test".to_string(),
                version: "1.0".to_string(),
                protocol_version: "1.0".to_string(),
                capabilities: vec!["test".to_string()],
            }
        }
    }

    #[tokio::test]
    async fn test_custom_handler() {
        let handler = TestHandler;
        let server = McpServer::new(ServerConfig::default(), handler);
        let response = server
            .process_request("test", Some(json!({})))
            .await
            .unwrap();
        assert_eq!(response, json!({"success": true}));
    }
}
