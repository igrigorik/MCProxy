//! The core proxy server that aggregates multiple MCP servers.

use crate::config::{HttpConfig, McpConfig, ServerConfig, StdioConfig};
use futures::future;
use rmcp::{
    model::*,
    service::{RequestContext, RoleServer, ServiceExt},
    transport::streamable_http_client::StreamableHttpClientTransport,
    Error as McpError, ServerHandler,
};
use std::{collections::HashMap, sync::Arc};
use tokio::process::{Child, Command};
use tokio::sync::RwLock;
use tracing::{info, warn};

/// The main proxy server that aggregates multiple MCP servers
#[derive(Clone)]
pub struct ProxyServer {
    /// Connected MCP servers mapped by name
    pub servers: Arc<RwLock<HashMap<String, ConnectedServer>>>,
    /// Configuration for all servers
    config: Arc<McpConfig>,
    /// Process handles for stdio servers that need cleanup
    process_handles: Arc<RwLock<HashMap<String, Child>>>,
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
    pub client: Arc<rmcp::service::RunningService<rmcp::RoleClient, ()>>,
}

impl ProxyServer {
    /// Creates a new proxy server and connects to all configured MCP servers
    pub async fn new(config: McpConfig) -> Result<Self, String> {
        let servers = Arc::new(RwLock::new(HashMap::new()));
        let process_handles = Arc::new(RwLock::new(HashMap::new()));
        let proxy = ProxyServer {
            servers: servers.clone(),
            config: Arc::new(config.clone()),
            process_handles,
        };

        // Connect to all configured servers
        proxy.connect_to_all_servers().await?;

        Ok(proxy)
    }

