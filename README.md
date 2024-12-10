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
use mcp::{McpServer, ServerConfig};

#[tokio::main]
async fn main() -> Result<(), mcp::error::McpError> {
    // Create server with default configuration
    let server = McpServer::new(ServerConfig::default());
    
    // Run the server
    server.run().await
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
pub struct McpServer {
    pub config: ServerConfig,
    pub resource_manager: Arc<ResourceManager>,
    pub tool_manager: Arc<ToolManager>,
    pub prompt_manager: Arc<PromptManager>,
    pub logging_manager: Arc<tokio::sync::Mutex<LoggingManager>>,
    // ... internal fields
}

impl McpServer {
    // Create a new server instance
    pub async fn new(config: ServerConfig) -> Self;
    
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
    
    // Create and run server
    let mut server = McpServer::new(config).await;
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
