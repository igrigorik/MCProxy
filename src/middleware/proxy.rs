//! Defines the `ProxyMiddleware` trait for operating on aggregated results.

use async_trait::async_trait;
use rmcp::model::{Prompt, Resource, Tool};

/// A trait for middleware that operates on aggregated results from all downstream servers.
///
/// This middleware is useful for tasks like filtering, enrichment, or access control
/// on the final list of items before they are returned to the client.
#[async_trait]
pub trait ProxyMiddleware: Send + Sync {
    /// Modify the final, aggregated list of tools.
    async fn on_list_tools(&self, _tools: &mut Vec<Tool>);

    /// Modify the final, aggregated list of prompts.
    async fn on_list_prompts(&self, _prompts: &mut Vec<Prompt>);

    /// Modify the final, aggregated list of resources.
    async fn on_list_resources(&self, _resources: &mut Vec<Resource>);
}

#[async_trait]
impl ProxyMiddleware for () {
    async fn on_list_tools(&self, _tools: &mut Vec<Tool>) {}
    async fn on_list_prompts(&self, _prompts: &mut Vec<Prompt>) {}
    async fn on_list_resources(&self, _resources: &mut Vec<Resource>) {}
} 