use std::future::Future;

use crate::runtime::{Component, Runtime};

use super::message::Message;

/// Handler for messages, used by dispatcher.
///
/// This is not user-facing. End users implement the WIT `handler` interface in
/// their Wasm components or plain components whose invocations will be mapped
/// from messages by the activator. The dispatcher delivers messages through
/// this trait without knowing which kind of handler it is.
pub trait Handler: Send + Sync {
    fn handle(&self, msg: Message) -> impl Future<Output = Result<(), String>> + Send;
}

/// Maps a message to a component function invocation.
pub trait Mapper: Send + Sync {
    fn map(&self, msg: &Message) -> Result<Invocation, String>;
}

/// A resolved function call: function key and JSON arguments.
pub struct Invocation {
    pub function_key: String,
    pub args: Vec<serde_json::Value>,
}

/// Config-driven mapper. Resolves the target function and translates the
/// message body into arguments. The target function must have 0 or 1
/// parameters. If no function key is provided, the component must export
/// exactly one function.
pub struct DefaultMapper {
    function_key: String,
    param_count: usize,
}

impl DefaultMapper {
    // Create from a component, optionally specifying the target function.
    // If `function_key` is `None`, the component must export exactly one
    // function. The target function must have 0 or 1 parameters.
    fn from_component(component: &Component, function_key: Option<String>) -> Result<Self, String> {
        let function_key = match function_key {
            Some(key) => key,
            None => {
                let functions = &component.functions;
                if functions.len() != 1 {
                    return Err(format!(
                        "default mapping currently requires exactly 1 exported function, \
                         '{}' has {}",
                        component.name,
                        functions.len()
                    ));
                }
                functions.keys().next().unwrap().clone()
            }
        };

        let function = component.functions.get(&function_key).ok_or_else(|| {
            format!(
                "function '{}' not found in '{}'",
                function_key, component.name
            )
        })?;
        let param_count = function.params().len();
        if param_count > 1 {
            return Err(format!(
                "default mapping currently requires 0 or 1 parameters, \
                 '{}' has {}",
                function_key, param_count
            ));
        }

        Ok(Self {
            function_key,
            param_count,
        })
    }
}

impl Mapper for DefaultMapper {
    fn map(&self, msg: &Message) -> Result<Invocation, String> {
        let args = if self.param_count == 0 {
            vec![]
        } else {
            let content_type = msg.headers().content_type().unwrap_or("application/json");
            let value = match content_type {
                "application/json" => serde_json::from_slice(msg.body())
                    .map_err(|e| format!("failed to parse body as JSON: {e}"))?,
                "text/plain" => {
                    let text = std::str::from_utf8(msg.body())
                        .map_err(|e| format!("body is not valid UTF-8: {e}"))?;
                    serde_json::Value::String(text.to_string())
                }
                other => return Err(format!("unsupported content-type: {other}")),
            };
            vec![value]
        };
        Ok(Invocation {
            function_key: self.function_key.clone(),
            args,
        })
    }
}

/// Handler that invokes a Wasm component per message.
///
/// For domain components (mapped mode), uses a `Mapper` to translate the
/// message into a function call. For components exporting the WIT `handler`
/// interface (direct mode), bypasses the mapper entirely.
pub struct Activator {
    runtime: Runtime,
    component_name: String,
    mode: InvocationMode,
}

enum InvocationMode {
    Direct,
    Mapped { mapper: Box<dyn Mapper> },
}

impl Activator {
    /// Create an activator for the named component.
    ///
    /// If `mapper` is `None`, creates a `DefaultMapper` that requires the
    /// component to export exactly one function. The default mapper also
    /// currently requires the target function to have 0 or 1 parameters.
    pub fn new(
        runtime: Runtime,
        component_name: &str,
        mapper: Option<Box<dyn Mapper>>,
    ) -> Result<Self, String> {
        let component = runtime
            .get_component(component_name)
            .ok_or_else(|| format!("component '{component_name}' not found"))?;

        let mode = if Self::exports_handler_interface(&component) {
            InvocationMode::Direct
        } else {
            let mapper = match mapper {
                Some(m) => m,
                None => Box::new(DefaultMapper::from_component(&component, None)?),
            };
            InvocationMode::Mapped { mapper }
        };

        Ok(Self {
            runtime,
            component_name: component_name.to_string(),
            mode,
        })
    }

    fn exports_handler_interface(component: &Component) -> bool {
        component.functions.values().any(|f| {
            f.interface()
                .is_some_and(|iface| iface.as_str().starts_with("modulewise:messaging/handler"))
        })
    }
}

