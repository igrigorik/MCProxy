//! The core proxy server that aggregates multiple MCP servers.
//! 
//! This module implements the main proxy logic that:
//! - Connects to multiple MCP servers (both stdio and HTTP)
//! - Manages server lifecycles and graceful shutdown
//! - Aggregates tools, prompts, and resources from all connected servers
//! - Routes tool calls to the appropriate backend server
//! 
//! # Server Name Prefixing
//! 
//! To avoid naming conflicts between servers, all tool, prompt, and resource names
//! are prefixed with the server name using the format: `server_name___item_name`
//! 
//! # Connection Management
//! 
//! The proxy maintains persistent connections to all configured servers and handles:
//! - Automatic reconnection on failure (TODO)
//! - Graceful shutdown with configurable timeout
//! - Process management for stdio-based servers

use crate::config::{HttpConfig, McpConfig, ServerConfig, StdioConfig};
use crate::error::{ProxyError, Result};
use crate::middleware::{client::MiddlewareAppliedClient, MiddlewareManager};
use futures::future;
use futures::stream::BoxStream;
use rmcp::{
    model::*,
    service::{RequestContext, RoleServer, ServiceExt},
    transport::streamable_http_client::{
        StreamableHttpClient, StreamableHttpClientTransport,
        StreamableHttpClientTransportConfig, StreamableHttpError, StreamableHttpPostResponse,
    },
    Error as McpError, ServerHandler,
};
use sse_stream::{Error as SseError, Sse};
use std::{collections::HashMap, sync::Arc};
use tokio::process::{Child, Command};
use tokio::sync::RwLock;
use tracing::{info, warn};
use reqwest;



/// The main proxy server that aggregates multiple MCP servers
#[derive(Clone)]
pub struct ProxyServer {
    /// Connected MCP servers mapped by name
    pub servers: Arc<RwLock<HashMap<String, ConnectedServer>>>,
    /// Configuration for all servers
    config: Arc<McpConfig>,
    /// Process handles for stdio servers that need cleanup
    process_handles: Arc<RwLock<HashMap<String, Child>>>,
    /// The middleware manager
    middleware: Arc<MiddlewareManager>,
}

/// Represents a connected MCP server
#[derive(Debug, Clone)]
#[allow(dead_code)] // Fields will be used when we implement server introspection API
pub struct ConnectedServer {
    /// Server name
    pub name: String,
    /// Available tools from this server
    pub tools: Vec<Tool>,
    /// Available prompts from this server
    pub prompts: Vec<Prompt>,
    /// Available resources from this server
    pub resources: Vec<Resource>,
    /// Client connection to this server
    pub client: Arc<MiddlewareAppliedClient>,
}

/// A wrapper for a StreamableHttpClient that adds a Bearer token to every request.
#[derive(Clone)]
struct BearerAuthClient<C> {
    inner: C,
    token: String,
}

impl<C> BearerAuthClient<C> {
    fn new(inner: C, token: String) -> Self {
        Self { inner, token }
    }
}

impl<C> StreamableHttpClient for BearerAuthClient<C>
where
    C: StreamableHttpClient + Send + Sync,
    C::Error: 'static,
{
    type Error = C::Error;

    fn get_stream<'a>(
        &'a self,
        uri: Arc<str>,
        session_id: Arc<str>,
        last_event_id: Option<String>,
        _auth_token: Option<String>,
    ) -> impl std::future::Future<Output = std::result::Result<BoxStream<'static, std::result::Result<Sse, SseError>>, StreamableHttpError<Self::Error>>> + Send + 'a {
        self.inner
            .get_stream(uri, session_id, last_event_id, Some(self.token.clone()))
    }

    fn post_message<'a>(
        &'a self,
        uri: Arc<str>,
        message: JsonRpcMessage<ClientRequest, ClientResult, ClientNotification>,
        session_id: Option<Arc<str>>,
        _auth_token: Option<String>,
    ) -> impl std::future::Future<Output = std::result::Result<StreamableHttpPostResponse, StreamableHttpError<Self::Error>>> + Send + 'a {
        self.inner
            .post_message(uri, message, session_id, Some(self.token.clone()))
    }

    fn delete_session<'a>(
        &'a self,
        uri: Arc<str>,
        session_id: Arc<str>,
        _auth_token: Option<String>,
    ) -> impl std::future::Future<Output = std::result::Result<(), StreamableHttpError<Self::Error>>> + Send + 'a {
        self.inner
            .delete_session(uri, session_id, Some(self.token.clone()))
    }
}

