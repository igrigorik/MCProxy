//! The core proxy server that aggregates multiple MCP servers.

use crate::config::{HttpConfig, McpConfig, ServerConfig, StdioConfig};
use futures::future;
use rmcp::{
    model::{
        CallToolRequestParam, CallToolResult, ClientInfo, ListToolsResult, PaginatedRequestParam,
        ServerInfo, Tool,
    },
    service::{serve_client, NotificationContext, RoleClient, RunningService},
    transport::{
        streamable_http_client::{
            StreamableHttpClientTransport, StreamableHttpClientTransportConfig,
        },
        TokioChildProcess,
    },
    ClientHandler, Error as McpError, Peer, ServerHandler,
};
use reqwest::header::{self, HeaderValue};
use std::{collections::HashMap, sync::Arc};
use tokio::process::Command;

/// The main proxy server that aggregates tools from multiple MCP servers.
#[derive(Clone)]
pub struct ProxyServer {
    /// Active MCP client connections by server name
    clients: HashMap<String, Peer<RoleClient>>,
    /// All available tools with prefixed names (server:tool)
    tools: HashMap<String, Tool>,
}

impl ProxyServer {
    pub fn new() -> Self {
        Self {
            clients: HashMap::new(),
            tools: HashMap::new(),
        }
    }

    /// Connect to all servers defined in config and perform initial tool discovery
    pub async fn connect_and_discover(&mut self, config: McpConfig) -> Result<(), String> {
        let total_servers = config.mcp_servers.len();
        tracing::info!("Attempting to connect to {} servers...", total_servers);
        
        let connection_futures = config
            .mcp_servers
            .into_iter()
            .map(|(name, server_config)| self.connect_to_server(name, server_config));

        let results = future::join_all(connection_futures).await;

        // Collect successful connections and track failures
        let mut successful_connections = Vec::new();
        let mut failed_connections = Vec::new();
        
        for result in results {
            match result {
                Ok((server_name, client)) => {
                    tracing::info!(server_name = %server_name, "✓ Successfully connected to server");
                    self.clients.insert(server_name.clone(), client.peer().clone());
                    successful_connections.push((server_name, client));
                }
                Err(e) => {
                    // Extract server name from error message if possible
                    let server_name = if e.contains("stdio server '") {
                        e.split("stdio server '").nth(1)
                            .and_then(|s| s.split("'").next())
                            .unwrap_or("unknown")
                    } else if e.contains("HTTP server '") {
                        e.split("HTTP server '").nth(1)
                            .and_then(|s| s.split("'").next())
                            .unwrap_or("unknown")
                    } else {
                        "unknown"
                    };
                    
                    tracing::error!(server_name = %server_name, error = %e, "✗ Failed to connect to server");
                    failed_connections.push((server_name.to_string(), e));
                }
            }
        }

        // Print connection summary
        if !failed_connections.is_empty() {
            tracing::warn!("Connection Summary:");
            tracing::warn!("  ✓ Connected: {}", successful_connections.len());
            tracing::warn!("  ✗ Failed: {}", failed_connections.len());
            
            for (server_name, error) in &failed_connections {
                let failure_reason = if error.contains("No such file or directory") {
                    "Command not found"
                } else if error.contains("connection closed: initialize response") {
                    "Server failed to initialize"
                } else if error.contains("HTTP status client error (401 Unauthorized)") {
                    "Authentication failed"
                } else if error.contains("expect initialized response, but received") {
                    "Protocol mismatch - unexpected message"
                } else {
                    "Unknown error"
                };
                tracing::warn!("    {} - {}", server_name, failure_reason);
            }
        }

        // Perform initial tool discovery
        tracing::info!("Discovering tools from {} connected servers...", successful_connections.len());
        let discovery_futures = successful_connections
            .iter()
            .map(|(server_name, client)| {
                let peer = client.peer().clone();
                let server_name = server_name.clone();
                async move {
                    tracing::debug!(server_name = %server_name, "Requesting tools from server...");
                    match peer.list_all_tools().await {
                        Ok(tools) => {
                            let tool_names: Vec<_> = tools.iter().map(|t| t.name.to_string()).collect();
                            tracing::info!(server_name = %server_name, "  ✓ Found {} tools: {:?}", tools.len(), tool_names);
                            Some((server_name, tools))
                        }
                        Err(e) => {
                            tracing::error!(server_name = %server_name, error = %e, "  ✗ Failed to discover tools");
                            None
                        }
                    }
                }
            });

        let discovery_results = future::join_all(discovery_futures).await;
        
        // Register discovered tools
        for result in discovery_results.into_iter().flatten() {
            let (server_name, tools) = result;
            self.register_tools(&server_name, tools);
        }

        // Final summary
        tracing::info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
        tracing::info!("MCP Proxy Initialization Complete:");
        tracing::info!("  Servers configured: {}", total_servers);
        tracing::info!("  Servers connected: {}", self.clients.len());
        tracing::info!("  Total tools available: {}", self.tools.len());
        
        if !failed_connections.is_empty() {
            tracing::warn!("  Failed connections: {} (see errors above)", failed_connections.len());
        }
        
        tracing::info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
        Ok(())
    }

