//! Desktop listener for the reusable MCP service adapter.

const DEFAULT_ADDR: &str = "127.0.0.1:8009";

pub async fn serve(application: devicehub_runtime::RuntimeClient<std::path::PathBuf>) {
    let address = std::env::var("DEVICEHUB_MCP_ADDR").unwrap_or_else(|_| DEFAULT_ADDR.into());
    if !address.starts_with("127.0.0.1:")
        && !address.starts_with("[::1]:")
        && !address.starts_with("localhost:")
    {
        tracing::warn!(
            address,
            "MCP has no authentication and is binding beyond loopback"
        );
    }
    let router = devicehub_server::mcp::router(application);
    match tokio::net::TcpListener::bind(&address).await {
        Ok(listener) => {
            tracing::info!(address = %address, "MCP server listening");
            if let Err(error) = axum::serve(listener, router).await {
                tracing::error!(error = %error, "MCP server stopped");
            }
        }
        Err(error) => {
            tracing::warn!(address = %address, error = %error, "MCP server failed to bind")
        }
    }
}
