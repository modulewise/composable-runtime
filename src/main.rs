use anyhow::Result;
use clap::{Parser, Subcommand};
use composable_runtime::{Component, ComponentGraph, FunctionParam, Runtime, Selector};
use rustyline::Editor;
use rustyline::error::ReadlineError;
use rustyline::history::DefaultHistory;
use std::collections::HashMap;
use std::path::PathBuf;
use tracing_subscriber::EnvFilter;

#[derive(Parser)]
#[command(name = "composable")]
#[command(about = "An inversion of control runtime for wasm components")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Call a component function
    Invoke {
        /// Component definition files (.toml) and standalone .wasm files
        #[arg(required = true)]
        definitions: Vec<PathBuf>,

        /// Propagation context entries (KEY=VALUE)
        #[arg(long = "ctx", value_parser = parse_key_value)]
        ctx: Vec<(String, String)>,

        /// Environment variables (KEY=VALUE)
        #[arg(long = "env", value_parser = parse_key_value)]
        env: Vec<(String, String)>,

        /// Target and arguments, passed after --
        #[arg(last = true)]
        target_args: Vec<String>,
    },
    /// Run as a long-lived process with gateway(s) and/or messaging
    Run {
        /// Component definition files (.toml) and standalone .wasm files
        #[arg(required = true)]
        definitions: Vec<PathBuf>,
    },
    /// Interactive shell for dev and debugging
    Shell {
        /// Component definition files (.toml) and standalone .wasm files
        #[arg(required = true)]
        definitions: Vec<PathBuf>,

        /// Filter components by selector (e.g. labels.domain=payments, !dependents, name in (foo, bar))
        #[arg(long)]
        selector: Option<String>,

        /// Environment variables (KEY=VALUE)
        #[arg(long = "env", value_parser = parse_key_value)]
        env: Vec<(String, String)>,
    },
    /// Publish a message to a channel
    Publish {
        /// Component definition files (.toml) and standalone .wasm files
        #[arg(required = true)]
        definitions: Vec<PathBuf>,

        /// Channel name to publish to
        #[arg(long)]
        channel: String,

        /// Message body
        #[arg(long)]
        body: String,

        /// Content type
        #[arg(long, default_value = "text/plain")]
        content_type: String,
    },
    /// Inspect the dependency graph
    Graph {
        /// Component definition files (.toml) and standalone .wasm files
        #[arg(required = true)]
        definitions: Vec<PathBuf>,

        /// Export to DOT format (graph.dot)
        #[arg(long)]
        dot: bool,
    },
}

enum ShellCommand {
    List,
    Describe { target: String },
    Invoke { target: String, args: Vec<String> },
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Command::Graph { definitions, dot } => {
            let graph = build_graph(&definitions)?;
            if dot {
                graph.write_dot_file("graph.dot")?;
                println!("Graph exported to graph.dot");
            } else {
                println!("{graph:#?}");
            }
        }
        Command::Shell {
            definitions,
            selector,
            env,
        } => {
            let selector = selector.map(|s| Selector::parse(&s)).transpose()?;
            let env = vec_to_option_map(env);
            let runtime = Runtime::builder().from_paths(&definitions).build().await?;
            runtime.start()?;
            run_shell(&runtime, selector.as_ref(), env.as_ref()).await?;
            runtime.shutdown().await;
        }
        Command::Invoke {
            definitions,
            ctx,
            env,
            target_args,
        } => {
            let context = vec_to_option_map(ctx);
            let env = vec_to_option_map(env);
            let runtime = Runtime::builder().from_paths(&definitions).build().await?;
            runtime.start()?;
            run_invoke(&runtime, target_args, context, env).await?;
            runtime.shutdown().await;
        }
        Command::Publish {
            definitions,
            channel,
            body,
            content_type,
        } => {
            tracing_subscriber::fmt()
                .with_env_filter(
                    EnvFilter::from_default_env()
                        .add_directive("composable_runtime::messaging=info".parse().unwrap()),
                )
                .init();

            let runtime = Runtime::builder().from_paths(&definitions).build().await?;
            runtime.start()?;

            let publisher = runtime.publisher();
            let headers =
                std::collections::HashMap::from([("content-type".to_string(), content_type)]);
            publisher
                .publish(&channel, body.into_bytes(), headers)
                .await?;

            runtime.shutdown().await;
        }
        Command::Run { definitions } => {
            tracing_subscriber::fmt()
                .with_env_filter(EnvFilter::from_default_env())
                .init();
            let runtime = Runtime::builder().from_paths(&definitions).build().await?;
            runtime.run().await?;
        }
    }

    Ok(())
}

fn build_graph(definitions: &[PathBuf]) -> Result<ComponentGraph> {
    tracing::info!("Loading definitions from: {definitions:?}");
    ComponentGraph::builder().from_paths(definitions).build()
}

async fn run_invoke(
    runtime: &Runtime,
    target_args: Vec<String>,
    context: Option<HashMap<String, String>>,
    env: Option<HashMap<String, String>>,
) -> Result<()> {
    let (target, args) = target_args
        .split_first()
        .ok_or_else(|| anyhow::anyhow!("missing target after --"))?;

    let (component_name, func_name) = target
        .split_once('.')
        .ok_or_else(|| anyhow::anyhow!(
            "invalid target '{target}'. Expected 'component.function' or 'component.interface.function'"
        ))?;

    let component = runtime
        .get_component(component_name)
        .ok_or_else(|| anyhow::anyhow!("component '{component_name}' not found"))?;
    let function = component.functions.get(func_name).ok_or_else(|| {
        anyhow::anyhow!("function '{func_name}' not found in component '{component_name}'")
    })?;

    let final_args =
        parse_invoke_args(args, function.params()).map_err(|e| anyhow::anyhow!("{e}"))?;

    let result = runtime
        .invoker()
        .invoke(component_name, func_name, final_args, context, env)
        .await?;
    println!("{}", serde_json::to_string_pretty(&result)?);
    Ok(())
}

