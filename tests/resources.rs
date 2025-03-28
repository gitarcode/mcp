use std::env;

use mcp_rs::{
    error::McpError,
    protocol::BasicRequestHandler,
    resource::FileSystemProvider,
    server::{
        config::{ResourceSettings, ServerConfig},
        McpServer,
    },
};

#[tokio::test]
async fn test_resource_loading_integration() -> Result<(), McpError> {
    let root_dir = env::current_dir().unwrap();

    // Create test server config
    let config = ServerConfig {
        resources: Some(ResourceSettings {
            root_path: root_dir.join("tests/resources/test"),
            allowed_schemes: vec!["file".to_string()],
            max_file_size: 10 * 1024 * 1024,
            enable_templates: true,
        }),
        ..ServerConfig::default()
    };

    // Initialize server
    let handler = BasicRequestHandler::new("test-server".to_string(), "0.1.0".to_string());
    let server = McpServer::new(config, handler);

    // Register file system provider
    let fs_provider = std::sync::Arc::new(FileSystemProvider::new(
        &server.config.resources.as_ref().unwrap().root_path,
    ));
    server
        .resource_manager
        .register_provider("file".to_string(), fs_provider)
        .await;

    // Test listing resources
    let list_result = server.resource_manager.list_resources(None).await?;
    assert!(
        !list_result.resources.is_empty(),
        "Should find test resources"
    );

    // Find hello.txt
    let hello_resource = list_result
        .resources
        .iter()
        .find(|r| r.name == "hello.txt")
        .expect("Should find hello.txt");

    // Read hello.txt content
    let read_result = server
        .resource_manager
        .read_resource(&hello_resource.uri)
        .await?;
    assert_eq!(read_result.contents.len(), 1);

    if let Some(text) = &read_result.contents[0].text {
        assert!(text.contains("Hello from the Model Context Protocol!"));
    } else {
        panic!("Expected text content");
    }

    // Test JSON resource
    let json_resource = list_result
        .resources
        .iter()
        .find(|r| r.name == "data.json")
        .expect("Should find data.json");

    let json_result = server
        .resource_manager
        .read_resource(&json_resource.uri)
        .await?;
    assert_eq!(json_result.contents.len(), 1);

    println!("JSON result: {:?}", json_result);

    if let Some(text) = &json_result.contents[0].text {
        let json: serde_json::Value = serde_json::from_str(text).unwrap();
        assert_eq!(json["message"], "Test JSON data");
    } else {
        panic!("Expected JSON content");
    }

    Ok(())
}

#[tokio::test]
async fn test_resource_templates() -> Result<(), McpError> {
    let root_dir = env::current_dir().unwrap();

    // Create test server config
    let config = ServerConfig {
        resources: Some(ResourceSettings {
            root_path: root_dir.join("tests/resources/test"),
            allowed_schemes: vec!["file".to_string()],
            max_file_size: 10 * 1024 * 1024,
            enable_templates: true,
        }),
        ..ServerConfig::default()
    };

    // Initialize server
    let handler = BasicRequestHandler::new("test-server".to_string(), "0.1.0".to_string());
    let server = McpServer::new(config, handler);

    // Register file system provider
    let fs_provider = std::sync::Arc::new(FileSystemProvider::new(
        &server.config.resources.as_ref().unwrap().root_path,
    ));
    server
        .resource_manager
        .register_provider("file".to_string(), fs_provider)
        .await;

    // Test listing templates
    let templates = server.resource_manager.list_templates().await?;
    assert!(
        !templates.resource_templates.is_empty(),
        "Should have default templates"
    );

    Ok(())
}

#[tokio::test]
async fn test_resource_errors() -> Result<(), McpError> {
    let root_dir = env::current_dir().unwrap();

    // Create test server config
    let config = ServerConfig {
        resources: Some(ResourceSettings {
            root_path: root_dir.join("tests/resources/test"),
            allowed_schemes: vec!["file".to_string()],
            max_file_size: 10 * 1024 * 1024,
            enable_templates: true,
        }),
        ..ServerConfig::default()
    };

    // Initialize server
    let handler = BasicRequestHandler::new("test-server".to_string(), "0.1.0".to_string());
    let server = McpServer::new(config, handler);

    // Register file system provider
    let fs_provider = std::sync::Arc::new(FileSystemProvider::new(
        &server.config.resources.as_ref().unwrap().root_path,
    ));
    server
        .resource_manager
        .register_provider("file".to_string(), fs_provider)
        .await;

    // Test non-existent resource
    let result = server
        .resource_manager
        .read_resource("file://nonexistent.txt")
        .await;
    assert!(result.is_err());
    match result {
        Err(McpError::ResourceNotFound(_)) => (),
        _ => panic!("Expected ResourceNotFound error"),
    }

    // Test path traversal attempt
    let result = server
        .resource_manager
        .read_resource("file://../secret.txt")
        .await;
    assert!(result.is_err());
    match result {
        Err(McpError::AccessDenied(_)) => (),
        _ => panic!("Expected AccessDenied error"),
    }

    Ok(())
}
