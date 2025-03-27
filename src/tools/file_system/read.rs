use async_trait::async_trait;
use futures::future::try_join_all;
use serde_json::{json, Value};
use std::collections::HashMap;
use tokio::fs;

use crate::{
    error::McpError,
    tools::{CallToolArgs, Tool, ToolContent, ToolInputSchema, ToolProvider, ToolResult},
};

pub struct ReadFileTool;

impl ReadFileTool {
    pub fn new() -> Self {
        Self
    }

    async fn read_single_file(path: &str) -> Result<String, McpError> {
        fs::read_to_string(path).await.map_err(|e| {
            tracing::error!("Failed to read file {}: {}", path, e);
            McpError::IoError
        })
    }

    async fn read_multiple_files(
        paths: &[String],
    ) -> Result<Vec<(String, Result<String, McpError>)>, McpError> {
        let futures: Vec<_> = paths
            .iter()
            .map(|path| {
                let path = path.clone();
                async move {
                    let result = Self::read_single_file(&path).await;
                    Ok((path, result))
                }
            })
            .collect();

        try_join_all(futures).await
    }
}

#[async_trait]
impl ToolProvider for ReadFileTool {
    async fn get_tool(&self) -> Tool {
        let mut schema_properties = HashMap::new();
        schema_properties.insert(
            "operation".to_string(),
            json!({
                "type": "string",
                "enum": ["read_file", "read_multiple_files"]
            }),
        );
        schema_properties.insert(
            "path".to_string(),
            json!({
                "type": "string",
                "description": "Path to the file to read"
            }),
        );
        schema_properties.insert(
            "paths".to_string(),
            json!({
                "type": "array",
                "items": {
                    "type": "string"
                },
                "description": "List of file paths to read"
            }),
        );

        Tool {
            name: "read_file".to_string(),
            description: "Read the complete contents of one or more files from the file system. \
                Handles various text encodings and provides detailed error messages."
                .to_string(),
            input_schema: ToolInputSchema {
                schema_type: "object".to_string(),
                properties: Some(schema_properties),
                required: Some(vec!["operation".to_string()]),
            },
        }
    }

    async fn execute(
        &self,
        arguments: Value,
        _metadata: Option<CallToolArgs>,
    ) -> Result<ToolResult, McpError> {
        match arguments["operation"].as_str() {
            Some("read_file") => {
                let path = arguments["path"].as_str().ok_or(McpError::InvalidParams)?;
                let content = Self::read_single_file(path).await?;

                Ok(ToolResult {
                    content: vec![ToolContent::Text { text: content }],
                    is_error: false,
                    _meta: None,
                })
            }
            Some("read_multiple_files") => {
                let paths = arguments["paths"]
                    .as_array()
                    .ok_or(McpError::InvalidParams)?
                    .iter()
                    .filter_map(|p| p.as_str().map(String::from))
                    .collect::<Vec<_>>();

                let results = Self::read_multiple_files(&paths).await?;
                let mut contents = Vec::new();

                for (path, result) in results {
                    match result {
                        Ok(content) => contents.push(ToolContent::Text {
                            text: format!("File: {}\n{}", path, content),
                        }),
                        Err(e) => contents.push(ToolContent::Text {
                            text: format!("Error reading {}: {}", path, e),
                        }),
                    }
                }

                Ok(ToolResult {
                    content: contents,
                    is_error: false,
                    _meta: None,
                })
            }
            _ => Err(McpError::InvalidParams),
        }
    }
}