    /// Register tools from a server with proper prefixing
    fn register_tools(&mut self, server_name: &str, tools: Vec<Tool>) {
        // Remove old tools for this server
        self.tools.retain(|k, _| !k.starts_with(&format!("{}:", server_name)));

        // Add new tools with prefixed names
        for mut tool in tools {
            let prefixed_name = format!("{}:{}", server_name, tool.name);
            tool.name = prefixed_name.clone().into();
            self.tools.insert(prefixed_name, tool);
        }
    }

    /// Connect to a single server based on its configuration
    async fn connect_to_server(
        &self,
        name: String,
        config: ServerConfig,
    ) -> Result<(String, RunningService<RoleClient, ProxyClientHandler>), String> {
        let handler = ProxyClientHandler::new(name.clone());
        
        tracing::debug!(server_name = %name, "Connecting to server...");
        
        let client = match config {
            ServerConfig::Stdio(stdio_config) => {
                tracing::debug!(server_name = %name, command = %stdio_config.command, "Connecting via stdio");
                self.connect_stdio_server(&name, stdio_config, handler).await?
            }
            ServerConfig::Http(http_config) => {
                tracing::debug!(server_name = %name, url = %http_config.url, "Connecting via HTTP");
                self.connect_http_server(&name, http_config, handler).await?
            }
        };

        Ok((name, client))
    }

    async fn connect_stdio_server(
        &self,
        server_name: &str,
        config: StdioConfig,
        handler: ProxyClientHandler,
    ) -> Result<RunningService<RoleClient, ProxyClientHandler>, String> {
        // Try to connect with retry logic for servers that send logging notifications during initialization
        let mut retries = 3;
        let mut last_error = String::new();
        
        while retries > 0 {
            let mut command = Command::new(&config.command);
            command.args(&config.args);
            command.envs(&config.env);
            
            // Configure stdio for MCP protocol communication
            // stdin/stdout are used for JSON-RPC communication
            // stderr should be piped to capture logs separately
            command
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped());

            #[cfg(unix)]
            unsafe {
                command.pre_exec(|| {
                    // Create a new session to detach from controlling terminal
                    if libc::setsid() < 0 {
                        return Err(std::io::Error::last_os_error());
                    }
                    Ok(())
                });
            }

            let transport = match TokioChildProcess::new(command) {
                Ok(t) => t,
                Err(e) => {
                    return Err(format!("Failed to start command for stdio server '{}': {}", server_name, e));
                }
            };
            
            let result = serve_client(handler.clone(), transport).await;
            
            match result {
                Ok(service) => return Ok(service),
                Err(e) => {
                    let error_msg = e.to_string();
                    last_error = error_msg.clone();
                    
                    // Check if this is a protocol mismatch due to logging notifications
                    if error_msg.contains("expect initialized response, but received") && 
                       error_msg.contains("LoggingMessageNotification") {
                        tracing::warn!(
                            server_name = %server_name, 
                            retries_left = retries - 1,
                            "Server sent logging notification during initialization, retrying..."
                        );
                        
                        // Wait a bit before retrying
                        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
                        retries -= 1;
                        continue;
                    } else {
                        // For other errors, don't retry
                        return Err(format!("Failed to connect to stdio server '{}': {}", server_name, e));
                    }
                }
            }
        }
        
