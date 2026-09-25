//! Function Adapter CLI
//!
//! Example:
//!   wasm-tools component wit calc.wasm > calc.wit
//!   function-adapter calc.wit -o calc-function.wasm --function calc.add --description "Adds two ints"

use anyhow::{Context, Result, bail};
use clap::Parser;
use std::path::PathBuf;

/// Build a function component that adapts a function of a target component.
#[derive(Parser)]
#[command(name = "function-adapter")]
struct Cli {
    /// The target's WIT
    wit: PathBuf,

    /// The world in the WIT whose exported functions may be adapted
    #[arg(long, default_value = "root")]
    world: String,

    /// The target function to expose, if it exports more than one
    #[arg(long)]
    function: Option<String>,

    /// A description, returned in the function's metadata
    #[arg(long)]
    description: Option<String>,

    /// Where to write the function component
    #[arg(short, long)]
    output: PathBuf,
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    let wit = std::fs::read_to_string(&cli.wit)
        .with_context(|| format!("reading {}", cli.wit.display()))?;

    match function_adapter::build(wit, Some(cli.world), cli.function, cli.description) {
        Ok(bytes) => {
            std::fs::write(&cli.output, &bytes)
                .with_context(|| format!("writing {}", cli.output.display()))?;
            eprintln!("wrote {} ({} bytes)", cli.output.display(), bytes.len());
            Ok(())
        }
        Err(e) => bail!("build failed: {e:#}"),
    }
}
