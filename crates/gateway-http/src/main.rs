use std::path::PathBuf;

use anyhow::Result;
use clap::Parser;
use composable_runtime::Runtime;

use composable_gateway_http::HttpGatewayService;

#[derive(Parser)]
#[command(
    name = "composable-http-gateway",
    about = "HTTP Gateway for Composable Runtime"
)]
struct Cli {
    /// Definition files (TOML, .wasm, etc.)
    #[arg(required = true)]
    definitions: Vec<PathBuf>,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let cli = Cli::parse();

    let runtime = Runtime::builder()
        .from_paths(&cli.definitions)
        .with_service::<HttpGatewayService>()
        .build()
        .await?;

    runtime.run().await
}
