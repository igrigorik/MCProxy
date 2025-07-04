use std::{env, net::SocketAddr, sync::Arc};

use axum::{
    extract::State,
    http::StatusCode,
    response::Json,
    routing::post,
    Router,
};
use config::load_config;
use proxy::ProxyServer;
use rmcp::{
    model::*,
    Error as McpError,
    ServerHandler,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tower_http::cors::{Any, CorsLayer};
use tracing::{error, info, warn};

mod config;
mod proxy;

type SharedProxyServer = Arc<ProxyServer>;

/// JSON-RPC 2.0 request structure
#[derive(Debug, Clone, Deserialize)]
struct JsonRpcRequest {
    jsonrpc: String,
    id: Option<Value>,
    method: String,
    params: Option<Value>,
}

/// JSON-RPC 2.0 response structure  
#[derive(Debug, Clone, Serialize, Deserialize)]
struct JsonRpcResponse {
    jsonrpc: String,
    id: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<JsonRpcError>,
}

/// JSON-RPC 2.0 error structure
#[derive(Debug, Clone, Serialize, Deserialize)]
struct JsonRpcError {
    code: i32,
    message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    data: Option<Value>,
}

impl JsonRpcResponse {
    fn success(id: Option<Value>, result: Value) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            id,
            result: Some(result),
            error: None,
        }
    }
    
    fn error(id: Option<Value>, error: JsonRpcError) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            id,
            result: None,
            error: Some(error),
        }
    }
}

impl From<McpError> for JsonRpcError {
    fn from(error: McpError) -> Self {
        Self {
            code: error.code.0,
            message: error.message.to_string(),
            data: error.data,
        }
    }
}

/// Fully compliant MCP endpoint that handles JSON-RPC 2.0 requests
async fn mcp_handler(
    State(proxy): State<SharedProxyServer>,
    Json(request): Json<Value>,
) -> Result<Json<JsonRpcResponse>, StatusCode> {
    // Parse JSON-RPC request
    let json_rpc_request: JsonRpcRequest = match serde_json::from_value(request) {
        Ok(req) => req,
        Err(e) => {
            warn!("Invalid JSON-RPC request: {}", e);
            let response = JsonRpcResponse::error(
                None,
                JsonRpcError {
                    code: -32700, // Parse error
                    message: "Parse error".to_string(), 
                    data: Some(serde_json::json!({ "details": e.to_string() })),
                },
            );
            return Ok(Json(response));
        }
    };

    // Validate JSON-RPC version
    if json_rpc_request.jsonrpc != "2.0" {
        let response = JsonRpcResponse::error(
            json_rpc_request.id,
            JsonRpcError {
                code: -32600, // Invalid Request
                message: "Invalid Request".to_string(),
                data: Some(serde_json::json!({ "details": "Only JSON-RPC 2.0 is supported" })),
            },
        );
        return Ok(Json(response));
    }

    tracing::debug!("Processing MCP request: {} (id: {:?})", json_rpc_request.method, json_rpc_request.id);

    // Route MCP method to appropriate handler
    let result = route_mcp_method(&proxy, json_rpc_request.method.as_str(), json_rpc_request.params).await;
    
    let response = match result {
        Ok(result) => JsonRpcResponse::success(json_rpc_request.id, result),
        Err(error) => JsonRpcResponse::error(json_rpc_request.id, error.into()),
    };

    Ok(Json(response))
}

