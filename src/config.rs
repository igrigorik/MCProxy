//! Configuration management for the MCP proxy.

use crate::error::{ProxyError, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct McpConfig {
    #[serde(rename = "mcpServers")]
    pub mcp_servers: HashMap<String, ServerConfig>,
    #[serde(rename = "httpServer")]
    pub http_server: Option<HttpServerConfig>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(untagged)]
pub enum ServerConfig {
    // Matches when "command" field is present (stdio transport)
    Stdio {
        #[serde(rename = "type", skip_serializing_if = "Option::is_none")]
        server_type: Option<String>,
        command: String,
        #[serde(default)]
        args: Vec<String>,
        #[serde(default)]
        env: HashMap<String, String>,
    },
    // Matches when "url" field is present (http transport)
    Http {
        #[serde(rename = "type", skip_serializing_if = "Option::is_none")]
        server_type: Option<String>,
        url: String,
        #[serde(default, rename = "authorizationToken")]
        authorization_token: String,
        #[serde(default)]
        headers: HashMap<String, String>,
    },
}

/// Configuration for the HTTP server that serves the MCP proxy
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct HttpServerConfig {
    /// Host to bind the HTTP server to (default: "localhost")
    #[serde(default = "default_host")]
    pub host: String,
    
    /// Port to bind the HTTP server to (default: 8080)
    #[serde(default = "default_port")]
    pub port: u16,
    
    /// Whether to enable CORS support (default: true)
    #[serde(default = "default_cors_enabled", rename = "corsEnabled")]
    pub cors_enabled: bool,
    
    /// List of allowed CORS origins (default: ["*"] for development)
    #[serde(default = "default_cors_origins", rename = "corsOrigins")]
    pub cors_origins: Vec<String>,
    
    /// Timeout in seconds for graceful shutdown (default: 5)
    #[serde(default = "default_shutdown_timeout", rename = "shutdownTimeout")]
    pub shutdown_timeout: u64,
}

fn default_host() -> String {
    "localhost".to_string()
}

fn default_port() -> u16 {
    8080
}

fn default_cors_enabled() -> bool {
    true
}

fn default_cors_origins() -> Vec<String> {
    vec!["*".to_string()]
}

fn default_shutdown_timeout() -> u64 {
    5
}

impl Default for HttpServerConfig {
    fn default() -> Self {
        Self {
            host: default_host(),
            port: default_port(),
            cors_enabled: default_cors_enabled(),
            cors_origins: default_cors_origins(),
            shutdown_timeout: default_shutdown_timeout(),
        }
    }
}

// Re-export for backward compatibility
pub type StdioConfig = StdioTransportConfig;
pub type HttpConfig = HttpTransportConfig;

#[derive(Clone, Debug)]
pub struct StdioTransportConfig {
    pub command: String,
    pub args: Vec<String>,
    pub env: HashMap<String, String>,
}

#[derive(Clone, Debug)]
pub struct HttpTransportConfig {
    pub url: String,
    pub authorization_token: String,
    pub headers: HashMap<String, String>,
}

impl ServerConfig {
    pub fn as_stdio(&self) -> Option<StdioTransportConfig> {
        match self {
            ServerConfig::Stdio { command, args, env, .. } => Some(StdioTransportConfig {
                command: command.clone(),
                args: args.clone(),
                env: env.clone(),
            }),
            _ => None,
        }
    }

    pub fn as_http(&self) -> Option<HttpTransportConfig> {
        match self {
            ServerConfig::Http { url, authorization_token, headers, .. } => Some(HttpTransportConfig {
                url: url.clone(),
                authorization_token: authorization_token.clone(),
                headers: headers.clone(),
            }),
            _ => None,
        }
    }
}

/// Load configuration from a JSON file
pub fn load_config(path: &str) -> Result<McpConfig> {
    let content = std::fs::read_to_string(path)
        .map_err(|e| ProxyError::config(format!("Failed to read config file '{}': {}", path, e)))?;
    let mut config: McpConfig = serde_json::from_str(&content)?;
    
    // Ensure HTTP server config exists with defaults
    if config.http_server.is_none() {
        config.http_server = Some(HttpServerConfig::default());
    }
    
    Ok(config)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_config() {
        let json_data = r#"
        {
          "mcpServers": {
            "stdio-server": {
              "command": "my-command",
              "args": ["arg1", "arg2"],
              "env": {
                "VAR1": "VALUE1"
              }
            },
            "http-server": {
              "url": "http://localhost:8080",
              "authorizationToken": "bearer-token",
              "headers": {
                "Authorization": "Bearer test-token",
                "X-Custom-Header": "custom-value"
              }
            }
          },
          "httpServer": {
            "host": "0.0.0.0",
            "port": 9000
          }
        }
        "#;

        let config: McpConfig = serde_json::from_str(json_data).expect("Failed to parse config");

        assert_eq!(config.mcp_servers.len(), 2);
        assert!(config.mcp_servers.contains_key("stdio-server"));
        assert!(config.mcp_servers.contains_key("http-server"));

        // Test stdio server config
        if let Some(stdio) = config.mcp_servers["stdio-server"].as_stdio() {
            assert_eq!(stdio.command, "my-command");
            assert_eq!(stdio.args, vec!["arg1", "arg2"]);
            assert_eq!(stdio.env.get("VAR1"), Some(&"VALUE1".to_string()));
        } else {
            panic!("Expected stdio server config");
        }

        // Test HTTP server config
        if let Some(http) = config.mcp_servers["http-server"].as_http() {
            assert_eq!(http.url, "http://localhost:8080");
            assert_eq!(http.authorization_token, "bearer-token");
            assert_eq!(http.headers.get("Authorization"), Some(&"Bearer test-token".to_string()));
        } else {
            panic!("Expected HTTP server config");
        }

        // Test HTTP server configuration
        let http_server_config = config.http_server.expect("HTTP server config should be present");
        assert_eq!(http_server_config.host, "0.0.0.0");
        assert_eq!(http_server_config.port, 9000);
    }

    #[test]
    fn test_default_values() {
        let json_data = r#"
        {
          "mcpServers": {
            "minimal-stdio": {
              "command": "echo"
            },
            "minimal-http": {
              "url": "http://localhost:8080"
            }
          }
        }
        "#;

        let config: McpConfig = serde_json::from_str(json_data).expect("Failed to parse config");

        // Test defaults for stdio
        if let Some(stdio) = config.mcp_servers["minimal-stdio"].as_stdio() {
            assert_eq!(stdio.command, "echo");
            assert!(stdio.args.is_empty());
            assert!(stdio.env.is_empty());
        } else {
            panic!("Expected stdio server config");
        }

        // Test defaults for HTTP
        if let Some(http) = config.mcp_servers["minimal-http"].as_http() {
            assert_eq!(http.url, "http://localhost:8080");
            assert!(http.authorization_token.is_empty());
            assert!(http.headers.is_empty());
        } else {
            panic!("Expected HTTP server config");
        }
    }

    #[test]
    fn test_load_config_success() {
        let config_content = r#"
        {
          "mcpServers": {
            "test-server": {
              "command": "test-command"
            }
          }
        }
        "#;

        let temp_file = std::env::temp_dir().join("test_config.json");
        std::fs::write(&temp_file, config_content).expect("Failed to write temp file");

        let config = load_config(temp_file.to_str().unwrap()).expect("Failed to load config");
        assert_eq!(config.mcp_servers.len(), 1);
        assert!(config.mcp_servers.contains_key("test-server"));
        
        // HTTP server config should be added with defaults
        let http_config = config.http_server.expect("HTTP server config should be present");
        assert_eq!(http_config.host, "localhost");
        assert_eq!(http_config.port, 8080);

        std::fs::remove_file(temp_file).ok();
    }

    #[test]
    fn test_load_config_file_not_found() {
        let result = load_config("non_existent_file.json");
        assert!(result.is_err());
    }
} 