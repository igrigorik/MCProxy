//! The middleware module provides a way to intercept and modify requests and responses.
//!
//! This module contains the building blocks for creating middleware that can
//! be used to add functionality to the proxy server without modifying the core
//! proxy logic.

pub mod client;
pub mod examples;
pub mod proxy;

use self::client::ClientMiddleware;
use self::examples::{create_default_client_middleware, create_default_proxy_middleware};
use self::proxy::ProxyMiddleware;
use crate::config::McpConfig;
use std::sync::Arc;

/// Manages and applies all registered middleware.
///
/// The `MiddlewareManager` is responsible for loading middleware based on the
/// application's configuration and providing access to them.
#[derive(Default, Clone)]
pub struct MiddlewareManager {
    pub proxy: Vec<Arc<dyn ProxyMiddleware>>,
}

impl MiddlewareManager {
    /// Creates a new `MiddlewareManager` and registers middleware based on the provided config.
    pub fn from_config(_config: &McpConfig) -> Self {
        // For now, we'll use the default example middleware.
        // In the future, this can be configured based on the config file.
        Self {
            proxy: create_default_proxy_middleware(),
        }
    }

    /// Creates client middleware for a specific server.
    pub fn create_client_middleware_for_server(&self, server_name: &str) -> Vec<Arc<dyn ClientMiddleware>> {
        create_default_client_middleware(server_name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::McpConfig;
    use rmcp::model::Tool;

    #[tokio::test]
    async fn test_proxy_middleware_filtering() {
        let config = McpConfig::default();
        let manager = MiddlewareManager::from_config(&config);
        
        // Create test tools including some with "test" in the name
        let mut tools = vec![
            Tool {
                name: "production_tool".into(),
                description: Some("A production tool".into()),
                input_schema: Arc::new(serde_json::Map::new()),
                annotations: None,
            },
            Tool {
                name: "test_helper".into(),
                description: Some("A test helper".into()),
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
        
        // Verify test tool was filtered out
        assert_eq!(tools.len(), 2);
        assert!(!tools.iter().any(|t| t.name.contains("test")));
        
        // Verify descriptions were enriched
        for tool in &tools {
            if let Some(ref desc) = tool.description {
                assert!(desc.contains("(via mcproxy)"));
            }
        }
    }

    #[test]
    fn test_middleware_manager_creation() {
        let config = McpConfig::default();
        let manager = MiddlewareManager::from_config(&config);
        
        // Should have default proxy middleware
        assert!(!manager.proxy.is_empty());
        
        // Should be able to create server-specific middleware
        let server_middleware = manager.create_client_middleware_for_server("test_server");
        assert!(!server_middleware.is_empty());
    }
} 