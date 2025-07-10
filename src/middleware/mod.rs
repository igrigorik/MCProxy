//! The middleware module provides a way to intercept and modify requests and responses.
//!
//! This module contains the building blocks for creating middleware that can
//! be used to add functionality to the proxy server without modifying the core
//! proxy logic.

pub mod client;
pub mod client_middleware;
pub mod proxy;
pub mod proxy_middleware;
pub mod registry;

use self::client::ClientMiddleware;
use self::proxy::ProxyMiddleware;
use self::registry::MiddlewareRegistry;
use crate::config::{McpConfig, MiddlewareSpec};
use crate::error::{ProxyError, Result};
use std::sync::Arc;

/// Factory trait for creating proxy middleware from configuration
pub trait ProxyMiddlewareFactory: Send + Sync {
    /// Create a new proxy middleware instance from configuration
    fn create(&self, config: &serde_json::Value) -> Result<Arc<dyn ProxyMiddleware>>;
    
    /// Get the type name this factory handles
    fn middleware_type(&self) -> &'static str;
}

/// Factory trait for creating client middleware from configuration
pub trait ClientMiddlewareFactory: Send + Sync {
    /// Create a new client middleware instance from configuration
    /// server_name is provided for middleware that need server context
    fn create(&self, server_name: &str, config: &serde_json::Value) -> Result<Arc<dyn ClientMiddleware>>;
    
    /// Get the type name this factory handles
    fn middleware_type(&self) -> &'static str;
}

/// Manages and applies all registered middleware.
///
/// The `MiddlewareManager` is responsible for loading middleware based on the
/// application's configuration and providing access to them.
#[derive(Clone)]
pub struct MiddlewareManager {
    pub proxy: Vec<Arc<dyn ProxyMiddleware>>,
    registry: Arc<MiddlewareRegistry>,
}

impl std::fmt::Debug for MiddlewareManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MiddlewareManager")
            .field("proxy", &format!("Vec<Arc<dyn ProxyMiddleware>> [len={}]", self.proxy.len()))
            .field("registry", &self.registry)
            .finish()
    }
}

impl MiddlewareManager {
    /// Creates a new `MiddlewareManager` and registers middleware based on the provided config.
    pub fn from_config(config: &McpConfig) -> Result<Self> {
        let registry = Arc::new(MiddlewareRegistry::new());
        
        // Get middleware config from HTTP server config
        let default_middleware = crate::config::MiddlewareConfig::default();
        let middleware_config = config.http_server
            .as_ref()
            .map(|http| &http.middleware)
            .unwrap_or(&default_middleware);
        
        // Create proxy middleware from config
        let proxy = Self::create_proxy_middleware(&middleware_config.proxy, &registry)?;
        
        Ok(Self {
            proxy,
            registry,
        })
    }

    /// Creates client middleware for a specific server based on configuration.
    pub fn create_client_middleware_for_server(&self, server_name: &str, config: &McpConfig) -> Result<Vec<Arc<dyn ClientMiddleware>>> {
        // Get middleware config from HTTP server config
        let default_middleware = crate::config::MiddlewareConfig::default();
        let middleware_config = config.http_server
            .as_ref()
            .map(|http| &http.middleware)
            .unwrap_or(&default_middleware);
        
        // Check if there are server-specific middleware configs
        let middleware_specs = if let Some(server_specs) = middleware_config.client.servers.get(server_name) {
            server_specs
        } else {
            &middleware_config.client.default
        };
        
        Self::create_client_middleware(server_name, middleware_specs, &self.registry)
    }
    
    /// Create proxy middleware from specifications
    fn create_proxy_middleware(
        specs: &[MiddlewareSpec], 
        registry: &MiddlewareRegistry
    ) -> Result<Vec<Arc<dyn ProxyMiddleware>>> {
        let mut middleware = Vec::new();
        
        for spec in specs {
            if !spec.enabled {
                continue;
            }
            
            let factory = registry.get_proxy_factory(&spec.middleware_type)
                .ok_or_else(|| ProxyError::config(format!(
                    "Unknown proxy middleware type: {}", 
                    spec.middleware_type
                )))?;
            
            let instance = factory.create(&spec.config)?;
            middleware.push(instance);
        }
        
        Ok(middleware)
    }
    