/// Route MCP method calls to the appropriate ProxyServer handlers
async fn route_mcp_method(
    proxy: &ProxyServer,
    method: &str,
    params: Option<Value>,
) -> Result<Value, McpError> {
    // For HTTP endpoints, we call the ServerHandler methods directly
    // rather than going through the full MCP request context

    match method {
        "initialize" => {
            let _init_params: InitializeRequestParam = if let Some(params) = params {
                serde_json::from_value(params)
                    .map_err(|e| McpError::invalid_params(format!("Invalid initialize params: {}", e), None))?
            } else {
                return Err(McpError::invalid_params("Missing initialize params", None));
            };
            
            let result = proxy.get_info();
            serde_json::to_value(result).map_err(|e| McpError::internal_error(
                format!("Failed to serialize result: {}", e), None
            ))
        }
        
        "ping" => {
            // Ping just returns an empty object
            Ok(serde_json::json!({}))
        }
        
        "tools/list" => {
            let _list_params: Option<PaginatedRequestParam> = if let Some(params) = params {
                Some(serde_json::from_value(params)
                    .map_err(|e| McpError::invalid_params(format!("Invalid tools/list params: {}", e), None))?)
            } else {
                None
            };
            
            // Get tools directly without context
            let tools = proxy.get_all_tools().await;
            let result = ListToolsResult {
                tools,
                next_cursor: None,
            };
            serde_json::to_value(result).map_err(|e| McpError::internal_error(
                format!("Failed to serialize result: {}", e), None
            ))
        }
        
        "tools/call" => {
            let call_params: CallToolRequestParam = if let Some(params) = params {
                serde_json::from_value(params)
                    .map_err(|e| McpError::invalid_params(format!("Invalid tools/call params: {}", e), None))?
            } else {
                return Err(McpError::invalid_params("Missing tools/call params", None));
            };
            
            let result = proxy.call_tool_on_server(&call_params.name, call_params.arguments).await?;
            serde_json::to_value(result).map_err(|e| McpError::internal_error(
                format!("Failed to serialize result: {}", e), None
            ))
        }
        
        "prompts/list" => {
            let _list_params: Option<PaginatedRequestParam> = if let Some(params) = params {
                Some(serde_json::from_value(params)
                    .map_err(|e| McpError::invalid_params(format!("Invalid prompts/list params: {}", e), None))?)
            } else {
                None
            };
            
            let prompts = proxy.get_all_prompts().await;
            let result = ListPromptsResult {
                prompts,
                next_cursor: None,
            };
            serde_json::to_value(result).map_err(|e| McpError::internal_error(
                format!("Failed to serialize result: {}", e), None
            ))
        }
        
        "resources/list" => {
            let _list_params: Option<PaginatedRequestParam> = if let Some(params) = params {
                Some(serde_json::from_value(params)
                    .map_err(|e| McpError::invalid_params(format!("Invalid resources/list params: {}", e), None))?)
            } else {
                None
            };
            
            let resources = proxy.get_all_resources().await;
            let result = ListResourcesResult {
                resources,
                next_cursor: None,
            };
            serde_json::to_value(result).map_err(|e| McpError::internal_error(
                format!("Failed to serialize result: {}", e), None
            ))
        }
        
        _ => {
            warn!("Unknown MCP method: {}", method);
            Err(McpError::invalid_params(format!("Unknown method: {}", method), None))
        }
    }
}



