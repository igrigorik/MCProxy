use serde::Deserialize;
use std::collections::HashMap;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpConfig {
    pub mcp_servers: HashMap<String, ServerConfig>,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub enum ServerConfig {
    Stdio(StdioConfig),
    Http(HttpConfig),
}

#[derive(Debug, Deserialize)]
pub struct StdioConfig {
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: HashMap<String, String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HttpConfig {
    pub url: String,
    #[serde(default)]
    pub authorization_token: String,
    #[serde(default)]
    pub headers: HashMap<String, String>,
}

pub fn load_config(path: &str) -> anyhow::Result<McpConfig> {
    let content = std::fs::read_to_string(path)?;
    let config: McpConfig = serde_json::from_str(&content)?;
    Ok(config)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::NamedTempFile;

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
          }
        }
        "#;

        let config: McpConfig = serde_json::from_str(json_data).unwrap();
        assert_eq!(config.mcp_servers.len(), 2);

        // Test stdio server
        let stdio_server = config.mcp_servers.get("stdio-server").unwrap();
        if let ServerConfig::Stdio(stdio_config) = stdio_server {
            assert_eq!(stdio_config.command, "my-command");
            assert_eq!(stdio_config.args, vec!["arg1", "arg2"]);
            assert_eq!(stdio_config.env.get("VAR1"), Some(&"VALUE1".to_string()));
        } else {
            panic!("Expected Stdio server config");
        }

        // Test http server
        let http_server = config.mcp_servers.get("http-server").unwrap();
        if let ServerConfig::Http(http_config) = http_server {
            assert_eq!(http_config.url, "http://localhost:8080");
            assert_eq!(http_config.authorization_token, "bearer-token");
            assert_eq!(http_config.headers.get("Authorization"), Some(&"Bearer test-token".to_string()));
            assert_eq!(http_config.headers.get("X-Custom-Header"), Some(&"custom-value".to_string()));
        } else {
            panic!("Expected Http server config");
        }
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

        let config: McpConfig = serde_json::from_str(json_data).unwrap();

        // Test stdio server with defaults
        let stdio_server = config.mcp_servers.get("minimal-stdio").unwrap();
        if let ServerConfig::Stdio(stdio_config) = stdio_server {
            assert_eq!(stdio_config.command, "echo");
            assert!(stdio_config.args.is_empty());
            assert!(stdio_config.env.is_empty());
        } else {
            panic!("Expected Stdio server config");
        }

        // Test http server with defaults
        let http_server = config.mcp_servers.get("minimal-http").unwrap();
        if let ServerConfig::Http(http_config) = http_server {
            assert_eq!(http_config.url, "http://localhost:8080");
            assert!(http_config.authorization_token.is_empty());
            assert!(http_config.headers.is_empty());
        } else {
            panic!("Expected Http server config");
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

        let temp_file = NamedTempFile::new().unwrap();
        fs::write(temp_file.path(), config_content).unwrap();

        let config = load_config(temp_file.path().to_str().unwrap()).unwrap();
        assert_eq!(config.mcp_servers.len(), 1);
        assert!(config.mcp_servers.contains_key("test-server"));
    }

    #[test]
    fn test_load_config_file_not_found() {
        let result = load_config("nonexistent-file.json");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("No such file or directory"));
    }

    #[test]
    fn test_load_config_invalid_json() {
        let invalid_json = r#"
        {
          "mcpServers": {
            "test-server": {
              "command": "test-command"
            }
          // Missing closing brace
        "#;

        let temp_file = NamedTempFile::new().unwrap();
        fs::write(temp_file.path(), invalid_json).unwrap();

        let result = load_config(temp_file.path().to_str().unwrap());
        assert!(result.is_err());
        // JSON parsing errors can have different error messages, just ensure it's an error
        let error_msg = result.unwrap_err().to_string();
        assert!(!error_msg.is_empty());
    }

    #[test]
    fn test_load_config_missing_required_fields() {
        let invalid_config = r#"
        {
          "mcpServers": {
            "invalid-stdio": {
              "args": ["arg1"]
            },
            "invalid-http": {
              "headers": {"X-Test": "value"}
            }
          }
        }
        "#;

        let temp_file = NamedTempFile::new().unwrap();
        fs::write(temp_file.path(), invalid_config).unwrap();

        let result = load_config(temp_file.path().to_str().unwrap());
        assert!(result.is_err());
    }

    #[test]
    fn test_empty_config() {
        let empty_config = r#"
        {
          "mcpServers": {}
        }
        "#;

        let config: McpConfig = serde_json::from_str(empty_config).unwrap();
        assert!(config.mcp_servers.is_empty());
    }

    #[test]
    fn test_mixed_server_types() {
        let mixed_config = r#"
        {
          "mcpServers": {
            "stdio1": {
              "command": "cmd1"
            },
            "http1": {
              "url": "http://localhost:8080"
            },
            "stdio2": {
              "command": "cmd2",
              "args": ["--verbose"]
            },
            "http2": {
              "url": "https://api.example.com",
              "headers": {
                "Authorization": "Bearer token123"
              }
            }
          }
        }
        "#;

        let config: McpConfig = serde_json::from_str(mixed_config).unwrap();
        assert_eq!(config.mcp_servers.len(), 4);

        // Verify different server types are correctly parsed
        assert!(matches!(config.mcp_servers.get("stdio1"), Some(ServerConfig::Stdio(_))));
        assert!(matches!(config.mcp_servers.get("http1"), Some(ServerConfig::Http(_))));
        assert!(matches!(config.mcp_servers.get("stdio2"), Some(ServerConfig::Stdio(_))));
        assert!(matches!(config.mcp_servers.get("http2"), Some(ServerConfig::Http(_))));
    }

    #[test]
    fn test_headers_with_special_characters() {
        let config_with_special_headers = r#"
        {
          "mcpServers": {
            "test-server": {
              "url": "https://api.example.com",
              "headers": {
                "X-Custom-Header": "value with spaces",
                "X-Special-Chars": "!@#$%^&*()",
                "Authorization": "Bearer token-with-dashes_and_underscores"
              }
            }
          }
        }
        "#;

        let config: McpConfig = serde_json::from_str(config_with_special_headers).unwrap();
        let server = config.mcp_servers.get("test-server").unwrap();
        
        if let ServerConfig::Http(http_config) = server {
            assert_eq!(http_config.headers.get("X-Custom-Header"), Some(&"value with spaces".to_string()));
            assert_eq!(http_config.headers.get("X-Special-Chars"), Some(&"!@#$%^&*()".to_string()));
            assert_eq!(http_config.headers.get("Authorization"), Some(&"Bearer token-with-dashes_and_underscores".to_string()));
        } else {
            panic!("Expected Http server config");
        }
    }

    #[test]
    fn test_backward_compatibility_authorization_token() {
        let config_with_auth_token = r#"
        {
          "mcpServers": {
            "legacy-server": {
              "url": "https://api.example.com",
              "authorizationToken": "legacy-token"
            }
          }
        }
        "#;

        let config: McpConfig = serde_json::from_str(config_with_auth_token).unwrap();
        let server = config.mcp_servers.get("legacy-server").unwrap();
        
        if let ServerConfig::Http(http_config) = server {
            assert_eq!(http_config.authorization_token, "legacy-token");
            assert!(http_config.headers.is_empty());
        } else {
            panic!("Expected Http server config");
        }
    }
} 