#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[tokio::test]
    async fn test_basic_request_handler() {
        let handler = BasicRequestHandler::new(
            "test_server".to_string(),
            "1.0.0".to_string(),
        );

        // Test server_info request
        let result = handler.handle_request("server_info", None).await.unwrap();
        let caps = serde_json::from_value::<ServerCapabilities>(result).unwrap();
        
        assert_eq!(caps.name, "test_server");
        assert_eq!(caps.version, "1.0.0");
        assert_eq!(caps.protocol_version, "0.1.0");
        assert!(caps.capabilities.contains(&"server_info".to_string()));
        
        // Test invalid method
        let err = handler.handle_request("invalid_method", None).await.unwrap_err();
        assert!(matches!(err, McpError::MethodNotFound));
    }

    #[tokio::test]
    async fn test_custom_request_handler() {
        struct CustomHandler {
            basic: BasicRequestHandler,
        }

        #[async_trait]
        impl RequestHandler for CustomHandler {
            async fn handle_request(&self, method: &str, params: Option<Value>) -> Result<Value, McpError> {
                match method {
                    "custom_method" => Ok(json!({ "result": "custom" })),
                    _ => self.basic.handle_request(method, params).await,
                }
            }

            async fn handle_notification(&self, method: &str, params: Option<Value>) -> Result<(), McpError> {
                self.basic.handle_notification(method, params).await
            }

            fn get_capabilities(&self) -> ServerCapabilities {
                let mut caps = self.basic.get_capabilities();
                caps.capabilities.push("custom_method".to_string());
                caps
            }
        }

        let handler = CustomHandler {
            basic: BasicRequestHandler::new("custom_server".to_string(), "1.0.0".to_string()),
        };

        // Test custom method
        let result = handler.handle_request("custom_method", None).await.unwrap();
        assert_eq!(result, json!({ "result": "custom" }));

        // Verify capabilities include custom method
        let caps = handler.get_capabilities();
        assert!(caps.capabilities.contains(&"custom_method".to_string()));
    }
} 