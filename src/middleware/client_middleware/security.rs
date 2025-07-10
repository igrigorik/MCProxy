//! Security middleware that inspects tool call inputs and blocks unsafe calls.

use async_trait::async_trait;
use regex::Regex;
use rmcp::{
    model::CallToolRequestParam,
};
use serde::Deserialize;
use std::sync::Arc;
use tracing::{info, warn};
use uuid::Uuid;

use crate::error::{ProxyError, Result};
use crate::middleware::client::{ClientMiddleware, MiddlewareResult};
use crate::middleware::ClientMiddlewareFactory;

/// Configuration for security middleware
#[derive(Debug, Clone, Deserialize)]
pub struct SecurityConfig {
    /// List of security rules to check
    #[serde(default)]
    pub rules: Vec<SecurityRule>,
    
    /// Whether to log blocked calls
    #[serde(default = "default_log_blocked")]
    pub log_blocked: bool,
}

fn default_log_blocked() -> bool {
    true
}

impl Default for SecurityConfig {
    fn default() -> Self {
        Self {
            rules: vec![
                // Default security rules
                SecurityRule {
                    name: "no_system_commands".to_string(),
                    description: "Block system commands and shell injection attempts".to_string(),
                    pattern: r"(?i)(rm\s+-rf|sudo\s+|passwd\s+|shutdown|reboot|format|del\s+/|rd\s+/s)".to_string(),
                    block_message: "System commands are not allowed".to_string(),
                    enabled: true,
                },
                SecurityRule {
                    name: "no_sensitive_files".to_string(),
                    description: "Block access to sensitive system files".to_string(),
                    pattern: r"(?i)(/etc/passwd|/etc/shadow|\.ssh/|\.aws/|\.env|config\.json|secrets?)".to_string(),
                    block_message: "Access to sensitive files is not allowed".to_string(),
                    enabled: true,
                },
                SecurityRule {
                    name: "no_network_commands".to_string(),
                    description: "Block network-related commands".to_string(),
                    pattern: r"(?i)(curl|wget|nc|netcat|telnet|ssh|ftp|sftp)\s+".to_string(),
                    block_message: "Network commands are not allowed".to_string(),
                    enabled: true,
                },
            ],
            log_blocked: true,
        }
    }
}

/// A security rule that checks tool call inputs
#[derive(Debug, Clone, Deserialize)]
pub struct SecurityRule {
    /// Name of the rule
    pub name: String,
    
    /// Description of what the rule checks
    pub description: String,
    
    /// Regex pattern to match against tool call inputs
    pub pattern: String,
    
    /// Message to return when the rule is violated
    pub block_message: String,
    
    /// Whether the rule is enabled
    #[serde(default = "default_enabled")]
    pub enabled: bool,
}

fn default_enabled() -> bool {
    true
}

/// Security middleware that inspects tool call inputs and blocks unsafe calls.
/// 
/// This middleware checks tool call arguments against a set of configurable security rules.
/// If any rule matches, the call is blocked and a security error is returned.
#[derive(Debug)]
pub struct SecurityClientMiddleware {
    server_name: String,
    rules: Vec<CompiledSecurityRule>,
    log_blocked: bool,
}

/// A compiled security rule with regex pattern
#[derive(Debug)]
struct CompiledSecurityRule {
    name: String,
    #[allow(dead_code)] // Used for debugging
    description: String,
    pattern: Regex,
    block_message: String,
    enabled: bool,
}

impl SecurityClientMiddleware {
    /// Creates a new SecurityClientMiddleware with the given configuration
    pub fn new(server_name: String, config: SecurityConfig) -> Result<Self> {
        let mut rules = Vec::new();
        
        for rule in config.rules {
            if !rule.enabled {
                continue;
            }
            
            let compiled_regex = Regex::new(&rule.pattern).map_err(|e| {
                ProxyError::config(format!(
                    "Invalid security rule '{}' regex pattern '{}': {}",
                    rule.name, rule.pattern, e
                ))
            })?;
            
            rules.push(CompiledSecurityRule {
                name: rule.name,
                description: rule.description,
                pattern: compiled_regex,
                block_message: rule.block_message,
                enabled: rule.enabled,
            });
        }
        
        Ok(Self {
            server_name,
            rules,
            log_blocked: config.log_blocked,
        })
    }
    
