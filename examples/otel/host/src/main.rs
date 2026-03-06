use anyhow::Result;
use composable_runtime::{ComponentGraph, Runtime};
use grpc_capability::GrpcCapability;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    let config_file = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "config-with-host-capability.toml".to_string());

    let graph = ComponentGraph::builder()
        .load_file(std::path::PathBuf::from(&config_file))
        .build()?;

    let runtime = Runtime::builder(&graph)
        .with_host_extension::<GrpcCapability>("grpc")
        .build()
        .await?;

    let message = format!("testing {}", config_file);

    let result = runtime
        .invoke("guest", "test.log", vec![serde_json::json!(message)])
        .await?;
    println!("Result: {}", result);

    Ok(())
}
