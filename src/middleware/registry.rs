//! Middleware registry for managing factory registration and lookup.

use super::{ClientMiddlewareFactory, ProxyMiddlewareFactory};
use std::collections::HashMap;
use std::sync::Arc;

/// Registry for middleware factories, handling both proxy and client middleware
pub struct MiddlewareRegistry {
    proxy_factories: HashMap<String, Arc<dyn ProxyMiddlewareFactory>>,
    client_factories: HashMap<String, Arc<dyn ClientMiddlewareFactory>>,
}

impl std::fmt::Debug for MiddlewareRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MiddlewareRegistry")
            .field("proxy_factories", &format!("HashMap [keys={:?}]", self.proxy_factories.keys().collect::<Vec<_>>()))
            .field("client_factories", &format!("HashMap [keys={:?}]", self.client_factories.keys().collect::<Vec<_>>()))
            .finish()
    }
}

impl MiddlewareRegistry {
    /// Create a new registry with built-in middleware pre-registered
    pub fn new() -> Self {
        let mut registry = Self {
            proxy_factories: HashMap::new(),
            client_factories: HashMap::new(),
        };
        
        // Register built-in middleware
        registry.register_builtin_middleware();
        
        registry
    }
    
    /// Register a proxy middleware factory
    pub fn register_proxy_factory(&mut self, factory: Arc<dyn ProxyMiddlewareFactory>) {
        let middleware_type = factory.middleware_type().to_string();
        self.proxy_factories.insert(middleware_type, factory);
    }
    
    /// Register a client middleware factory  
    pub fn register_client_factory(&mut self, factory: Arc<dyn ClientMiddlewareFactory>) {
        let middleware_type = factory.middleware_type().to_string();
        self.client_factories.insert(middleware_type, factory);
    }
    
    /// Get a proxy middleware factory by type name
    pub fn get_proxy_factory(&self, middleware_type: &str) -> Option<&Arc<dyn ProxyMiddlewareFactory>> {
        self.proxy_factories.get(middleware_type)
    }
    
    /// Get a client middleware factory by type name
    pub fn get_client_factory(&self, middleware_type: &str) -> Option<&Arc<dyn ClientMiddlewareFactory>> {
        self.client_factories.get(middleware_type)
    }
    

    
    /// Register all built-in middleware factories
    fn register_builtin_middleware(&mut self) {
        use super::proxy_middleware::DescriptionEnricherFactory;
        use super::client_middleware::{LoggingClientFactory, SecurityClientFactory, ToolFilterClientFactory};
        
        // Register proxy middleware
        self.register_proxy_factory(Arc::new(DescriptionEnricherFactory));
        
        // Register client middleware
        self.register_client_factory(Arc::new(LoggingClientFactory));
        self.register_client_factory(Arc::new(SecurityClientFactory));
        self.register_client_factory(Arc::new(ToolFilterClientFactory));
    }
}

impl Default for MiddlewareRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_registry_creation() {
        let registry = MiddlewareRegistry::new();
        
        // Should have built-in middleware registered
        assert!(registry.get_proxy_factory("description_enricher").is_some());
        assert!(registry.get_client_factory("logging").is_some());
        assert!(registry.get_client_factory("security").is_some());
        assert!(registry.get_client_factory("tool_filter").is_some());
    }
    
    #[test]
    fn test_factory_lookup() {
        let registry = MiddlewareRegistry::new();
        
        // Should be able to find built-in middleware
        assert!(registry.get_proxy_factory("description_enricher").is_some());
        assert!(registry.get_client_factory("logging").is_some());
        assert!(registry.get_client_factory("security").is_some());
        assert!(registry.get_client_factory("tool_filter").is_some());
        
        // Should return None for unknown types
        assert!(registry.get_proxy_factory("unknown").is_none());
        assert!(registry.get_client_factory("unknown").is_none());
    }
} 