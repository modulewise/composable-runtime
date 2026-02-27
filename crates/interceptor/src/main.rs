use anyhow::Result;
use clap::Parser;
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "waspect")]
#[command(about = "Generate interceptor components for any target WIT interface")]
struct Cli {
    /// World whose exports define the interceptor contract
    #[arg(long)]
    world: String,

    /// Path to WIT file or directory
    #[arg(long, default_value = "wit/")]
    wit: PathBuf,

    /// Output path for the generated interceptor component
    #[arg(long, short)]
    output: PathBuf,

    /// Match pattern for selective interception (repeatable, omit to intercept all)
    #[arg(long, value_name = "PATTERN")]
    r#match: Vec<String>,
}

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .with_writer(std::io::stderr)
        .with_target(false)
        .without_time()
        .init();

    let cli = Cli::parse();
    let patterns: Vec<&str> = cli.r#match.iter().map(|s| s.as_str()).collect();

    let component_bytes =
        composable_runtime_interceptor::create_from_wit(&cli.wit, &cli.world, &patterns)?;

    std::fs::write(&cli.output, &component_bytes)?;
    tracing::info!("Wrote interceptor to {}", cli.output.display());

    Ok(())
}
