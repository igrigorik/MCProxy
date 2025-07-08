//! MCP (Model Context Protocol) Proxy Server
//! 
//! This application acts as a proxy that aggregates multiple MCP servers and exposes
//! them through a single HTTP endpoint. It supports both stdio (subprocess) and HTTP
//! MCP servers as backends.
//! 
//! # Architecture
//! 
//! The proxy consists of several key components:
//! 
//! - **Main Module**: Entry point and application lifecycle management
//! - **Config Module**: Configuration management with environment variable support
//! - **Error Module**: Centralized error handling with `ProxyError` type
//! - **HTTP Server Module**: HTTP/JSON-RPC server implementation
//! - **Proxy Module**: Core proxy logic for aggregating MCP servers
//! 
//! # Usage
//! 
//! ```bash
//! mcproxy <config_file>
//! ```
//! 
//! The configuration file should be in JSON format specifying the MCP servers to
//! connect to and HTTP server settings.

use std::{env, sync::Arc};

use config::load_config;
use proxy::ProxyServer;
use tracing::{error, info, warn};

mod config;
mod error;
mod http_server;
mod proxy;
mod middleware;

/// Apply environment variable overrides to the configuration
fn apply_env_overrides(config: &mut config::McpConfig) {
    // Ensure HTTP server config exists
    if config.http_server.is_none() {
        config.http_server = Some(config::HttpServerConfig::default());
    }
    
    if let Some(http_config) = config.http_server.as_mut() {
        // Override host if MCPROXY_HTTP_HOST is set
        if let Ok(host) = env::var("MCPROXY_HTTP_HOST") {
            info!("Overriding HTTP host from environment: {}", host);
            http_config.host = host;
        }
        
        // Override port if MCPROXY_HTTP_PORT is set
        if let Ok(port_str) = env::var("MCPROXY_HTTP_PORT") {
            if let Ok(port) = port_str.parse::<u16>() {
                info!("Overriding HTTP port from environment: {}", port);
                http_config.port = port;
            } else {
                warn!("Invalid MCPROXY_HTTP_PORT value: {}", port_str);
            }
        }
        
        // Override CORS enabled if MCPROXY_CORS_ENABLED is set
        if let Ok(cors_str) = env::var("MCPROXY_CORS_ENABLED") {
            match cors_str.to_lowercase().as_str() {
                "true" | "1" | "yes" | "on" => {
                    info!("Enabling CORS from environment");
                    http_config.cors_enabled = true;
                }
                "false" | "0" | "no" | "off" => {
                    info!("Disabling CORS from environment");
                    http_config.cors_enabled = false;
                }
                _ => warn!("Invalid MCPROXY_CORS_ENABLED value: {}", cors_str),
            }
        }
        
        // Override CORS origins if MCPROXY_CORS_ORIGINS is set
        if let Ok(origins_str) = env::var("MCPROXY_CORS_ORIGINS") {
            let origins: Vec<String> = origins_str
                .split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect();
            if !origins.is_empty() {
                info!("Overriding CORS origins from environment: {:?}", origins);
                http_config.cors_origins = origins;
            }
        }
        
        // Override shutdown timeout if MCPROXY_SHUTDOWN_TIMEOUT is set
        if let Ok(timeout_str) = env::var("MCPROXY_SHUTDOWN_TIMEOUT") {
            if let Ok(timeout) = timeout_str.parse::<u64>() {
                info!("Overriding shutdown timeout from environment: {} seconds", timeout);
                http_config.shutdown_timeout = timeout;
            } else {
                warn!("Invalid MCPROXY_SHUTDOWN_TIMEOUT value: {}", timeout_str);
            }
        }
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    // Configure logging with reduced verbosity
    let log_level = env::var("RUST_LOG").unwrap_or_else(|_| "mcproxy=info,rmcp=warn".to_string());
    
    let subscriber = tracing_subscriber::fmt()
        .with_env_filter(&log_level)
        .with_target(false);
    
    // Use JSON format in production, human-readable format in development
    if env::var("RUST_LOG_FORMAT").unwrap_or_default() == "json" {
        subscriber.json().init();
    } else {
        subscriber.init();
    }

    info!("Starting MCP Proxy HTTP Server");

    // Parse command line arguments
    let args: Vec<String> = env::args().collect();
    if args.len() != 2 {
        eprintln!("Usage: {} <config_file>", args[0]);
        std::process::exit(1);
    }

    let config_path = &args[1];
    let mut config = load_config(config_path)?;
    
    // Apply environment variable overrides
    apply_env_overrides(&mut config);

    // Create the proxy server and connect to all MCP servers
    info!("Connecting to {} MCP servers...", config.mcp_servers.len());
    let proxy_server = ProxyServer::new(config.clone()).await?;
    let shared_proxy = Arc::new(proxy_server);

    // Get HTTP server configuration with defaults
    let http_config = config
        .http_server
        .as_ref()
        .ok_or("HTTP server configuration is required")?;

    let bind_addr = http_server::parse_bind_address(http_config)?;
    info!("Binding HTTP server to {}", bind_addr);

    // Create HTTP server using the http_server module
    let app = http_server::create_router(shared_proxy.clone(), http_config);

    info!("🚀 MCP Proxy HTTP Server listening on http://{}", bind_addr);
    info!("   📡 MCP endpoint: http://{}/mcp", bind_addr);
    info!("   🔍 Health check: http://{}/health", bind_addr);
    info!("   🔗 Connected to {} MCP servers", config.mcp_servers.len());

    // List connected servers with their tools
    let servers = shared_proxy.servers.read().await;
    for (name, server) in servers.iter() {
        info!("   └─ 🔌 {}", name);
        
        // Display tools for each server
        if !server.tools.is_empty() {
            for tool in &server.tools {
                info!("      └─ 🔧 {}", tool.name);
            }
        } else {
            info!("      └─ (no tools available)");
        }
    }
    drop(servers);

    // Start the HTTP server with graceful shutdown
    let listener = tokio::net::TcpListener::bind(bind_addr).await?;
    
    // Set up graceful shutdown signal handler
    let shared_proxy_shutdown = shared_proxy.clone();
    let shutdown_signal = async move {
        tokio::signal::ctrl_c()
            .await
            .expect("Failed to install CTRL+C signal handler");
        info!("🛑 Received shutdown signal, gracefully shutting down...");
        
        // Shutdown the proxy server and all connected MCP servers
        shared_proxy_shutdown.shutdown().await;
    };
    
    // Start server with graceful shutdown
    let server = axum::serve(listener, app).with_graceful_shutdown(shutdown_signal);
    
    if let Err(e) = server.await {
        error!("HTTP server error: {}", e);
    } else {
        info!("✅ Server shutdown complete");
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::McpConfig;
    use axum::{
        body::Body,
        http::{header, Method, Request, StatusCode},
        Router,
    };
    use http_body_util::BodyExt;
    use serde_json::{json, Value};
    use std::collections::HashMap;
    use tower::ServiceExt;
    
    /// Test fixture builder for creating test configurations and apps
    struct TestFixture {
        config: McpConfig,
    }
    
    impl TestFixture {
        /// Create a new test fixture builder
        fn builder() -> TestFixtureBuilder {
            TestFixtureBuilder::default()
        }
        
        /// Build a test app from this fixture
        async fn build_app(self) -> Router {
            let proxy = ProxyServer::new(self.config).await.unwrap();
            create_test_app(proxy).await
        }
    }
    
    #[derive(Default)]
    struct TestFixtureBuilder {
        mcp_servers: HashMap<String, config::ServerConfig>,
        host: Option<String>,
        port: Option<u16>,
        cors_enabled: Option<bool>,
        cors_origins: Option<Vec<String>>,
        shutdown_timeout: Option<u64>,
    }
    
    impl TestFixtureBuilder {
        #[allow(dead_code)]
        fn with_server(mut self, name: &str, config: config::ServerConfig) -> Self {
            self.mcp_servers.insert(name.to_string(), config);
            self
        }
        
        #[allow(dead_code)]
        fn with_host(mut self, host: &str) -> Self {
            self.host = Some(host.to_string());
            self
        }
        
        #[allow(dead_code)]
        fn with_port(mut self, port: u16) -> Self {
            self.port = Some(port);
            self
        }
        
        #[allow(dead_code)]
        fn with_cors(mut self, enabled: bool, origins: Vec<&str>) -> Self {
            self.cors_enabled = Some(enabled);
            self.cors_origins = Some(origins.into_iter().map(|s| s.to_string()).collect());
            self
        }
        
        #[allow(dead_code)]
        fn with_shutdown_timeout(mut self, timeout: u64) -> Self {
            self.shutdown_timeout = Some(timeout);
            self
        }
        
        fn build(self) -> TestFixture {
            let config = McpConfig {
                mcp_servers: self.mcp_servers,
                http_server: Some(config::HttpServerConfig {
                    host: self.host.unwrap_or_else(|| "127.0.0.1".to_string()),
                    port: self.port.unwrap_or(0),
                    cors_enabled: self.cors_enabled.unwrap_or(true),
                    cors_origins: self.cors_origins.unwrap_or_else(|| vec!["*".to_string()]),
                    shutdown_timeout: self.shutdown_timeout.unwrap_or(5),
                }),
            };
            TestFixture { config }
        }
    }

    /// Create a test configuration
    fn create_test_config() -> McpConfig {
        TestFixture::builder().build().config
    }

    /// Helper to create a test app with proxy server
    async fn create_test_app(proxy: ProxyServer) -> Router {
        let shared_proxy = Arc::new(proxy);
        let test_config = config::HttpServerConfig {
            host: "127.0.0.1".to_string(),
            port: 0,
            cors_enabled: true,
            cors_origins: vec!["*".to_string()],
            shutdown_timeout: 5,
        };
        http_server::create_router(shared_proxy, &test_config)
    }

    /// Helper to create a test app with default config
    async fn create_default_test_app() -> Router {
        TestFixture::builder().build().build_app().await
    }

    #[tokio::test]
    async fn test_health_endpoint() {
        let config = create_test_config();
        let proxy = ProxyServer::new(config).await.unwrap();
        let app = create_test_app(proxy).await;

        let request = Request::builder()
            .method(Method::GET)
            .uri("/health")
            .body(Body::empty())
            .unwrap();

        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let body = response.into_body().collect().await.unwrap().to_bytes();
        let body: Value = serde_json::from_slice(&body).unwrap();
        
        assert_eq!(body["service"], "mcproxy");
        assert_eq!(body["status"], "healthy");
    }

    #[tokio::test]
    async fn test_cors_headers() {
        let app = create_default_test_app().await;

        let request = Request::builder()
            .method(Method::OPTIONS)
            .uri("/mcp")
            .header("Origin", "https://example.com")
            .header("Access-Control-Request-Method", "POST")
            .header("Access-Control-Request-Headers", "Content-Type")
            .body(Body::empty())
            .unwrap();

        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let headers = response.headers();
        assert!(headers.contains_key("access-control-allow-origin"));
        assert!(headers.contains_key("access-control-allow-methods"));
        assert!(headers.contains_key("access-control-allow-headers"));
    }

    #[tokio::test]
    async fn test_jsonrpc_parse_error() {
        let app = create_default_test_app().await;

        let request = Request::builder()
            .method(Method::POST)
            .uri("/mcp")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from("invalid json"))
            .unwrap();

        let response = app.oneshot(request).await.unwrap();
        // Axum returns 400 (Bad Request) for malformed JSON, which is acceptable
        assert!(
            response.status() == StatusCode::OK || response.status() == StatusCode::BAD_REQUEST
        );
    }

    #[tokio::test]
    async fn test_jsonrpc_invalid_version() {
        let app = create_default_test_app().await;

        let request_body = json!({
            "jsonrpc": "1.0",
            "id": 1,
            "method": "ping"
        });

        let request = Request::builder()
            .method(Method::POST)
            .uri("/mcp")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(serde_json::to_string(&request_body).unwrap()))
            .unwrap();

        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let body = response.into_body().collect().await.unwrap().to_bytes();
        let body: Value = serde_json::from_slice(&body).unwrap();
        
        assert_eq!(body["jsonrpc"], "2.0");
        assert!(body["error"].is_object());
        assert_eq!(body["error"]["code"], -32600); // Invalid Request
    }

    #[tokio::test]
    async fn test_ping_method() {
        let app = create_default_test_app().await;

        let request_body = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "ping"
        });

        let request = Request::builder()
            .method(Method::POST)
            .uri("/mcp")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(serde_json::to_string(&request_body).unwrap()))
            .unwrap();

        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let body = response.into_body().collect().await.unwrap().to_bytes();
        let body: Value = serde_json::from_slice(&body).unwrap();
        
        assert_eq!(body["jsonrpc"], "2.0");
        assert_eq!(body["id"], 1);
        assert!(body["result"].is_object());
        assert!(body.get("error").is_none());
    }

    #[tokio::test]
    async fn test_initialize_method() {
        let app = create_default_test_app().await;

        let request_body = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2024-11-05",
                "capabilities": {
                    "roots": {
                        "listChanged": true
                    },
                    "sampling": {}
                },
                "clientInfo": {
                    "name": "TestClient",
                    "version": "1.0.0"
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

        let body = response.into_body().collect().await.unwrap().to_bytes();
        let body: Value = serde_json::from_slice(&body).unwrap();
        
        assert_eq!(body["jsonrpc"], "2.0");
        assert_eq!(body["id"], 1);
        assert!(body["result"].is_object());
        assert!(body.get("error").is_none());

        // Verify the result contains server info
        let result = &body["result"];
        assert!(result.get("protocolVersion").is_some());
        assert!(result.get("capabilities").is_some());
        assert!(result.get("serverInfo").is_some());
    }

    #[tokio::test]
    async fn test_tools_list_method() {
        let app = create_default_test_app().await;

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

        let body = response.into_body().collect().await.unwrap().to_bytes();
        let body: Value = serde_json::from_slice(&body).unwrap();
        
        assert_eq!(body["jsonrpc"], "2.0");
        assert_eq!(body["id"], 1);
        assert!(body["result"].is_object());
        assert!(body.get("error").is_none());

        // Verify the result has the expected structure
        let result = &body["result"];
        assert!(result.get("tools").is_some());
        // Note: next_cursor is None when there are no more results, so it's omitted from JSON
    }

    #[tokio::test]
    async fn test_tools_call_missing_params() {
        let app = create_default_test_app().await;

        let request_body = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/call"
        });

        let request = Request::builder()
            .method(Method::POST)
            .uri("/mcp")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(serde_json::to_string(&request_body).unwrap()))
            .unwrap();

        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let body = response.into_body().collect().await.unwrap().to_bytes();
        let body: Value = serde_json::from_slice(&body).unwrap();
        
        assert_eq!(body["jsonrpc"], "2.0");
        assert_eq!(body["id"], 1);
        assert!(body.get("result").is_none());
        assert!(body["error"].is_object());
        assert_eq!(body["error"]["code"], -32602); // Invalid params
    }

    #[tokio::test]
    async fn test_unknown_method() {
        let app = create_default_test_app().await;

        let request_body = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "unknown/method"
        });

        let request = Request::builder()
            .method(Method::POST)
            .uri("/mcp")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(serde_json::to_string(&request_body).unwrap()))
            .unwrap();

        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let body = response.into_body().collect().await.unwrap().to_bytes();
        let body: Value = serde_json::from_slice(&body).unwrap();
        
        assert_eq!(body["jsonrpc"], "2.0");
        assert_eq!(body["id"], 1);
        assert!(body.get("result").is_none());
        assert!(body["error"].is_object());
        assert_eq!(body["error"]["code"], -32602); // Invalid params (our current implementation)
    }

    #[tokio::test]
    async fn test_prompts_list_method() {
        let app = create_default_test_app().await;

        let request_body = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "prompts/list"
        });

        let request = Request::builder()
            .method(Method::POST)
            .uri("/mcp")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(serde_json::to_string(&request_body).unwrap()))
            .unwrap();

        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let body = response.into_body().collect().await.unwrap().to_bytes();
        let body: Value = serde_json::from_slice(&body).unwrap();
        
        assert_eq!(body["jsonrpc"], "2.0");
        assert_eq!(body["id"], 1);
        assert!(body["result"].is_object());
        assert!(body.get("error").is_none());

        // Verify the result has the expected structure
        let result = &body["result"];
        assert!(result.get("prompts").is_some());
        // Note: next_cursor is None when there are no more results, so it's omitted from JSON
    }

    #[tokio::test]
    async fn test_resources_list_method() {
        let app = create_default_test_app().await;

        let request_body = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "resources/list"
        });

        let request = Request::builder()
            .method(Method::POST)
            .uri("/mcp")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(serde_json::to_string(&request_body).unwrap()))
            .unwrap();

        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let body = response.into_body().collect().await.unwrap().to_bytes();
        let body: Value = serde_json::from_slice(&body).unwrap();
        
        assert_eq!(body["jsonrpc"], "2.0");
        assert_eq!(body["id"], 1);
        assert!(body["result"].is_object());
        assert!(body.get("error").is_none());

        // Verify the result has the expected structure
        let result = &body["result"];
        assert!(result.get("resources").is_some());
        // Note: next_cursor is None when there are no more results, so it's omitted from JSON
    }

    #[tokio::test]
    async fn test_tools_call_empty_name() {
        let app = create_default_test_app().await;

        let request_body = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/call",
            "params": {
                "name": "",
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

        let body = response.into_body().collect().await.unwrap().to_bytes();
        let body: Value = serde_json::from_slice(&body).unwrap();
        
        assert_eq!(body["jsonrpc"], "2.0");
        assert_eq!(body["id"], 1);
        assert!(body.get("result").is_none());
        assert!(body["error"].is_object());
        // Should get invalid format error
    }

    #[tokio::test]
    async fn test_tools_call_invalid_format() {
        let app = create_default_test_app().await;

        let request_body = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/call",
            "params": {
                "name": "invalid_tool_name_without_prefix",
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

        let body = response.into_body().collect().await.unwrap().to_bytes();
        let body: Value = serde_json::from_slice(&body).unwrap();
        
        assert_eq!(body["jsonrpc"], "2.0");
        assert_eq!(body["id"], 1);
        assert!(body.get("result").is_none());
        assert!(body["error"].is_object());
        // Should get invalid format error
    }

    #[tokio::test]
    async fn test_tools_call_server_not_found() {
        let app = create_default_test_app().await;

        let request_body = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/call",
            "params": {
                "name": "nonexistent_server___some_tool",
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

        let body = response.into_body().collect().await.unwrap().to_bytes();
        let body: Value = serde_json::from_slice(&body).unwrap();
        
        assert_eq!(body["jsonrpc"], "2.0");
        assert_eq!(body["id"], 1);
        assert!(body.get("result").is_none());
        assert!(body["error"].is_object());
        // Should get server not found error
    }

    #[tokio::test]
    async fn test_empty_config_no_servers() {
        // Test that proxy works even with no servers configured
        let fixture = TestFixture::builder().build();
        let app = fixture.build_app().await;

        // Test tools/list returns empty array
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

        let body = response.into_body().collect().await.unwrap().to_bytes();
        let body: Value = serde_json::from_slice(&body).unwrap();
        
        assert_eq!(body["jsonrpc"], "2.0");
        assert_eq!(body["id"], 1);
        assert!(body["result"].is_object());
        assert!(body["result"]["tools"].is_array());
        assert_eq!(body["result"]["tools"].as_array().unwrap().len(), 0);
    }

    #[tokio::test]
    async fn test_request_without_id() {
        let app = create_default_test_app().await;

        let request_body = json!({
            "jsonrpc": "2.0",
            "method": "ping"
        });

        let request = Request::builder()
            .method(Method::POST)
            .uri("/mcp")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(serde_json::to_string(&request_body).unwrap()))
            .unwrap();

        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let body = response.into_body().collect().await.unwrap().to_bytes();
        let body: Value = serde_json::from_slice(&body).unwrap();
        
        assert_eq!(body["jsonrpc"], "2.0");
        assert!(body.get("id").is_none() || body["id"].is_null());
        assert!(body["result"].is_object());
        assert!(body.get("error").is_none());
    }


}
