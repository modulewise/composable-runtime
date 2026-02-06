use anyhow::Result;
use composable_runtime::{ComponentGraph, Runtime};

#[tokio::main]
async fn main() -> Result<()> {
    let graph = ComponentGraph::builder()
        .load_file(std::path::PathBuf::from("config.toml"))
        .build()?;

    let runtime = Runtime::builder(&graph).build().await?;

    let result = runtime
        .invoke("greeter", "greet", vec![serde_json::json!("World")])
        .await?;

    println!("Result: {}", result);

    Ok(())
}
