//! Tool filtering middleware that removes tools containing "test" in their name.

use async_trait::async_trait;
use rmcp::model::{Prompt, Resource, Tool};
use std::sync::Arc;
use tracing::info;

use crate::error::Result;
use crate::middleware::proxy::ProxyMiddleware;
use crate::middleware::ProxyMiddlewareFactory;

/// Example filtering middleware that removes tools containing "test" in their name.
#[derive(Debug)]
pub struct ToolFilterMiddleware;

#[async_trait]
impl ProxyMiddleware for ToolFilterMiddleware {
    async fn on_list_tools(&self, tools: &mut Vec<Tool>) {
        let initial_count = tools.len();
        tools.retain(|tool| !tool.name.to_lowercase().contains("test"));
        let filtered_count = initial_count - tools.len();
        
        if filtered_count > 0 {
            info!("🔍 Filtered out {} test tools", filtered_count);
        }
    }

    async fn on_list_prompts(&self, _prompts: &mut Vec<Prompt>) {
        // This middleware only filters tools
    }

    async fn on_list_resources(&self, _resources: &mut Vec<Resource>) {
        // This middleware only filters tools
    }
}

/// Factory for creating ToolFilterMiddleware from configuration
#[derive(Debug)]
pub struct ToolFilterFactory;

impl ProxyMiddlewareFactory for ToolFilterFactory {
    fn create(&self, _config: &serde_json::Value) -> Result<Arc<dyn ProxyMiddleware>> {
        // ToolFilterMiddleware doesn't need configuration
        Ok(Arc::new(ToolFilterMiddleware))
    }
    
    fn middleware_type(&self) -> &'static str {
        "tool_filter"
    }
} 