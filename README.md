# MCP Proxy

An efficient, Rust-based MCP (Model-Context-Protocol) proxy that aggregates tools from multiple upstream servers and exposes them through a single `stdio` interface.

## Features

- **Tool Aggregation**: Connects to multiple upstream MCP servers (via `stdio` or `http`) and exposes an aggregated list of all their tools.
- **Dynamic Tool Updates**: Listens for `toolListChanged` notifications from upstream servers and dynamically updates its aggregated tool list.
- **Structured Logging**: Uses `tracing` to emit structured JSON logs for observability.
- **Concurrent Connections**: Connects to all configured upstream servers concurrently for fast startup.

## Configuration

The proxy is configured using a JSON file (e.g., `mcp_servers.json`) passed as a command-line argument. The file should contain a single object with an `mcpServers` key.

### Example `mcp_servers.json`:

```json
{
  "mcpServers": {
    "git-tools": {
      "command": "uvx",
      "args": ["mcp-git-ingest"]
    },
    "github": {
      "url": "https://api.githubcopilot.com/mcp/",
      "authorizationToken": "Bearer ghp_1234567890"
    }
  }
}
```

### Server Configuration Types:

1.  **`stdio` servers**: For servers that run as a local child process.
    -   `command`: The executable to run.
    -   `args`: (Optional) A list of command-line arguments.
    -   `env`: (Optional) A map of environment variables to set for the process.

2.  **`http` servers**: For servers that are accessible via HTTP.
    -   `url`: The base URL of the MCP server.
    -   `authorizationToken`: (Optional) A string to be used as a `Bearer` token in the `Authorization` header.

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

## Development

-   **Logging**: The application uses the `RUST_LOG` environment variable to control log levels (e.g., `RUST_LOG=info,mcproxy=debug`).
-   **Testing**: Run tests with `cargo test`. 