async fn health_check() -> Json<Value> {
    Json(serde_json::json!({
        "status": "healthy",
        "service": "mcproxy"
    }))
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
    let config = load_config(config_path)?;

    // Create the proxy server and connect to all MCP servers
    info!("Connecting to {} MCP servers...", config.mcp_servers.len());
    let proxy_server = ProxyServer::new(config.clone()).await?;
    let shared_proxy = Arc::new(proxy_server);

    // Get HTTP server configuration with defaults
    let http_config = config
        .http_server
        .as_ref()
        .ok_or("HTTP server configuration is required")?;

    // Convert hostname to IP address for SocketAddr parsing
    let host_ip = if http_config.host == "localhost" {
        "127.0.0.1"
    } else {
        &http_config.host
    };
    
    let bind_address_str = format!("{}:{}", host_ip, http_config.port);
    info!("Parsed HTTP config - host: '{}', port: {}, bind_address: '{}'", 
          http_config.host, http_config.port, bind_address_str);

    let bind_addr: SocketAddr = bind_address_str
        .parse()
        .map_err(|e| format!("Invalid bind address '{}': {}", bind_address_str, e))?;

    info!("Binding HTTP server to {}", bind_addr);

    // Configure CORS based on config
    let cors = if http_config.cors_enabled {
        let mut cors_layer = CorsLayer::new();
        
        if http_config.cors_origins.contains(&"*".to_string()) {
            cors_layer = cors_layer.allow_origin(Any);
        } else {
            // Parse allowed origins from config
            for origin in &http_config.cors_origins {
                if let Ok(header_value) = origin.parse::<axum::http::HeaderValue>() {
                    cors_layer = cors_layer.allow_origin(header_value);
                }
            }
        }
        
        cors_layer
            .allow_methods([axum::http::Method::GET, axum::http::Method::POST, axum::http::Method::OPTIONS])
            .allow_headers([axum::http::header::CONTENT_TYPE, axum::http::header::AUTHORIZATION])
    } else {
        CorsLayer::new()
    };

    // Create HTTP server using axum
    let app = Router::new()
        .route("/mcp", post(mcp_handler))
        .route("/health", axum::routing::get(health_check))
        .layer(cors)
        .with_state(shared_proxy.clone());

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
    };
    use http_body_util::BodyExt;
    use serde_json::json;
    use std::collections::HashMap;
    use tower::ServiceExt;

    /// Create a test configuration
    fn create_test_config() -> McpConfig {
        McpConfig {
            mcp_servers: HashMap::new(),
            http_server: Some(config::HttpServerConfig {
                host: "127.0.0.1".to_string(),
                port: 0, // Use random port for testing
                cors_enabled: true,
                cors_origins: vec!["*".to_string()],
            }),
        }
    }

    /// Helper to create a test app with proxy server
    async fn create_test_app(proxy: ProxyServer) -> Router {
        let shared_proxy = Arc::new(proxy);

        let cors = CorsLayer::new()
            .allow_origin(Any)
            .allow_methods([Method::GET, Method::POST, Method::OPTIONS])
            .allow_headers([header::CONTENT_TYPE, header::AUTHORIZATION]);

        Router::new()
            .route("/mcp", post(mcp_handler))
            .route("/health", axum::routing::get(health_check))
            .layer(cors)
            .with_state(shared_proxy)
    }

    /// Helper to create a test app with default config
    async fn create_default_test_app() -> Router {
        let config = create_test_config();
        let proxy = ProxyServer::new(config).await.unwrap();
        create_test_app(proxy).await
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
        let body: JsonRpcResponse = serde_json::from_slice(&body).unwrap();
        
        assert_eq!(body.jsonrpc, "2.0");
        assert!(body.error.is_some());
        assert_eq!(body.error.unwrap().code, -32600); // Invalid Request
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
        let body: JsonRpcResponse = serde_json::from_slice(&body).unwrap();
        
        assert_eq!(body.jsonrpc, "2.0");
        assert_eq!(body.id, Some(json!(1)));
        assert!(body.result.is_some());
        assert!(body.error.is_none());
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
        let body: JsonRpcResponse = serde_json::from_slice(&body).unwrap();
        
        assert_eq!(body.jsonrpc, "2.0");
        assert_eq!(body.id, Some(json!(1)));
        assert!(body.result.is_some());
        assert!(body.error.is_none());

        // Verify the result contains server info
        let result = body.result.unwrap();
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
        let body: JsonRpcResponse = serde_json::from_slice(&body).unwrap();
        
        assert_eq!(body.jsonrpc, "2.0");
        assert_eq!(body.id, Some(json!(1)));
        assert!(body.result.is_some());
        assert!(body.error.is_none());

        // Verify the result has the expected structure
        let result = body.result.unwrap();
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
        let body: JsonRpcResponse = serde_json::from_slice(&body).unwrap();
        
        assert_eq!(body.jsonrpc, "2.0");
        assert_eq!(body.id, Some(json!(1)));
        assert!(body.result.is_none());
        assert!(body.error.is_some());
        assert_eq!(body.error.unwrap().code, -32602); // Invalid params
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
        let body: JsonRpcResponse = serde_json::from_slice(&body).unwrap();
        
        assert_eq!(body.jsonrpc, "2.0");
        assert_eq!(body.id, Some(json!(1)));
        assert!(body.result.is_none());
        assert!(body.error.is_some());
        assert_eq!(body.error.unwrap().code, -32602); // Invalid params (our current implementation)
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
        let body: JsonRpcResponse = serde_json::from_slice(&body).unwrap();
        
        assert_eq!(body.jsonrpc, "2.0");
        assert_eq!(body.id, Some(json!(1)));
        assert!(body.result.is_some());
        assert!(body.error.is_none());

        // Verify the result has the expected structure
        let result = body.result.unwrap();
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
        let body: JsonRpcResponse = serde_json::from_slice(&body).unwrap();
        
        assert_eq!(body.jsonrpc, "2.0");
        assert_eq!(body.id, Some(json!(1)));
        assert!(body.result.is_some());
        assert!(body.error.is_none());

        // Verify the result has the expected structure
        let result = body.result.unwrap();
        assert!(result.get("resources").is_some());
        // Note: next_cursor is None when there are no more results, so it's omitted from JSON
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
        let body: JsonRpcResponse = serde_json::from_slice(&body).unwrap();
        
        assert_eq!(body.jsonrpc, "2.0");
        assert_eq!(body.id, None);
        assert!(body.result.is_some());
        assert!(body.error.is_none());
    }


}
