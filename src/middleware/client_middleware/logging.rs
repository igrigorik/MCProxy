//! Logging middleware that logs all client operations with timing.

use async_trait::async_trait;
use rmcp::{
    model::{
        CallToolRequestParam, CallToolResult, ListPromptsResult, ListResourcesResult,
        ListToolsResult,
    },
    service::ServiceError,
};
use std::sync::Arc;
use std::time::Instant;
use tracing::{info, warn};
use tokio::sync::RwLock;
use std::collections::HashMap;
use uuid::Uuid;

use crate::error::Result;
use crate::middleware::client::{ClientMiddleware, MiddlewareResult};
use crate::middleware::ClientMiddlewareFactory;

/// Request timing information with operation context
#[derive(Debug, Clone)]
struct RequestTiming {
    operation: String,
    start_time: Instant,
}

/// Logging middleware that logs all client operations with timing.
/// 
/// This middleware uses request IDs to ensure each request gets its own
/// timing context, making it fully concurrency-safe with accurate timing.
#[derive(Debug, Clone)]
pub struct LoggingClientMiddleware {
    server_name: String,
    /// Thread-safe storage for request timing information
    request_timings: Arc<RwLock<HashMap<Uuid, RequestTiming>>>,
}

impl LoggingClientMiddleware {
    pub fn new(server_name: String) -> Self {
        Self {
            server_name,
            request_timings: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Set start time for a request
    async fn set_start_time(&self, request_id: Uuid, operation: &str) {
        let mut timings = self.request_timings.write().await;
        timings.insert(request_id, RequestTiming {
            operation: operation.to_string(),
            start_time: Instant::now(),
        });
    }

    /// Get and remove the duration for a request
    async fn take_duration(&self, request_id: Uuid) -> Option<(String, std::time::Duration)> {
        let mut timings = self.request_timings.write().await;
        let timing = timings.remove(&request_id)?;
        Some((timing.operation, timing.start_time.elapsed()))
    }

    /// Log the start of an operation
    fn log_start(&self, operation: &str, request_id: Uuid) {
        let emoji = match operation {
            "list_tools" => "📋",
            "call_tool" => "🔧", 
            "list_prompts" => "💬",
            "list_resources" => "📁",
            _ => "🔄",
        };
        info!("{} [{}] {} ({})", emoji, self.server_name, 
              Self::operation_description(operation), 
              Self::short_id(request_id));
    }

    /// Log the completion of an operation
    fn log_completion(&self, operation: &str, request_id: Uuid, duration: std::time::Duration, success: bool, details: &str) {
        let emoji = if success { "✅" } else { "❌" };
        let action = if success { "completed" } else { "failed" };
        
        info!("{} [{}] {} {} in {:?} - {} ({})", 
              emoji, self.server_name, 
              Self::operation_description(operation),
              action, duration, details,
              Self::short_id(request_id));
    }

    /// Get a human-readable description for an operation
    fn operation_description(operation: &str) -> &'static str {
        match operation {
            "list_tools" => "Listing tools",
            "call_tool" => "Tool call",
            "list_prompts" => "Listing prompts", 
            "list_resources" => "Listing resources",
            _ => "Operation",
        }
    }

    /// Get a short version of UUID for logging
    fn short_id(uuid: Uuid) -> String {
        uuid.to_string()[..8].to_string()
    }
}

#[async_trait]
impl ClientMiddleware for LoggingClientMiddleware {
    async fn before_list_tools(&self, request_id: Uuid) {
        self.log_start("list_tools", request_id);
        self.set_start_time(request_id, "list_tools").await;
    }

    async fn after_list_tools(&self, request_id: Uuid, result: &std::result::Result<ListToolsResult, ServiceError>) {
        if let Some((operation, duration)) = self.take_duration(request_id).await {
            match result {
                Ok(tools) => {
                    let details = format!("{} tools", tools.tools.len());
                    self.log_completion(&operation, request_id, duration, true, &details);
                },
                Err(e) => {
                    self.log_completion(&operation, request_id, duration, false, &e.to_string());
                },
            }
        } else {
            // Fallback logging without timing (shouldn't happen in normal operation)
            match result {
                Ok(tools) => info!(
                    "✅ [{}] Listed {} tools successfully ({})",
                    self.server_name, tools.tools.len(), Self::short_id(request_id)
                ),
                Err(e) => warn!(
                    "❌ [{}] Failed to list tools: {} ({})",
                    self.server_name, e, Self::short_id(request_id)
                ),
            }
        }
    }

    async fn before_call_tool(&self, request_id: Uuid, request: &CallToolRequestParam) -> MiddlewareResult {
        info!("🔧 [{}] Calling tool: {} ({})", self.server_name, request.name, Self::short_id(request_id));
        self.set_start_time(request_id, "call_tool").await;
        MiddlewareResult::Continue
    }

    async fn after_call_tool(&self, request_id: Uuid, result: &std::result::Result<CallToolResult, ServiceError>) {
        if let Some((operation, duration)) = self.take_duration(request_id).await {
            match result {
                Ok(_) => {
                    self.log_completion(&operation, request_id, duration, true, "success");
                },
                Err(e) => {
                    self.log_completion(&operation, request_id, duration, false, &e.to_string());
                },
            }
        } else {
            // Fallback logging without timing (shouldn't happen in normal operation)
            match result {
                Ok(_) => info!("✅ [{}] Tool call successful ({})", self.server_name, Self::short_id(request_id)),
                Err(e) => warn!(
                    "❌ [{}] Tool call failed: {} ({})",
                    self.server_name, e, Self::short_id(request_id)
                ),
            }
        }
    }