    /// Create client middleware from specifications
    fn create_client_middleware(
        server_name: &str,
        specs: &[MiddlewareSpec], 
        registry: &MiddlewareRegistry
    ) -> Result<Vec<Arc<dyn ClientMiddleware>>> {
        let mut middleware = Vec::new();
        
        for spec in specs {
            if !spec.enabled {
                continue;
            }
            
            let factory = registry.get_client_factory(&spec.middleware_type)
                .ok_or_else(|| ProxyError::config(format!(
                    "Unknown client middleware type: {}", 
                    spec.middleware_type
                )))?;
            
            let instance = factory.create(server_name, &spec.config)?;
            middleware.push(instance);
        }
        
        Ok(middleware)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{McpConfig, MiddlewareConfig, MiddlewareSpec, ClientMiddlewareConfig};
    use rmcp::model::Tool;
    use std::collections::HashMap;

    #[tokio::test]
    async fn test_proxy_middleware_description_enrichment() {
        let mut config = McpConfig::default();
        if let Some(ref mut http_server) = config.http_server {
            http_server.middleware = MiddlewareConfig {
                proxy: vec![
                    MiddlewareSpec {
                        middleware_type: "description_enricher".to_string(),
                        enabled: true,
                        config: serde_json::Value::Null,
                    },
                ],
                client: ClientMiddlewareConfig::default(),
            };
        }
        let manager = MiddlewareManager::from_config(&config).unwrap();
        
        // Create test tools
        let mut tools = vec![
            Tool {
                name: "production_tool".into(),
                description: Some("A production tool".into()),
                input_schema: Arc::new(serde_json::Map::new()),
                annotations: None,
            },
            Tool {
                name: "another_tool".into(),
                description: Some("Another tool".into()),
                input_schema: Arc::new(serde_json::Map::new()),
                annotations: None,
            },
        ];
        
        // Apply proxy middleware
        for mw in manager.proxy.iter() {
            mw.on_list_tools(&mut tools).await;
        }
        
        // Verify descriptions were enriched
        for tool in &tools {
            if let Some(ref desc) = tool.description {
                assert!(desc.contains("(via mcproxy)"));
            }
        }
        
        assert_eq!(tools.len(), 2);
    }

    #[test]
    fn test_middleware_manager_creation() {
        let config = McpConfig::default();
        let manager = MiddlewareManager::from_config(&config).unwrap();
        
        // Should have default proxy middleware (empty for default config)
        assert!(manager.proxy.is_empty());
        
        // Should be able to create server-specific middleware
        let server_middleware = manager.create_client_middleware_for_server("test_server", &config).unwrap();
        assert!(server_middleware.is_empty()); // Default client config is empty
    }
    
    #[test]
    fn test_config_based_proxy_middleware() {
        let mut config = McpConfig::default();
        if let Some(ref mut http_server) = config.http_server {
            http_server.middleware = MiddlewareConfig {
                proxy: vec![
                    MiddlewareSpec {
                        middleware_type: "description_enricher".to_string(),
                        enabled: true,
                        config: serde_json::Value::Null,
                    },
                ],
                client: ClientMiddlewareConfig::default(),
            };
        }
        
        let manager = MiddlewareManager::from_config(&config).unwrap();
        assert_eq!(manager.proxy.len(), 1);
    }
    
    #[test]
    fn test_config_based_client_middleware() {
        let mut config = McpConfig::default();
        if let Some(ref mut http_server) = config.http_server {
            http_server.middleware = MiddlewareConfig {
                proxy: vec![],
                client: ClientMiddlewareConfig {
                    default: vec![
                        MiddlewareSpec {
                            middleware_type: "logging".to_string(),
                            enabled: true,
                            config: serde_json::Value::Null,
                        },
                    ],
                    servers: HashMap::new(),
                },
            };
        }
        
        let manager = MiddlewareManager::from_config(&config).unwrap();
        let client_middleware = manager.create_client_middleware_for_server("any_server", &config).unwrap();
        assert_eq!(client_middleware.len(), 1);
    }

    #[test]
    fn test_client_middleware_tool_filter() {
        let mut config = McpConfig::default();
        let mut servers = HashMap::new();
        servers.insert("github".to_string(), vec![
            MiddlewareSpec {
                middleware_type: "tool_filter".to_string(),
                enabled: true,
                config: serde_json::json!({
                    "allow": "issue"
                }),
            },
        ]);
        
        if let Some(ref mut http_server) = config.http_server {
            http_server.middleware = MiddlewareConfig {
                proxy: vec![],
                client: ClientMiddlewareConfig {
                    default: vec![],
                    servers,
                },
            };
        }
        
        let manager = MiddlewareManager::from_config(&config).unwrap();
        let client_middleware = manager.create_client_middleware_for_server("github", &config).unwrap();
        assert_eq!(client_middleware.len(), 1);
        
        // Test that other servers don't get the tool filter
        let other_middleware = manager.create_client_middleware_for_server("other", &config).unwrap();
        assert_eq!(other_middleware.len(), 0);
    }
    
    #[test]
    fn test_server_specific_middleware_override() {
        let mut config = McpConfig::default();
        let mut servers = HashMap::new();
        servers.insert("special_server".to_string(), vec![
            MiddlewareSpec {
                middleware_type: "logging".to_string(),
                enabled: true,
                config: serde_json::json!({"level": "debug"}),
            },
        ]);
        
        if let Some(ref mut http_server) = config.http_server {
            http_server.middleware = MiddlewareConfig {
                proxy: vec![],
                client: ClientMiddlewareConfig {
                    default: vec![
                        MiddlewareSpec {
                            middleware_type: "logging".to_string(),
                            enabled: true,
                            config: serde_json::json!({"level": "info"}),
                        },
                    ],
                    servers,
                },
            };
        }
        
        let manager = MiddlewareManager::from_config(&config).unwrap();
        
        // Regular server should get default middleware
        let default_middleware = manager.create_client_middleware_for_server("regular_server", &config).unwrap();
        assert_eq!(default_middleware.len(), 1);
        
        // Special server should get its own middleware
        let special_middleware = manager.create_client_middleware_for_server("special_server", &config).unwrap();
        assert_eq!(special_middleware.len(), 1);
    }
    
    #[test]
    fn test_disabled_middleware_not_created() {
        let mut config = McpConfig::default();
        if let Some(ref mut http_server) = config.http_server {
            http_server.middleware = MiddlewareConfig {
                proxy: vec![
                    MiddlewareSpec {
                        middleware_type: "description_enricher".to_string(),
                        enabled: true,
                        config: serde_json::Value::Null,
                    },
                    MiddlewareSpec {
                        middleware_type: "description_enricher".to_string(),
                        enabled: false, // Disabled
                        config: serde_json::Value::Null,
                    },
                ],
                client: ClientMiddlewareConfig::default(),
            };
        }
        
        let manager = MiddlewareManager::from_config(&config).unwrap();
        // Should only have 1 middleware (the enabled one)
        assert_eq!(manager.proxy.len(), 1);
    }
    
    #[test]
    fn test_unknown_middleware_type_error() {
        let mut config = McpConfig::default();
        if let Some(ref mut http_server) = config.http_server {
            http_server.middleware = MiddlewareConfig {
                proxy: vec![
                    MiddlewareSpec {
                        middleware_type: "nonexistent_middleware".to_string(),
                        enabled: true,
                        config: serde_json::Value::Null,
                    },
                ],
                client: ClientMiddlewareConfig::default(),
            };
        }
        
        let result = MiddlewareManager::from_config(&config);
        assert!(result.is_err());
        let error = result.unwrap_err();
        assert!(format!("{}", error).contains("Unknown proxy middleware type"));
    }
} 