impl ProxyServer {
    /// Prefix a name with the server name using the standard separator
    fn prefix_with_server(server_name: &str, name: &str) -> String {
        format!("{}___{}", server_name, name)
    }
    
    /// Extract server name and actual name from a prefixed string
    fn extract_server_and_name(prefixed: &str) -> std::result::Result<(&str, &str), ProxyError> {
        prefixed.find("___")
            .map(|pos| (&prefixed[..pos], &prefixed[pos + 3..]))
            .ok_or_else(|| ProxyError::invalid_format(
                format!("Invalid prefixed name format: {}", prefixed)
            ))
    }
    


    /// Creates a new proxy server and connects to all configured MCP servers
    pub async fn new(config: McpConfig) -> Result<Self> {
        let servers = Arc::new(RwLock::new(HashMap::new()));
        let process_handles = Arc::new(RwLock::new(HashMap::new()));
        let middleware = Arc::new(MiddlewareManager::from_config(&config)?);
        let proxy = ProxyServer {
            servers: servers.clone(),
            config: Arc::new(config.clone()),
            process_handles,
            middleware,
        };

        // Connect to all configured servers
        proxy.connect_to_all_servers().await?;

        // Initialize tool search index after all servers are connected
        proxy.initialize_tool_search_index().await;

        Ok(proxy)
    }

    /// Gracefully shutdown all connected servers
    pub async fn shutdown(&self) {
        info!("Shutting down proxy server and all connected MCP servers...");
        
        // Get shutdown timeout from config
        let shutdown_timeout = self.config.http_server
            .as_ref()
            .map(|config| config.shutdown_timeout)
            .unwrap_or(5);

        // Clear server connections first. This will drop client services.
        self.servers.write().await.clear();

        let mut handles = self.process_handles.write().await;
        let mut shutdown_futs = Vec::new();

        for (name, mut child) in handles.drain() {
            let timeout_secs = shutdown_timeout;
            let fut = async move {
                info!("Shutting down MCP server process: {}", name);

                #[cfg(unix)]
                if let Some(pid) = child.id() {
                    info!(
                        "Sending SIGTERM to process group for {} (pgid: {})",
                        name, pid
                    );
                    // Use libc to send SIGTERM to the entire process group for graceful shutdown
                    unsafe {
                        libc::kill(-(pid as i32), libc::SIGTERM);
                    }
                }

                #[cfg(not(unix))]
                {
                    // On Windows, start_kill is the best we can do for now
                    let _ = child.start_kill();
                }

                match tokio::time::timeout(std::time::Duration::from_secs(timeout_secs), child.wait()).await {
                    Ok(Ok(status)) => info!("Process {} exited with status: {}", name, status),
                    Ok(Err(e)) => warn!("Error waiting for process {}: {}", name, e),
                    Err(_) => {
                        warn!(
                            "Process {} did not terminate gracefully, sending SIGKILL",
                            name
                        );
                        if let Err(e) = child.kill().await {
                            warn!("Failed to kill process {}: {}", name, e);
                        }
                    }
                }
            };
            shutdown_futs.push(tokio::spawn(fut));
        }

        future::join_all(shutdown_futs).await;

        info!("Proxy server shutdown complete");
    }

