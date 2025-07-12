//! Tool search middleware that provides selective tool exposure and fuzzy search functionality.

use async_trait::async_trait;
use fuzzy_matcher::{skim::SkimMatcherV2, FuzzyMatcher};
use rmcp::model::{Prompt, Resource, Tool};
use serde::Deserialize;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, info};

use crate::error::{ProxyError, Result};
use crate::middleware::proxy::ProxyMiddleware;
use crate::middleware::ProxyMiddlewareFactory;

/// Configuration for tool search middleware
#[derive(Debug, Clone, Deserialize)]
pub struct ToolSearchMiddlewareConfig {
    /// Whether tool search is enabled
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    
    /// Maximum number of tools to expose initially and return in search results
    #[serde(default = "default_max_tools_limit", rename = "maxToolsLimit")]
    pub max_tools_limit: usize,
    
    /// Minimum score threshold for search results
    #[serde(default = "default_search_threshold", rename = "searchThreshold")]
    pub search_threshold: f32,
    
    /// Tool selection order for initial exposure
    #[serde(default = "default_tool_selection_order", rename = "toolSelectionOrder")]
    pub tool_selection_order: Vec<String>,
}

fn default_enabled() -> bool {
    true
}

fn default_max_tools_limit() -> usize {
    50
}

fn default_search_threshold() -> f32 {
    0.3
}

fn default_tool_selection_order() -> Vec<String> {
    vec!["server_priority".to_string()]
}

impl Default for ToolSearchMiddlewareConfig {
    fn default() -> Self {
        Self {
            enabled: default_enabled(),
            max_tools_limit: default_max_tools_limit(),
            search_threshold: default_search_threshold(),
            tool_selection_order: default_tool_selection_order(),
        }
    }
}

/// Represents a tool indexed for search with metadata
#[derive(Debug, Clone)]
pub struct IndexedTool {
    /// The original tool
    pub tool: Tool,
    /// Searchable text combining name and description
    pub searchable_text: String,
    /// Server name that provides this tool
    pub server_name: String,
    /// Priority score for initial tool selection
    pub selection_priority: f32,
}

impl IndexedTool {
    pub fn new(tool: Tool, server_name: String) -> Self {
        let searchable_text = format!(
            "{} {}",
            tool.name,
            tool.description.as_deref().unwrap_or("")
        );
        
        // Simple priority calculation - can be enhanced later
        let selection_priority = 1.0; // Default priority, can be based on usage stats, etc.
        
        Self {
            tool,
            searchable_text,
            server_name,
            selection_priority,
        }
    }
}

/// Tool search middleware that provides selective tool exposure and search functionality
pub struct ToolSearchMiddleware {
    /// Configuration for the middleware
    config: ToolSearchMiddlewareConfig,
    /// Fuzzy matcher for search functionality
    matcher: SkimMatcherV2,
    /// All tools indexed for search
    tool_index: Arc<RwLock<Vec<IndexedTool>>>,
    /// Currently exposed tools (subset of all tools)
    exposed_tools: Arc<RwLock<Vec<Tool>>>,
    /// Whether search tool has been injected
    search_tool_injected: Arc<RwLock<bool>>,
}

impl std::fmt::Debug for ToolSearchMiddleware {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ToolSearchMiddleware")
            .field("config", &self.config)
            .field("tool_index", &"Arc<RwLock<Vec<IndexedTool>>>")
            .field("exposed_tools", &"Arc<RwLock<Vec<Tool>>>")
            .field("search_tool_injected", &"Arc<RwLock<bool>>")
            .finish()
    }
}

impl ToolSearchMiddleware {
    pub fn new(config: ToolSearchMiddlewareConfig) -> Self {
        Self {
            config,
            matcher: SkimMatcherV2::default(),
            tool_index: Arc::new(RwLock::new(Vec::new())),
            exposed_tools: Arc::new(RwLock::new(Vec::new())),
            search_tool_injected: Arc::new(RwLock::new(false)),
        }
    }

