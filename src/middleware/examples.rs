//! Example middleware implementations for demonstration and testing.

use async_trait::async_trait;
use rmcp::{
    model::{
        CallToolRequestParam, CallToolResult, ListPromptsResult, ListResourcesResult,
        ListToolsResult, Prompt, Resource, Tool,
    },
    service::ServiceError,
};
use std::cell::RefCell;
use std::sync::Arc;
use std::time::Instant;
use tracing::{info, warn};
use tokio::task_local;

use super::{client::ClientMiddleware, proxy::ProxyMiddleware};

// Task-local storage for request-scoped timing
task_local! {
    static LIST_TOOLS_START: RefCell<Option<Instant>>;
    static CALL_TOOL_START: RefCell<Option<Instant>>;
    static LIST_PROMPTS_START: RefCell<Option<Instant>>;
    static LIST_RESOURCES_START: RefCell<Option<Instant>>;
}

/// Example logging middleware that logs all client operations with timing.
#[derive(Debug, Clone)]
pub struct LoggingClientMiddleware {
    server_name: String,
}

impl LoggingClientMiddleware {
    pub fn new(server_name: String) -> Self {
        Self { server_name }
    }

    /// Get duration from task-local storage - panics if not found (programming error)
    fn get_duration(task_local: &'static tokio::task::LocalKey<RefCell<Option<Instant>>>) -> std::time::Duration {
        task_local.with(|cell| {
            cell.borrow()
                .expect("Start time should have been set in before_* method")
                .elapsed()
        })
    }

    /// Helper function to safely set start time in task-local storage
    fn set_start_time(task_local: &'static tokio::task::LocalKey<RefCell<Option<Instant>>>) {
        let _ = task_local.try_with(|cell| {
            *cell.borrow_mut() = Some(Instant::now());
        });
    }
}

#[async_trait]
impl ClientMiddleware for LoggingClientMiddleware {
    async fn before_list_tools(&self) {
        info!("📋 [{}] Listing tools...", self.server_name);
        Self::set_start_time(&LIST_TOOLS_START);
    }

    async fn after_list_tools(&self, result: &Result<ListToolsResult, ServiceError>) {
        let duration = Self::get_duration(&LIST_TOOLS_START);

        match result {
            Ok(tools) => info!(
                "✅ [{}] Listed {} tools successfully in {:?}",
                self.server_name,
                tools.tools.len(),
                duration
            ),
            Err(e) => warn!(
                "❌ [{}] Failed to list tools after {:?}: {}",
                self.server_name, duration, e
            ),
        }
    }

    async fn before_call_tool(&self, request: &CallToolRequestParam) {
        info!("🔧 [{}] Calling tool: {}", self.server_name, request.name);
        Self::set_start_time(&CALL_TOOL_START);
    }

    async fn after_call_tool(&self, result: &Result<CallToolResult, ServiceError>) {
        let duration = Self::get_duration(&CALL_TOOL_START);

        match result {
            Ok(_) => info!("✅ [{}] Tool call successful in {:?}", self.server_name, duration),
            Err(e) => warn!(
                "❌ [{}] Tool call failed after {:?}: {}",
                self.server_name, duration, e
            ),
        }
    }

    async fn before_list_prompts(&self) {
        info!("💬 [{}] Listing prompts...", self.server_name);
        Self::set_start_time(&LIST_PROMPTS_START);
    }

    async fn after_list_prompts(&self, result: &Result<ListPromptsResult, ServiceError>) {
        let duration = Self::get_duration(&LIST_PROMPTS_START);

        match result {
            Ok(prompts) => info!(
                "✅ [{}] Listed {} prompts successfully in {:?}",
                self.server_name,
                prompts.prompts.len(),
                duration
            ),
            Err(e) => warn!(
                "❌ [{}] Failed to list prompts after {:?}: {}",
                self.server_name, duration, e
            ),
        }
    }

