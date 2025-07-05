//! HTTP server module for serving the MCP proxy over HTTP
//! 
//! This module provides the HTTP/JSON-RPC interface for the MCP proxy:
//! - JSON-RPC 2.0 compliant endpoint at `/mcp`
//! - Health check endpoint at `/health`
//! - Configurable CORS support
//! - Request routing to appropriate MCP methods
//! 
//! # JSON-RPC Implementation
//! 
//! The server implements a subset of the MCP protocol over JSON-RPC 2.0:
//! - `initialize`: Get server information and capabilities
//! - `ping`: Simple connectivity check
//! - `tools/list`: List all available tools from all servers
//! - `tools/call`: Execute a tool on the appropriate server
//! - `prompts/list`: List all available prompts
//! - `resources/list`: List all available resources
//! 
//! # Error Handling
//! 
//! All errors are returned as JSON-RPC error responses with appropriate error codes:
//! - `-32700`: Parse error (malformed JSON)
//! - `-32600`: Invalid request (wrong JSON-RPC version)
//! - `-32602`: Invalid params (missing or invalid parameters)
//! - `-32603`: Internal error (server-side errors)

use std::{net::SocketAddr, sync::Arc};

use axum::{
    extract::State,
    http::StatusCode,
    response::Json,
    routing::post,
    Router,
};
use rmcp::{
    model::*,
    Error as McpError,
    ServerHandler,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tower_http::cors::{Any, CorsLayer};
use tracing::{info, warn};

use crate::{config::HttpServerConfig, proxy::ProxyServer};

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

/// Create the HTTP router for the MCP proxy
pub fn create_router(proxy_server: Arc<ProxyServer>, http_config: &HttpServerConfig) -> Router {
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

    Router::new()
        .route("/mcp", post(mcp_handler))
        .route("/health", axum::routing::get(health_check))
        .layer(cors)
        .with_state(proxy_server)
}

/// Parse bind address from HTTP config
pub fn parse_bind_address(http_config: &HttpServerConfig) -> Result<SocketAddr, String> {
    // Convert hostname to IP address for SocketAddr parsing
    let host_ip = if http_config.host == "localhost" {
        "127.0.0.1"
    } else {
        &http_config.host
    };
    
    let bind_address_str = format!("{}:{}", host_ip, http_config.port);
    info!("Parsed HTTP config - host: '{}', port: {}, bind_address: '{}'", 
          http_config.host, http_config.port, bind_address_str);

    bind_address_str
        .parse()
        .map_err(|e| format!("Invalid bind address '{}': {}", bind_address_str, e))
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

/// Helper to parse optional parameters
fn parse_optional_params<T: serde::de::DeserializeOwned>(
    params: Option<Value>,
    param_name: &str,
) -> Result<Option<T>, McpError> {
    match params {
        Some(p) => Ok(Some(serde_json::from_value(p)
            .map_err(|e| McpError::invalid_params(format!("Invalid {} params: {}", param_name, e), None))?)),
        None => Ok(None),
    }
}

/// Helper to parse required parameters
fn parse_required_params<T: serde::de::DeserializeOwned>(
    params: Option<Value>,
    param_name: &str,
) -> Result<T, McpError> {
    match params {
        Some(p) => serde_json::from_value(p)
            .map_err(|e| McpError::invalid_params(format!("Invalid {} params: {}", param_name, e), None)),
        None => Err(McpError::invalid_params(format!("Missing {} params", param_name), None)),
    }
}

/// Helper to serialize result
fn serialize_result<T: Serialize>(result: T) -> Result<Value, McpError> {
    serde_json::to_value(result)
        .map_err(|e| McpError::internal_error(format!("Failed to serialize result: {}", e), None))
}

/// Route MCP method calls to the appropriate ProxyServer handlers
async fn route_mcp_method(
    proxy: &ProxyServer,
    method: &str,
    params: Option<Value>,
) -> Result<Value, McpError> {
    match method {
        "initialize" => {
            let _init_params: InitializeRequestParam = parse_required_params(params, "initialize")?;
            serialize_result(proxy.get_info())
        }
        
        "ping" => Ok(serde_json::json!({})),
        
        "tools/list" => {
            let _list_params: Option<PaginatedRequestParam> = parse_optional_params(params, "tools/list")?;
            let tools = proxy.get_all_tools().await;
            serialize_result(ListToolsResult {
                tools,
                next_cursor: None,
            })
        }
        
        "tools/call" => {
            let call_params: CallToolRequestParam = parse_required_params(params, "tools/call")?;
            let result = proxy.call_tool_on_server(&call_params.name, call_params.arguments).await?;
            serialize_result(result)
        }
        
        "prompts/list" => {
            let _list_params: Option<PaginatedRequestParam> = parse_optional_params(params, "prompts/list")?;
            let prompts = proxy.get_all_prompts().await;
            serialize_result(ListPromptsResult {
                prompts,
                next_cursor: None,
            })
        }
        
        "resources/list" => {
            let _list_params: Option<PaginatedRequestParam> = parse_optional_params(params, "resources/list")?;
            let resources = proxy.get_all_resources().await;
            serialize_result(ListResourcesResult {
                resources,
                next_cursor: None,
            })
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