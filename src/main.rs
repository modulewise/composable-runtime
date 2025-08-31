use anyhow::Result;
use clap::{Parser, Subcommand};
use composable_runtime::{
    ComponentSpec, Function, Invoker, RuntimeFeatureRegistry, build_registries, load_definitions,
};
use rustyline::Editor;
use rustyline::error::ReadlineError;
use rustyline::history::DefaultHistory;
use std::collections::HashMap;
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "composable-runtime")]
#[command(about = "A runtime for Wasm Components")]
struct Cli {
    /// Perform a dry run, printing the dependency graph without building the registry
    #[arg(long)]
    dry_run: bool,

    /// Component definition files (.toml) and standalone .wasm files
    #[arg(required = true)]
    definitions: Vec<PathBuf>,
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
    let graph = load_definitions(&cli.definitions)?;

    if cli.dry_run {
        println!("--- Component Dependency Graph (Dry Run) ---");
        println!("{:#?}", graph);
        println!("--------------------------------------------");
    } else {
        println!("Building registries...");
        let (runtime_feature_registry, component_registry) = build_registries(&graph).await?;
        println!(
            "Successfully built registry with {} exposed components.",
            component_registry.get_components().count()
        );

        let invoker = Invoker::new()?;
        let mut exposed_functions: HashMap<String, (&Function, &ComponentSpec)> = HashMap::new();
        for spec in component_registry.get_components() {
            if let Some(functions) = &spec.functions {
                for function in functions.values() {
                    let target = format!("{}.{}", spec.name, function.function_name());
                    exposed_functions.insert(target, (function, spec));
                }
            }
        }

        println!("Starting interactive session. Type 'help' for commands.");
        let mut rl = Editor::<(), DefaultHistory>::new()?;
        loop {
            let readline = rl.readline("> ");
            match readline {
                Ok(line) => {
                    let _ = rl.add_history_entry(line.as_str());
                    if handle_command(
                        line,
                        &exposed_functions,
                        &invoker,
                        &runtime_feature_registry,
                    )
                    .await
                    .is_err()
                    {
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
                    println!("Error: {:?}", err);
                    break;
                }
            }
        }
    }

    Ok(())
}

async fn handle_command(
    line: String,
    exposed_functions: &HashMap<String, (&Function, &ComponentSpec)>,
    invoker: &Invoker,
    runtime_feature_registry: &RuntimeFeatureRegistry,
) -> Result<(), ()> {
    let parts: Vec<&str> = line.trim().split_whitespace().collect();

    if let Some(command_str) = parts.first() {
        let command = match *command_str {
            "list" => Some(Commands::List),
            "describe" => parts.get(1).map_or_else(
                || {
                    println!("Usage: describe <target>");
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
                    println!("Usage: invoke <target> [args...]");
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
                println!("Unknown command. Type 'help' for a list of commands.");
                None
            }
        };

        if let Some(command) = command {
            match command {
                Commands::List => {
                    let mut targets: Vec<_> = exposed_functions.keys().collect();
                    targets.sort();
                    for target in targets {
                        println!("- {}", target);
                    }
                }
                Commands::Describe { target } => {
                    if let Some((function, _spec)) = exposed_functions.get(&target) {
                        println!("Target: {}", target);
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
                        println!("Error: Target '{}' not found.", target);
                    }
                }
                Commands::Invoke { target, args } => {
                    if let Some((function, spec)) = exposed_functions.get(&target) {
                        let params = function.params();
                        let mut final_args: Vec<serde_json::Value> = Vec::new();

                        for (i, arg_str) in args.iter().enumerate() {
                            // First, parse as any valid JSON value, falling back to a string.
                            let mut json_val = serde_json::from_str(arg_str)
                                .unwrap_or_else(|_| serde_json::Value::String(arg_str.clone()));

                            // Proactively convert numbers to strings if the parameter's schema expects a string.
                            if let Some(param) = params.get(i) {
                                if let Some("string") =
                                    param.json_schema.get("type").and_then(|v| v.as_str())
                                {
                                    if let serde_json::Value::Number(n) = &json_val {
                                        json_val = serde_json::Value::String(n.to_string());
                                    }
                                }
                            }
                            final_args.push(json_val);
                        }

                        println!("Invoking {}...", target);
                        match invoker
                            .invoke(
                                &spec.bytes,
                                &spec.runtime_features,
                                runtime_feature_registry,
                                (*function).clone(),
                                final_args,
                            )
                            .await
                        {
                            Ok(result) => {
                                println!("{}", serde_json::to_string_pretty(&result).unwrap());
                            }
                            Err(e) => println!("Error: {}", e),
                        }
                    } else {
                        println!("Error: Target '{}' not found.", target);
                    }
                }
            }
        }
    }
    Ok(())
}
