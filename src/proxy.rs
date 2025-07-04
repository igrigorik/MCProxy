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
        let mut command = Command::new(config.command);
        command.args(config.args);
        command.envs(config.env);
        command
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());

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

        let transport = TokioChildProcess::new(command).map_err(|e| {
            format!("Failed to start command for stdio server '{}': {}", server_name, e)
        })?;
        serve_client(handler, transport)
            .await
            .map_err(|e| format!("Failed to connect to stdio server '{}': {}", server_name, e))
    }

    async fn connect_http_server(
        &self,
        server_name: &str,
        config: HttpConfig,
        handler: ProxyClientHandler,
    ) -> Result<RunningService<RoleClient, ProxyClientHandler>, String> {
        let mut headers = header::HeaderMap::new();
        
        // Add headers from the headers field
        for (key, value) in &config.headers {
            let header_name = header::HeaderName::from_bytes(key.as_bytes())
                .map_err(|e| format!("Invalid header name '{}' for HTTP server '{}': {}", key, server_name, e))?;
            let mut header_value = HeaderValue::from_str(value)
                .map_err(|e| format!("Invalid header value for '{}' in HTTP server '{}': {}", key, server_name, e))?;
            
            // Mark authorization headers as sensitive
            if header_name == header::AUTHORIZATION {
                header_value.set_sensitive(true);
            }
            
            headers.insert(header_name, header_value);
        }
        
        // Backward compatibility: if authorization_token is provided and no Authorization header exists
        if !config.authorization_token.is_empty() && !headers.contains_key(header::AUTHORIZATION) {
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

#[cfg(test)]
mod tests {
    use super::*;
    use rmcp::model::Tool;
    use serde_json::Map;
    use std::sync::Arc;

    // Helper function to create test proxy with public access to internal state
    impl ProxyServer {
        #[cfg(test)]
        pub fn test_tools_len(&self) -> usize {
            self.tools.len()
        }

        #[cfg(test)]
        pub fn test_has_tool(&self, key: &str) -> bool {
            self.tools.contains_key(key)
        }

        #[cfg(test)]
        pub fn test_clients_len(&self) -> usize {
            self.clients.len()
        }
    }

    #[test]
    fn test_proxy_server_new() {
        let proxy = ProxyServer::new();
        assert_eq!(proxy.test_clients_len(), 0);
        assert_eq!(proxy.test_tools_len(), 0);
    }

    #[test]
    fn test_register_tools() {
        let mut proxy = ProxyServer::new();
        
        // Create mock tools - using minimal structure to avoid schema issues
        let tools = vec![
            Tool {
                name: "tool1".into(),
                description: Some("Test tool 1".into()),
                input_schema: Arc::new(Map::new()),
                annotations: None,
            },
            Tool {
                name: "tool2".into(),
                description: Some("Test tool 2".into()),
                input_schema: Arc::new(Map::new()),
                annotations: None,
            },
        ];

        proxy.register_tools("test-server", tools);

        assert_eq!(proxy.test_tools_len(), 2);
        assert!(proxy.test_has_tool("test-server:tool1"));
        assert!(proxy.test_has_tool("test-server:tool2"));
    }

    #[test]
    fn test_register_tools_replaces_existing() {
        let mut proxy = ProxyServer::new();
        
        // Register initial tools
        let initial_tools = vec![
            Tool {
                name: "old-tool".into(),
                description: Some("Old tool".into()),
                input_schema: Arc::new(Map::new()),
                annotations: None,
            },
        ];
        proxy.register_tools("test-server", initial_tools);
        assert_eq!(proxy.test_tools_len(), 1);
        assert!(proxy.test_has_tool("test-server:old-tool"));

        // Register new tools for same server
        let new_tools = vec![
            Tool {
                name: "new-tool".into(),
                description: Some("New tool".into()),
                input_schema: Arc::new(Map::new()),
                annotations: None,
            },
        ];
        proxy.register_tools("test-server", new_tools);

        // Old tools should be replaced
        assert_eq!(proxy.test_tools_len(), 1);
        assert!(!proxy.test_has_tool("test-server:old-tool"));
        assert!(proxy.test_has_tool("test-server:new-tool"));
    }

    #[test]
    fn test_register_tools_different_servers() {
        let mut proxy = ProxyServer::new();
        
        // Register tools for first server
        let tools1 = vec![
            Tool {
                name: "tool1".into(),
                description: Some("Tool from server 1".into()),
                input_schema: Arc::new(Map::new()),
                annotations: None,
            },
        ];
        proxy.register_tools("server1", tools1);

        // Register tools for second server
        let tools2 = vec![
            Tool {
                name: "tool1".into(), // Same name, different server
                description: Some("Tool from server 2".into()),
                input_schema: Arc::new(Map::new()),
                annotations: None,
            },
        ];
        proxy.register_tools("server2", tools2);

        // Both tools should exist with different prefixes
        assert_eq!(proxy.test_tools_len(), 2);
        assert!(proxy.test_has_tool("server1:tool1"));
        assert!(proxy.test_has_tool("server2:tool1"));
    }

    // Note: Async tests with ServerHandler are complex due to rmcp's RequestContext structure
    // Testing the core business logic through unit tests instead

    #[test]
    fn test_get_info() {
        let proxy = ProxyServer::new();
        let info = proxy.get_info();
        
        // Should have tools capability
        assert!(info.capabilities.tools.is_some());
    }

    #[test]
    fn test_proxy_client_handler_new() {
        let handler = ProxyClientHandler::new("test-server".to_string());
        assert_eq!(handler.server_name, "test-server");
    }

    #[test]
    fn test_proxy_client_handler_get_info() {
        let handler = ProxyClientHandler::new("test-server".to_string());
        let info = handler.get_info();
        
        // Should return default client info
        assert_eq!(info, ClientInfo::default());
    }

    #[test]
    fn test_tool_name_parsing() {
        // Test valid tool name parsing
        let tool_name = "server:tool";
        let parts: Vec<&str> = tool_name.splitn(2, ':').collect();
        assert_eq!(parts.len(), 2);
        assert_eq!(parts[0], "server");
        assert_eq!(parts[1], "tool");

        // Test tool name with multiple colons
        let tool_name = "server:namespace:tool";
        let parts: Vec<&str> = tool_name.splitn(2, ':').collect();
        assert_eq!(parts.len(), 2);
        assert_eq!(parts[0], "server");
        assert_eq!(parts[1], "namespace:tool");

        // Test invalid tool name (no colon)
        let tool_name = "invalid-tool";
        let parts: Vec<&str> = tool_name.splitn(2, ':').collect();
        assert_eq!(parts.len(), 1);
    }

    #[test]
    fn test_error_message_parsing() {
        // Test server name extraction from error messages
        let stdio_error = "Failed to connect to stdio server 'my-server': Connection failed";
        let server_name = if stdio_error.contains("stdio server '") {
            stdio_error.split("stdio server '").nth(1)
                .and_then(|s| s.split("'").next())
                .unwrap_or("unknown")
        } else {
            "unknown"
        };
        assert_eq!(server_name, "my-server");

        let http_error = "Failed to connect to HTTP server 'api-server': 401 Unauthorized";
        let server_name = if http_error.contains("HTTP server '") {
            http_error.split("HTTP server '").nth(1)
                .and_then(|s| s.split("'").next())
                .unwrap_or("unknown")
        } else {
            "unknown"
        };
        assert_eq!(server_name, "api-server");

        let generic_error = "Some generic error message";
        let server_name = if generic_error.contains("stdio server '") {
            generic_error.split("stdio server '").nth(1)
                .and_then(|s| s.split("'").next())
                .unwrap_or("unknown")
        } else if generic_error.contains("HTTP server '") {
            generic_error.split("HTTP server '").nth(1)
                .and_then(|s| s.split("'").next())
                .unwrap_or("unknown")
        } else {
            "unknown"
        };
        assert_eq!(server_name, "unknown");
    }

    #[test]
    fn test_failure_reason_classification() {
        // Test different error classifications
        let test_cases = vec![
            ("No such file or directory", "Command not found"),
            ("connection closed: initialize response", "Server failed to initialize"),
            ("HTTP status client error (401 Unauthorized)", "Authentication failed"),
            ("expect initialized response, but received", "Protocol mismatch - unexpected message"),
            ("Some random error", "Unknown error"),
        ];

        for (error_msg, expected_reason) in test_cases {
            let actual_reason = if error_msg.contains("No such file or directory") {
                "Command not found"
            } else if error_msg.contains("connection closed: initialize response") {
                "Server failed to initialize"
            } else if error_msg.contains("HTTP status client error (401 Unauthorized)") {
                "Authentication failed"
            } else if error_msg.contains("expect initialized response, but received") {
                "Protocol mismatch - unexpected message"
            } else {
                "Unknown error"
            };
            
            assert_eq!(actual_reason, expected_reason, "Failed for error: {}", error_msg);
        }
    }
} 