async fn run_shell(
    runtime: &Runtime,
    selector: Option<&Selector>,
    env: Option<&HashMap<String, String>>,
) -> Result<()> {
    let components = runtime.list_components(selector);

    if selector.is_some() {
        println!(
            "Shell session with {} selected components.",
            components.len()
        );
    } else {
        println!(
            "Successfully built runtime with {} components.",
            components.len()
        );
    }
    println!("Starting interactive session. Type 'help' for commands.");
    let mut rl = Editor::<(), DefaultHistory>::new()?;
    loop {
        let readline = rl.readline("> ");
        match readline {
            Ok(line) => {
                let _ = rl.add_history_entry(line.as_str());
                if handle_command(line, runtime, &components, env)
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
                eprintln!("Error: {err:?}");
                break;
            }
        }
    }

    Ok(())
}

async fn handle_command(
    line: String,
    runtime: &Runtime,
    components: &[&Component],
    env: Option<&HashMap<String, String>>,
) -> Result<(), ()> {
    let parts = parse_quoted_args(&line);

    if let Some(command_str) = parts.first() {
        let command = match command_str.as_str() {
            "list" => Some(ShellCommand::List),
            "describe" => parts.get(1).map_or_else(
                || {
                    eprintln!("Usage: describe <target>");
                    None
                },
                |target| {
                    Some(ShellCommand::Describe {
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
                    Some(ShellCommand::Invoke {
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
                ShellCommand::List => {
                    let mut targets = Vec::new();
                    for component in components {
                        for func_name in component.functions.keys() {
                            targets.push(format!("{}.{}", component.metadata.name, func_name));
                        }
                    }
                    targets.sort();
                    for target in targets {
                        println!("- {target}");
                    }
                }
                ShellCommand::Describe { target } => {
                    if let Some((component_name, func_name)) = target.split_once('.') {
                        if let Some(component) = find_component(components, component_name) {
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
                ShellCommand::Invoke { target, args } => {
                    if let Some((component_name, func_name)) = target.split_once('.') {
                        if let Some(component) = find_component(components, component_name) {
                            if let Some(function) = component.functions.get(func_name) {
                                match parse_invoke_args(&args, function.params()) {
                                    Ok(final_args) => {
                                        println!("Invoking {target}...");
                                        match runtime
                                            .invoker()
                                            .invoke(
                                                component_name,
                                                func_name,
                                                final_args,
                                                None,
                                                env.cloned(),
                                            )
                                            .await
                                        {
                                            Ok(result) => {
                                                println!(
                                                    "{}",
                                                    serde_json::to_string_pretty(&result).unwrap()
                                                );
                                            }
                                            Err(e) => eprintln!("Error: {e}"),
                                        }
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

fn parse_invoke_args(
    args: &[String],
    params: &[FunctionParam],
) -> Result<Vec<serde_json::Value>, String> {
    if args.len() > params.len() {
        return Err(format!(
            "too many arguments. Expected at most {}, got {}",
            params.len(),
            args.len()
        ));
    }

    let mut final_args: Vec<serde_json::Value> = Vec::new();

    for (i, arg_str) in args.iter().enumerate() {
        let trimmed = arg_str.trim();

        // First, parse as any valid JSON value, falling back to a string.
        let mut json_val = serde_json::from_str(trimmed)
            .unwrap_or_else(|_| serde_json::Value::String(trimmed.to_string()));

        // Convert numbers/objects/arrays to strings if the parameter's schema expects a string.
        if let Some(param) = params.get(i)
            && let Some("string") = param.json_schema.get("type").and_then(|v| v.as_str())
        {
            match &json_val {
                serde_json::Value::Number(n) => {
                    json_val = serde_json::Value::String(n.to_string());
                }
                serde_json::Value::Object(_) | serde_json::Value::Array(_) => {
                    json_val = serde_json::Value::String(
                        serde_json::to_string(&json_val).unwrap_or_else(|_| json_val.to_string()),
                    );
                }
                _ => {}
            }
        }
        final_args.push(json_val);
    }

    // Handle missing parameters: pad with nulls for optional, error for required
    for param in params.iter().skip(args.len()) {
        if param.is_optional {
            final_args.push(serde_json::Value::Null);
        } else {
            return Err(format!("missing required parameter: {}", param.name));
        }
    }

    Ok(final_args)
}

fn find_component<'a>(components: &[&'a Component], name: &str) -> Option<&'a Component> {
    components.iter().find(|c| c.metadata.name == name).copied()
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

fn parse_key_value(s: &str) -> Result<(String, String), String> {
    let (key, value) = s
        .split_once('=')
        .ok_or_else(|| format!("expected KEY=VALUE, got '{s}'"))?;
    Ok((key.to_string(), value.to_string()))
}

fn vec_to_option_map(pairs: Vec<(String, String)>) -> Option<HashMap<String, String>> {
    (!pairs.is_empty()).then(|| pairs.into_iter().collect())
}
