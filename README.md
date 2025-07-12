# MCP Proxy

Rust-based MCP (Model-Context-Protocol) proxy that aggregates tools from multiple upstream servers and exposes them through a single streamable HTTP interface.

- **Tool aggregation**: Connects to multiple upstream MCP servers (via `stdio` or `http`) and exposes an aggregated list of all their tools.
- **Dynamic tool updates**: Listens for `toolListChanged` notifications from upstream servers and dynamically updates its aggregated tool list.
- **Tool search**: When a large number of tools are available, proxy can automatically limit initial list and provide search functionality to search/update exposed tools on demand.
- **Middleware system**: Flexible middleware system for intercepting and modifying requests/responses:
  - **ClientMiddleware**: Operates on individual server calls (logging, tool filtering, payload inspection for security, etc)
  - **ProxyMiddleware**: Operates on aggregated tool/prompt/resources from servers (description enrichment, tool search, etc)

**Built-in middleware** (see [docs/MIDDLEWARE.md](docs/MIDDLEWARE.md)): 
  - **Logging**: Logs all operations with timing information per server
  - **Tool filtering**: Filters tools based on configurable regex patterns (allow/disallow)
  - **Tool security**: Inspects tool call inputs and blocks calls that match security rules
  - **Tool enrichment**: Augments tool/prompt/resource descriptions (demo)
  - **Tool search**: Provides intelligent tool management with selective exposure and search functionality (see [docs/SEARCH.md](docs/SEARCH.md))


## Usage

1.  **Build the project**:
    ```sh
    cargo build --release
    ```

2.  **Create a configuration file** (e.g., `mcp_servers.json`) as described above.

3.  **Run the proxy**:
    ```sh
    ./target/release/mcproxy mcp_servers.json
    ```
    The proxy will start, connect to the configured servers, and listen for MCP messages on its `stdio`.


## Configuration

The proxy is configured using a JSON file passed as a command-line argument. The file should contain the `mcpServers` key (MCP configuration you use for VSCode/Cursor/Claude/etc), plus `httpServer` key to configure the MCP proxy: 

```json
{
  "mcpServers": {
    "stdio-mcp-service": {
      "type": "stdio",
      "command": "node",
      "args": ["script.js"],
      "env": {
        "ENV_FOO_VAR": "BAR"
      }
    },
    "http-mcp-github": {
      "url": "https://api.githubcopilot.com/mcp/",
      "authorizationToken": "Bearer GitHUB_PAT_TOKEN"
    }
  },
  "httpServer": {
    "host": "127.0.0.1",
    "port": 8081,
    "corsEnabled": true,
    "corsOrigins": [
      "*"
    ]
  }
}
```

In example above, we are aggregating tools from a local (stdio) MCP server and remote (HTTP) MCP server and exposing it on `http://127.0.0.1:8081/mcp` endpoint. To access these tools, we can configure the MCP client with:

```json
{
  "mcpServers": {
    "tool-proxy": {
      "type": "http",
      "url": "http://127.0.0.1:8081/mcp"
    }
  }
}
```

See [example-config.json](example-config.json) for a complete example with custom middleware and search configuration.

## License
This project is licensed under the MIT License - see [LICENSE.md](LICENSE.md).
