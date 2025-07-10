//! Tool filtering client middleware that filters tools based on configurable regex patterns.

use async_trait::async_trait;
use regex::Regex;
use rmcp::model::ListToolsResult;
use serde::Deserialize;
use std::sync::Arc;
use tracing::info;
use uuid::Uuid;

use crate::error::{ProxyError, Result};
use crate::middleware::client::ClientMiddleware;
use crate::middleware::ClientMiddlewareFactory;

/// Configuration for tool filtering middleware
#[derive(Debug, Clone, Deserialize)]
pub struct ToolFilterConfig {
    /// Regex pattern for tools to allow (if specified, only matching tools are kept)
    #[serde(default)]
    pub allow: Option<String>,
    
    /// Regex pattern for tools to disallow (if specified, matching tools are removed)
    #[serde(default)]
    pub disallow: Option<String>,
}

impl Default for ToolFilterConfig {
    fn default() -> Self {
        Self {
            allow: Some(".*".to_string()), // Default behavior: allow everything
            disallow: None,
        }
    }
}

/// Client middleware that filters tools from individual servers based on regex patterns.
/// 
/// If both `allow` and `disallow` patterns are specified, the `allow` pattern takes precedence.
/// If only `allow` is specified, only tools matching the pattern are kept.
/// If only `disallow` is specified, tools matching the pattern are removed.
/// If neither is specified, all tools are kept.
#[derive(Debug)]
pub struct ToolFilterClientMiddleware {
    allow_regex: Option<Regex>,
    disallow_regex: Option<Regex>,
    config: ToolFilterConfig,
    server_name: String,
}

impl ToolFilterClientMiddleware {
    /// Creates a new ToolFilterClientMiddleware with the given configuration
    pub fn new(server_name: String, config: ToolFilterConfig) -> Result<Self> {
        let allow_regex = if let Some(ref pattern) = config.allow {
            Some(Regex::new(pattern).map_err(|e| {
                ProxyError::config(format!("Invalid allow regex pattern '{}': {}", pattern, e))
            })?)
        } else {
            None
        };
        
        let disallow_regex = if let Some(ref pattern) = config.disallow {
            Some(Regex::new(pattern).map_err(|e| {
                ProxyError::config(format!("Invalid disallow regex pattern '{}': {}", pattern, e))
            })?)
        } else {
            None
        };
        
        Ok(Self {
            allow_regex,
            disallow_regex,
            config,
            server_name,
        })
    }
    
    /// Checks if a tool should be kept based on the configured patterns
    fn should_keep_tool(&self, tool_name: &str) -> bool {
        // If allow pattern is specified, only keep tools that match it
        if let Some(ref allow_regex) = self.allow_regex {
            return allow_regex.is_match(tool_name);
        }
        
        // If disallow pattern is specified, remove tools that match it
        if let Some(ref disallow_regex) = self.disallow_regex {
            return !disallow_regex.is_match(tool_name);
        }
        
        // No patterns specified, keep all tools
        true
    }
}

#[async_trait]
impl ClientMiddleware for ToolFilterClientMiddleware {
    async fn modify_list_tools_result(&self, _request_id: Uuid, result: &mut ListToolsResult) {
        let initial_count = result.tools.len();
        result.tools.retain(|tool| self.should_keep_tool(&tool.name));
        let filtered_count = initial_count - result.tools.len();
        
        if filtered_count > 0 {
            if let Some(ref allow_pattern) = self.config.allow {
                info!("🔍 [{}] Filtered to {} tools matching allow pattern: '{}'", 
                      self.server_name, result.tools.len(), allow_pattern);
            } else if let Some(ref disallow_pattern) = self.config.disallow {
                info!("🔍 [{}] Filtered out {} tools matching disallow pattern: '{}'", 
                      self.server_name, filtered_count, disallow_pattern);
            }
        }
    }
}

/// Factory for creating ToolFilterClientMiddleware from configuration
#[derive(Debug)]
pub struct ToolFilterClientFactory;

impl ClientMiddlewareFactory for ToolFilterClientFactory {
    fn create(&self, server_name: &str, config: &serde_json::Value) -> Result<Arc<dyn ClientMiddleware>> {
        let filter_config: ToolFilterConfig = if config.is_null() {
            // Use default configuration if none provided
            ToolFilterConfig::default()
        } else {
            serde_json::from_value(config.clone()).map_err(|e| {
                ProxyError::config(format!("Invalid tool filter configuration: {}", e))
            })?
        };
        
        let middleware = ToolFilterClientMiddleware::new(server_name.to_string(), filter_config)?;
        Ok(Arc::new(middleware))
    }
    