    /// Connect to all configured MCP servers
    async fn connect_to_all_servers(&self) -> Result<()> {
        let connection_futures: Vec<_> = self
            .config
            .mcp_servers
            .iter()
            .map(|(name, server_config)| {
                let name = name.clone();
                let server_config = server_config.clone();
                let servers = self.servers.clone();
                let process_handles = self.process_handles.clone();
                let middleware = self.middleware.clone();

                async move {
                    match Self::connect_to_server_impl(
                        name.clone(),
                        server_config,
                        process_handles,
                        middleware,
                        &self.config,
                    )
                    .await
                    {
                        Ok((server_name, mut connected_server)) => {
                            // Populate tools, prompts, and resources for the server
                            Self::populate_server_capabilities(&mut connected_server).await;
                            servers
                                .write()
                                .await
                                .insert(server_name.clone(), connected_server);
                            Ok(server_name)
                        }
                        Err(e) => {
                            tracing::error!("Failed to connect to server {}: {}", name, e);
                            Err((name, e))
                        }
                    }
                }
            })
            .collect();

        // Wait for all connections to complete, but don't fail if some fail
        let results = future::join_all(connection_futures).await;

        let mut successful_connections = Vec::new();
        let mut failed_connections = Vec::new();

        for result in results {
            match result {
                Ok(server_name) => successful_connections.push(server_name),
                Err((server_name, error)) => failed_connections.push((server_name, error)),
            }
        }

        let connected_count = successful_connections.len();
        let failed_count = failed_connections.len();

        if connected_count > 0 {
            tracing::info!("Successfully connected to {} servers", connected_count);
            if failed_count > 0 {
                tracing::warn!("Failed to connect to {} servers:", failed_count);
                for (name, error) in failed_connections {
                    tracing::warn!("  └─ ❌ {}: {}", name, error);
                }
            }
            Ok(())
        } else if self.config.mcp_servers.is_empty() {
            // Allow starting with no servers configured (useful for testing)
            tracing::info!("No MCP servers configured");
            Ok(())
        } else {
            Err(ProxyError::config("Failed to connect to any MCP servers"))
        }
    }

    /// Populate tools, prompts, and resources for a connected server
    async fn populate_server_capabilities(server: &mut ConnectedServer) {
        // Fetch all capabilities in parallel
        let (tools_result, prompts_result, resources_result) = tokio::join!(
            server.client.list_tools(),
            server.client.list_prompts(),
            server.client.list_resources()
        );

        // Process tools
        if let Ok(tools) = tools_result {
            server.tools = tools.tools;
            tracing::debug!(
                "Populated {} tools for server {}",
                server.tools.len(),
                server.name
            );
        } else if let Err(e) = tools_result {
            tracing::warn!(
                "Failed to fetch tools from server {}: {}",
                server.name,
                e
            );
        }

        // Process prompts
        if let Ok(prompts) = prompts_result {
            server.prompts = prompts.prompts;
            tracing::debug!(
                "Populated {} prompts for server {}",
                server.prompts.len(),
                server.name
            );
        } else if let Err(e) = prompts_result {
            tracing::warn!(
                "Failed to fetch prompts from server {}: {}",
                server.name,
                e
            );
        }

        // Process resources
        if let Ok(resources) = resources_result {
            server.resources = resources.resources;
            tracing::debug!(
                "Populated {} resources for server {}",
                server.resources.len(),
                server.name
            );
        } else if let Err(e) = resources_result {
            tracing::warn!(
                "Failed to fetch resources from server {}: {}",
                server.name,
                e
            );
        }
    }

    /// Connect to a single server based on its configuration
    async fn connect_to_server_impl(
        name: String,
        config: ServerConfig,
        process_handles: Arc<RwLock<HashMap<String, Child>>>,
        middleware: Arc<MiddlewareManager>,
        full_config: &McpConfig,
    ) -> Result<(String, ConnectedServer)> {
        if let Some(stdio_config) = config.as_stdio() {
            tracing::info!("Connecting to stdio server: {}", name);
            let handler = ();
            Self::connect_stdio_server(&name, stdio_config, handler, process_handles)
                .await
                .and_then(|client| {
                    let server_middleware = middleware.create_client_middleware_for_server(&name, full_config)?;
                    let wrapped_client = Arc::new(MiddlewareAppliedClient::new(
                        Arc::new(client),
                        server_middleware,
                    ));
                    let connected = ConnectedServer {
                        name: name.clone(),
                        tools: Vec::new(), // Will be populated after connection
                        prompts: Vec::new(),
                        resources: Vec::new(),
                        client: wrapped_client,
                    };
                    Ok((name, connected))
                })
        } else if let Some(http_config) = config.as_http() {
            tracing::info!("Connecting to HTTP server: {}", name);
            let handler = ();
            Self::connect_http_server(&name, http_config, handler)
                .await
                .and_then(|client| {
                    let server_middleware = middleware.create_client_middleware_for_server(&name, full_config)?;
                    let wrapped_client = Arc::new(MiddlewareAppliedClient::new(
                        Arc::new(client),
                        server_middleware,
                    ));
                    let connected = ConnectedServer {
                        name: name.clone(),
                        tools: Vec::new(), // Will be populated after connection
                        prompts: Vec::new(),
                        resources: Vec::new(),
                        client: wrapped_client,
                    };
                    Ok((name, connected))
                })
        } else {
            Err(ProxyError::config(format!(
                "Invalid server configuration for {}: unsupported server type",
                name
            )))
        }
    }

