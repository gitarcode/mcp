use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use crate::{
    prompts::{Prompt, PromptCapabilities},
    resource::ResourceCapabilities,
    tools::{ToolCapabilities, ToolType},
};

// Server Configuration
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerConfig {
    pub server: ServerSettings,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resources: Option<ResourceSettings>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub security: Option<SecuritySettings>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub logging: Option<LoggingSettings>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_settings: Option<ToolSettings>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<ToolType>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub prompts: Vec<Prompt>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub capabilities: Option<ServerCapabilities>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerCapabilities {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resources: Option<ResourceCapabilities>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prompts: Option<PromptCapabilities>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<ToolCapabilities>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerSettings {
    pub name: String,
    pub version: String,
    pub transport: TransportType,
    pub host: String,
    pub port: u16,
    pub max_connections: usize,
    pub timeout_ms: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResourceSettings {
    pub root_path: PathBuf,
    pub allowed_schemes: Vec<String>,
    pub max_file_size: usize,
    pub enable_templates: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SecuritySettings {
    pub enable_auth: bool,
    pub token_secret: Option<String>,
    pub rate_limit: RateLimitSettings,
    pub allowed_origins: Vec<String>,
}

impl Default for SecuritySettings {
    fn default() -> Self {
        SecuritySettings {
            enable_auth: false,
            token_secret: None,
            rate_limit: RateLimitSettings {
                requests_per_minute: 60,
                burst_size: 10,
            },
            allowed_origins: vec!["*".to_string()],
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RateLimitSettings {
    pub requests_per_minute: u32,
    pub burst_size: u32,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LoggingSettings {
    pub level: String,
    pub file: Option<PathBuf>,
    pub format: LogFormat,
}

impl Default for LoggingSettings {
    fn default() -> Self {
        LoggingSettings {
            level: "info".to_string(),
            file: None,
            format: LogFormat::Pretty,
        }
    }
}

// Add new tool settings struct
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolSettings {
    pub enabled: bool,
    pub require_confirmation: bool,
    pub allowed_tools: Vec<String>,
    pub max_execution_time_ms: u64,
    pub rate_limit: RateLimitSettings,
}

impl Default for ToolSettings {
    fn default() -> Self {
        ToolSettings {
            enabled: true,
            require_confirmation: true,
            allowed_tools: vec!["*".to_string()], // Allow all tools by default
            max_execution_time_ms: 30000,         // 30 seconds
            rate_limit: RateLimitSettings {
                requests_per_minute: 30,
                burst_size: 5,
            },
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TransportType {
    Stdio,
    Sse,
    WebSocket,
    #[cfg(unix)]
    Unix,
}

impl From<&str> for TransportType {
    fn from(s: &str) -> Self {
        match s {
            "stdio" => TransportType::Stdio,
            "sse" => TransportType::Sse,
            "ws" => TransportType::WebSocket,
            #[cfg(unix)]
            "unix" => TransportType::Unix,
            _ => TransportType::Stdio,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LogFormat {
    Json,
    Pretty,
    Compact,
}

impl Default for ServerConfig {
    fn default() -> Self {
        ServerConfig {
            server: ServerSettings {
                name: "mcp-server".to_string(),
                version: env!("CARGO_PKG_VERSION").to_string(),
                transport: TransportType::Stdio,
                host: "127.0.0.1".to_string(),
                port: 3000,
                max_connections: 100,
                timeout_ms: 30000,
            },
            resources: None,
            security: None,
            logging: None,
            tool_settings: None,
            tools: vec![],
            prompts: vec![],
            capabilities: Some(ServerCapabilities {
                resources: Some(ResourceCapabilities {
                    list_changed: false,
                    subscribe: false,
                }),
                prompts: Some(PromptCapabilities {
                    list_changed: false,
                }),
                tools: Some(ToolCapabilities {
                    list_changed: false,
                }),
            }),
        }
    }
}
