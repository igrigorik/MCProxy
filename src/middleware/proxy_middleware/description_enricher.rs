//! Description enrichment middleware that adds metadata to tool descriptions.

use async_trait::async_trait;
use rmcp::model::{Prompt, Resource, Tool};
use std::sync::Arc;
use tracing::info;

use crate::error::Result;
use crate::middleware::proxy::ProxyMiddleware;
use crate::middleware::ProxyMiddlewareFactory;

/// Example middleware that adds metadata to tool descriptions.
#[derive(Debug)]
pub struct DescriptionEnricherMiddleware;

#[async_trait]
impl ProxyMiddleware for DescriptionEnricherMiddleware {
    async fn on_list_tools(&self, tools: &mut Vec<Tool>) {
        for tool in tools.iter_mut() {
            if let Some(ref mut description) = tool.description {
                *description = format!("{} (via mcproxy)", description).into();
            } else {
                tool.description = Some("(via mcproxy)".into());
            }
        }
        
        if !tools.is_empty() {
            info!("✨ Enriched descriptions for {} tools", tools.len());
        }
    }

    async fn on_list_prompts(&self, prompts: &mut Vec<Prompt>) {
        for prompt in prompts.iter_mut() {
            if let Some(ref description) = prompt.description.clone() {
                prompt.description = Some(format!("{} (via mcproxy)", description));
            } else {
                prompt.description = Some("(via mcproxy)".to_string());
            }
        }
        
        if !prompts.is_empty() {
            info!("✨ Enriched descriptions for {} prompts", prompts.len());
        }
    }

    async fn on_list_resources(&self, resources: &mut Vec<Resource>) {
        for resource in resources.iter_mut() {
            if let Some(ref description) = resource.description.clone() {
                resource.description = Some(format!("{} (via mcproxy)", description));
            } else {
                resource.description = Some("(via mcproxy)".to_string());
            }
        }
        
        if !resources.is_empty() {
            info!("✨ Enriched descriptions for {} resources", resources.len());
        }
    }
}

/// Factory for creating DescriptionEnricherMiddleware from configuration
#[derive(Debug)]
pub struct DescriptionEnricherFactory;

impl ProxyMiddlewareFactory for DescriptionEnricherFactory {
    fn create(&self, _config: &serde_json::Value) -> Result<Arc<dyn ProxyMiddleware>> {
        // DescriptionEnricherMiddleware doesn't need configuration
        Ok(Arc::new(DescriptionEnricherMiddleware))
    }
    
    fn middleware_type(&self) -> &'static str {
        "description_enricher"
    }
} 