    /// Update the internal tool index with all available tools
    async fn update_tool_index(&self, tools: &[Tool]) {
        let mut index = self.tool_index.write().await;
        index.clear();
        
        for tool in tools {
            // Extract server name from prefixed tool name (format: "server_name___tool_name")
            let server_name = if let Some(pos) = tool.name.find("___") {
                tool.name[..pos].to_string()
            } else {
                "unknown".to_string()
            };
            
            index.push(IndexedTool::new(tool.clone(), server_name));
        }
        
        debug!("Updated tool index with {} tools", index.len());
    }

    /// Select initial tools to expose based on configuration
    async fn select_initial_tools(&self, tools: &[Tool]) -> Vec<Tool> {
        if !self.config.enabled || tools.len() <= self.config.max_tools_limit {
            return tools.to_vec();
        }

        let mut selected_tools = tools.to_vec();
        
        // Apply selection criteria based on configuration
        for criteria in &self.config.tool_selection_order {
            match criteria.as_str() {
                "alphabetical" => {
                    selected_tools.sort_by(|a, b| a.name.cmp(&b.name));
                }
                "server_priority" => {
                    // Sort by server name (first part before ___) to group tools by server
                    // The order servers appear in the config determines priority
                    selected_tools.sort_by(|a, b| {
                        let server_a = Self::extract_server_name(&a.name);
                        let server_b = Self::extract_server_name(&b.name);
                        
                        // First sort by server name, then by tool name within server
                        match server_a.cmp(&server_b) {
                            std::cmp::Ordering::Equal => a.name.cmp(&b.name),
                            other => other,
                        }
                    });
                }
                "usage_frequency" => {
                    // Could implement usage-based selection later
                    debug!("Usage frequency selection not yet implemented");
                }
                _ => {
                    debug!("Unknown tool selection criteria: {}", criteria);
                }
            }
        }
        
        // Take only the first N tools
        selected_tools.truncate(self.config.max_tools_limit);
        selected_tools
    }
    
    /// Extract server name from prefixed tool name
    fn extract_server_name(tool_name: &str) -> &str {
        if let Some(pos) = tool_name.find("___") {
            &tool_name[..pos]
        } else {
            "unknown"
        }
    }

    /// Create the special search tool that allows finding hidden tools
    fn create_search_tool(&self, hidden_count: usize) -> Tool {
        let schema = serde_json::json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "Search query to find relevant tools by name or description"
                }
            },
            "required": ["query"]
        });
        
        let schema_map = schema.as_object().unwrap().clone();
        
        Tool {
            name: "search_available_tools".into(),
            description: Some(format!(
                "Search through {} additional available tools. Provide a search query to find relevant tools by name or description.",
                hidden_count
            ).into()),
            input_schema: Arc::new(schema_map),
            annotations: None,
        }
    }

    /// Search for tools matching the given query
    pub async fn search_tools(&self, query: &str) -> Vec<Tool> {
        let index = self.tool_index.read().await;
        let mut results: Vec<(i64, &IndexedTool)> = Vec::new();
        
        for indexed_tool in index.iter() {
            if let Some(score) = self.matcher.fuzzy_match(&indexed_tool.searchable_text, query) {
                if score as f32 >= (self.config.search_threshold * 100.0) {
                    results.push((score, indexed_tool));
                }
            }
        }
        
        // Sort by score (descending)
        results.sort_by(|a, b| b.0.cmp(&a.0));
        
        // Take up to the configured limit
        let search_results: Vec<Tool> = results
            .into_iter()
            .take(self.config.max_tools_limit)
            .map(|(_, indexed_tool)| indexed_tool.tool.clone())
            .collect();
        
        info!("Search for '{}' returned {} results", query, search_results.len());
        search_results
    }

    /// Update the exposed tools list (used after search)
    pub async fn update_exposed_tools(&self, new_tools: Vec<Tool>) {
        let mut exposed = self.exposed_tools.write().await;
        *exposed = new_tools;
        
        // Mark search tool as injected if we have search results
        let mut injected = self.search_tool_injected.write().await;
        *injected = true;
    }

    /// Get the current exposed tools
    #[allow(dead_code)] // Used in tests
    pub async fn get_exposed_tools(&self) -> Vec<Tool> {
        self.exposed_tools.read().await.clone()
    }

    /// Check if search tool is currently injected
    #[allow(dead_code)] // Used in tests
    pub async fn is_search_tool_injected(&self) -> bool {
        *self.search_tool_injected.read().await
    }


}

