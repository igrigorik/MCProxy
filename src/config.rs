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
} 