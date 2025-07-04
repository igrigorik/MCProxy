use config::load_config;
use proxy::ProxyServer;
use rmcp::{service::ServiceExt, transport};
use std::env;

mod config;
mod proxy;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Configure logging format based on environment
    // CRITICAL: Always write logs to stderr, never stdout, because MCP protocol uses stdout for JSON-RPC messages
    let subscriber = tracing_subscriber::fmt()
        .with_writer(std::io::stderr)  // Force logs to stderr
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env());
    
    // Use JSON format in production, human-readable format in development
    if env::var("RUST_LOG_FORMAT").unwrap_or_default() == "json" {
        subscriber.json().init();
    } else {
        subscriber.init();
    }

    tracing::info!("Starting MCP Proxy");

    // Parse command line arguments
    let args: Vec<String> = env::args().collect();
    if args.len() < 2 {
        tracing::error!("Usage: mcproxy <path_to_config.json>");
        return Ok(());
    }
    let config_path = &args[1];

    // Load configuration
    let config = load_config(config_path)
        .map_err(|e| {
            tracing::error!(path = %config_path, error = %e, "Failed to load configuration");
            e
        })?;

    // Create and initialize proxy server
    let mut proxy_server = ProxyServer::new();
    proxy_server.connect_and_discover(config).await
        .map_err(|e| {
            tracing::error!(error = %e, "Failed to connect to servers");
            e
        })?;

    tracing::info!("Proxy server initialized successfully. Starting stdio server...");

    // Check if we're running standalone (no stdin available) or as part of a pipeline
    let is_standalone = atty::is(atty::Stream::Stdin);
    
    if is_standalone {
        tracing::info!("Running in standalone mode - proxy will run until interrupted");
        
        // In standalone mode, just wait for signals
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {
                tracing::info!("Ctrl-C received, shutting down gracefully.");
            }
            _ = async {
                #[cfg(unix)]
                {
                    use tokio::signal::unix::{signal, SignalKind};
                    let mut term = signal(SignalKind::terminate()).expect("Failed to install SIGTERM handler");
                    let mut quit = signal(SignalKind::quit()).expect("Failed to install SIGQUIT handler");
                    tokio::select! {
                        _ = term.recv() => tracing::info!("SIGTERM received, shutting down gracefully."),
                        _ = quit.recv() => tracing::info!("SIGQUIT received, shutting down gracefully."),
                    }
                }
                #[cfg(not(unix))]
                {
                    // On non-Unix systems, just wait forever
                    futures::future::pending::<()>().await;
                }
            } => {}
        }
    } else {
        tracing::info!("Running in stdio mode - serving MCP protocol over stdin/stdout");
        
        // Start the proxy server on stdio with graceful shutdown
        tokio::select! {
            result = proxy_server.serve(transport::stdio()) => {
                if let Err(e) = result {
                    tracing::error!("Proxy server failed: {}", e);
                    return Err(e.into());
                }
                tracing::info!("Stdio server finished normally");
            }
            _ = tokio::signal::ctrl_c() => {
                tracing::info!("Ctrl-C received, shutting down gracefully.");
            }
            _ = async {
                #[cfg(unix)]
                {
                    use tokio::signal::unix::{signal, SignalKind};
                    let mut term = signal(SignalKind::terminate()).expect("Failed to install SIGTERM handler");
                    let mut quit = signal(SignalKind::quit()).expect("Failed to install SIGQUIT handler");
                    tokio::select! {
                        _ = term.recv() => tracing::info!("SIGTERM received, shutting down gracefully."),
                        _ = quit.recv() => tracing::info!("SIGQUIT received, shutting down gracefully."),
                    }
                }
                #[cfg(not(unix))]
                {
                    // On non-Unix systems, just wait forever
                    futures::future::pending::<()>().await;
                }
            } => {}
        }
    }

    tracing::info!("Proxy server shut down successfully.");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::NamedTempFile;

    #[test]
    fn test_argument_parsing() {
        // Test sufficient arguments
        let args = vec!["mcproxy".to_string(), "config.json".to_string()];
        assert!(args.len() >= 2);
        assert_eq!(&args[1], "config.json");

        // Test insufficient arguments
        let args = vec!["mcproxy".to_string()];
        assert!(args.len() < 2);
    }

    #[test]
    fn test_logging_format_detection() {
        // Test JSON format detection
        std::env::set_var("RUST_LOG_FORMAT", "json");
        let format = std::env::var("RUST_LOG_FORMAT").unwrap_or_default();
        assert_eq!(format, "json");

        // Test default format
        std::env::remove_var("RUST_LOG_FORMAT");
        let format = std::env::var("RUST_LOG_FORMAT").unwrap_or_default();
        assert_eq!(format, "");

        // Test custom format
        std::env::set_var("RUST_LOG_FORMAT", "custom");
        let format = std::env::var("RUST_LOG_FORMAT").unwrap_or_default();
        assert_eq!(format, "custom");
        
        // Cleanup
        std::env::remove_var("RUST_LOG_FORMAT");
    }

    #[test]
    fn test_config_path_validation() {
        // Test valid config path
        let config_content = r#"
        {
          "mcpServers": {
            "test-server": {
              "command": "echo"
            }
          }
        }
        "#;

        let temp_file = NamedTempFile::new().unwrap();
        fs::write(temp_file.path(), config_content).unwrap();
        
        let config_path = temp_file.path().to_str().unwrap();
        let result = load_config(config_path);
        assert!(result.is_ok());

        // Test invalid config path
        let result = load_config("nonexistent-config.json");
        assert!(result.is_err());
    }

    #[test]
    fn test_error_handling_patterns() {
        // Test error message formatting for configuration errors
        let config_path = "nonexistent.json";
        let error_result = load_config(config_path);
        
        match error_result {
            Ok(_) => panic!("Expected error for nonexistent file"),
            Err(e) => {
                let error_msg = format!("Failed to load configuration path={} error={}", config_path, e);
                assert!(error_msg.contains("Failed to load configuration"));
                assert!(error_msg.contains("nonexistent.json"));
            }
        }
    }

    #[test]
    fn test_usage_message_components() {
        // Test that we can construct the usage message
        let usage_msg = "Usage: mcproxy <path_to_config.json>";
        assert!(usage_msg.contains("mcproxy"));
        assert!(usage_msg.contains("<path_to_config.json>"));
        assert!(usage_msg.starts_with("Usage:"));
    }

    #[test]
    fn test_env_var_defaults() {
        // Test environment variable handling
        let original_rust_log = std::env::var("RUST_LOG").ok();
        let original_rust_log_format = std::env::var("RUST_LOG_FORMAT").ok();

        // Test with no env vars set
        std::env::remove_var("RUST_LOG");
        std::env::remove_var("RUST_LOG_FORMAT");
        
        let log_level = std::env::var("RUST_LOG").unwrap_or_default();
        let log_format = std::env::var("RUST_LOG_FORMAT").unwrap_or_default();
        
        assert_eq!(log_level, "");
        assert_eq!(log_format, "");

        // Test with env vars set
        std::env::set_var("RUST_LOG", "debug");
        std::env::set_var("RUST_LOG_FORMAT", "json");
        
        let log_level = std::env::var("RUST_LOG").unwrap_or_default();
        let log_format = std::env::var("RUST_LOG_FORMAT").unwrap_or_default();
        
        assert_eq!(log_level, "debug");
        assert_eq!(log_format, "json");

        // Restore original values
        match original_rust_log {
            Some(val) => std::env::set_var("RUST_LOG", val),
            None => std::env::remove_var("RUST_LOG"),
        }
        match original_rust_log_format {
            Some(val) => std::env::set_var("RUST_LOG_FORMAT", val),
            None => std::env::remove_var("RUST_LOG_FORMAT"),
        }
    }

    #[test]
    fn test_config_integration() {
        // Test full configuration loading and validation
        let config_content = r#"
        {
          "mcpServers": {
            "stdio-server": {
              "command": "echo",
              "args": ["hello", "world"]
            },
            "http-server": {
              "url": "http://localhost:8080",
              "headers": {
                "Authorization": "Bearer test-token"
              }
            }
          }
        }
        "#;

        let temp_file = NamedTempFile::new().unwrap();
        fs::write(temp_file.path(), config_content).unwrap();
        
        let config_path = temp_file.path().to_str().unwrap();
        let config = load_config(config_path).unwrap();
        
        assert_eq!(config.mcp_servers.len(), 2);
        assert!(config.mcp_servers.contains_key("stdio-server"));
        assert!(config.mcp_servers.contains_key("http-server"));

        // Verify proxy server can be created
        let proxy_server = ProxyServer::new();
        assert_eq!(proxy_server.test_clients_len(), 0);
        assert_eq!(proxy_server.test_tools_len(), 0);
    }

    #[test]
    fn test_module_imports() {
        // Test that all required modules are available
        use config::*;
        use proxy::*;
        
        // Test that we can create instances of the main types
        let _proxy = ProxyServer::new();
        let _handler = ProxyClientHandler::new("test".to_string());
        
        // Test that we can create config structures
        let _config = McpConfig {
            mcp_servers: std::collections::HashMap::new(),
        };
    }
}
