use clap::{Parser, Subcommand};
use mcp_rs::{
    client::{Client, ClientInfo},
    error::McpError,
    transport::sse::SseTransport,
    transport::stdio::StdioTransport,
    transport::ws::WebSocketTransport,
};
use serde_json::json;

#[derive(Parser, Debug)]
#[command(name = "mcp-client", version, about = "MCP Client CLI")]
struct Cli {
    /// Server URL for SSE or WebSocket transport
    #[arg(short, long)]
    server: Option<String>,

    /// Transport type (stdio, sse, ws)
    #[arg(short, long, default_value = "stdio")] // Changed default to stdio
    transport: String,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// List available resources
    ListResources {
        #[arg(short, long)]
        cursor: Option<String>,
    },
    /// Read a resource
    ReadResource {
        #[arg(short, long)]
        uri: String,
    },
    /// List resource templates
    //ListTemplates,
    /// Subscribe to resource changes
    Subscribe {
        #[arg(short, long)]
        uri: String,
    },
    /// List available prompts
    ListPrompts {
        #[arg(short, long)]
        cursor: Option<String>,
    },
    /// Get a prompt
    GetPrompt {
        #[arg(short, long)]
        name: String,
        #[arg(short, long)]
        args: Option<String>,
    },
    /// List available tools
    ListTools {
        #[arg(short, long)]
        cursor: Option<String>,
    },
    /// Call a tool
    CallTool {
        #[arg(short, long)]
        name: String,
        #[arg(short, long)]
        args: String,
    },
    /// Set log level
    SetLogLevel {
        #[arg(short, long)]
        level: String,
    },
}

