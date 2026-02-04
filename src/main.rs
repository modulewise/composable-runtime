use anyhow::Result;
use clap::{Args, Parser, Subcommand};
use composable_runtime::{ComponentGraph, Runtime};
use rustyline::Editor;
use rustyline::error::ReadlineError;
use rustyline::history::DefaultHistory;
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "composable-runtime")]
#[command(about = "A runtime for Wasm Components")]
struct Cli {
    #[command(flatten)]
    mode: ModeArgs,

    /// Component definition files (.toml) and standalone .wasm files
    #[arg(required = true)]
    definitions: Vec<PathBuf>,
}

#[derive(Args)]
#[group(required = true, multiple = false)]
struct ModeArgs {
    /// Perform a dry run, printing the dependency graph without building the registry
    #[arg(long, short)]
    dry_run: bool,

    /// Export component graph to DOT file (graph.dot)
    #[arg(long, short)]
    export: bool,

    /// Start interactive session for invoking component functions
    #[arg(long, short)]
    interactive: bool,
}

#[derive(Subcommand)]
enum Commands {
    /// List available component functions
    List,
    /// Show details for a specific function
    Describe {
        /// The target function, e.g., component.function
        target: String,
    },
    /// Call a function with arguments
    Invoke {
        /// The target function, e.g., component.function
        target: String,
        /// The arguments to pass to the function
        #[arg()]
        args: Vec<String>,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    println!("Loading definitions from: {:?}...", cli.definitions);
    let mut builder = ComponentGraph::builder();
    for path in &cli.definitions {
        builder = builder.load_file(path);
    }
    let graph = builder.build()?;

    if cli.mode.dry_run {
        println!("--- Component Dependency Graph (Dry Run) ---");
        println!("{graph:#?}");
        println!("--------------------------------------------");
    } else if cli.mode.export {
        let filename = "graph.dot";
        graph.write_dot_file(filename)?;
        println!("Graph exported to {filename}");
    } else if cli.mode.interactive {
        run_interactive_session(&graph).await?;
    }

    Ok(())
}

async fn run_interactive_session(graph: &ComponentGraph) -> Result<()> {
    println!("Building runtime...");
    let runtime = Runtime::builder(graph).build().await?;
    let components = runtime.list_components();
    println!(
        "Successfully built runtime with {} exposed components.",
        components.len()
    );

    println!("Starting interactive session. Type 'help' for commands.");
    let mut rl = Editor::<(), DefaultHistory>::new()?;
    loop {
        let readline = rl.readline("> ");
        match readline {
            Ok(line) => {
                let _ = rl.add_history_entry(line.as_str());
                if handle_command(line, &runtime).await.is_err() {
                    break;
                }
            }
            Err(ReadlineError::Interrupted) => {
                println!("CTRL-C");
                break;
            }
            Err(ReadlineError::Eof) => {
                println!("CTRL-D");
                break;
            }
            Err(err) => {
                eprintln!("Error: {err:?}");
                break;
            }
        }
    }

    Ok(())
}

async fn handle_command(line: String, runtime: &Runtime) -> Result<(), ()> {
    let parts = parse_quoted_args(&line);

    if let Some(command_str) = parts.first() {
        let command = match command_str.as_str() {
            "list" => Some(Commands::List),
            "describe" => parts.get(1).map_or_else(
                || {
                    eprintln!("Usage: describe <target>");
                    None
                },
                |target| {
                    Some(Commands::Describe {
                        target: target.to_string(),
                    })
                },
            ),
            "invoke" => parts.get(1).map_or_else(
                || {
                    eprintln!("Usage: invoke <target> [args...]");
                    None
                },
                |target| {
                    Some(Commands::Invoke {
                        target: target.to_string(),
                        args: parts
                            .get(2..)
                            .unwrap_or(&[])
                            .iter()
                            .map(|s| s.to_string())
                            .collect(),
                    })
                },
            ),
            "help" => {
                println!("Available commands:");
                println!("  list                            - List available component functions");
                println!(
                    "  describe <target>               - Show details for a specific function"
                );
                println!("  invoke <target> [args...]       - Call a function with arguments");
                println!("  help                            - Show this help message");
                println!("  exit, quit                      - Exit the interactive session");
                None
            }
            "exit" | "quit" => return Err(()),
            _ => {
                eprintln!("Unknown command. Type 'help' for a list of commands.");
                None
            }
        };

        if let Some(command) = command {
            match command {
                Commands::List => {
                    let mut targets = Vec::new();
                    for component in runtime.list_components() {
                        for func_name in component.functions.keys() {
                            targets.push(format!("{}.{}", component.name, func_name));
                        }
                    }
                    targets.sort();
                    for target in targets {
                        println!("- {target}");
                    }
                }
                Commands::Describe { target } => {
                    if let Some((component_name, func_name)) = target.split_once('.') {
                        if let Some(component) = runtime.get_component(component_name) {
                            if let Some(function) = component.functions.get(func_name) {
                                println!("Target: {target}");
                                if !function.docs().is_empty() {
                                    println!("Docs: {}", function.docs());
                                }
                                println!("Params:");
                                if function.params().is_empty() {
                                    println!("  (none)");
                                } else {
                                    for param in function.params() {
                                        println!(
                                            "- {}: {} (optional: {})",
                                            param.name, param.json_schema, param.is_optional
                                        );
                                    }
                                }
                                println!(
                                    "Result: {}",
                                    function
                                        .result()
                                        .map(|s| s.to_string())
                                        .unwrap_or_else(|| "null".to_string())
                                );
                            } else {
                                eprintln!(
                                    "Error: Function '{func_name}' not found in component '{component_name}'."
                                );
                            }
                        } else {
                            eprintln!("Error: Component '{component_name}' not found.");
                        }
                    } else {
                        eprintln!("Error: Invalid target format. Use 'component.function'.");
                    }
                }
                Commands::Invoke { target, args } => {
                    if let Some((component_name, func_name)) = target.split_once('.') {
                        if let Some(component) = runtime.get_component(component_name) {
                            if let Some(function) = component.functions.get(func_name) {
                                let params = function.params();
                                let mut final_args: Vec<serde_json::Value> = Vec::new();

                                if args.len() > params.len() {
                                    eprintln!(
                                        "Error: Too many arguments. Expected at most {}, got {}",
                                        params.len(),
                                        args.len()
                                    );
                                    return Ok(());
                                }

                                for (i, arg_str) in args.iter().enumerate() {
                                    let trimmed = arg_str.trim();

                                    // First, parse as any valid JSON value, falling back to a string.
                                    let mut json_val = serde_json::from_str(trimmed)
                                        .unwrap_or_else(|_| {
                                            serde_json::Value::String(trimmed.to_string())
                                        });

                                    // Convert numbers/objects/arrays to strings if the parameter's schema expects a string.
                                    if let Some(param) = params.get(i)
                                        && let Some("string") =
                                            param.json_schema.get("type").and_then(|v| v.as_str())
                                    {
                                        match &json_val {
                                            serde_json::Value::Number(n) => {
                                                json_val = serde_json::Value::String(n.to_string());
                                            }
                                            serde_json::Value::Object(_)
                                            | serde_json::Value::Array(_) => {
                                                json_val = serde_json::Value::String(
                                                    serde_json::to_string(&json_val)
                                                        .unwrap_or_else(|_| json_val.to_string()),
                                                );
                                            }
                                            _ => {
                                                // Already a string or other type, keep as is
                                            }
                                        }
                                    }
                                    final_args.push(json_val);
                                }

                                // Handle missing parameters: pad with nulls for optional, error for required
                                for i in args.len()..params.len() {
                                    if let Some(param) = params.get(i) {
                                        if param.is_optional {
                                            final_args.push(serde_json::Value::Null);
                                        } else {
                                            eprintln!(
                                                "Error: Missing required parameter: {}",
                                                param.name
                                            );
                                            return Ok(());
                                        }
                                    }
                                }

                                println!("Invoking {target}...");
                                match runtime.invoke(component_name, func_name, final_args).await {
                                    Ok(result) => {
                                        println!(
                                            "{}",
                                            serde_json::to_string_pretty(&result).unwrap()
                                        );
                                    }
                                    Err(e) => eprintln!("Error: {e}"),
                                }
                            } else {
                                eprintln!(
                                    "Error: Function '{func_name}' not found in component '{component_name}'."
                                );
                            }
                        } else {
                            eprintln!("Error: Component '{component_name}' not found.");
                        }
                    } else {
                        eprintln!("Error: Invalid target format. Use 'component.function'.");
                    }
                }
            }
        }
    }
    Ok(())
}

fn parse_quoted_args(line: &str) -> Vec<String> {
    let mut parts = Vec::new();
    let mut current = String::new();
    let mut quote_char: Option<char> = None;

    for ch in line.trim().chars() {
        match (ch, quote_char) {
            ('"', None) | ('\'', None) => {
                // Starting a quoted string
                quote_char = Some(ch);
            }
            (ch, Some(open_char)) if ch == open_char => {
                // Closing a quoted string
                quote_char = None;
            }
            (' ', None) => {
                if !current.is_empty() {
                    parts.push(current);
                    current = String::new();
                }
            }
            (ch, _) => {
                current.push(ch);
            }
        }
    }
    if !current.is_empty() {
        parts.push(current);
    }
    parts
}
