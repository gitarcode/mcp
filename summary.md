# mcp.rs Summary

**mcp.rs** is a high-performance, type-safe Rust implementation of the Model Context Protocol (MCP) for enabling seamless communication between AI applications and their integrations.

## Key Features

- **Multiple Transport Types**: stdio, HTTP with SSE, extensible transport system
- **Resource Management**: File system resources, templating, real-time updates, subscriptions
- **Security**: Access controls, path traversal protection, rate limiting, CORS support
- **Flexible Configuration**: YAML/JSON files, environment variables, CLI arguments

## Quick Commands

### Client Usage
```bash
# List resources
cargo run --bin client list-resources

# Read file
cargo run --bin client read-resource -u "file:///path/to/file"

# Use prompt
cargo run --bin client get-prompt -n "code_review" -a '{"code": "fn main() {}", "language": "rust"}'

# Call tool
cargo run --bin client call-tool --name "file_system" --args '{"operation": "read_file", "path": "Config.toml"}'
```

### Server Usage
```bash
# Run server with config
cargo run --bin server -- --config "../servers/test.json"

# Run with stdio transport
mcp-server -t stdio

# Run with SSE on port 3000
mcp-server -t sse -p 3000
```

## Architecture

- **Transport Layer**: Handles client-server communication
- **Protocol Layer**: Implements MCP message format and routing
- **Resource Layer**: Manages external resource access
- **Configuration**: Server settings and capabilities

## Installation

Add to `Cargo.toml`:
```toml
[dependencies]
mcp = "0.1.0"
```

## Development

- **Requirements**: Rust 1.70+, Cargo
- **Testing**: `cargo test`
- **Documentation**: `cargo doc --open`

## Core Components

- **McpServer**: Main server handling MCP protocol
- **ServerConfig**: Configuration management
- **RequestHandler**: Custom protocol handling
- **ResourceManager**: Resource access and management
- **ToolManager**: Tool execution
- **PromptManager**: Prompt templating

## License

MIT License