    /// Checks if a tool call should be blocked based on security rules
    fn check_security_rules(&self, request: &CallToolRequestParam) -> Option<String> {
        // Convert arguments to a searchable string representation
        let search_content = self.extract_searchable_content(request);
        
        for rule in &self.rules {
            if rule.enabled && rule.pattern.is_match(&search_content) {
                if self.log_blocked {
                    // Log the blocked content for debugging, but truncate if too long
                    let content_preview = if search_content.len() > 200 {
                        format!("{}...", &search_content[..200])
                    } else {
                        search_content.clone()
                    };
                    
                    warn!(
                        "🚫 [{}] Security rule '{}' blocked tool call: {} - {} | Blocked content: {}",
                        self.server_name, rule.name, request.name, rule.block_message, content_preview
                    );
                }
                return Some(rule.block_message.clone());
            }
        }
        
        None
    }
    
    /// Extracts searchable content from a tool call request
    fn extract_searchable_content(&self, request: &CallToolRequestParam) -> String {
        let mut content = format!("tool_name:{}", request.name);
        
        if let Some(ref args) = request.arguments {
            // Convert JSON arguments to a searchable string
            let args_str = serde_json::to_string(args).unwrap_or_default();
            content.push_str(&format!(" args:{}", args_str));
        }
        
        content
    }
}

#[async_trait]
impl ClientMiddleware for SecurityClientMiddleware {
    async fn before_call_tool(&self, request_id: Uuid, request: &CallToolRequestParam) -> MiddlewareResult {
        if self.log_blocked {
            info!(
                "🔒 [{}] Security check for tool: {} ({})",
                self.server_name, request.name, request_id
            );
        }
        
        if let Some(block_message) = self.check_security_rules(request) {
            MiddlewareResult::Block(block_message)
        } else {
            MiddlewareResult::Continue
        }
    }
}

/// Factory for creating SecurityClientMiddleware from configuration
#[derive(Debug)]
pub struct SecurityClientFactory;

impl ClientMiddlewareFactory for SecurityClientFactory {
    fn create(&self, server_name: &str, config: &serde_json::Value) -> Result<Arc<dyn ClientMiddleware>> {
        let security_config: SecurityConfig = if config.is_null() {
            // Use default security configuration
            SecurityConfig::default()
        } else {
            serde_json::from_value(config.clone()).map_err(|e| {
                ProxyError::config(format!("Invalid security configuration: {}", e))
            })?
        };
        
        let middleware = SecurityClientMiddleware::new(server_name.to_string(), security_config)?;
        Ok(Arc::new(middleware))
    }
    
    fn middleware_type(&self) -> &'static str {
        "security"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::middleware::client_middleware::logging::LoggingClientMiddleware;
    use serde_json::json;
    
    #[test]
    fn test_security_middleware_creation() {
        let factory = SecurityClientFactory;
        let config = json!({
            "rules": [
                {
                    "name": "test_rule",
                    "description": "Test rule",
                    "pattern": "dangerous",
                    "block_message": "Dangerous operation",
                    "enabled": true
                }
            ]
        });
        
        let _middleware = factory.create("test_server", &config).unwrap();
        // Should create successfully
        assert!(factory.middleware_type() == "security");
    }
    
    #[test]
    fn test_default_security_rules() {
        let config = SecurityConfig::default();
        let middleware = SecurityClientMiddleware::new("test".to_string(), config).unwrap();
        
        // Test system command blocking
        let mut args = serde_json::Map::new();
        args.insert("command".to_string(), json!("rm -rf /"));
        let request = CallToolRequestParam {
            name: "shell".into(),
            arguments: Some(args),
        };
        
        let result = middleware.check_security_rules(&request);
        assert!(result.is_some());
        assert!(result.unwrap().contains("System commands are not allowed"));
    }
    