    async fn connect_stdio_server(
        server_name: &str,
        config: StdioConfig,
        handler: (),
        process_handles: Arc<RwLock<HashMap<String, Child>>>,
    ) -> Result<rmcp::service::RunningService<rmcp::RoleClient, ()>> {
        tracing::debug!("Creating stdio transport for server: {}", server_name);

        let mut cmd = Command::new(&config.command);
        cmd.args(&config.args);
        cmd.envs(&config.env);

        #[cfg(unix)]
        cmd.process_group(0);

        tracing::info!("Spawning process for {}: {} {:?}", server_name, &config.command, &config.args);

        let mut child = cmd
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::inherit())
            .spawn()
            .map_err(|e| ProxyError::process_spawn(format!("Failed to spawn child process: {}", e)))?;

        tracing::debug!("Process spawned successfully for {}, PID: {:?}", server_name, child.id());

        let stdout = child.stdout.take().expect("child stdout is piped");
        let stdin = child.stdin.take().expect("child stdin is piped");

        // Store the process handle
        process_handles
            .write()
            .await
            .insert(server_name.to_string(), child);

        // Create transport from the tuple (stdout, stdin) which automatically implements IntoTransport
        let transport = (stdout, stdin);

        tracing::debug!("Serving client for server: {}", server_name);
        