        Err(format!("Failed to connect to stdio server '{}' after retries: {}", server_name, last_error))
    }

    async fn connect_http_server(
        &self,
        server_name: &str,
        config: HttpConfig,
        handler: ProxyClientHandler,
    ) -> Result<RunningService<RoleClient, ProxyClientHandler>, String> {
        let mut headers = header::HeaderMap::new();
        if !config.authorization_token.is_empty() {
            let mut auth_value = HeaderValue::from_str(&config.authorization_token)
                .map_err(|e| format!("Invalid authorization token for HTTP server '{}': {}", server_name, e))?;
            auth_value.set_sensitive(true);
            headers.insert(header::AUTHORIZATION, auth_value);
        }

        let http_client = reqwest::Client::builder()
            .default_headers(headers)
            .build()
            .map_err(|e| format!("Failed to create HTTP client for server '{}': {}", server_name, e))?;

        let transport = StreamableHttpClientTransport::with_client(
            http_client,
            StreamableHttpClientTransportConfig {
                uri: Arc::from(config.url.as_str()),
                ..Default::default()
            },
        );

        serve_client(handler, transport)
            .await
            .map_err(|e| format!("Failed to connect to HTTP server '{}': {}", server_name, e))
    }
}

impl ServerHandler for ProxyServer {
    fn list_tools(
        &self,
        _req: Option<PaginatedRequestParam>,
        _ctx: rmcp::service::RequestContext<rmcp::service::RoleServer>,
    ) -> impl future::Future<Output = Result<ListToolsResult, McpError>> + Send + '_ {
        let tools = self.tools.values().cloned().collect();
        future::ready(Ok(ListToolsResult {
            tools,
            next_cursor: None,
        }))
    }

    async fn call_tool(
        &self,
        req: CallToolRequestParam,
        _ctx: rmcp::service::RequestContext<rmcp::service::RoleServer>,
    ) -> Result<CallToolResult, McpError> {
        let tool_name = req.name.to_string();
        let parts: Vec<&str> = tool_name.splitn(2, ':').collect();
        
        if parts.len() != 2 {
            return Err(McpError::invalid_params(
                "Invalid tool name format. Expected 'server:tool'",
                None,
            ));
        }

        let server_name = parts[0].to_string();
        let original_tool_name = parts[1].to_string();

        match self.clients.get(&server_name) {
            Some(client) => {
                client
                    .call_tool(CallToolRequestParam {
                        name: original_tool_name.into(),
                        arguments: req.arguments,
                    })
                    .await
                    .map_err(|e| McpError::internal_error(e.to_string(), None))
            }
            None => Err(McpError::resource_not_found(
                format!("Server '{}' not found for tool '{}'", server_name, tool_name),
                None,
            )),
        }
    }

    fn get_info(&self) -> ServerInfo {
        let mut server_info = ServerInfo::default();
        server_info.capabilities.tools = Some(Default::default());
        server_info
    }
}

/// Handles notifications from upstream MCP servers (tool list changes, etc.)
#[derive(Clone)]
pub struct ProxyClientHandler {
    server_name: String,
}

impl ProxyClientHandler {
    pub fn new(server_name: String) -> Self {
        Self { server_name }
    }
}

impl ClientHandler for ProxyClientHandler {
    fn on_tool_list_changed(
        &self,
        _context: NotificationContext<RoleClient>,
    ) -> impl std::future::Future<Output = ()> + Send {
        let server_name = self.server_name.clone();

        async move {
            tracing::info!(server_name = %server_name, "Tool list changed, but proxy doesn't support dynamic updates yet");
            // TODO: Implement dynamic tool updates by notifying the proxy
            // For now, we just log the event
        }
    }

    fn get_info(&self) -> ClientInfo {
        ClientInfo::default()
    }
} 