impl Handler for Activator {
    fn handle(&self, msg: Message) -> impl Future<Output = Result<(), String>> + Send {
        async move {
            match &self.mode {
                InvocationMode::Direct => {
                    Err("direct handler mode not yet implemented".to_string())
                }
                InvocationMode::Mapped { mapper } => {
                    let invocation = mapper.map(&msg)?;
                    self.runtime
                        .invoke(
                            &self.component_name,
                            &invocation.function_key,
                            invocation.args,
                        )
                        .await
                        .map_err(|e| e.to_string())?;
                    Ok(())
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write;
    use std::path::PathBuf;

    use tempfile::{Builder, NamedTempFile};

    use super::*;
    use crate::graph::ComponentGraph;
    use crate::messaging::MessageBuilder;

    fn create_wasm_file(wat: &str) -> NamedTempFile {
        let component_bytes = wat::parse_str(wat).unwrap();
        let mut temp_file = Builder::new().suffix(".wasm").tempfile().unwrap();
        temp_file.write_all(&component_bytes).unwrap();
        temp_file
    }

    fn create_toml_file(content: &str) -> NamedTempFile {
        let mut temp_file = Builder::new().suffix(".toml").tempfile().unwrap();
        write!(temp_file, "{}", content).unwrap();
        temp_file
    }

    async fn build_runtime(paths: &[PathBuf]) -> Runtime {
        let mut builder = ComponentGraph::builder();
        for path in paths {
            builder = builder.load_file(path);
        }
        let graph = builder.build().unwrap();
        Runtime::builder(&graph).build().await.unwrap()
    }

    // Component that exports a single bare function: process(value: u32)
    fn single_function_wasm() -> NamedTempFile {
        let wat = r#"
            (component
                (core module $m
                    (func (export "process") (param i32))
                )
                (core instance $i (instantiate $m))
                (func $process (param "value" u32) (canon lift (core func $i "process")))
                (export "process" (func $process))
            )
        "#;
        create_wasm_file(wat)
    }

    // Component that exports two bare functions
    fn two_function_wasm() -> NamedTempFile {
        let wat = r#"
            (component
                (core module $m
                    (func (export "run"))
                    (func (export "stop"))
                )
                (core instance $i (instantiate $m))
                (func $run (canon lift (core func $i "run")))
                (func $stop (canon lift (core func $i "stop")))
                (export "run" (func $run))
                (export "stop" (func $stop))
            )
        "#;
        create_wasm_file(wat)
    }

    #[tokio::test]
    async fn convention_mapping_invokes_single_function_component() {
        let wasm = single_function_wasm();
        let toml_content = format!(
            r#"
            [guest]
            uri = "{}"
            exposed = true
            "#,
            wasm.path().display()
        );
        let toml = create_toml_file(&toml_content);
        let runtime = build_runtime(&[toml.path().to_path_buf()]).await;

        let activator = Activator::new(runtime, "guest", None).unwrap();

        let msg = MessageBuilder::new(b"42".to_vec()).build();
        let result = activator.handle(msg).await;
        assert!(result.is_ok(), "handle failed: {:?}", result.err());
    }

    #[tokio::test]
    async fn multi_function_component_without_function_key_errors() {
        let wasm = two_function_wasm();
        let toml_content = format!(
            r#"
            [guest]
            uri = "{}"
            exposed = true
            "#,
            wasm.path().display()
        );
        let toml = create_toml_file(&toml_content);
        let runtime = build_runtime(&[toml.path().to_path_buf()]).await;

        match Activator::new(runtime, "guest", None) {
            Ok(_) => panic!("expected error for multi-function component without mapper"),
            Err(err) => assert!(
                err.contains("default mapping currently requires exactly 1 exported function"),
                "unexpected error: {err}"
            ),
        }
    }

    #[tokio::test]
    async fn component_not_found_errors() {
        let wasm = single_function_wasm();
        let toml_content = format!(
            r#"
            [guest]
            uri = "{}"
            exposed = true
            "#,
            wasm.path().display()
        );
        let toml = create_toml_file(&toml_content);
        let runtime = build_runtime(&[toml.path().to_path_buf()]).await;

        match Activator::new(runtime, "nonexistent", None) {
            Ok(_) => panic!("expected error for nonexistent component"),
            Err(err) => assert!(
                err.contains("component 'nonexistent' not found"),
                "unexpected error: {err}"
            ),
        }
    }

    #[tokio::test]
    async fn custom_mapper_is_used() {
        let wasm = single_function_wasm();
        let toml_content = format!(
            r#"
            [guest]
            uri = "{}"
            exposed = true
            "#,
            wasm.path().display()
        );
        let toml = create_toml_file(&toml_content);
        let runtime = build_runtime(&[toml.path().to_path_buf()]).await;

        struct TestMapper;
        impl Mapper for TestMapper {
            fn map(&self, _msg: &Message) -> Result<Invocation, String> {
                Ok(Invocation {
                    function_key: "process".to_string(),
                    args: vec![serde_json::json!(99)],
                })
            }
        }

        let activator = Activator::new(runtime, "guest", Some(Box::new(TestMapper))).unwrap();

        let msg = MessageBuilder::new(b"ignored".to_vec()).build();
        let result = activator.handle(msg).await;
        assert!(result.is_ok(), "handle failed: {:?}", result.err());
    }
}
