use anyhow::Result;
use composable_http_server::HttpService;
use composable_otel::OtelService;
use composable_runtime::Runtime;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .try_init()
        .ok();

    let runtime = Runtime::builder()
        .from_path(std::path::PathBuf::from("config.toml"))
        .with_service::<OtelService>()
        .with_service::<HttpService>()
        .build()
        .await?;

    runtime.run().await
}