        // Try to connect, but clean up the process if it fails
        match handler.serve(transport).await {
            Ok(client) => {
                tracing::info!("Successfully connected to stdio server: {}", server_name);
                Ok(client)
            }
            Err(e) => {
                // Connection failed, clean up the spawned process
                tracing::error!("Failed to connect to stdio server {}: {}", server_name, e);
                
                // Remove and kill the process
                if let Some(mut child) = process_handles.write().await.remove(server_name) {
                    tracing::info!("Cleaning up failed stdio process for {}", server_name);
                    let _ = child.kill().await;
                }
                
                Err(ProxyError::connection(server_name, format!("Failed to serve client: {}", e)))
            }
        }
    }

    async fn connect_http_server(
        server_name: &str,
        config: HttpConfig,
        handler: (),
    ) -> Result<rmcp::service::RunningService<rmcp::RoleClient, ()>> {
        tracing::debug!("Creating HTTP transport for server: {}", server_name);

        let uri: Arc<str> = Arc::from(config.url.as_str());
        let http_client = reqwest::Client::new();
        let transport_config = StreamableHttpClientTransportConfig::with_uri(uri);

        // If an auth token is provided, wrap the standard client with our BearerAuthClient.
        // Otherwise, use the standard client directly.
        if !config.authorization_token.is_empty() {
            tracing::debug!("Bearer authentication enabled for server: {}", server_name);
            let auth_client = BearerAuthClient::new(http_client, config.authorization_token);
            let transport = StreamableHttpClientTransport::with_client(
                auth_client,
                transport_config,
            );
            handler.serve(transport).await
        } else {
            let transport = StreamableHttpClientTransport::with_client(
                http_client,
                transport_config,
            );
            handler.serve(transport).await
        }
        .map_err(|e| ProxyError::connection(server_name, format!("Failed to serve HTTP client: {}", e)))
    }

    /// Get all tools from all connected servers
    pub async fn get_all_tools(&self) -> Vec<Tool> {
        let mut all_tools = Vec::new();
        let servers = self.servers.read().await;

        for (server_name, server) in servers.iter() {
            if let Ok(tools_result) = server.client.list_tools().await {
                for mut tool in tools_result.tools {
                    // Prefix tool names with server name to avoid conflicts
                    tool.name = Self::prefix_with_server(server_name, &tool.name).into();
                    all_tools.push(tool);
                }
            } else if let Err(e) = server.client.list_tools().await {
                tracing::warn!("Failed to get tools from server {}: {}", server_name, e);
            }
        }

        // Apply proxy middleware
        for mw in self.middleware.proxy.iter() {
            mw.on_list_tools(&mut all_tools).await;
        }

        all_tools
    }

    /// Get all prompts from all connected servers
    pub async fn get_all_prompts(&self) -> Vec<Prompt> {
        let mut all_prompts = Vec::new();
        let servers = self.servers.read().await;

        for (server_name, server) in servers.iter() {
            if let Ok(prompts_result) = server.client.list_prompts().await {
                for mut prompt in prompts_result.prompts {
                    // Prefix prompt names with server name for disambiguation
                    prompt.name = Self::prefix_with_server(server_name, &prompt.name);
                    all_prompts.push(prompt);
                }
            } else if let Err(e) = server.client.list_prompts().await {
                tracing::warn!("Failed to list prompts from server '{}': {}", server_name, e);
            }
        }
        
        // Apply proxy middleware
        for mw in self.middleware.proxy.iter() {
            mw.on_list_prompts(&mut all_prompts).await;
        }

        all_prompts
    }

    /// Get all resources from all connected servers
    pub async fn get_all_resources(&self) -> Vec<Resource> {
        let mut all_resources = Vec::new();
        let servers = self.servers.read().await;

        for (server_name, server) in servers.iter() {
            if let Ok(resources_result) = server.client.list_resources().await {
                for mut resource in resources_result.resources {
                    // Prefix resource URIs with server name for disambiguation
                    resource.uri = Self::prefix_with_server(server_name, &resource.uri);
                    all_resources.push(resource);
                }
            } else if let Err(e) = server.client.list_resources().await {
                tracing::warn!(
                    "Failed to list resources from server '{}': {}",
                    server_name,
                    e
                );
            }
        }

        // Apply proxy middleware
        for mw in self.middleware.proxy.iter() {
            mw.on_list_resources(&mut all_resources).await;
        }

        all_resources
    }



    /// Find and call a tool on the appropriate server
    pub async fn call_tool_on_server(
        &self,
        tool_name: &str,
        arguments: Option<JsonObject>,
    ) -> std::result::Result<CallToolResult, McpError> {
        tracing::info!("Routing tool call: {}", tool_name);
        tracing::info!("Tool arguments: {:?}", arguments);
        
        // Check if this is the special search tool
        if tool_name == "search_available_tools" {
            return self.handle_search_tool_call(arguments).await;
        }
        
        // Parse the prefixed tool name (format: "server_name___tool_name")
        let (server_name, actual_tool_name) = Self::extract_server_and_name(tool_name)?;
        
        tracing::info!("Extracted server: {}, tool: {}", server_name, actual_tool_name);

        let servers = self.servers.read().await;
        let server = servers
            .get(server_name)
            .ok_or_else(|| ProxyError::server_not_found(server_name))?;

        // Call the tool on the specific server
        let param = CallToolRequestParam {
            name: actual_tool_name.to_string().into(),
            arguments,
        };

        tracing::info!("Calling tool '{}' on server '{}'", actual_tool_name, server_name);
        
        match server.client.call_tool(param).await {
            Ok(result) => {
                tracing::info!("Tool call successful for '{}' on server '{}'", actual_tool_name, server_name);
                tracing::debug!("Tool result: {:?}", result);
                Ok(result)
            }
            Err(e) => {
                tracing::error!("Tool call failed for '{}' on server '{}': {}", actual_tool_name, server_name, e);
                // Convert ServiceError to McpError for the final response
                let mcp_error = match e {
                    rmcp::service::ServiceError::McpError(err) => err,
                    _ => McpError::internal_error(format!("Failed to call tool on server {}: {}", server_name, e), None),
                };
                Err(mcp_error)
            }
        }
    }

    /// Handle the special search_for_available_tools call
    async fn handle_search_tool_call(&self, arguments: Option<JsonObject>) -> std::result::Result<CallToolResult, McpError> {
        // Extract the search query from arguments
        let query = arguments
            .as_ref()
            .and_then(|args| args.get("query"))
            .and_then(|v| v.as_str())
            .ok_or_else(|| McpError::invalid_params("Missing required 'query' parameter".to_string(), None))?;
        
        tracing::info!("Processing search query: '{}'", query);
        
        // Find the tool search middleware
        let search_middleware = self.middleware.proxy.iter()
            .find_map(|mw| {
                mw.as_any()
                    .and_then(|any| any.downcast_ref::<crate::middleware::proxy_middleware::tool_search::ToolSearchMiddleware>())
            });
        
        let search_middleware = search_middleware
            .ok_or_else(|| McpError::internal_error("Tool search middleware not found".to_string(), None))?;
        
        // Perform the search - index should already be built and current
        let search_results = search_middleware.search_tools(query).await;
        
        if search_results.is_empty() {
            // No results found
            Ok(CallToolResult {
                content: vec![
                    Content::text(format!("No tools found matching '{}'. Try a different search term.", query))
                ],
                is_error: Some(false),
            })
        } else {
            // Return search results as formatted text
            let mut result_text = format!("Found {} tool(s) matching '{}':\n\n", search_results.len(), query);
            
            for (idx, tool) in search_results.iter().enumerate() {
                result_text.push_str(&format!("{}. **{}**", idx + 1, tool.name));
                if let Some(ref desc) = tool.description {
                    result_text.push_str(&format!("\n   {}", desc));
                }
                result_text.push_str("\n\n");
            }
            
            result_text.push_str("These tools are now available in your current session. Use `tools/list` to see the updated tool list.");
            
            // Update the exposed tools with search results
            search_middleware.update_exposed_tools(search_results).await;
            
            Ok(CallToolResult {
                content: vec![
                    Content::text(result_text)
                ],
                is_error: Some(false),
            })
        }
    }

    /// Get all tools from cached server objects (used for search indexing)
    /// This uses the cached tools that were fetched during server boot instead of refetching
    async fn get_all_tools_from_cache(&self) -> Vec<Tool> {
        let mut all_tools = Vec::new();
        let servers = self.servers.read().await;

        for (server_name, server) in servers.iter() {
            // Use cached tools from the server object
            for tool in &server.tools {
                let mut prefixed_tool = tool.clone();
                // Prefix tool names with server name to avoid conflicts
                prefixed_tool.name = Self::prefix_with_server(server_name, &tool.name).into();
                all_tools.push(prefixed_tool);
            }
        }

        tracing::debug!("Retrieved {} tools from cache for indexing", all_tools.len());
        all_tools
    }

    /// Initialize the tool search index after all servers are connected
    async fn initialize_tool_search_index(&self) {
        tracing::info!("Initializing tool search index...");
        let current_tools = self.get_all_tools_from_cache().await;
        
        // Find the tool search middleware
        if let Some(search_middleware) = self.middleware.proxy.iter()
            .find_map(|mw| {
                mw.as_any()
                    .and_then(|any| any.downcast_ref::<crate::middleware::proxy_middleware::tool_search::ToolSearchMiddleware>())
            })
        {
            search_middleware.refresh_tool_index(&current_tools).await;
            tracing::info!("Tool search index initialized with {} tools", current_tools.len());
        } else {
            tracing::debug!("Tool search middleware not found, skipping index initialization");
        }
    }
}

