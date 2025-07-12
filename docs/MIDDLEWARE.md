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
- **`tool_search`**: Provides intelligent tool management with selective exposure and search functionality

#### Client Middleware  
- **`logging`**: Logs all operations with timing information per server
- **`tool_filter`**: Filters tools based on configurable regex patterns (allow/disallow)
- **`security`**: Inspects tool call inputs and blocks calls that match security rules

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


## Available Middleware

### Logging (Client Middleware)
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

### Tool Filter (Client Middleware)
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

### Security (Client Middleware)
Inspects tool call inputs and blocks calls that match security rules:

```rust
let config = SecurityConfig {
    rules: vec![
        SecurityRule {
            name: "no_system_commands".to_string(),
            description: "Block dangerous system commands".to_string(),
            pattern: r"(?i)(rm\s+-rf|sudo\s+|passwd\s+)".to_string(),
            block_message: "System commands are not allowed".to_string(),
            enabled: true,
        }
    ],
    log_blocked: true,
};
let middleware = SecurityClientMiddleware::new("server-name".to_string(), config)?;
```

**Configuration options:**
- `rules`: Array of security rules to apply
- `log_blocked`: Whether to log blocked calls (default: true)

**Security Rule fields:**
- `name`: Unique name for the rule
- `description`: Human-readable description
- `pattern`: Regex pattern to match against tool call content
- `block_message`: Error message returned when rule is violated
- `enabled`: Whether the rule is active (default: true)

**Default security rules:**
- **no_system_commands**: Blocks `rm -rf`, `sudo`, `passwd`, `shutdown`, `reboot`, `format`, etc.
- **no_sensitive_files**: Blocks access to `/etc/passwd`, `/etc/shadow`, `.ssh/`, `.aws/`, `.env`, `config.json`, `secrets`
- **no_network_commands**: Blocks `curl`, `wget`, `nc`, `netcat`, `telnet`, `ssh`, `ftp`, `sftp`

**How it works:**
1. Extracts searchable content from tool calls (tool name + JSON-serialized arguments)
2. Checks content against each enabled security rule's regex pattern
3. Returns blocked result with error message if any rule matches
4. Logs blocked calls for security monitoring

**Basic security configuration:**
```json
{
  "middleware": {
    "client": {
      "default": [
        {
          "type": "security",
          "enabled": true,
          "config": {
            "rules": [
              {
                "name": "no_system_commands",
                "description": "Block dangerous system commands",
                "pattern": "(?i)(rm\\s+-rf|sudo\\s+|passwd\\s+|shutdown|reboot)",
                "block_message": "System commands are not allowed",
                "enabled": true
              }
            ],
            "log_blocked": true
          }
        }
      ]
    }
  }
}
```

**Server-specific security rules:**
```json
{
  "middleware": {
    "client": {
      "servers": {
        "fetch": [
          {
            "type": "security",
            "enabled": true,
            "config": {
              "rules": [
                {
                  "name": "no_private_urls",
                  "description": "Block private network URLs",
                  "pattern": "(?i)(localhost|127\\.0\\.0\\.1|192\\.168\\.|file://)",
                  "block_message": "Private network URLs are not allowed",
                  "enabled": true
                }
              ]
            }
          }
        ]
      }
    }
  }
}
```

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

### Description Enricher (Proxy Middleware)
Adds "(via mcproxy)" suffix to all tool/prompt/resource descriptions:

```rust
let middleware = DescriptionEnricherMiddleware;
```

**Example:** "Search files" becomes "Search files (via mcproxy)"

### Tool Search (Proxy Middleware)
Provides intelligent tool management when large number of tools are available:

```rust
let config = ToolSearchMiddlewareConfig {
    enabled: true,
    max_tools_limit: 50,
    search_threshold: 0.3,
    tool_selection_order: vec!["server_priority".to_string()],
};
let middleware = ToolSearchMiddleware::new(config);
```

**How it works:**
1. **Selective Exposure**: Limits initial tools to `max_tools_limit` count
2. **Search Tool Injection**: Adds a special `search_available_tools` tool when tools are limited
3. **Fuzzy Search**: Uses fuzzy matching on tool names and descriptions
4. **Dynamic Updates**: Replaces exposed tools with search results

**Configuration options:**
- `enabled`: Enable/disable the feature (default: true)
- `max_tools_limit`: Maximum tools to expose initially and return in search results (default: 50)
- `search_threshold`: Minimum similarity score (0.0-1.0, default: 0.3)
- `tool_selection_order`: Criteria for selecting initial tools (default: ["server_priority"])

**Search tool usage:**
```json
{
  "name": "search_available_tools",
  "arguments": {
    "query": "file operations"
  }
}
```

**Example middleware configuration:**
```json
{
  "httpServer": {
    "middleware": {
      "proxy": [
        {
          "type": "tool_search",
          "enabled": true,
          "config": {
            "maxToolsLimit": 25,
            "searchThreshold": 0.5,
            "toolSelectionOrder": ["server_priority"]
          }
        }
      ]
    }
  }
}
```

## Implementation Details

Custom ProxyMiddleware Example

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
    
    // before_call_tool can return MiddlewareResult::Block to prevent the call
    async fn before_call_tool(&self, request: &CallToolRequestParam) -> MiddlewareResult {
        MiddlewareResult::Continue
    }
    async fn after_call_tool(&self, result: &Result<CallToolResult, ServiceError>) {}
    
    async fn before_list_prompts(&self) {}
    async fn after_list_prompts(&self, result: &Result<ListPromptsResult, ServiceError>) {}
    
    async fn before_list_resources(&self) {}
    async fn after_list_resources(&self, result: &Result<ListResourcesResult, ServiceError>) {}
}
```

### MiddlewareResult

```rust
pub enum MiddlewareResult {
    /// The operation should proceed normally  
    Continue,
    /// The operation should be blocked with the given error message
    Block(String),
}
```

Any middleware can block tool calls by returning `MiddlewareResult::Block(message)` from `before_call_tool`. When a middleware blocks a call:
1. Processing stops immediately (short-circuits remaining middleware)
2. A blocked `CallToolResult` with `is_error: true` is returned
3. The actual tool call to downstream servers is never made
4. All middleware still receive the `after_call_tool` notification