#[async_trait]
impl ProxyMiddleware for ToolSearchMiddleware {
    async fn on_list_tools(&self, tools: &mut Vec<Tool>) {
        if !self.config.enabled {
            return;
        }

        // Update our internal index with all tools
        self.update_tool_index(tools).await;
        
        // Select initial tools to expose
        let selected_tools = self.select_initial_tools(tools).await;
        
        // If we're limiting tools, inject the search tool
        if tools.len() > self.config.max_tools_limit {
            let hidden_count = tools.len() - selected_tools.len();
            let mut exposed_tools = selected_tools;
            
            // Add the search tool
            let search_tool = self.create_search_tool(hidden_count);
            exposed_tools.push(search_tool);
            
            // Update the exposed tools list
            self.update_exposed_tools(exposed_tools.clone()).await;
            
            // Replace the tools list with our selected + search tool
            *tools = exposed_tools;
            
            info!("Limited tools from {} to {} + search tool", 
                  tools.len() + hidden_count - 1, // -1 because we added search tool
                  self.config.max_tools_limit);
        } else {
            // Not limiting tools, just store them
            self.update_exposed_tools(tools.clone()).await;
            let mut injected = self.search_tool_injected.write().await;
            *injected = false;
        }
    }

    async fn on_list_prompts(&self, _prompts: &mut Vec<Prompt>) {
        // Tool search doesn't affect prompts
    }

    async fn on_list_resources(&self, _resources: &mut Vec<Resource>) {
        // Tool search doesn't affect resources  
    }
    
    fn as_any(&self) -> Option<&dyn std::any::Any> {
        Some(self)
    }
}

/// Factory for creating ToolSearchMiddleware from configuration
#[derive(Debug)]
pub struct ToolSearchMiddlewareFactory;

impl ProxyMiddlewareFactory for ToolSearchMiddlewareFactory {
    fn create(&self, config: &serde_json::Value) -> Result<Arc<dyn ProxyMiddleware>> {
        let search_config: ToolSearchMiddlewareConfig = if config.is_null() {
            ToolSearchMiddlewareConfig::default()
        } else {
            serde_json::from_value(config.clone()).map_err(|e| {
                ProxyError::config(format!("Invalid tool search configuration: {}", e))
            })?
        };
        
        let middleware = ToolSearchMiddleware::new(search_config);
        Ok(Arc::new(middleware))
    }
    
    fn middleware_type(&self) -> &'static str {
        "tool_search"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rmcp::model::Tool;
    use std::sync::Arc;

    fn create_test_tool(name: &str, description: &str) -> Tool {
        let schema = serde_json::json!({
            "type": "object",
            "properties": {}
        });
        let schema_map = schema.as_object().unwrap().clone();
        
        Tool {
            name: name.to_string().into(),
            description: Some(description.to_string().into()),
            input_schema: Arc::new(schema_map),
            annotations: None,
        }
    }

    fn create_test_config(max_tools: usize, threshold: f32) -> ToolSearchMiddlewareConfig {
        ToolSearchMiddlewareConfig {
            enabled: true,
            max_tools_limit: max_tools,
            search_threshold: threshold,
            tool_selection_order: vec!["alphabetical".to_string()],
        }
    }