    async fn before_list_resources(&self) {
        info!("📁 [{}] Listing resources...", self.server_name);
        Self::set_start_time(&LIST_RESOURCES_START);
    }

    async fn after_list_resources(&self, result: &Result<ListResourcesResult, ServiceError>) {
        let duration = Self::get_duration(&LIST_RESOURCES_START);

        match result {
            Ok(resources) => info!(
                "✅ [{}] Listed {} resources successfully in {:?}",
                self.server_name,
                resources.resources.len(),
                duration
            ),
            Err(e) => warn!(
                "❌ [{}] Failed to list resources after {:?}: {}",
                self.server_name, duration, e
            ),
        }
    }
}

/// Example filtering middleware that removes tools containing "test" in their name.
#[derive(Debug)]
pub struct TestToolFilterMiddleware;

#[async_trait]
impl ProxyMiddleware for TestToolFilterMiddleware {
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

/// Factory function to create default middleware for a server.
pub fn create_default_client_middleware(server_name: &str) -> Vec<Arc<dyn ClientMiddleware>> {
    vec![Arc::new(LoggingClientMiddleware::new(server_name.to_string()))]
}

/// Factory function to create default proxy middleware.
pub fn create_default_proxy_middleware() -> Vec<Arc<dyn ProxyMiddleware>> {
    vec![
        Arc::new(TestToolFilterMiddleware),
        Arc::new(DescriptionEnricherMiddleware),
    ]
} 

#[cfg(test)]
mod tests {
    use super::*;
    use rmcp::model::ListToolsResult;
    use std::time::Duration;
    use tokio::time::sleep;

    /// Test that timing functionality works correctly
    #[tokio::test]
    async fn test_logging_middleware_timing() {
        let middleware = LoggingClientMiddleware::new("test-server".to_string());
        
        // Test list_tools timing - need to scope task-local storage
        LIST_TOOLS_START.scope(RefCell::new(None), async {
            middleware.before_list_tools().await;
            
            // Simulate some work
            sleep(Duration::from_millis(100)).await;
            
            let result = Ok(ListToolsResult {
                tools: vec![],
                next_cursor: None,
            });
            
            middleware.after_list_tools(&result).await;
        }).await;
        
        // Test call_tool timing
        CALL_TOOL_START.scope(RefCell::new(None), async {
            let request = CallToolRequestParam {
                name: "test_tool".to_string().into(),
                arguments: None,
            };
            
            middleware.before_call_tool(&request).await;
            sleep(Duration::from_millis(50)).await;
            
            let call_result = Ok(CallToolResult {
                content: vec![],
                is_error: None,
            });
            
            middleware.after_call_tool(&call_result).await;
        }).await;
    }

    /// Test that timing works correctly with concurrent requests
    #[tokio::test]
    async fn test_concurrent_timing() {
        let middleware = LoggingClientMiddleware::new("test-server".to_string());
        
        // Simulate two concurrent list_tools requests
        let handle1 = tokio::spawn({
            let mw = middleware.clone();
            async move {
                LIST_TOOLS_START.scope(RefCell::new(None), async {
                    mw.before_list_tools().await;
                    sleep(Duration::from_millis(100)).await;
                    let result = Ok(ListToolsResult {
                        tools: vec![],
                        next_cursor: None,
                    });
                    mw.after_list_tools(&result).await;
                }).await;
            }
        });
        
        let handle2 = tokio::spawn({
            let mw = middleware.clone();
            async move {
                LIST_TOOLS_START.scope(RefCell::new(None), async {
                    mw.before_list_tools().await;
                    sleep(Duration::from_millis(50)).await;
                    let result = Ok(ListToolsResult {
                        tools: vec![],
                        next_cursor: None,
                    });
                    mw.after_list_tools(&result).await;
                }).await;
            }
        });
        
        // Both should complete successfully without interfering with each other
        let _ = tokio::try_join!(handle1, handle2).unwrap();
    }


} 