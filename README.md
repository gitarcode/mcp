# mcp.rs

A Rust implementation of the Model Context Protocol (MCP), providing a standardized way for AI models to access external context and resources.

## Quickstart 

### Client
```
# List resources
cargo run --bin client list-resources

# Read a specific file
cargo run --bin client read-resource -u "file:///path/to/file"

# Use a prompt
cargo run --bin client get-prompt -n "code_review" -a '{"code": "fn main() {}", "language": "rust"}'

# Call a tool
cargo run --bin client -- --server "http://127.0.0.1:3000" call-tool --name "file_system" --args '{\"operation\": \"read_file\", \"path\": \"Config.toml\"}'

# Set log level
cargo run --bin client set-log-level -l "debug"

# Use SSE transport
cargo run --bin client -t sse -s http://localhost:3000 list-resources
```

### Server
```
# Run with test config
cargo run --bin server -- --config "../servers/test.json"
```

## Overview

mcp.rs is a high-performance, type-safe Rust implementation of the Model Context Protocol, designed to enable seamless communication between AI applications and their integrations. It provides a robust foundation for building MCP servers that can expose various types of resources (files, data, APIs) to AI models.

## Features

- **Multiple Transport Types**:
  - Standard Input/Output (stdio) transport for CLI tools
  - HTTP with Server-Sent Events (SSE) for web integrations
  - Extensible transport system for custom implementations

- **Resource Management**:
  - File system resource provider
  - Resource templating support
  - Real-time resource updates
  - Resource subscription capabilities

- **Flexible Configuration**:
  - YAML/JSON configuration files
  - Environment variable overrides
  - Command-line arguments
  - Sensible defaults

- **Security**:
  - Built-in access controls
  - Path traversal protection
  - Rate limiting
  - CORS support

## Installation

Add mcp.rs to your project's `Cargo.toml`:

```toml
[dependencies]
mcp = "0.1.0"
```

## Quick Start

1. Create a basic MCP server:

```rust
use mcp::{
    server::{McpServer, ServerConfig},
    protocol::BasicRequestHandler
};

#[tokio::main]
async fn main() -> Result<(), mcp::error::McpError> {
    // Create server with default configuration
    let config = ServerConfig::default();
    let handler = BasicRequestHandler::new("my-server".to_string(), "1.0.0".to_string());
    let mut server = McpServer::new(config, handler);
    
    // Run the server
    server.run_stdio_transport().await
}
```

2. Configure via command line:

```bash
# Run with stdio transport
mcp-server -t stdio

# Run with SSE transport on port 3000
mcp-server -t sse -p 3000

# Enable debug logging
mcp-server -l debug
```

3. Or use a configuration file:

```yaml
server:
  name: "my-mcp-server"
  version: "1.0.0"
  transport: sse
  port: 3000

resources:
  root_path: "./resources"
  allowed_schemes:
    - file
  max_file_size: 10485760

security:
  enable_auth: false
  allowed_origins:
    - "*"

logging:
  level: "info"
  format: "pretty"
```

## Architecture

mcp.rs follows a modular architecture:

- **Transport Layer**: Handles communication between clients and servers
- **Protocol Layer**: Implements the MCP message format and routing
- **Resource Layer**: Manages access to external resources
- **Configuration**: Handles server settings and capabilities

## Configuration Options

| Option | Description | Default |
|--------|-------------|---------|
| `transport` | Transport type (stdio, sse) | stdio |
| `port` | Server port for network transports | 3000 |
| `log_level` | Logging level | info |
| `resource_root` | Root directory for resources | ./resources |

## API Reference

For detailed API documentation, run:
```bash
cargo doc --open
```

## Examples (TODO)

Check out the `/examples` directory for:
- Basic server implementation
- Custom resource provider
- Configuration examples
- Integration patterns

## Contributing

Contributions are welcome! Please read our [Contributing Guidelines](CONTRIBUTING.md) before submitting pull requests.

### Development Requirements

- Rust 1.70 or higher
- Cargo
- Optional: Docker for containerized testing

### Running Tests

```bash
# Run all tests
cargo test

# Run with logging
RUST_LOG=debug cargo test
```
## License

This project is licensed under the [MIT License](LICENSE).

## Acknowledgments

This implementation is based on the Model Context Protocol specification and inspired by the reference implementation.

## Contact