    fn middleware_type(&self) -> &'static str {
        "tool_filter"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rmcp::model::Tool;
    use std::sync::Arc;

    #[tokio::test]
    async fn test_tool_filter_disallow_pattern() {
        let config = ToolFilterConfig {
            allow: None,
            disallow: Some("test|debug".to_string()),
        };
        let middleware = ToolFilterClientMiddleware::new("test-server".to_string(), config).unwrap();

        let mut result = ListToolsResult {
            tools: vec![
                Tool {
                    name: "production_tool".into(),
                    description: None,
                    input_schema: Arc::new(serde_json::Map::new()),
                    annotations: None,
                },
                Tool {
                    name: "test_helper".into(),
                    description: None,
                    input_schema: Arc::new(serde_json::Map::new()),
                    annotations: None,
                },
                Tool {
                    name: "debug_tool".into(),
                    description: None,
                    input_schema: Arc::new(serde_json::Map::new()),
                    annotations: None,
                },
                Tool {
                    name: "normal_tool".into(),
                    description: None,
                    input_schema: Arc::new(serde_json::Map::new()),
                    annotations: None,
                },
            ],
            next_cursor: None,
        };

        middleware.modify_list_tools_result(Uuid::new_v4(), &mut result).await;

        assert_eq!(result.tools.len(), 2);
        assert!(result.tools.iter().any(|t| t.name == "production_tool"));
        assert!(result.tools.iter().any(|t| t.name == "normal_tool"));
        assert!(!result.tools.iter().any(|t| t.name == "test_helper"));
        assert!(!result.tools.iter().any(|t| t.name == "debug_tool"));
    }

    #[tokio::test]
    async fn test_tool_filter_allow_pattern() {
        let config = ToolFilterConfig {
            allow: Some("^file_|^search_".to_string()),
            disallow: None,
        };
        let middleware = ToolFilterClientMiddleware::new("test-server".to_string(), config).unwrap();

        let mut result = ListToolsResult {
            tools: vec![
                Tool {
                    name: "file_read".into(),
                    description: None,
                    input_schema: Arc::new(serde_json::Map::new()),
                    annotations: None,
                },
                Tool {
                    name: "search_files".into(),
                    description: None,
                    input_schema: Arc::new(serde_json::Map::new()),
                    annotations: None,
                },
                Tool {
                    name: "other_tool".into(),
                    description: None,
                    input_schema: Arc::new(serde_json::Map::new()),
                    annotations: None,
                },
            ],
            next_cursor: None,
        };

        middleware.modify_list_tools_result(Uuid::new_v4(), &mut result).await;

        assert_eq!(result.tools.len(), 2);
        assert!(result.tools.iter().any(|t| t.name == "file_read"));
        assert!(result.tools.iter().any(|t| t.name == "search_files"));
        assert!(!result.tools.iter().any(|t| t.name == "other_tool"));
    }

    #[tokio::test]
    async fn test_default_config_allows_everything() {
        let config = ToolFilterConfig::default();
        let middleware = ToolFilterClientMiddleware::new("test-server".to_string(), config).unwrap();

        let mut result = ListToolsResult {
            tools: vec![
                Tool {
                    name: "production_tool".into(),
                    description: None,
                    input_schema: Arc::new(serde_json::Map::new()),
                    annotations: None,
                },
                Tool {
                    name: "test_helper".into(),
                    description: None,
                    input_schema: Arc::new(serde_json::Map::new()),
                    annotations: None,
                },
                Tool {
                    name: "debug_tool".into(),
                    description: None,
                    input_schema: Arc::new(serde_json::Map::new()),
                    annotations: None,
                },
            ],
            next_cursor: None,
        };

        let initial_count = result.tools.len();
        middleware.modify_list_tools_result(Uuid::new_v4(), &mut result).await;

        // Default config should allow everything
        assert_eq!(result.tools.len(), initial_count);
        assert!(result.tools.iter().any(|t| t.name == "production_tool"));
        assert!(result.tools.iter().any(|t| t.name == "test_helper"));
        assert!(result.tools.iter().any(|t| t.name == "debug_tool"));
    }

    #[test]
    fn test_invalid_regex_pattern() {
        let config = ToolFilterConfig {
            allow: None,
            disallow: Some("[invalid_regex".to_string()),
        };
        
        let result = ToolFilterClientMiddleware::new("test-server".to_string(), config);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Invalid disallow regex pattern"));
    }
} 