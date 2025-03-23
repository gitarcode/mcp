use async_trait::async_trait;
use serde_json::{json, Value};
use std::{collections::HashMap, sync::Arc};

use crate::{
    error::McpError,
    protocol::{RequestHandler, ServerCapabilities},
};

use super::{CallToolArgs, Tool, ToolContent, ToolInputSchema, ToolProvider, ToolResult};

// Domain types
#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
enum Operation {
    // Basic arithmetic
    Add,
    Subtract,
    Multiply,
    Divide,
    // Advanced operations
    Power,
    Root,
    Modulo,
    // Scientific operations
    Log,
    Ln,
    // Trigonometric operations
    Sin,
    Cos,
    Tan,
}

#[derive(Debug, serde::Deserialize)]
struct CalculatorParams {
    operation: Operation,
    a: f64,
    b: Option<f64>, // Some operations like ln() only need one parameter
}

// Domain error types
#[derive(Debug, thiserror::Error)]
pub enum CalculatorError {
    #[error("Division by zero")]
    DivisionByZero,
    #[error("Invalid input: {0}")]
    InvalidInput(String),
    #[error("Operation failed: {0}")]
    OperationFailed(String),
}

// Calculator service implementation
struct CalculatorService;

impl CalculatorService {
    fn new() -> Self {
        Self
    }

    fn calculate(&self, params: &CalculatorParams) -> Result<f64, CalculatorError> {
        match params.operation {
            // Basic arithmetic
            Operation::Add => Ok(params.a
                + params.b.ok_or_else(|| {
                    CalculatorError::InvalidInput("Second operand required".to_string())
                })?),
            Operation::Subtract => Ok(params.a
                - params.b.ok_or_else(|| {
                    CalculatorError::InvalidInput("Second operand required".to_string())
                })?),
            Operation::Multiply => Ok(params.a
                * params.b.ok_or_else(|| {
                    CalculatorError::InvalidInput("Second operand required".to_string())
                })?),
            Operation::Divide => {
                let b = params.b.ok_or_else(|| {
                    CalculatorError::InvalidInput("Second operand required".to_string())
                })?;
                if b == 0.0 {
                    Err(CalculatorError::DivisionByZero)
                } else {
                    Ok(params.a / b)
                }
            }
            // Advanced operations
            Operation::Power => {
                Ok(params.a.powf(params.b.ok_or_else(|| {
                    CalculatorError::InvalidInput("Exponent required".to_string())
                })?))
            }
            Operation::Root => {
                let b = params.b.ok_or_else(|| {
                    CalculatorError::InvalidInput("Root degree required".to_string())
                })?;
                if b == 0.0 {
                    Err(CalculatorError::InvalidInput(
                        "Root degree cannot be zero".to_string(),
                    ))
                } else {
                    Ok(params.a.powf(1.0 / b))
                }
            }
            Operation::Modulo => {
                let b = params
                    .b
                    .ok_or_else(|| CalculatorError::InvalidInput("Modulus required".to_string()))?;
                if b == 0.0 {
                    Err(CalculatorError::DivisionByZero)
                } else {
                    Ok(params.a % b)
                }
            }
            // Scientific operations
            Operation::Log => {
                let base = params.b.unwrap_or(10.0);
                if params.a <= 0.0 {
                    Err(CalculatorError::InvalidInput(
                        "Logarithm argument must be positive".to_string(),
                    ))
                } else if base <= 0.0 || base == 1.0 {
                    Err(CalculatorError::InvalidInput(
                        "Invalid logarithm base".to_string(),
                    ))
                } else {
                    Ok(params.a.log(base))
                }
            }
            Operation::Ln => {
                if params.a <= 0.0 {
                    Err(CalculatorError::InvalidInput(
                        "Natural logarithm argument must be positive".to_string(),
                    ))
                } else {
                    Ok(params.a.ln())
                }
            }
            // Trigonometric operations
            Operation::Sin => Ok(params.a.sin()),
            Operation::Cos => Ok(params.a.cos()),
            Operation::Tan => Ok(params.a.tan()),
        }
    }
}

// Tool provider implementation
pub struct CalculatorTool {
    service: Arc<CalculatorService>,
}

impl Default for CalculatorTool {
    fn default() -> Self {
        Self::new()
    }
}

impl CalculatorTool {
    pub fn new() -> Self {
        Self {
            service: Arc::new(CalculatorService::new()),
        }
    }
}

#[async_trait]
impl ToolProvider for CalculatorTool {
    async fn get_tool(&self) -> Tool {
        let mut schema_properties = HashMap::new();
        schema_properties.insert(
            "operation".to_string(),
            json!({
                "type": "string",
                "enum": [
                    "add", "subtract", "multiply", "divide",
                    "power", "root", "modulo",
                    "log", "ln",
                    "sin", "cos", "tan"
                ]
            }),
        );

        schema_properties.insert(
            "a".to_string(),
            json!({
                "type": "number",
                "description": "First operand or primary value for single-argument operations"
            }),
        );

        schema_properties.insert(
            "b".to_string(),
            json!({
                "type": "number",
                "description": "Second operand (optional for some operations)"
            }),
        );

        Tool {
            name: "calculator".to_string(),
            description: "Advanced calculator supporting arithmetic, scientific, and trigonometric operations".to_string(),
            input_schema: ToolInputSchema {
                schema_type: "object".to_string(),
                properties: schema_properties,
                required: vec!["operation".to_string(), "a".to_string()],
            },
        }
    }