- Issue Tracker: [GitHub Issues](https://github.com/EmilLindfors/mcp/issues)
- Source Code: [GitHub Repository](https://github.com/EmilLindfors/mcp)

## Using as a Library

### Core Server Components

#### McpServer

The main server struct that handles MCP protocol communication.

```rust
pub struct McpServer<H: RequestHandler> {
    pub config: ServerConfig,
    pub handler: Arc<H>,
    pub resource_manager: Arc<ResourceManager>,
    pub tool_manager: Arc<ToolManager>,
    pub prompt_manager: Arc<PromptManager>,
    pub logging_manager: Arc<tokio::sync::Mutex<LoggingManager>>,
    // ... internal fields
}

impl<H: RequestHandler> McpServer<H> {
    // Create a new server instance with a request handler
    pub fn new(config: ServerConfig, handler: H) -> Self;
    
    // Start server with stdio transport
    pub async fn run_stdio_transport(&mut self) -> Result<(), McpError>;
    
    // Start server with SSE transport
    pub async fn run_sse_transport(&mut self) -> Result<(), McpError>;
}
```

#### ServerConfig

Configuration struct for the MCP server.

```rust
pub struct ServerConfig {
    pub server: ServerSettings,
    pub resources: ResourceSettings,
    pub security: SecuritySettings,
    pub logging: LoggingSettings,
    pub tool_settings: ToolSettings,
    pub tools: Vec<ToolType>,
    pub prompts: Vec<Prompt>,
}

// Server-specific settings
pub struct ServerSettings {
    pub name: String,
    pub version: String,
    pub transport: TransportType,
    pub host: String,
    pub port: u16,
    pub max_connections: usize,
    pub timeout_ms: u64,
}

// Resource settings
pub struct ResourceSettings {
    pub root_path: PathBuf,
    pub allowed_schemes: Vec<String>,
    pub max_file_size: usize,
    pub enable_templates: bool,
}

// Security configuration
pub struct SecuritySettings {
    pub enable_auth: bool,
    pub token_secret: Option<String>,
    pub rate_limit: RateLimitSettings,
    pub allowed_origins: Vec<String>,
}

pub struct RateLimitSettings {
    pub requests_per_minute: u32,
    pub burst_size: u32,
}

// Logging configuration
pub struct LoggingSettings {
    pub level: String,
    pub file: Option<PathBuf>,
    pub format: LogFormat,
}

// Tool settings
pub struct ToolSettings {
    pub enabled: bool,
    pub require_confirmation: bool,
    pub allowed_tools: Vec<String>,
    pub max_execution_time_ms: u64,
    pub rate_limit: RateLimitSettings,
}

// Transport type enum
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TransportType {
    Stdio,
    Sse,
    WebSocket,
}

// Log format enum
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LogFormat {
    Json,
    Pretty,
    Compact,
}
```

### Tool Implementation

To implement a custom tool:

```rust
use async_trait::async_trait;
use serde_json::Value;

#[async_trait]
pub trait ToolProvider: Send + Sync {
    // Return tool definition
    async fn get_tool(&self) -> Tool;
    
    // Execute tool with given arguments
    async fn execute(&self, arguments: Value) -> Result<ToolResult, McpError>;
}

// Tool definition struct
pub struct Tool {
    pub name: String,
    pub description: String,
    pub input_schema: ToolInputSchema,
}

// Tool execution result
pub struct ToolResult {
    pub content: Vec<ToolContent>,
    pub is_error: bool,
}
```

### Resource Implementation

Resources provide access to files and other data sources:

```rust
pub struct ResourceManager {
    pub capabilities: ResourceCapabilities,
    // ... internal fields
}

impl ResourceManager {
    // Register a new resource provider
    pub async fn register_provider(
        &self,
        scheme: String,
        provider: Arc<dyn ResourceProvider>
    );
    
    // List available resources
    pub async fn list_resources(
        &self,
        cursor: Option<String>
    ) -> Result<ListResourcesResponse, McpError>;
    
    // Read resource content
    pub async fn read_resource(
        &self,
        uri: &str
    ) -> Result<ReadResourceResponse, McpError>;
}
```

### Prompt Implementation

Prompts allow defining reusable text templates:

```rust
pub struct PromptManager {
    pub capabilities: PromptCapabilities,
    // ... internal fields
}

impl PromptManager {
    // Register a new prompt
    pub async fn register_prompt(&self, prompt: Prompt);
    
    // List available prompts
    pub async fn list_prompts(
        &self,
        cursor: Option<String>
    ) -> Result<ListPromptsResponse, McpError>;
    
    // Get prompt with arguments
    pub async fn get_prompt(
        &self,
        name: &str,
        arguments: Option<Value>
    ) -> Result<PromptResult, McpError>;
}
```

### Example Usage

Basic example of creating and running an MCP server:

```rust
use mcp_rs::{
    server::{McpServer, ServerConfig},
    protocol::BasicRequestHandler,
    tools::ToolType,
};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Create default config
    let mut config = ServerConfig::default();
    
    // Customize config
    config.server.name = "my-mcp-server".to_string();
    config.server.version = "1.0.0".to_string();
    
    // Add tools
    config.tools.push(ToolType::Calculator);
    
    // Create request handler
    let handler = BasicRequestHandler::new(
        config.server.name.clone(),
        config.server.version.clone()
    );
    
    // Create and run server
    let mut server = McpServer::new(config, handler);
    server.run_stdio_transport().await?;
    
    Ok(())
}
```

For more detailed examples, see the `examples/` directory and the test files.

### Custom Transport Implementation

The MCP server can work with any transport layer that implements the `Transport` trait:

```rust
#[async_trait]
pub trait Transport: Send + Sync {
    // Receive incoming messages
    async fn receive(&mut self) -> Result<JsonRpcMessage, McpError>;
    
    // Send outgoing messages
    async fn send(&mut self, message: JsonRpcMessage) -> Result<(), McpError>;
    
    // Handle transport-specific commands
    async fn handle_command(&mut self, command: TransportCommand) -> Result<(), McpError>;
    
    // Close the transport
    async fn close(&mut self) -> Result<(), McpError>;
}

// Message types that transports must handle
#[derive(Debug, Serialize, Deserialize)]
pub struct JsonRpcMessage {
    pub jsonrpc: String,
    pub id: Option<Value>,
    pub method: Option<String>,
    pub params: Option<Value>,
    pub result: Option<Value>,
    pub error: Option<JsonRpcError>,
}

// Commands that transports must handle
#[derive(Debug)]
pub enum TransportCommand {
    Close,
    // Add custom commands as needed
}
```

Example of connecting a custom transport:

```rust
use mcp_rs::{
    server::{McpServer, ServerConfig},
    transport::{Transport, JsonRpcMessage, TransportCommand},
    error::McpError,
};

struct MyCustomTransport {
    // Your transport-specific fields
}

#[async_trait]
impl Transport for MyCustomTransport {
    async fn receive(&mut self) -> Result<JsonRpcMessage, McpError> {
        // Implement message receiving logic
    }
    
    async fn send(&mut self, message: JsonRpcMessage) -> Result<(), McpError> {
        // Implement message sending logic
    }
    
    async fn handle_command(&mut self, command: TransportCommand) -> Result<(), McpError> {
        match command {
            TransportCommand::Close => {
                // Handle close command
                Ok(())
            }
        }
    }
    
    async fn close(&mut self) -> Result<(), McpError> {
        // Implement cleanup logic
        Ok(())
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut config = ServerConfig::default();
    let mut server = McpServer::new(config).await;
    
    // Create and connect custom transport
    let transport = MyCustomTransport { /* ... */ };
    let protocol = server.register_protocol_handlers(
        Protocol::builder(Some(ProtocolOptions {
            enforce_strict_capabilities: true,
        }))
    ).build();
    
    let protocol_handle = protocol.connect(transport).await?;
    
    // Handle shutdown gracefully
    tokio::signal::ctrl_c().await?;
    protocol_handle.close().await?;
    
    Ok(())
}
```

The custom transport can use any underlying communication mechanism (sockets, pipes, shared memory, etc.) as long as it properly serializes and deserializes the MCP protocol messages.

## Transport Protocols

### Unix Domain Sockets Transport

The Unix transport uses Unix domain sockets for communication between client and server. Messages are exchanged using a line-based protocol where each JSON-RPC message is sent as a complete line terminated by a newline character (`\n`).

#### Protocol Details

- **Socket Location**: By default, the socket file is created at the specified path
- **Message Format**: Line-delimited JSON-RPC messages
- **Message Framing**: Each message must be a complete JSON object on a single line, terminated by `\n`
- **Encoding**: UTF-8

#### Server Mode

```rust
use mcp::transport::UnixTransport;

// Create server transport
let transport = UnixTransport::new_server(
    PathBuf::from("/tmp/mcp.sock"), // Socket path
    Some(4092)                      // Optional buffer size
);
```

The server:
1. Creates the Unix domain socket at the specified path
2. Accepts a single client connection
3. Removes the socket file when shutting down

#### Client Mode

```rust
use mcp::transport::UnixTransport;

// Create client transport
let transport = UnixTransport::new_client(
    PathBuf::from("/tmp/mcp.sock"), // Socket path
    Some(4092)                      // Optional buffer size
);
```

The client:
1. Connects to the existing Unix domain socket
2. Maintains a single persistent connection
3. Automatically handles reconnection on connection loss

#### Message Exchange

Messages are sent as complete JSON-RPC objects, one per line:

```json
{"jsonrpc": "2.0", "method": "list_resources", "params": {}, "id": 1}\n
{"jsonrpc": "2.0", "result": {"resources": []}, "id": 1}\n
```

#### Command Line Usage

Start server with Unix transport:
```bash
cargo run --bin server -- -t unix -s /tmp/mcp.sock
```

Connect client using Unix transport:
```bash
cargo run --bin client -- -t unix -s /tmp/mcp.sock list-resources
```

#### Error Handling

- Connection failures return `McpError::IoError`
- Invalid JSON messages return `McpError::ParseError`
- Socket permission errors return `McpError::IoError`

#### Security Considerations

- Socket files have standard Unix file permissions
- Only one client can connect at a time
- Socket file is automatically removed on server shutdown
- Path traversal protection is enforced
- Socket files should be created in appropriate directories with correct permissions

## Implementing Custom Request Handlers

The MCP server allows customizing how different requests are handled through the `RequestHandler` trait. Here's how to implement custom handling:

### Basic Request Handler

```rust
use mcp::{
    protocol::{RequestHandler, JsonRpcRequest, JsonRpcResponse},
    error::McpError,
};

struct MyRequestHandler {
    // Your handler's state
    resource_path: PathBuf,
}

#[async_trait]
impl RequestHandler for MyRequestHandler {
    async fn handle_request(&self, request: JsonRpcRequest) -> Result<JsonRpcResponse, McpError> {
        match request.method.as_str() {
            // Handle resource listing
            "list_resources" => {
                let resources = self.list_my_resources().await?;
                Ok(JsonRpcResponse::success(request.id, resources))
            },
            
            // Handle resource reading
            "read_resource" => {
                let uri = request.params.get("uri")
                    .ok_or(McpError::InvalidParams)?;
                let content = self.read_my_resource(uri).await?;
                Ok(JsonRpcResponse::success(request.id, content))
            },
            
            // Unknown method
            _ => Ok(JsonRpcResponse::error(
                request.id,
                McpError::MethodNotFound.into()
            ))
        }
    }
}

impl MyRequestHandler {
    async fn list_my_resources(&self) -> Result<Vec<Resource>, McpError> {
        // Your resource listing implementation
    }

    async fn read_my_resource(&self, uri: &str) -> Result<ResourceContent, McpError> {
        // Your resource reading implementation
    }
}
```

### Registering the Handler

```rust
use mcp::{McpServer, ServerConfig};

#[tokio::main]
async fn main() -> Result<(), McpError> {
    let config = ServerConfig::default();
    let mut server = McpServer::new(config).await;
    
    // Create and register your handler
    let handler = MyRequestHandler {
        resource_path: PathBuf::from("/path/to/resources"),
    };
    server.set_request_handler(handler);
    
    // Run the server
    server.run().await
}
```

### Handling Specific Methods

You can implement handlers for the standard MCP methods:

```rust
impl MyRequestHandler {
    // List available resources
    async fn handle_list_resources(&self, params: Value) -> Result<Value, McpError> {
        let resources = vec![
            Resource {
                uri: "file:///example.txt".into(),
                metadata: ResourceMetadata {
                    name: "Example File".into(),
                    size: 1024,
                    ..Default::default()
                }
            }
        ];
        Ok(serde_json::to_value(resources)?)
    }

    // Read resource content
    async fn handle_read_resource(&self, params: Value) -> Result<Value, McpError> {
        let uri = params.get("uri")
            .and_then(Value::as_str)
            .ok_or(McpError::InvalidParams)?;
            
        let content = tokio::fs::read_to_string(uri).await
            .map_err(|_| McpError::ResourceNotFound)?;
            
        Ok(serde_json::json!({
            "content": content,
            "metadata": {
                "type": "text/plain"
            }
        }))
    }

    // Handle tool execution
    async fn handle_execute_tool(&self, params: Value) -> Result<Value, McpError> {
        let tool_name = params.get("name")
            .and_then(Value::as_str)
            .ok_or(McpError::InvalidParams)?;
            
        match tool_name {
            "my_tool" => {
                // Tool implementation
                Ok(serde_json::json!({ "result": "success" }))
            }
            _ => Err(McpError::ToolNotFound)
        }
    }
}
```

### Error Handling

The handler can return standard MCP errors or custom errors:

```rust
#[derive(Debug, thiserror::Error)]
pub enum MyHandlerError {
    #[error("Resource quota exceeded")]
    QuotaExceeded,
    
    #[error("Invalid resource format: {0}")]
    InvalidFormat(String),
}

impl From<MyHandlerError> for McpError {
    fn from(err: MyHandlerError) -> Self {
        match err {
            MyHandlerError::QuotaExceeded => 
                McpError::Custom { 
                    code: -32000, 
                    message: err.to_string() 
                },
            MyHandlerError::InvalidFormat(msg) => 
                McpError::Custom { 
                    code: -32001, 
                    message: format!("Format error: {}", msg)
                },
        }
    }
}
```

### Middleware Support

You can also add middleware for cross-cutting concerns:

```rust
struct LoggingMiddleware<H>(H);

#[async_trait]
impl<H: RequestHandler> RequestHandler for LoggingMiddleware<H> {
    async fn handle_request(&self, request: JsonRpcRequest) -> Result<JsonRpcResponse, McpError> {
        tracing::info!("Handling request: {:?}", request);
        let result = self.0.handle_request(request).await;
        tracing::info!("Request result: {:?}", result);
        result
    }
}

// Usage
let handler = LoggingMiddleware(MyRequestHandler { ... });
server.set_request_handler(handler);
```

This allows you to customize every aspect of how the MCP server handles requests while maintaining protocol compatibility.

### Core MCP Protocol Handlers

A complete MCP server implementation should handle these core capabilities:

```rust
use mcp::{
    protocol::{RequestHandler, JsonRpcRequest, JsonRpcResponse},
    resource::{Resource, ResourceContent, ResourceMetadata},
    tools::{Tool, ToolResult},
    prompts::{Prompt, PromptResult},
    error::McpError,
};

struct MyMcpHandler {
    // Handler state
    resources: Arc<ResourceManager>,
    tools: Arc<ToolManager>,
    prompts: Arc<PromptManager>,
}

#[async_trait]
impl RequestHandler for MyMcpHandler {
    async fn handle_request(&self, request: JsonRpcRequest) -> Result<JsonRpcResponse, McpError> {
        match request.method.as_str() {
            // Resource Methods
            "list_resources" => {
                let resources = self.list_resources(request.params).await?;
                Ok(JsonRpcResponse::success(request.id, resources))
            }
            "read_resource" => {
                let content = self.read_resource(request.params).await?;
                Ok(JsonRpcResponse::success(request.id, content))
            }
            "watch_resources" => {
                let subscription = self.watch_resources(request.params).await?;
                Ok(JsonRpcResponse::success(request.id, subscription))
            }

            // Tool Methods
            "list_tools" => {
                let tools = self.list_tools(request.params).await?;
                Ok(JsonRpcResponse::success(request.id, tools))
            }
            "execute_tool" => {
                let result = self.execute_tool(request.params).await?;
                Ok(JsonRpcResponse::success(request.id, result))
            }

            // Prompt Methods
            "list_prompts" => {
                let prompts = self.list_prompts(request.params).await?;
                Ok(JsonRpcResponse::success(request.id, prompts))
            }
            "get_prompt" => {
                let prompt = self.get_prompt(request.params).await?;
                Ok(JsonRpcResponse::success(request.id, prompt))
            }

            // Server Info/Capabilities
            "server_info" => {
                let info = self.get_server_info().await?;
                Ok(JsonRpcResponse::success(request.id, info))
            }
            "get_capabilities" => {
                let caps = self.get_capabilities().await?;
                Ok(JsonRpcResponse::success(request.id, caps))
            }

            _ => Ok(JsonRpcResponse::error(
                request.id,
                McpError::MethodNotFound.into()
            ))
        }
    }

    // Optional: Handle notifications
    async fn handle_notification(&self, notification: JsonRpcNotification) -> Result<(), McpError> {
        match notification.method.as_str() {
            "resource_changed" => {
                self.handle_resource_change(notification.params).await?;
                Ok(())
            }
            _ => Ok(())
        }
    }
}

// Resource Management
impl MyMcpHandler {
    async fn list_resources(&self, params: Option<Value>) -> Result<Vec<Resource>, McpError> {
        // List available resources with optional filtering
        let filter = params.and_then(|p| p.get("filter"))
            .and_then(Value::as_str);
            
        let resources = match filter {
            Some("code") => self.resources.list_code_files().await?,
            Some("data") => self.resources.list_data_files().await?,
            _ => self.resources.list_all().await?,
        };
        
        Ok(resources)
    }

    async fn read_resource(&self, params: Option<Value>) -> Result<ResourceContent, McpError> {
        let uri = params
            .and_then(|p| p.get("uri"))
            .and_then(Value::as_str)
            .ok_or(McpError::InvalidParams)?;

        self.resources.read(uri).await
    }

    async fn watch_resources(&self, params: Option<Value>) -> Result<String, McpError> {
        // Set up resource watching and return subscription ID
        let patterns = params
            .and_then(|p| p.get("patterns"))
            .and_then(Value::as_array)
            .ok_or(McpError::InvalidParams)?;

        self.resources.create_watch(patterns).await
    }
}

// Tool Management 
impl MyMcpHandler {
    async fn list_tools(&self, _params: Option<Value>) -> Result<Vec<Tool>, McpError> {
        Ok(vec![
            Tool {
                name: "code_analysis".into(),
                description: "Analyzes code structure and quality".into(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "path": {"type": "string"},
                        "language": {"type": "string"}
                    }
                }),
            },
            // Other available tools...
        ])
    }

    async fn execute_tool(&self, params: Option<Value>) -> Result<ToolResult, McpError> {
        let tool_name = params
            .and_then(|p| p.get("name"))
            .and_then(Value::as_str)
            .ok_or(McpError::InvalidParams)?;

        let tool_args = params
            .and_then(|p| p.get("arguments"))
            .ok_or(McpError::InvalidParams)?;

        match tool_name {
            "code_analysis" => self.tools.analyze_code(tool_args).await,
            "data_processor" => self.tools.process_data(tool_args).await,
            _ => Err(McpError::ToolNotFound)
        }
    }
}

// Prompt Management
impl MyMcpHandler {
    async fn list_prompts(&self, _params: Option<Value>) -> Result<Vec<Prompt>, McpError> {
        Ok(vec![
            Prompt {
                name: "code_review".into(),
                description: "Reviews code changes".into(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "code": {"type": "string"},
                        "language": {"type": "string"}
                    }
                }),
                template: "Review this {language} code:\n\n{code}".into(),
            },
            // Other available prompts...
        ])
    }

    async fn get_prompt(&self, params: Option<Value>) -> Result<PromptResult, McpError> {
        let name = params
            .and_then(|p| p.get("name"))
            .and_then(Value::as_str)
            .ok_or(McpError::InvalidParams)?;

        let args = params
            .and_then(|p| p.get("arguments"))
            .unwrap_or(&json!({}));

        match name {
            "code_review" => self.prompts.render_code_review(args).await,
            "data_analysis" => self.prompts.render_data_analysis(args).await,
            _ => Err(McpError::PromptNotFound)
        }
    }
}

// Server Information
impl MyMcpHandler {
    async fn get_server_info(&self) -> Result<Value, McpError> {
        Ok(json!({
            "name": "My MCP Server",
            "version": "1.0.0",
            "description": "Custom MCP implementation"
        }))
    }

    async fn get_capabilities(&self) -> Result<Value, McpError> {
        Ok(json!({
            "resources": {
                "supports_watching": true,
                "supported_schemes": ["file", "http"],
                "max_file_size": 10485760
            },
            "tools": {
                "supports_async": true,
                "supports_streaming": false
            },
            "prompts": {
                "supports_templates": true,
                "supports_streaming": false
            }
        }))
    }
}
```

This implementation shows how to handle all core MCP protocol methods, including:
- Resource listing, reading, and watching
- Tool listing and execution
- Prompt listing and rendering
- Server information and capabilities
- Notification handling

Each method can be customized to provide the specific functionality needed by your server while maintaining protocol compatibility.
