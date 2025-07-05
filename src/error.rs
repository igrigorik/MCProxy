//! Error types for the MCP proxy.

use rmcp::Error as McpError;
use std::io;
use thiserror::Error;

/// Main error type for the proxy server
#[derive(Debug, Error)]
pub enum ProxyError {
    /// Configuration-related errors
    #[error("Configuration error: {0}")]
    Config(String),
    
    /// Connection failures to MCP servers
    #[error("Connection failed for server '{server}': {message}")]
    Connection {
        server: String,
        message: String,
    },
    
    /// MCP protocol errors
    #[error("MCP error: {0}")]
    Mcp(#[from] McpError),
    
    /// IO errors
    #[error("IO error: {0}")]
    Io(#[from] io::Error),
    
    /// JSON serialization/deserialization errors
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
    
    /// Server not found errors
    #[error("Server '{0}' not found")]
    ServerNotFound(String),
    
    /// Invalid format errors
    #[error("Invalid format: {0}")]
    InvalidFormat(String),
    
    /// Process spawn errors
    #[error("Failed to spawn process: {0}")]
    ProcessSpawn(String),
    
    /// HTTP server errors
    #[error("HTTP server error: {0}")]
    HttpServer(String),
}

/// Type alias for Results using ProxyError
pub type Result<T> = std::result::Result<T, ProxyError>;

impl ProxyError {
    /// Create a configuration error
    pub fn config(message: impl Into<String>) -> Self {
        Self::Config(message.into())
    }
    
    /// Create a connection error
    pub fn connection(server: impl Into<String>, message: impl Into<String>) -> Self {
        Self::Connection {
            server: server.into(),
            message: message.into(),
        }
    }
    
    /// Create a server not found error
    pub fn server_not_found(server: impl Into<String>) -> Self {
        Self::ServerNotFound(server.into())
    }
    
    /// Create an invalid format error
    pub fn invalid_format(message: impl Into<String>) -> Self {
        Self::InvalidFormat(message.into())
    }
    
    /// Create a process spawn error
    pub fn process_spawn(message: impl Into<String>) -> Self {
        Self::ProcessSpawn(message.into())
    }
    
    /// Create an HTTP server error
    pub fn http_server(message: impl Into<String>) -> Self {
        Self::HttpServer(message.into())
    }
}

/// Convert ProxyError to McpError for protocol compatibility
impl From<ProxyError> for McpError {
    fn from(error: ProxyError) -> Self {
        match error {
            ProxyError::Mcp(mcp_error) => mcp_error,
            ProxyError::ServerNotFound(server) => {
                McpError::invalid_params(format!("Server not found: {}", server), None)
            }
            ProxyError::InvalidFormat(msg) => {
                McpError::invalid_params(msg, None)
            }
            _ => McpError::internal_error(error.to_string(), None),
        }
    }
} 