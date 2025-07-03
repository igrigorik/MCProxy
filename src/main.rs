use config::load_config;
use proxy::ProxyServer;
use rmcp::{service::ServiceExt, transport};
use std::env;

mod config;
mod proxy;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Configure logging format based on environment
    let subscriber = tracing_subscriber::fmt()
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