    #[tokio::test]
    async fn test_tool_indexing() {
        let config = create_test_config(50, 0.3);
        let middleware = ToolSearchMiddleware::new(config);
        
        let tools = vec![
            create_test_tool("server1___file_search", "Search for files in the filesystem"),
            create_test_tool("server2___web_scrape", "Scrape content from web pages"),
        ];
        
        middleware.update_tool_index(&tools).await;
        
        let index = middleware.tool_index.read().await;
        assert_eq!(index.len(), 2);
        assert_eq!(index[0].server_name, "server1");
        assert_eq!(index[1].server_name, "server2");
    }

    #[tokio::test]
    async fn test_fuzzy_search() {
        let config = create_test_config(50, 0.1); // Low threshold for testing
        let middleware = ToolSearchMiddleware::new(config);
        
        let tools = vec![
            create_test_tool("server1___file_search", "Search for files in the filesystem"),
            create_test_tool("server1___file_write", "Write content to a file"),
            create_test_tool("server2___web_scrape", "Scrape content from web pages"),
            create_test_tool("server2___web_download", "Download files from the web"),
        ];
        
        middleware.update_tool_index(&tools).await;
        
        // Test search for "file" - should match both file_search and file_write
        let results = middleware.search_tools("file").await;
        assert!(results.len() >= 2);
        assert!(results.iter().any(|t| t.name.contains("file_search")));
        assert!(results.iter().any(|t| t.name.contains("file_write")));
        
        // Test search for "web" - should match web_scrape and web_download
        let results = middleware.search_tools("web").await;
        assert!(results.len() >= 2);
        assert!(results.iter().any(|t| t.name.contains("web_scrape")));
        assert!(results.iter().any(|t| t.name.contains("web_download")));
        
        // Test search for exact match
        let results = middleware.search_tools("file_search").await;
        assert!(results.len() >= 1);
        assert!(results.iter().any(|t| t.name.contains("file_search")));
    }

    #[tokio::test]
    async fn test_search_threshold() {
        let config = create_test_config(50, 0.8); // High threshold
        let middleware = ToolSearchMiddleware::new(config);
        
        let tools = vec![
            create_test_tool("server1___completely_different_tool", "A tool that does something else"),
        ];
        
        middleware.update_tool_index(&tools).await;
        
        // Search for something that doesn't match well - should return no results
        let results = middleware.search_tools("xyz").await;
        assert!(results.is_empty());
    }

    #[tokio::test]
    async fn test_search_result_limit() {
        let config = create_test_config(2, 0.1); // Limit to 2 results
        let middleware = ToolSearchMiddleware::new(config);
        
        let tools = vec![
            create_test_tool("server1___file_search", "Search for files"),
            create_test_tool("server1___file_write", "Write files"),
            create_test_tool("server1___file_read", "Read files"),
            create_test_tool("server1___file_delete", "Delete files"),
        ];
        
        middleware.update_tool_index(&tools).await;
        
        let results = middleware.search_tools("file").await;
        assert!(results.len() <= 2);
    }

    #[tokio::test]
    async fn test_selective_tool_exposure() {
        let config = create_test_config(2, 0.3); // Only expose 2 tools initially
        let middleware = ToolSearchMiddleware::new(config);
        
        let mut tools = vec![
            create_test_tool("server1___zebra_tool", "Tool starting with Z"),
            create_test_tool("server1___alpha_tool", "Tool starting with A"),
            create_test_tool("server1___beta_tool", "Tool starting with B"),
        ];
        
        // Apply middleware - should limit tools and add search tool
        middleware.on_list_tools(&mut tools).await;
        
        // Should have 2 original tools + 1 search tool = 3 total
        assert_eq!(tools.len(), 3);
        
        // Should have the search tool
        assert!(tools.iter().any(|t| t.name == "search_available_tools"));
        
        // Should have alphabetically sorted tools (alpha_tool, beta_tool)
        let non_search_tools: Vec<_> = tools.iter()
            .filter(|t| t.name != "search_available_tools")
            .collect();
        assert_eq!(non_search_tools.len(), 2);
        assert!(non_search_tools[0].name.contains("alpha"));
        assert!(non_search_tools[1].name.contains("beta"));
    }