    async fn before_list_prompts(&self, request_id: Uuid) {
        self.log_start("list_prompts", request_id);
        self.set_start_time(request_id, "list_prompts").await;
    }

    async fn after_list_prompts(&self, request_id: Uuid, result: &std::result::Result<ListPromptsResult, ServiceError>) {
        if let Some((operation, duration)) = self.take_duration(request_id).await {
            match result {
                Ok(prompts) => {
                    let details = format!("{} prompts", prompts.prompts.len());
                    self.log_completion(&operation, request_id, duration, true, &details);
                },
                Err(e) => {
                    self.log_completion(&operation, request_id, duration, false, &e.to_string());
                },
            }
        } else {
            // Fallback logging without timing (shouldn't happen in normal operation)
            match result {
                Ok(prompts) => info!(
                    "✅ [{}] Listed {} prompts successfully ({})",
                    self.server_name, prompts.prompts.len(), Self::short_id(request_id)
                ),
                Err(e) => warn!(
                    "❌ [{}] Failed to list prompts: {} ({})",
                    self.server_name, e, Self::short_id(request_id)
                ),
            }
        }
    }

    async fn before_list_resources(&self, request_id: Uuid) {
        self.log_start("list_resources", request_id);
        self.set_start_time(request_id, "list_resources").await;
    }

    async fn after_list_resources(&self, request_id: Uuid, result: &std::result::Result<ListResourcesResult, ServiceError>) {
        if let Some((operation, duration)) = self.take_duration(request_id).await {
            match result {
                Ok(resources) => {
                    let details = format!("{} resources", resources.resources.len());
                    self.log_completion(&operation, request_id, duration, true, &details);
                },
                Err(e) => {
                    self.log_completion(&operation, request_id, duration, false, &e.to_string());
                },
            }
        } else {
            // Fallback logging without timing (shouldn't happen in normal operation)
            match result {
                Ok(resources) => info!(
                    "✅ [{}] Listed {} resources successfully ({})",
                    self.server_name, resources.resources.len(), Self::short_id(request_id)
                ),
                Err(e) => warn!(
                    "❌ [{}] Failed to list resources: {} ({})",
                    self.server_name, e, Self::short_id(request_id)
                ),
            }
        }
    }
}

/// Factory for creating LoggingClientMiddleware from configuration
#[derive(Debug)]
pub struct LoggingClientFactory;

impl ClientMiddlewareFactory for LoggingClientFactory {
    fn create(&self, server_name: &str, _config: &serde_json::Value) -> Result<Arc<dyn ClientMiddleware>> {
        Ok(Arc::new(LoggingClientMiddleware::new(server_name.to_string())))
    }
    
    fn middleware_type(&self) -> &'static str {
        "logging"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rmcp::model::ListToolsResult;
    use std::time::Duration;
    use tokio::time::sleep;

    /// Test that timing functionality works correctly with request IDs
    #[tokio::test]
    async fn test_logging_middleware_with_request_ids() {
        let middleware = LoggingClientMiddleware::new("test-server".to_string());
        
        let request_id = Uuid::new_v4();
        middleware.before_list_tools(request_id).await;
        sleep(Duration::from_millis(100)).await;
        
        let result = Ok(ListToolsResult {
            tools: vec![],
            next_cursor: None,
        });
        
        middleware.after_list_tools(request_id, &result).await;
    }

    /// Test that concurrent requests work correctly with unique request IDs
    #[tokio::test]
    async fn test_concurrent_requests_with_unique_ids() {
        let middleware = LoggingClientMiddleware::new("test-server".to_string());
        
        // Two concurrent list_tools calls with different request IDs
        let handle1 = {
            let mw = middleware.clone();
            async move {
                let request_id = Uuid::new_v4();
                mw.before_list_tools(request_id).await;
                sleep(Duration::from_millis(100)).await;
                let result = Ok(ListToolsResult {
                    tools: vec![],
                    next_cursor: None,
                });
                mw.after_list_tools(request_id, &result).await;
            }
        };
        
        let handle2 = {
            let mw = middleware.clone();
            async move {
                let request_id = Uuid::new_v4();
                let _result = mw.before_call_tool(request_id, &CallToolRequestParam {
                    name: "test".to_string().into(),
                    arguments: None,
                }).await;
                sleep(Duration::from_millis(50)).await;
                let result = Ok(CallToolResult {
                    content: vec![],
                    is_error: None,
                });
                mw.after_call_tool(request_id, &result).await;
            }
        };
        
        // Both should complete successfully with accurate timing
        tokio::join!(handle1, handle2);
    }

    /// Test that different operations can run concurrently without interference
    #[tokio::test]
    async fn test_concurrent_same_operations() {
        let middleware = LoggingClientMiddleware::new("test-server".to_string());
        
        // Two concurrent list_tools calls - should not interfere with each other
        let request_id1 = Uuid::new_v4();
        let request_id2 = Uuid::new_v4();
        
        let handle1 = {
            let mw = middleware.clone();
            async move {
                mw.before_list_tools(request_id1).await;
                sleep(Duration::from_millis(100)).await;
                let result = Ok(ListToolsResult {
                    tools: vec![],
                    next_cursor: None,
                });
                mw.after_list_tools(request_id1, &result).await;
            }
        };
        
        let handle2 = {
            let mw = middleware.clone();
            async move {
                mw.before_list_tools(request_id2).await;
                sleep(Duration::from_millis(50)).await;
                let result = Ok(ListToolsResult {
                    tools: vec![],
                    next_cursor: None,
                });
                mw.after_list_tools(request_id2, &result).await;
            }
        };
        
        // Both should complete successfully with their own accurate timing
        tokio::join!(handle1, handle2);
    }
} 