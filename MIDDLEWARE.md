# Middleware Architecture

The MCP Proxy Server includes a flexible middleware system that allows you to intercept and modify requests and responses at two different levels:

## Architecture Overview

### Key Differences

| Aspect      | **ClientMiddleware**             | **ProxyMiddleware**               |
| ----------- | -------------------------------- | --------------------------------- |
| **Scope**   | Per-server (individual)          | Cross-server (aggregated)         |
| **When**    | Around each server call          | After collecting from all servers |
| **Data**    | Single server's request/response | Combined results from all servers |
| **Purpose** | Observability, retries, caching  | Filtering, security, enrichment   |

### 1. **ClientMiddleware** - Individual Server Interception  
- **Intercepts**: Each call to a specific downstream server
- **Sees**: Raw requests/responses from one server at a time
- **Examples**: Logging per server, retry logic, performance monitoring
- **Pattern**: `before_*` and `after_*` hooks around each operation

### 2. **ProxyMiddleware** - Aggregated Results Processing
- **Operates on**: Combined results from all connected servers
- **Sees**: Final aggregated list of tools/prompts/resources
- **Examples**: Security filtering, deduplication, description enrichment
- **Pattern**: Modify the final collections before returning to client

## Implementation Details

### ProxyMiddleware Interface

```rust
#[async_trait]
pub trait ProxyMiddleware: Send + Sync {
    /// Called after aggregating tools from all servers
    async fn on_list_tools(&self, tools: &mut Vec<Tool>) {}
    
    /// Called after aggregating prompts from all servers  
    async fn on_list_prompts(&self, prompts: &mut Vec<Prompt>) {}
    
    /// Called after aggregating resources from all servers
    async fn on_list_resources(&self, resources: &mut Vec<Resource>) {}
}
```

### ClientMiddleware Interface

```rust
#[async_trait]
pub trait ClientMiddleware: Send + Sync {
    // Before/after hooks for each MCP operation
    async fn before_list_tools(&self) {}
    async fn after_list_tools(&self, result: &Result<ListToolsResult, ServiceError>) {}
    
    async fn before_call_tool(&self, request: &CallToolRequestParam) {}
    async fn after_call_tool(&self, result: &Result<CallToolResult, ServiceError>) {}
    
    async fn before_list_prompts(&self) {}
    async fn after_list_prompts(&self, result: &Result<ListPromptsResult, ServiceError>) {}
    
    async fn before_list_resources(&self) {}
    async fn after_list_resources(&self, result: &Result<ListResourcesResult, ServiceError>) {}
}
```

## Middleware Examples

### LoggingClientMiddleware
Logs all operations for a specific server with detailed context:

```rust
let middleware = LoggingClientMiddleware::new("my-server".to_string());
```

**Output Example:**
```
📋 [my-server] Listing tools...
✅ [my-server] Listed 5 tools successfully
🔧 [my-server] Calling tool: file_search
✅ [my-server] Tool call successful
```

### ToolFilterClientMiddleware
Filters tools from individual servers based on configurable regex patterns:

```rust
let config = ToolFilterConfig {
    disallow: Some("test|debug".to_string()),
    allow: None,
};
let middleware = ToolFilterClientMiddleware::new("server-name".to_string(), config)?;
```

**Configuration options:**
- `disallow`: Regex pattern for tools to filter out
- `allow`: Regex pattern for tools to keep (takes precedence over disallow)

**Examples:**
- `{"disallow": "test"}` - filters out tools containing "test"
- `{"allow": "file_|search_"}` - only keeps tools starting with "file_" or "search_"
- `{"disallow": "test|debug|dev"}` - filters out tools containing "test", "debug", or "dev"
- `{}` - uses default behavior (allows everything)

**Server-specific filtering:**
```json
"servers": {
  "github": [
    {
      "type": "tool_filter",
      "config": { "allow": "issue" }
    }
  ],
  "filesystem": [
    {
      "type": "tool_filter",
      "config": { "disallow": "test|tmp" }
    }
  ]
}
```

### DescriptionEnricherMiddleware
Adds "(via mcproxy)" suffix to all tool/prompt/resource descriptions:

```rust
let middleware = DescriptionEnricherMiddleware;
```

**Example:** "Search files" becomes "Search files (via mcproxy)"

## Creating Custom Middleware

### Custom ProxyMiddleware Example

```rust
use async_trait::async_trait;
use rmcp::model::Tool;

#[derive(Debug)]
pub struct SecurityFilterMiddleware {
    allowed_tools: HashSet<String>,
}

#[async_trait]
impl ProxyMiddleware for SecurityFilterMiddleware {
    async fn on_list_tools(&self, tools: &mut Vec<Tool>) {
        tools.retain(|tool| self.allowed_tools.contains(&tool.name.to_string()));
        info!("Security filter applied, {} tools remaining", tools.len());
    }
}
```

## Configuration and Registration

Middleware are configured through the configuration file using a flexible JSON-based system. The system supports server-specific client middleware overrides and generic JSON configuration for maximum flexibility.

### Configuration Structure

```json
{
  "httpServer": {
    "host": "localhost",
    "port": 8080,
    "middleware": {
      "proxy": [
        {
          "type": "description_enricher",
          "enabled": true,
          "config": {}
        }
      ],
      "client": {
        "default": [
          {
            "type": "logging",
            "enabled": true,
            "config": {
              "level": "info"
            }
          }
        ],
        "servers": {
          "critical-server": [
            {
              "type": "logging",
              "enabled": true,
              "config": {
                "level": "debug"
              }
            }
          ],
          "github-server": [
            {
              "type": "tool_filter",
              "enabled": true,
              "config": {
                "allow": "issue"
              }
            }
          ]
        }
      }
    }
  }
}
```

### Built-in Middleware Types

#### Proxy Middleware
- **`description_enricher`**: Adds "(via mcproxy)" to tool/prompt/resource descriptions

#### Client Middleware  
- **`logging`**: Logs all operations with timing information per server
- **`tool_filter`**: Filters tools based on configurable regex patterns (allow/disallow)

### Configuration Options

Each middleware specification supports:
- **`type`** (required): The middleware type name
- **`enabled`** (optional, default: true): Whether to enable this middleware
- **`config`** (optional): Middleware-specific configuration as JSON

### Server-Specific Client Middleware

Client middleware can be configured per-server:
- **`default`**: Applied to all servers unless overridden
- **`servers`**: Server-specific overrides by server name

When a server has specific configuration, it completely replaces the default configuration for that server (no merging).

### Advanced Tool Filter Configuration Examples

The `tool_filter` middleware supports flexible regex-based filtering:

```json
{
  "type": "tool_filter",
  "enabled": true,
  "config": {
    "allow": "^(file_|search_|git_)"
  }
}
```
*Only allows tools starting with "file_", "search_", or "git_"*

```json
{
  "type": "tool_filter", 
  "enabled": true,
  "config": {
    "disallow": "test|debug|dev|tmp"
  }
}
```
*Filters out tools containing "test", "debug", "dev", or "tmp"*

```json
{
  "type": "tool_filter",
  "enabled": true,
  "config": {
    "allow": "^(?!.*test).*$"
  }
}
```
*Allows all tools except those containing "test" (using negative lookahead)*

**Priority**: If both `allow` and `disallow` are specified, `allow` takes precedence.