    async fn execute(
        &self,
        arguments: Value,
        _metadata: Option<CallToolArgs>,
    ) -> Result<ToolResult, McpError> {
        let params: CalculatorParams = serde_json::from_value(arguments).map_err(|e| {
            tracing::error!("Error parsing calculator arguments: {:?}", e);
            McpError::InvalidParams
        })?;

        match self.service.calculate(&params) {
            Ok(result) => Ok(ToolResult {
                content: vec![ToolContent::Text {
                    text: result.to_string(),
                }],
                is_error: false,
                _meta: None,
            }),
            Err(error) => Ok(ToolResult {
                content: vec![ToolContent::Text {
                    text: error.to_string(),
                }],
                is_error: true,
                _meta: None,
            }),
        }
    }
}

pub struct CalculatorHandler {
    calculator: CalculatorService,
}

#[async_trait]
impl RequestHandler for CalculatorHandler {
    async fn handle_request(&self, method: &str, params: Option<Value>) -> Result<Value, McpError> {
        match method {
            "calculate" => {
                let params: CalculatorParams =
                    serde_json::from_value(params.ok_or(McpError::InvalidParams)?)?;
                let result = self.calculator.calculate(&params)?;
                Ok(json!({"result": result}))
            }
            _ => Err(McpError::MethodNotFound),
        }
    }

    async fn handle_notification(
        &self,
        _method: &str,
        _params: Option<Value>,
    ) -> Result<(), McpError> {
        Ok(()) // Calculator doesn't handle any notifications
    }

    fn get_capabilities(&self) -> ServerCapabilities {
        ServerCapabilities {
            name: "Calculator".to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            protocol_version: "1.0".to_string(),
            capabilities: vec!["calculate".to_string()],
        }
    }
}

impl From<CalculatorError> for McpError {
    fn from(err: CalculatorError) -> Self {
        McpError::ToolExecutionError(err.to_string())
    }
}

// Additional test cases
#[cfg(test)]
mod tests {
    use crate::{
        protocol::BasicRequestHandler,
        server::{config::ServerConfig, McpServer},
        tools::ToolContent,
    };

    use super::*;

    #[tokio::test]
    async fn test_advanced_operations() {
        let config = ServerConfig::default();
        let handler = BasicRequestHandler::new("Calculator".to_string(), "1.0".to_string());
        let server = McpServer::new(config, handler);
        let tool_provider = Arc::new(CalculatorTool::new());
        server.tool_manager.register_tool(tool_provider).await;

        // Test power operation
        let result = server
            .tool_manager
            .call_tool(
                "calculator",
                json!({
                    "operation": "power",
                    "a": 2.0,
                    "b": 3.0
                }),
                Some(
                    CallToolArgs::builder()
                        .session_id(Some("session-1234".to_string()))
                        .tool_id(Some("calculator-1234".to_string()))
                        .build(),
                ),
            )
            .await
            .unwrap();

        match &result.content[0] {
            ToolContent::Text { text } => assert_eq!(text, "8"),
            _ => panic!("Expected text content"),
        }

        // Test natural logarithm
        let result = server
            .tool_manager
            .call_tool(
                "calculator",
                json!({
                    "operation": "ln",
                    "a": 2.718281828459045
                }),
                Some(
                    CallToolArgs::builder()
                        .session_id(Some("session-5678".to_string()))
                        .tool_id(Some("calculator-5678".to_string()))
                        .build(),
                ),
            )
            .await
            .unwrap();

        match &result.content[0] {
            ToolContent::Text { text } => {
                let value: f64 = text.parse().unwrap();
                assert!((value - 1.0).abs() < 1e-10);
            }
            _ => panic!("Expected text content"),
        }
    }

    #[tokio::test]
    async fn test_error_handling() {
        let config = ServerConfig::default();
        let handler = BasicRequestHandler::new("Calculator".to_string(), "1.0".to_string());
        let server = McpServer::new(config, handler);
        let tool_provider = Arc::new(CalculatorTool::new());
        server.tool_manager.register_tool(tool_provider).await;

        // Test negative logarithm
        let result = server
            .tool_manager
            .call_tool(
                "calculator",
                json!({
                    "operation": "log",
                    "a": -1.0,
                    "b": 10.0
                }),
                Some(
                    CallToolArgs::builder()
                        .session_id(Some("session-9999".to_string()))
                        .tool_id(Some("calculator-9999".to_string()))
                        .build(),
                ),
            )
            .await
            .unwrap();

        assert!(result.is_error);
        match &result.content[0] {
            ToolContent::Text { text } => {
                assert!(text.contains("must be positive"));
            }
            _ => panic!("Expected text content"),
        }
    }
}
