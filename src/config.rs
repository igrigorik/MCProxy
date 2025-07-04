//! Configuration management for the MCP proxy.

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
#[serde(untagged)] // Infer transport from fields: command = stdio, url = http
pub enum ServerConfig {
    Stdio(StdioConfig),
    Http(HttpConfig),
    TypedStdio(TypedStdioConfig),
    TypedHttp(TypedHttpConfig),
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct StdioConfig {
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: HashMap<String, String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct HttpConfig {
    pub url: String,
    #[serde(default, rename = "authorizationToken")]
    pub authorization_token: String,
    #[serde(default)]
    pub headers: HashMap<String, String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct TypedStdioConfig {
    #[serde(rename = "type")]
    pub server_type: String,
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: HashMap<String, String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct TypedHttpConfig {
    #[serde(rename = "type")]
    pub server_type: String,
    #[serde(alias = "command")] // Support both "url" and "command" for HTTP URLs
    pub url: String,
    #[serde(default, rename = "authorizationToken")]
    pub authorization_token: String,
    #[serde(default)]
    pub headers: HashMap<String, String>,
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

impl Default for HttpServerConfig {
    fn default() -> Self {
        Self {
            host: default_host(),
            port: default_port(),
            cors_enabled: default_cors_enabled(),
            cors_origins: default_cors_origins(),
        }
    }
}

impl ServerConfig {
    pub fn as_stdio(&self) -> Option<StdioConfig> {
        match self {
            ServerConfig::Stdio(config) => Some(config.clone()),
            ServerConfig::TypedStdio(config) => {
                if config.server_type == "stdio" {
                    Some(StdioConfig {
                        command: config.command.clone(),
                        args: config.args.clone(),
                        env: config.env.clone(),
                    })
                } else {
                    None
                }
            }
            _ => None,
        }
    }

    pub fn as_http(&self) -> Option<HttpConfig> {
        match self {
            ServerConfig::Http(config) => Some(config.clone()),
            ServerConfig::TypedHttp(config) => {
                if config.server_type == "http" {
                    Some(HttpConfig {
                        url: config.url.clone(),
                        authorization_token: config.authorization_token.clone(),
                        headers: config.headers.clone(),
                    })
                } else {
                    None
                }
            }
            _ => None,
        }
    }
}

/// Load configuration from a JSON file
pub fn load_config(path: &str) -> Result<McpConfig, Box<dyn std::error::Error + Send + Sync>> {
    let content = std::fs::read_to_string(path)?;
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
        if let ServerConfig::Stdio(stdio) = &config.mcp_servers["stdio-server"] {
            assert_eq!(stdio.command, "my-command");
            assert_eq!(stdio.args, vec!["arg1", "arg2"]);
            assert_eq!(stdio.env.get("VAR1"), Some(&"VALUE1".to_string()));
        } else {
            panic!("Expected stdio server config");
        }

        // Test HTTP server config
        if let ServerConfig::Http(http) = &config.mcp_servers["http-server"] {
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
        if let ServerConfig::Stdio(stdio) = &config.mcp_servers["minimal-stdio"] {
            assert_eq!(stdio.command, "echo");
            assert!(stdio.args.is_empty());
            assert!(stdio.env.is_empty());
        } else {
            panic!("Expected stdio server config");
        }

        // Test defaults for HTTP
        if let ServerConfig::Http(http) = &config.mcp_servers["minimal-http"] {
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