    /// Gracefully shutdown all connected servers
    pub async fn shutdown(&self) {
        info!("Shutting down proxy server and all connected MCP servers...");

        // Clear server connections first. This will drop client services.
        self.servers.write().await.clear();

        let mut handles = self.process_handles.write().await;
        let mut shutdown_futs = Vec::new();

        for (name, mut child) in handles.drain() {
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

                match tokio::time::timeout(std::time::Duration::from_secs(5), child.wait()).await {
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
    async fn connect_to_all_servers(&self) -> Result<(), String> {
        let connection_futures: Vec<_> = self
            .config
            .mcp_servers
            .iter()
            .map(|(name, server_config)| {
                let name = name.clone();
                let server_config = server_config.clone();
                let servers = self.servers.clone();
                let process_handles = self.process_handles.clone();

                async move {
                    match Self::connect_to_server_static(
                        name.clone(),
                        server_config,
                        process_handles,
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
            Err("Failed to connect to any MCP servers".to_string())
        }
    }

    /// Populate tools, prompts, and resources for a connected server
    async fn populate_server_capabilities(server: &mut ConnectedServer) {
        // Fetch tools
        match server.client.list_tools(None).await {
            Ok(tools_result) => {
                server.tools = tools_result.tools;
                tracing::debug!(
                    "Populated {} tools for server {}",
                    server.tools.len(),
                    server.name
                );
            }
            Err(e) => {
                tracing::warn!(
                    "Failed to fetch tools from server {}: {}",
                    server.name,
                    e
                );
            }
        }

        // Fetch prompts
        match server.client.list_prompts(None).await {
            Ok(prompts_result) => {
                server.prompts = prompts_result.prompts;
                tracing::debug!(
                    "Populated {} prompts for server {}",
                    server.prompts.len(),
                    server.name
                );
            }
            Err(e) => {
                tracing::warn!(
                    "Failed to fetch prompts from server {}: {}",
                    server.name,
                    e
                );
            }
        }

        // Fetch resources
        match server.client.list_resources(None).await {
            Ok(resources_result) => {
                server.resources = resources_result.resources;
                tracing::debug!(
                    "Populated {} resources for server {}",
                    server.resources.len(),
                    server.name
                );
            }
            Err(e) => {
                tracing::warn!(
                    "Failed to fetch resources from server {}: {}",
                    server.name,
                    e
                );
            }
        }
    }

    /// Connect to a single server based on its configuration
    async fn connect_to_server_static(
        name: String,
        config: ServerConfig,
        process_handles: Arc<RwLock<HashMap<String, Child>>>,
    ) -> Result<(String, ConnectedServer), String> {
        if let Some(stdio_config) = config.as_stdio() {
            tracing::info!("Connecting to stdio server: {}", name);
            let handler = ();
            Self::connect_stdio_server_static(&name, stdio_config, handler, process_handles)
                .await
                .map(|client| {
                    let connected = ConnectedServer {
                        name: name.clone(),
                        tools: Vec::new(), // Will be populated after connection
                        prompts: Vec::new(),
                        resources: Vec::new(),
                        client: Arc::new(client),
                    };
                    (name, connected)
                })
        } else if let Some(http_config) = config.as_http() {
            tracing::info!("Connecting to HTTP server: {}", name);
            let handler = ();
            Self::connect_http_server_static(&name, http_config, handler)
                .await
                .map(|client| {
                    let connected = ConnectedServer {
                        name: name.clone(),
                        tools: Vec::new(), // Will be populated after connection
                        prompts: Vec::new(),
                        resources: Vec::new(),
                        client: Arc::new(client),
                    };
                    (name, connected)
                })
        } else {
            Err(format!(
                "Invalid server configuration for {}: unsupported server type",
                name
            ))
        }
    }

    async fn connect_stdio_server_static(
        server_name: &str,
        config: StdioConfig,
        handler: (),
        process_handles: Arc<RwLock<HashMap<String, Child>>>,
    ) -> Result<rmcp::service::RunningService<rmcp::RoleClient, ()>, String> {
        tracing::debug!("Creating stdio transport for server: {}", server_name);

        let mut cmd = Command::new(&config.command);
        cmd.args(&config.args);
        cmd.envs(&config.env);

        #[cfg(unix)]
        cmd.process_group(0);

        let mut child = cmd
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::inherit())
            .spawn()
            .map_err(|e| format!("Failed to spawn child process: {}", e))?;

        let stdout = child.stdout.take().expect("child stdout is piped");
        let stdin = child.stdin.take().expect("child stdin is piped");

        process_handles
            .write()
            .await
            .insert(server_name.to_string(), child);

        // Create transport from the tuple (stdout, stdin) which automatically implements IntoTransport
        let transport = (stdout, stdin);

        tracing::debug!("Serving client for server: {}", server_name);
        let client = handler
            .serve(transport)
            .await
            .map_err(|e| format!("Failed to serve client: {}", e))?;

        tracing::info!("Successfully connected to stdio server: {}", server_name);
        Ok(client)
    }

    async fn connect_http_server_static(
        server_name: &str,
        config: HttpConfig,
        handler: (),
    ) -> Result<rmcp::service::RunningService<rmcp::RoleClient, ()>, String> {
        tracing::debug!("Creating HTTP transport for server: {}", server_name);

        let uri: Arc<str> = Arc::from(config.url.as_str());
        let transport = StreamableHttpClientTransport::from_uri(uri);

        tracing::debug!("Serving client for server: {}", server_name);
        let client = handler
            .serve(transport)
            .await
            .map_err(|e| format!("Failed to serve HTTP client: {}", e))?;

        tracing::info!("Successfully connected to HTTP server: {}", server_name);
        Ok(client)
    }

    /// Get all tools from all connected servers
    pub async fn get_all_tools(&self) -> Vec<Tool> {
        let mut all_tools = Vec::new();
        let servers = self.servers.read().await;

        for (server_name, server) in servers.iter() {
            match server.client.list_tools(None).await {
                Ok(tools_result) => {
                    for mut tool in tools_result.tools {
                        // Prefix tool names with server name to avoid conflicts
                        tool.name = format!("{}___{}", server_name, tool.name).into();
                        all_tools.push(tool);
                    }
                }
                Err(e) => {
                    tracing::warn!("Failed to get tools from server {}: {}", server_name, e);
                }
            }
        }

        all_tools
    }

    /// Get all prompts from all connected servers
    pub async fn get_all_prompts(&self) -> Vec<Prompt> {
        let mut all_prompts = Vec::new();
        let servers = self.servers.read().await;

        for (server_name, server) in servers.iter() {
            match server.client.list_prompts(None).await {
                Ok(prompts_result) => {
                    for mut prompt in prompts_result.prompts {
                        // Prefix prompt names with server name for disambiguation
                        prompt.name = format!("{}___{}", server_name, prompt.name);
                        all_prompts.push(prompt);
                    }
                }
                Err(e) => {
                    tracing::warn!("Failed to list prompts from server '{}': {}", server_name, e);
                }
            }
        }

        all_prompts
    }

    /// Get all resources from all connected servers
    pub async fn get_all_resources(&self) -> Vec<Resource> {
        let mut all_resources = Vec::new();
        let servers = self.servers.read().await;

        for (server_name, server) in servers.iter() {
            match server.client.list_resources(None).await {
                Ok(resources_result) => {
                    for mut resource in resources_result.resources {
                        // Prefix resource URIs with server name for disambiguation
                        resource.uri = format!("{}___{}", server_name, resource.uri);
                        all_resources.push(resource);
                    }
                }
                Err(e) => {
                    tracing::warn!(
                        "Failed to list resources from server '{}': {}",
                        server_name,
                        e
                    );
                }
            }
        }

        all_resources
    }

    /// Find and call a tool on the appropriate server
    pub async fn call_tool_on_server(
        &self,
        tool_name: &str,
        arguments: Option<JsonObject>,
    ) -> Result<CallToolResult, McpError> {
        // Parse the prefixed tool name (format: "server_name___tool_name")
        let (server_name, actual_tool_name) = if let Some(pos) = tool_name.find("___") {
            let server_name = &tool_name[..pos];
            let tool_name = &tool_name[pos + 3..];  // Skip the 3 underscores
            (server_name, tool_name)
        } else {
            return Err(McpError::invalid_params(
                format!("Invalid tool name format: {}", tool_name),
                None,
            ));
        };

        let servers = self.servers.read().await;
        let server = servers
            .get(server_name)
            .ok_or_else(|| McpError::invalid_params(format!("Server not found: {}", server_name), None))?;

        // Call the tool on the specific server
        let param = CallToolRequestParam {
            name: actual_tool_name.to_string().into(),
            arguments,
        };

        server
            .client
            .call_tool(param)
            .await
            .map_err(|e| McpError::internal_error(format!("Failed to call tool on server {}: {}", server_name, e), None))
    }
}

impl ServerHandler for ProxyServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo {
            protocol_version: ProtocolVersion::default(),
            capabilities: ServerCapabilities::builder()
                .enable_tools()
                .enable_prompts()
                .enable_resources()
                .build(),
            server_info: Implementation {
                name: "mcproxy".to_string(),
                version: env!("CARGO_PKG_VERSION").to_string(),
            },
            instructions: Some("MCP Proxy Server - aggregates multiple MCP servers".to_string()),
        }
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParam>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, McpError> {
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
    ) -> Result<CallToolResult, McpError> {
        self.call_tool_on_server(&request.name, request.arguments)
            .await
    }

    async fn list_prompts(
        &self,
        request: Option<PaginatedRequestParam>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListPromptsResult, McpError> {
        let mut all_prompts = Vec::new();
        let servers = self.servers.read().await;

        for (server_name, server) in servers.iter() {
            match server.client.list_prompts(request.clone()).await {
                Ok(prompts_result) => {
                    for mut prompt in prompts_result.prompts {
                        // Prefix prompt names with server name for disambiguation
                        prompt.name = format!("{}___{}", server_name, prompt.name);
                        all_prompts.push(prompt);
                    }
                }
                Err(e) => {
                    tracing::warn!("Failed to list prompts from server '{}': {}", server_name, e);
                    // Continue with other servers
                }
            }
        }

        Ok(ListPromptsResult {
            prompts: all_prompts,
            next_cursor: None, // TODO: Implement proper pagination
        })
    }

    async fn list_resources(
        &self,
        request: Option<PaginatedRequestParam>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListResourcesResult, McpError> {
        let mut all_resources = Vec::new();
        let servers = self.servers.read().await;

        for (server_name, server) in servers.iter() {
            match server.client.list_resources(request.clone()).await {
                Ok(resources_result) => {
                    for mut resource in resources_result.resources {
                        // Prefix resource URIs with server name for disambiguation
                        resource.uri = format!("{}___{}", server_name, resource.uri);
                        all_resources.push(resource);
                    }
                }
                Err(e) => {
                    tracing::warn!(
                        "Failed to list resources from server '{}': {}",
                        server_name,
                        e
                    );
                    // Continue with other servers
                }
            }
        }

        Ok(ListResourcesResult {
            resources: all_resources,
            next_cursor: None, // TODO: Implement proper pagination
        })
    }
} 