#[tokio::main]
async fn main() -> Result<(), McpError> {
    // Parse command line arguments
    let args = Cli::parse();

    // Set up logging
    tracing_subscriber::fmt().init();

    // Create and initialize client
    let mut client = Client::new();

    // Set up transport with better error handling
    match args.transport.as_str() {
        "stdio" => {
            let transport = StdioTransport::new(None);
            tracing::info!("Connecting using stdio transport...");
            match client.connect(transport).await {
                Ok(_) => tracing::info!("Successfully connected to server"),
                Err(e) => {
                    tracing::error!("Failed to connect: {}", e);
                    return Err(e);
                }
            }

            tracing::info!("Initializing client...");
            match client
                .initialize(ClientInfo {
                    name: "mcp-cli".to_string(),
                    version: env!("CARGO_PKG_VERSION").to_string(),
                })
                .await
            {
                Ok(result) => {
                    tracing::info!("Server info: {:?}", result.server_info);
                    tracing::info!("Server capabilities: {:?}", result.capabilities);
                }
                Err(e) => {
                    tracing::error!("Failed to initialize: {}", e);
                    return Err(e);
                }
            }
        }
        "sse" => {
            let server_url = args.server.as_ref().ok_or_else(|| {
                McpError::InvalidRequest("Server URL is required for SSE transport".to_string())
            })?;
            
            // Parse server URL to get host and port
            let url_parts: Vec<&str> = server_url.split(':').collect();
            if url_parts.len() != 2 {
                return Err(McpError::InvalidRequest(
                    "Server URL must be in format host:port for SSE transport".to_string(),
                ));
            }
            
            let host = url_parts[0].to_string();
            let port = url_parts[1].parse::<u16>().map_err(|_| {
                McpError::InvalidRequest("Invalid port number".to_string())
            })?;
            
            tracing::info!("Connecting to {}:{} using SSE transport...", host, port);
            let transport = SseTransport::new_client(host.clone(), port, 1024);
            match client.connect(transport).await {
                Ok(_) => tracing::info!("Successfully connected to server"),
                Err(e) => {
                    tracing::error!("Failed to connect: {}", e);
                    return Err(e);
                }
            }

            tracing::info!("Initializing client...");
            match client
                .initialize(ClientInfo {
                    name: "mcp-cli".to_string(),
                    version: env!("CARGO_PKG_VERSION").to_string(),
                })
                .await
            {
                Ok(result) => {
                    tracing::info!("Server info: {:?}", result.server_info);
                    tracing::info!("Server capabilities: {:?}", result.capabilities);
                }
                Err(e) => {
                    tracing::error!("Failed to initialize: {}", e);
                    return Err(e);
                }
            }
        }
        "ws" => {
            let server_url = args.server.as_ref().ok_or_else(|| {
                McpError::InvalidRequest("Server URL is required for WebSocket transport".to_string())
            })?;
            
            let host_port: Vec<&str> = server_url.split(':').collect();
            if host_port.len() != 2 {
                return Err(McpError::InvalidRequest(
                    "Server URL must be in format host:port for WebSocket transport".to_string(),
                ));
            }
            
            let host = host_port[0].to_string();
            let port = host_port[1].parse::<u16>().map_err(|_| {
                McpError::InvalidRequest("Invalid port number".to_string())
            })?;
            
            tracing::info!("Connecting to {}:{} using WebSocket transport with 'mcp' subprotocol...", host, port);
            let transport = WebSocketTransport::new_client(host.clone(), port, 1024);
            
            match client.connect(transport).await {
                Ok(_) => tracing::info!("Successfully connected to server"),
                Err(e) => {
                    tracing::error!("Failed to connect: {}", e);
                    return Err(e);
                }
            }

            tracing::info!("Initializing client...");
            match client
                .initialize(ClientInfo {
                    name: "mcp-cli".to_string(),
                    version: env!("CARGO_PKG_VERSION").to_string(),
                })
                .await
            {
                Ok(result) => {
                    tracing::info!("Server info: {:?}", result.server_info);
                    tracing::info!("Server capabilities: {:?}", result.capabilities);
                }
                Err(e) => {
                    tracing::error!("Failed to initialize: {}", e);
                    return Err(e);
                }
            }
        }
        _ => {
            return Err(McpError::InvalidRequest(format!(
                "Unsupported transport type: {}",
                args.transport
            )));
        }
    }

    // Initialize with better error handling and debugging
    tracing::debug!("Sending initialize request...");
    let _init_result = match tokio::time::timeout(
        std::time::Duration::from_secs(30), // Increased from 5 to 30 seconds
        client.initialize(ClientInfo {
            name: "mcp-cli".to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
        }),
    )
    .await
    {
        Ok(Ok(result)) => {
            tracing::info!("Connected to server: {:?}", result.server_info);
            result
        }
        Ok(Err(e)) => {
            tracing::error!("Failed to initialize: {}", e);
            return Err(e);
        }
        Err(_) => {
            tracing::error!("Initialize request timed out");
            return Err(McpError::RequestTimeout);
        }
    };

    // Execute command
    match args.command {
        Commands::ListResources { cursor } => {
            let res = client.list_resources(cursor).await?;
            println!("{}", json!(res));
        }

        Commands::ReadResource { uri } => {
            let res = client.read_resource(uri).await?;
            println!("{}", json!(res));
        }
        Commands::Subscribe { uri } => {
            client.subscribe_to_resource(uri).await?;
            println!("{}", json!(()));
        }

        Commands::ListPrompts { cursor } => {
            let res = client.list_prompts(cursor).await?;
            println!("{}", json!(res));
        }

        Commands::GetPrompt { name, args } => {
            let arguments = if let Some(args_str) = args {
                Some(
                    serde_json::from_str(&args_str)
                        .map_err(|e| McpError::InvalidRequest(e.to_string()))?,
                )
            } else {
                None
            };
            let res = client.get_prompt(name, arguments).await?;
            println!("{}", json!(res));
        }

        Commands::ListTools { cursor } => {
            let res = client.list_tools(cursor).await?;
            println!("{}", json!(res));
        }

        Commands::CallTool { name, args } => {
            let arguments =
                serde_json::from_str(&args).map_err(|e| McpError::InvalidRequest(e.to_string()))?;
            let res = client.call_tool(name, arguments).await?;
            println!("{}", json!(res));
        }

        Commands::SetLogLevel { level } => client.set_log_level(level).await?,
    };

    // Remove the Ctrl+C wait for stdio transport
    if args.transport == "sse" {
        tracing::info!("Client connected. Press Ctrl+C to exit...");
        tokio::signal::ctrl_c().await?;
    }

    // Shutdown client
    client.shutdown().await?;

    Ok(())
}