    #[test]
    fn test_sensitive_file_blocking() {
        let config = SecurityConfig::default();
        let middleware = SecurityClientMiddleware::new("test".to_string(), config).unwrap();
        
        let mut args = serde_json::Map::new();
        args.insert("path".to_string(), json!("/etc/passwd"));
        let request = CallToolRequestParam {
            name: "read_file".into(),
            arguments: Some(args),
        };
        
        let result = middleware.check_security_rules(&request);
        assert!(result.is_some());
        assert!(result.unwrap().contains("Access to sensitive files is not allowed"));
    }
    
    #[test]
    fn test_allowed_request() {
        let config = SecurityConfig::default();
        let middleware = SecurityClientMiddleware::new("test".to_string(), config).unwrap();
        
        let mut args = serde_json::Map::new();
        args.insert("data".to_string(), json!("hello world"));
        let request = CallToolRequestParam {
            name: "safe_tool".into(),
            arguments: Some(args),
        };
        
        let result = middleware.check_security_rules(&request);
        assert!(result.is_none());
    }
    
    #[test]
    fn test_security_logging_includes_blocked_content() {
        // Create a config with a custom rule that will definitely match
        let config = SecurityConfig {
            rules: vec![SecurityRule {
                name: "test_token_rule".to_string(),
                description: "Test rule for token detection".to_string(),
                pattern: "github_pat".to_string(),
                block_message: "Test token blocked".to_string(),
                enabled: true,
            }],
            log_blocked: true,
        };
        
        let middleware = SecurityClientMiddleware::new("test".to_string(), config).unwrap();
        
        // Create a request with a token that should be blocked
        let mut args = serde_json::Map::new();
        args.insert("url".to_string(), json!("https://example.com?token=github_pat_123"));
        let request = CallToolRequestParam {
            name: "fetch".into(),
            arguments: Some(args),
        };
        
        // This should be blocked
        let result = middleware.check_security_rules(&request);
        assert!(result.is_some());
        assert_eq!(result.unwrap(), "Test token blocked");
        
        // The logging would include the blocked content, but we can't easily test
        // log output in unit tests without complex setup. The important thing is
        // that the blocking functionality works correctly.
    }
    
    #[test]
    fn test_content_truncation_logic() {
        let config = SecurityConfig::default();
        let middleware = SecurityClientMiddleware::new("test".to_string(), config).unwrap();
        
        // Create a request with very long content to test truncation
        let long_content = "x".repeat(300);
        let mut args = serde_json::Map::new();
        args.insert("data".to_string(), json!(long_content));
        let request = CallToolRequestParam {
            name: "test_tool".into(),
            arguments: Some(args),
        };
        
        let search_content = middleware.extract_searchable_content(&request);
        // Should contain the tool name and the long args
        assert!(search_content.contains("test_tool"));
        assert!(search_content.len() > 200);
    }
    
    #[tokio::test]
    async fn test_middleware_blocking_behavior() {
        use uuid::Uuid;
        
        // Create a security middleware that blocks dangerous commands
        let config = SecurityConfig::default();
        let security_middleware = Arc::new(SecurityClientMiddleware::new("test".to_string(), config).unwrap());
        
        // Create a logging middleware
        let logging_middleware = Arc::new(LoggingClientMiddleware::new("test".to_string()));
        
        // Test that security middleware blocks dangerous requests
        let mut args = serde_json::Map::new();
        args.insert("command".to_string(), json!("rm -rf /"));
        let dangerous_request = CallToolRequestParam {
            name: "shell".into(),
            arguments: Some(args),
        };
        
        let request_id = Uuid::new_v4();
        
        // Security middleware should block
        match security_middleware.before_call_tool(request_id, &dangerous_request).await {
            MiddlewareResult::Block(message) => {
                assert!(message.contains("System commands are not allowed"));
            }
            MiddlewareResult::Continue => {
                panic!("Security middleware should have blocked this request");
            }
        }
        
        // Test that logging middleware allows all requests  
        match logging_middleware.before_call_tool(request_id, &dangerous_request).await {
            MiddlewareResult::Continue => {
                // Expected behavior
            }
            MiddlewareResult::Block(_) => {
                panic!("Logging middleware should not block any requests");
            }
        }
    }
} 