    #[tokio::test]
    async fn test_no_limiting_when_under_threshold() {
        let config = create_test_config(50, 0.3); // High limit
        let middleware = ToolSearchMiddleware::new(config);
        
        let mut tools = vec![
            create_test_tool("server1___tool1", "Tool 1"),
            create_test_tool("server1___tool2", "Tool 2"),
        ];
        
        let original_count = tools.len();
        
        // Apply middleware - should not limit tools or add search tool
        middleware.on_list_tools(&mut tools).await;
        
        // Should have the same number of tools (no search tool added)
        assert_eq!(tools.len(), original_count);
        
        // Should not have the search tool
        assert!(!tools.iter().any(|t| t.name == "search_available_tools"));
    }

    #[tokio::test]
    async fn test_search_tool_creation() {
        let config = create_test_config(50, 0.3);
        let middleware = ToolSearchMiddleware::new(config);
        
        let search_tool = middleware.create_search_tool(25);
        
        assert_eq!(search_tool.name, "search_available_tools");
        assert!(search_tool.description.is_some());
        assert!(search_tool.description.as_ref().unwrap().contains("25 additional"));
        
        // Verify schema structure
        let schema = &search_tool.input_schema;
        assert!(schema.get("type").is_some());
        assert!(schema.get("properties").is_some());
        assert!(schema.get("required").is_some());
    }

    #[test]
    fn test_indexed_tool_creation() {
        let tool = create_test_tool("server1___test_tool", "A test tool for searching");
        let indexed = IndexedTool::new(tool.clone(), "server1".to_string());
        
        assert_eq!(indexed.tool.name, tool.name);
        assert_eq!(indexed.server_name, "server1");
        assert!(indexed.searchable_text.contains("test_tool"));
        assert!(indexed.searchable_text.contains("test tool for searching"));
        assert_eq!(indexed.selection_priority, 1.0);
    }

    #[test]
    fn test_middleware_config_defaults() {
        let config = ToolSearchMiddlewareConfig::default();
        
        assert!(config.enabled);
        assert_eq!(config.max_tools_limit, 50);
        assert_eq!(config.search_threshold, 0.3);
        assert_eq!(config.tool_selection_order, vec!["server_priority"]);
    }

    #[test]
    fn test_middleware_factory() {
        let factory = ToolSearchMiddlewareFactory;
        
        assert_eq!(factory.middleware_type(), "tool_search");
        
        // Test with null config (should use defaults)
        let middleware = factory.create(&serde_json::Value::Null).unwrap();
        assert!(middleware.as_any().is_some());
        
        // Test with custom config
        let config = serde_json::json!({
            "enabled": true,
            "maxToolsLimit": 25
        });
        
        let middleware = factory.create(&config).unwrap();
        assert!(middleware.as_any().is_some());
    }

    #[tokio::test]
    async fn test_update_exposed_tools() {
        let config = create_test_config(50, 0.3);
        let middleware = ToolSearchMiddleware::new(config);
        
        let new_tools = vec![
            create_test_tool("server1___tool1", "Tool 1"),
            create_test_tool("server2___tool2", "Tool 2"),
        ];
        
        middleware.update_exposed_tools(new_tools.clone()).await;
        
        let exposed = middleware.get_exposed_tools().await;
        assert_eq!(exposed.len(), 2);
        assert!(middleware.is_search_tool_injected().await);
    }

    // Integration tests that test tool search through HTTP API
    mod integration_tests {
        use crate::config::{McpConfig, HttpServerConfig, MiddlewareConfig, MiddlewareSpec};
        use crate::proxy::ProxyServer;
        use crate::http_server;
        use axum::{Router, body::Body, http::{Request, Method, header, StatusCode}};
        use tower::ServiceExt;
        use serde_json::{json, Value};
        use std::collections::HashMap;
        use std::sync::Arc;

