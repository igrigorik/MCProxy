//! Defines the `ClientMiddleware` for intercepting calls to downstream servers.

use async_trait::async_trait;
use rmcp::{
    model::{
        CallToolRequestParam, CallToolResult, ListPromptsResult, ListResourcesResult,
        ListToolsResult,
    },
    service::ServiceError,
    RoleClient,
};
use std::sync::Arc;
use uuid::Uuid;

/// A running service with client middleware applied.
///
/// This struct wraps an existing `rmcp::service::RunningService` and applies
/// a stack of `ClientMiddleware` to its methods.
pub struct MiddlewareAppliedClient {
    /// The inner client that communicates with the downstream server.
    inner: Arc<rmcp::service::RunningService<RoleClient, ()>>,
    /// The stack of middleware to apply to client calls.
    middleware: Vec<Arc<dyn ClientMiddleware>>,
}

impl std::fmt::Debug for MiddlewareAppliedClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MiddlewareAppliedClient")
            .field("inner", &"<RunningService>")
            .field("middleware_count", &self.middleware.len())
            .finish()
    }
}

impl MiddlewareAppliedClient {
    /// Creates a new `MiddlewareAppliedClient` with the given inner client and middleware.
    pub fn new(
        inner: Arc<rmcp::service::RunningService<RoleClient, ()>>,
        middleware: Vec<Arc<dyn ClientMiddleware>>,
    ) -> Self {
        Self { inner, middleware }
    }

    /// Lists tools from the downstream server, applying middleware.
    pub async fn list_tools(&self) -> Result<ListToolsResult, ServiceError> {
        let request_id = Uuid::new_v4();
        
        // Apply pre-call middleware
        for mw in &self.middleware {
            mw.before_list_tools(request_id).await;
        }

        // Make the actual call
        let mut result = self.inner.list_tools(None).await;

        // Apply post-call middleware
        for mw in &self.middleware {
            mw.after_list_tools(request_id, &result).await;
        }

        // Apply result modification middleware (only if successful)
        if let Ok(ref mut tools_result) = result {
            for mw in &self.middleware {
                mw.modify_list_tools_result(request_id, tools_result).await;
            }
        }

        result
    }

    /// Calls a tool on the downstream server, applying middleware.
    pub async fn call_tool(
        &self,
        request: CallToolRequestParam,
    ) -> Result<CallToolResult, ServiceError> {
        let request_id = Uuid::new_v4();
        
        // Apply pre-call middleware
        for mw in &self.middleware {
            mw.before_call_tool(request_id, &request).await;
        }

        // Make the actual call
        let result = self.inner.call_tool(request).await;

        // Apply post-call middleware
        for mw in &self.middleware {
            mw.after_call_tool(request_id, &result).await;
        }

        result
    }

    /// Lists prompts from the downstream server, applying middleware.
    pub async fn list_prompts(&self) -> Result<ListPromptsResult, ServiceError> {
        let request_id = Uuid::new_v4();
        
        // Apply pre-call middleware
        for mw in &self.middleware {
            mw.before_list_prompts(request_id).await;
        }

        // Make the actual call
        let mut result = self.inner.list_prompts(None).await;

        // Apply post-call middleware
        for mw in &self.middleware {
            mw.after_list_prompts(request_id, &result).await;
        }

        // Apply result modification middleware (only if successful)
        if let Ok(ref mut prompts_result) = result {
            for mw in &self.middleware {
                mw.modify_list_prompts_result(request_id, prompts_result).await;
            }
        }

        result
    }

    /// Lists resources from the downstream server, applying middleware.
    pub async fn list_resources(&self) -> Result<ListResourcesResult, ServiceError> {
        let request_id = Uuid::new_v4();
        
        // Apply pre-call middleware
        for mw in &self.middleware {
            mw.before_list_resources(request_id).await;
        }

        // Make the actual call
        let mut result = self.inner.list_resources(None).await;

        // Apply post-call middleware
        for mw in &self.middleware {
            mw.after_list_resources(request_id, &result).await;
        }

        // Apply result modification middleware (only if successful)
        if let Ok(ref mut resources_result) = result {
            for mw in &self.middleware {
                mw.modify_list_resources_result(request_id, resources_result).await;
            }
        }

        result
    }
}

/// A trait for middleware that intercepts calls to a single downstream MCP server.
///
/// This middleware is useful for tasks like observability, caching, or adding
/// retry logic for individual downstream servers.
/// 
/// Each method receives a request_id UUID that uniquely identifies the request,
/// allowing middleware to correlate before/after calls and maintain per-request state.
#[async_trait]
pub trait ClientMiddleware: Send + Sync {
    /// Called before `list_tools` is invoked on the downstream server.
    async fn before_list_tools(&self, _request_id: Uuid) {}

    /// Called after `list_tools` completes (whether successful or failed).
    async fn after_list_tools(&self, _request_id: Uuid, _result: &Result<ListToolsResult, ServiceError>) {}

    /// Called to modify the successful result of `list_tools`.
    /// This is called after `after_list_tools` and only if the result was successful.
    async fn modify_list_tools_result(&self, _request_id: Uuid, _result: &mut ListToolsResult) {}

    /// Called before `call_tool` is invoked on the downstream server.
    async fn before_call_tool(&self, _request_id: Uuid, _request: &CallToolRequestParam) {}

    /// Called after `call_tool` completes (whether successful or failed).
    async fn after_call_tool(&self, _request_id: Uuid, _result: &Result<CallToolResult, ServiceError>) {}

    /// Called before `list_prompts` is invoked on the downstream server.
    async fn before_list_prompts(&self, _request_id: Uuid) {}

    /// Called after `list_prompts` completes (whether successful or failed).
    async fn after_list_prompts(&self, _request_id: Uuid, _result: &Result<ListPromptsResult, ServiceError>) {}

    /// Called to modify the successful result of `list_prompts`.
    /// This is called after `after_list_prompts` and only if the result was successful.
    async fn modify_list_prompts_result(&self, _request_id: Uuid, _result: &mut ListPromptsResult) {}

    /// Called before `list_resources` is invoked on the downstream server.
    async fn before_list_resources(&self, _request_id: Uuid) {}

    /// Called after `list_resources` completes (whether successful or failed).
    async fn after_list_resources(&self, _request_id: Uuid, _result: &Result<ListResourcesResult, ServiceError>) {}

    /// Called to modify the successful result of `list_resources`.
    /// This is called after `after_list_resources` and only if the result was successful.
    async fn modify_list_resources_result(&self, _request_id: Uuid, _result: &mut ListResourcesResult) {}
} 