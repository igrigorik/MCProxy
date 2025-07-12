# Tool Search Functionality

The MCP Proxy implements a Tantivy-powered tool search system that:
- **Indexes tools** from all connected MCP servers using Tantivy full-text search
- **Limits initial list of tools** to prevent overwhelming clients with hundreds of tools
- **Provides dynamic search** to discover hidden tools on-demand
- **Maintains real-time updates** when tool lists change

## How It Works

### 1. Tool Indexing

When the proxy starts or when server tool lists change:

```
MCP Servers → Proxy → Tantivy Index → Search Ready
```

- **In-memory indexing**: Uses Tantivy's in-memory index for fast searches
- **Multi-field indexing**: Indexes tool names, descriptions, and server metadata
- **Full-text search**: Supports boolean queries, phrase matching, and relevance scoring

### 2. Tool Selection

The proxy intelligently selects which tools to expose initially:

```rust
// Configuration example
{
  "maxToolsLimit": 50,           // Maximum tools to expose initially
  "searchThreshold": 0.1,        // Minimum relevance score (0-1)
  "toolSelectionOrder": ["server_priority", "alphabetical"]
}
```

### 3. Search Interface

When tools are limited, the proxy automatically injects a `search_available_tools` tool:

```json
{
  "name": "search_available_tools",
  "description": "Search through 150 additional available tools...",
  "input_schema": {
    "type": "object",
    "properties": {
      "query": {
        "type": "string",
        "description": "Search query to find relevant tools"
      }
    },
    "required": ["query"]
  }
}
```

## Usage Examples

### Basic Search

```bash
# Search for GitHub-related tools
curl -X POST http://localhost:8080/mcp \
  -H "Content-Type: application/json" \
  -d '{
    "jsonrpc": "2.0",
    "id": 1,
    "method": "tools/call",
    "params": {
      "name": "search_available_tools",
      "arguments": {
        "query": "github"
      }
    }
  }'
```

### Advanced Search Queries

The search system supports various query types:

```bash
# Boolean queries
"github AND issue"
"file OR document"

# Phrase matching
"pull request"
"issue comment"

# Field-specific (internal use)
server:github description:"create issue"
```

### Search Results

Search results are returned as formatted text with tool details:

```
Found 5 tool(s) matching 'github':

1. **github___create_issue**
   Create a new issue in a GitHub repository

2. **github___add_issue_comment**
   Add a comment to a specific issue in a GitHub repository

3. **github___search_issues**
   Search for issues in GitHub repositories

...

These tools are now available in your current session.
```

## Configuration

### Middleware Configuration

```json
{
  "middleware": {
    "proxy": [
      {
        "type": "tool_search",
        "enabled": true,
        "config": {
          "maxToolsLimit": 50,
          "searchThreshold": 0.1,
          "toolSelectionOrder": ["server_priority", "alphabetical"]
        }
      }
    ]
  }
}
```

### Configuration Options

| Option               | Type    | Default               | Description                              |
| -------------------- | ------- | --------------------- | ---------------------------------------- |
| `enabled`            | boolean | `true`                | Enable/disable tool search functionality |
| `maxToolsLimit`      | number  | `50`                  | Maximum tools to expose initially        |
| `searchThreshold`    | number  | `0.1`                 | Minimum relevance score (0-1 scale)      |
| `toolSelectionOrder` | array   | `["server_priority"]` | Tool selection criteria                  |

### Tool Selection Orders

- **`server_priority`**: Group tools by server, then alphabetically
- **`alphabetical`**: Sort all tools alphabetically

## Technical Implementation

### Architecture

```
┌─────────────────┐    ┌──────────────────┐    ┌─────────────────┐
│   MCP Servers   │───▶│   Tool Search    │───▶│   Client Apps   │
│                 │    │   Middleware     │    │                 │
│ • Server A      │    │                  │    │ • Limited Tools │
│ • Server B      │    │ • Tantivy Index  │    │ • Search Tool   │
│ • Server C      │    │ • Query Engine   │    │ • Dynamic Load  │
└─────────────────┘    └──────────────────┘    └─────────────────┘
```

### Index Schema

```rust
// Tantivy schema fields
name: TEXT | STORED           // Tool name (e.g., "github___create_issue")
description: TEXT | STORED    // Tool description
server: TEXT | STORED         // Server name (e.g., "github")
searchable_text: TEXT         // Combined searchable content
```

### Search Flow

1. **Query Parsing**: Parse user query with fallback to quoted strings
2. **Index Search**: Execute query against Tantivy index
3. **Relevance Scoring**: Apply BM25 scoring with threshold filtering
4. **Result Formatting**: Format results as structured text response
5. **Tool Exposure**: Update exposed tools list with search results

### Performance Characteristics

- **Index Creation**: O(n log n) - only when tools change
- **Search Operations**: O(log n) - sub-linear search time
- **Memory Usage**: ~1-5MB per 100 tools indexed
- **Index Lifecycle**: Built at startup, rebuilt on tool list changes

## Integration with SSE

The search system integrates with Server-Sent Events for real-time updates:

```javascript
// SSE response includes notification + results
data: {"jsonrpc": "2.0", "method": "notifications/tools/list_changed"}
data: {"jsonrpc": "2.0", "id": 1, "result": {"content": [...]}}
```

## Troubleshooting

### Common Issues

**No search results found:**
- Check `searchThreshold` configuration (try lowering to 0.05)
- Verify tool indexing with debug logs
- Try simpler query terms

**Search tool not appearing:**
- Ensure total tools > `maxToolsLimit`
- Verify middleware is enabled in configuration
- Check server connection status

**Performance issues:**
- Monitor index rebuild frequency in logs
- Adjust `maxToolsLimit` based on usage patterns
- Consider server-specific tool filtering

### Debug Logging

Enable debug logging to troubleshoot search issues:

```bash
RUST_LOG=mcproxy::middleware::proxy_middleware::tool_search=debug
```

Debug logs include:
- Index creation and tool count
- Search query parsing and execution
- Relevance scoring and threshold filtering
- Tool selection and exposure decisions