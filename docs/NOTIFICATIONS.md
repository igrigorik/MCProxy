# MCP Proxy Notifications

This document explains how the MCP Proxy handles notifications when search tools update the exposed tool list.

## Overview

The MCP Proxy uses Server-Sent Events (SSE) streaming to provide real-time notifications to clients. When clients use the `search_available_tools` tool to search for tools, the proxy:

1. **Searches** through all available tools using the query
2. **Updates** the exposed tools list with search results
3. **Sends** a `toolListChanged` notification via SSE stream

## How It Works

### SSE Streaming for Compatible Tools

When `search_available_tools` is called, the server automatically responds with SSE streaming:

```
Client -> search_available_tools -> SSE Stream: [notification + response]
```

### Notification Flow

1. **Tool Search**: Client calls `search_available_tools` with a query
2. **SSE Response**: Server sends both notification and response in a single SSE stream:
   - First event: `notifications/tools/list_changed` notification
   - Second event: Tool search results
3. **Client Action**: Client receives both events and can update its tool list accordingly

## Example SSE Stream

When a client calls `search_available_tools`, the server responds with:

**Response Headers:**
```
Content-Type: text/event-stream
Cache-Control: no-cache
Connection: keep-alive
```

**SSE Stream Events:**
```
data: {"jsonrpc": "2.0", "method": "notifications/tools/list_changed", "params": null}

data: {"jsonrpc": "2.0", "id": 1, "result": {"content": [{"type": "text", "text": "Found 3 tools..."}], "is_error": false}}

```

## Client Implementation

```javascript
// Make a tool search request
const response = await fetch('/mcp', {
  method: 'POST',
  headers: { 'Content-Type': 'application/json' },
  body: JSON.stringify({
    jsonrpc: '2.0',
    id: 1,
    method: 'tools/call',
    params: {
      name: 'search_available_tools',
      arguments: { query: 'file operations' }
    }
  })
});

// Handle SSE stream
const reader = response.body.getReader();
const decoder = new TextDecoder();

while (true) {
  const { done, value } = await reader.read();
  if (done) break;
  
  const chunk = decoder.decode(value);
  const lines = chunk.split('\n');
  
  for (const line of lines) {
    if (line.startsWith('data: ')) {
      const eventData = JSON.parse(line.slice(6));
      
      if (eventData.method === 'notifications/tools/list_changed') {
        console.log('Tools changed, should refresh tool list');
      } else if (eventData.result) {
        console.log('Search results:', eventData.result);
      }
    }
  }
}
```