impl ServerHandler for ProxyServer {
    fn get_info(&self) -> ServerInfo {
        // Create capabilities with listChanged notification support
        let capabilities = ServerCapabilities {
            tools: Some(ToolsCapability {
                list_changed: Some(true),
            }),
            prompts: Some(PromptsCapability {
                list_changed: Some(true),
            }),
            resources: Some(ResourcesCapability {
                list_changed: Some(true),
                subscribe: None,
            }),
            logging: None,
            completions: None,
            experimental: None,
        };

        ServerInfo {
            protocol_version: ProtocolVersion::default(),
            capabilities,
            server_info: Implementation {
                name: "mcproxy".to_string(),
                version: env!("CARGO_PKG_VERSION").to_string(),
            },
            instructions: Some("MCP Proxy Server aggregates tools from multiple MCP servers".to_string()),
        }
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParam>,
        _context: RequestContext<RoleServer>,
    ) -> std::result::Result<ListToolsResult, McpError> {
        let tools = self.get_all_tools().await;
        Ok(ListToolsResult {
            tools,
            next_cursor: None,
        })
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParam,
        _context: RequestContext<RoleServer>,
    ) -> std::result::Result<CallToolResult, McpError> {
        self.call_tool_on_server(&request.name, request.arguments)
            .await
    }

    async fn list_prompts(
        &self,
        _request: Option<PaginatedRequestParam>,
        _context: RequestContext<RoleServer>,
    ) -> std::result::Result<ListPromptsResult, McpError> {
        Ok(ListPromptsResult {
            prompts: self.get_all_prompts().await,
            next_cursor: None,
        })
    }

    async fn list_resources(
        &self,
        _request: Option<PaginatedRequestParam>,
        _context: RequestContext<RoleServer>,
    ) -> std::result::Result<ListResourcesResult, McpError> {
        Ok(ListResourcesResult {
            resources: self.get_all_resources().await,
            next_cursor: None,
        })
    }
} 

 