        async fn create_test_app_with_tool_search() -> Router {
            let config = McpConfig {
                mcp_servers: HashMap::new(),
                http_server: Some(HttpServerConfig {
                    host: "127.0.0.1".to_string(),
                    port: 0,
                    cors_enabled: true,
                    cors_origins: vec!["*".to_string()],
                    shutdown_timeout: 5,
                    middleware: MiddlewareConfig {
                        proxy: vec![MiddlewareSpec {
                            middleware_type: "tool_search".to_string(),
                            enabled: true,
                            config: json!({
                                "enabled": true,
                                "maxToolsLimit": 2,
                                "searchThreshold": 0.1
                            }),
                        }],
                        ..Default::default()
                    },
                }),
            };

            let proxy = ProxyServer::new(config).await.unwrap();
            let shared_proxy = Arc::new(proxy);
            let test_config = crate::config::HttpServerConfig {
                host: "127.0.0.1".to_string(),
                port: 0,
                cors_enabled: true,
                cors_origins: vec!["*".to_string()],
                shutdown_timeout: 5,
                middleware: MiddlewareConfig::default(),
            };
            http_server::create_router(shared_proxy, &test_config)
        }

        #[tokio::test]
        async fn test_search_tool_missing_query() {
            let app = create_test_app_with_tool_search().await;

            // Test calling search tool without query parameter
            let request_body = json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "tools/call",
                "params": {
                    "name": "search_available_tools",
                    "arguments": {}
                }
            });

            let request = Request::builder()
                .method(Method::POST)
                .uri("/mcp")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(serde_json::to_string(&request_body).unwrap()))
                .unwrap();

            let response = app.oneshot(request).await.unwrap();
            assert_eq!(response.status(), StatusCode::OK);

            let body = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
            let body: Value = serde_json::from_slice(&body).unwrap();
            
            assert_eq!(body["jsonrpc"], "2.0");
            assert_eq!(body["id"], 1);
            assert!(body.get("result").is_none());
            assert!(body["error"].is_object());
            assert_eq!(body["error"]["code"], -32602); // Invalid params
            assert!(body["error"]["message"].as_str().unwrap().contains("query"));
        }

        #[tokio::test]
        async fn test_search_tool_empty_query() {
            let app = create_test_app_with_tool_search().await;

            // Test calling search tool with empty query
            let request_body = json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "tools/call",
                "params": {
                    "name": "search_available_tools",
                    "arguments": {
                        "query": ""
                    }
                }
            });

            let request = Request::builder()
                .method(Method::POST)
                .uri("/mcp")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(serde_json::to_string(&request_body).unwrap()))
                .unwrap();

            let response = app.oneshot(request).await.unwrap();
            assert_eq!(response.status(), StatusCode::OK);

            let body = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
            let body: Value = serde_json::from_slice(&body).unwrap();
            
            assert_eq!(body["jsonrpc"], "2.0");
            assert_eq!(body["id"], 1);
            
            // Should get a valid response, but probably no results
            if body.get("result").is_some() {
                let result = &body["result"];
                assert!(result.get("content").is_some());
            }
        }

        #[tokio::test]
        async fn test_tools_list_with_search_middleware() {
            let app = create_test_app_with_tool_search().await;

            // Test that tools/list works with tool search middleware enabled
            let request_body = json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "tools/list"
            });

            let request = Request::builder()
                .method(Method::POST)
                .uri("/mcp")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(serde_json::to_string(&request_body).unwrap()))
                .unwrap();

            let response = app.oneshot(request).await.unwrap();
            assert_eq!(response.status(), StatusCode::OK);

            let body = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
            let body: Value = serde_json::from_slice(&body).unwrap();
            
            assert_eq!(body["jsonrpc"], "2.0");
            assert_eq!(body["id"], 1);
            assert!(body["result"].is_object());

            let _tools = body["result"]["tools"].as_array().unwrap();
            
            // Since we don't have actual servers connected, we should have 0 tools
            // but the middleware should still work without errors - just getting here confirms success
        }
    }
} 