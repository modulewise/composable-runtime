use std::future::Future;
use std::sync::Arc;

use crate::runtime::{Component, Runtime};

use super::channel::{Channel, ChannelRegistry, LocalChannel};
use super::message::{Message, MessageBuilder, header};

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
    registry: Option<Arc<ChannelRegistry<LocalChannel>>>,
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
        registry: Option<Arc<ChannelRegistry<LocalChannel>>>,
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
            registry,
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
                    let result = self
                        .runtime
                        .invoke(
                            &self.component_name,
                            &invocation.function_key,
                            invocation.args,
                        )
                        .await
                        .map_err(|e| e.to_string())?;

                    if let Some(reply_to) = msg.headers().reply_to() {
                        let registry = self.registry.as_ref().ok_or_else(|| {
                            format!("reply-to '{reply_to}' requested but no channel registry")
                        })?;
                        let channel = registry
                            .lookup(reply_to)
                            .ok_or_else(|| format!("reply-to channel '{reply_to}' not found"))?;
                        let content_type = msg.headers().content_type();
                        let body = match content_type {
                            Some("text/plain") => match &result {
                                serde_json::Value::String(s) => s.as_bytes().to_vec(),
                                other => other.to_string().into_bytes(),
                            },
                            _ => serde_json::to_vec(&result)
                                .map_err(|e| format!("failed to serialize reply: {e}"))?,
                        };
                        let mut builder = MessageBuilder::new(body);
                        if let Some(corr_id) = msg.headers().correlation_id() {
                            builder = builder.header(header::CORRELATION_ID, corr_id);
                        }
                        if let Some(ct) = content_type {
                            builder = builder.header(header::CONTENT_TYPE, ct);
                        }
                        let _receipt = channel
                            .publish(builder.build())
                            .await
                            .map_err(|e| format!("failed to publish reply: {e}"))?;
                    }

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
    use std::sync::Arc;

    use tempfile::{Builder, NamedTempFile};

    use super::*;
    use crate::composition::graph::ComponentGraph;
    use crate::messaging::{Channel, LocalChannel, MessageBuilder};

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

    // Component that exports a single bare function: double(value: u32) -> u32
    fn single_function_wasm() -> NamedTempFile {
        let wat = r#"
            (component
                (core module $m
                    (func (export "double") (param i32) (result i32)
                        (i32.mul (local.get 0) (i32.const 2))
                    )
                )
                (core instance $i (instantiate $m))
                (func $double (param "value" u32) (result u32)
                    (canon lift (core func $i "double"))
                )
                (export "double" (func $double))
            )
        "#;
        create_wasm_file(wat)
    }

    // Component that exports greet(name: string) -> string.
    // Returns "Hello, " + name by copying both parts into memory.
    fn string_function_wasm() -> NamedTempFile {
        let wat = r#"
            (component
                (core module $m
                    (memory (export "mem") 1)

                    ;; Simple bump allocator at offset 1024.
                    (global $bump (mut i32) (i32.const 1024))
                    (func (export "cabi_realloc") (param i32 i32 i32 i32) (result i32)
                        (call $realloc_internal (local.get 0) (local.get 1) (local.get 2) (local.get 3))
                    )

                    ;; "Hello, " stored at offset 0 (7 bytes).
                    (data (i32.const 0) "Hello, ")

                    ;; greet(name_ptr, name_len) -> ret_ptr
                    ;; Writes (ptr, len) pair for the result string at ret_ptr.
                    (func $realloc_internal (param $old_ptr i32) (param $old_len i32)
                                            (param $align i32) (param $new_len i32) (result i32)
                        (local $ptr i32)
                        (global.set $bump
                            (i32.and
                                (i32.add (global.get $bump) (i32.sub (local.get $align) (i32.const 1)))
                                (i32.xor (i32.sub (local.get $align) (i32.const 1)) (i32.const -1))
                            )
                        )
                        (local.set $ptr (global.get $bump))
                        (global.set $bump (i32.add (local.get $ptr) (local.get $new_len)))
                        (local.get $ptr)
                    )
                    (func (export "greet") (param $name_ptr i32) (param $name_len i32) (result i32)
                        (local $out_ptr i32)
                        (local $total_len i32)
                        (local $ret_ptr i32)

                        ;; total_len = 7 + name_len
                        (local.set $total_len (i32.add (i32.const 7) (local.get $name_len)))

                        ;; Allocate space for the result string (align 1).
                        (local.set $out_ptr
                            (call $realloc_internal (i32.const 0) (i32.const 0) (i32.const 1) (local.get $total_len)))

                        ;; Copy "Hello, " (7 bytes from offset 0).
                        (memory.copy (local.get $out_ptr) (i32.const 0) (i32.const 7))

                        ;; Copy name after "Hello, ".
                        (memory.copy
                            (i32.add (local.get $out_ptr) (i32.const 7))
                            (local.get $name_ptr)
                            (local.get $name_len)
                        )

                        ;; Allocate space for the (ptr, len) return pair (align 4).
                        (local.set $ret_ptr
                            (call $realloc_internal (i32.const 0) (i32.const 0) (i32.const 4) (i32.const 8)))

                        ;; Write ptr and len.
                        (i32.store (local.get $ret_ptr) (local.get $out_ptr))
                        (i32.store offset=4 (local.get $ret_ptr) (local.get $total_len))

                        (local.get $ret_ptr)
                    )

                    (func (export "cabi_post_greet") (param i32))
                )
                (core instance $i (instantiate $m))
                (func $greet (param "name" string) (result string)
                    (canon lift (core func $i "greet") (memory $i "mem")
                        (realloc (func $i "cabi_realloc"))
                        (post-return (func $i "cabi_post_greet"))
                    )
                )
                (export "greet" (func $greet))
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
            [component.guest]
            uri = "{}"
            "#,
            wasm.path().display()
        );
        let toml = create_toml_file(&toml_content);
        let runtime = build_runtime(&[toml.path().to_path_buf()]).await;

        let registry = Arc::new(ChannelRegistry::new());
        let replies = Arc::new(LocalChannel::with_defaults());
        registry.register("replies", replies.clone());

        let activator = Activator::new(runtime, "guest", None, Some(registry)).unwrap();

        let msg = MessageBuilder::new(b"21".to_vec())
            .header(header::CONTENT_TYPE, "application/json")
            .header(header::REPLY_TO, "replies")
            .build();

        let consumer = {
            let ch = replies.clone();
            tokio::spawn(async move { ch.consume("test").await })
        };
        tokio::task::yield_now().await;

        let result = activator.handle(msg).await;
        assert!(result.is_ok(), "handle failed: {:?}", result.err());

        let (reply, _) = consumer.await.unwrap().unwrap();
        // double(21) = 42
        assert_eq!(reply.body(), b"42");
    }

    #[tokio::test]
    async fn multi_function_component_without_function_key_errors() {
        let wasm = two_function_wasm();
        let toml_content = format!(
            r#"
            [component.guest]
            uri = "{}"
            "#,
            wasm.path().display()
        );
        let toml = create_toml_file(&toml_content);
        let runtime = build_runtime(&[toml.path().to_path_buf()]).await;

        match Activator::new(runtime, "guest", None, None) {
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
            [component.guest]
            uri = "{}"
            "#,
            wasm.path().display()
        );
        let toml = create_toml_file(&toml_content);
        let runtime = build_runtime(&[toml.path().to_path_buf()]).await;

        match Activator::new(runtime, "nonexistent", None, None) {
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
            [component.guest]
            uri = "{}"
            "#,
            wasm.path().display()
        );
        let toml = create_toml_file(&toml_content);
        let runtime = build_runtime(&[toml.path().to_path_buf()]).await;

        struct TestMapper;
        impl Mapper for TestMapper {
            fn map(&self, _msg: &Message) -> Result<Invocation, String> {
                Ok(Invocation {
                    function_key: "double".to_string(),
                    args: vec![serde_json::json!(7)],
                })
            }
        }

        let registry = Arc::new(ChannelRegistry::new());
        let replies = Arc::new(LocalChannel::with_defaults());
        registry.register("replies", replies.clone());

        let activator =
            Activator::new(runtime, "guest", Some(Box::new(TestMapper)), Some(registry)).unwrap();

        let msg = MessageBuilder::new(b"ignored".to_vec())
            .header(header::REPLY_TO, "replies")
            .build();

        let consumer = {
            let ch = replies.clone();
            tokio::spawn(async move { ch.consume("test").await })
        };
        tokio::task::yield_now().await;

        let result = activator.handle(msg).await;
        assert!(result.is_ok(), "handle failed: {:?}", result.err());

        let (reply, _) = consumer.await.unwrap().unwrap();
        // TestMapper sends 7 to double -> 14
        assert_eq!(reply.body(), b"14");
    }

    #[tokio::test]
    async fn reply_to_json() {
        let wasm = string_function_wasm();
        let toml_content = format!(
            r#"
            [component.guest]
            uri = "{}"
            "#,
            wasm.path().display()
        );
        let toml = create_toml_file(&toml_content);
        let runtime = build_runtime(&[toml.path().to_path_buf()]).await;

        let registry = Arc::new(ChannelRegistry::new());
        let replies = Arc::new(LocalChannel::with_defaults());
        registry.register("replies", replies.clone());

        let activator = Activator::new(runtime, "guest", None, Some(registry)).unwrap();

        let msg = MessageBuilder::new(b"\"World\"".to_vec())
            .header(header::CONTENT_TYPE, "application/json")
            .header(header::REPLY_TO, "replies")
            .header(header::CORRELATION_ID, "corr-1")
            .build();

        let consumer = {
            let ch = replies.clone();
            tokio::spawn(async move { ch.consume("test").await })
        };
        tokio::task::yield_now().await;

        let result = activator.handle(msg).await;
        assert!(result.is_ok(), "handle failed: {:?}", result.err());

        let (reply, _) = consumer.await.unwrap().unwrap();
        assert_eq!(reply.headers().correlation_id(), Some("corr-1"));
        assert_eq!(reply.headers().content_type(), Some("application/json"));
        // greet("World") -> "Hello, World", JSON-serialized with quotes
        assert_eq!(reply.body(), b"\"Hello, World\"");
    }

    #[tokio::test]
    async fn reply_to_text_plain() {
        let wasm = string_function_wasm();
        let toml_content = format!(
            r#"
            [component.guest]
            uri = "{}"
            "#,
            wasm.path().display()
        );
        let toml = create_toml_file(&toml_content);
        let runtime = build_runtime(&[toml.path().to_path_buf()]).await;

        let registry = Arc::new(ChannelRegistry::new());
        let replies = Arc::new(LocalChannel::with_defaults());
        registry.register("replies", replies.clone());

        let activator = Activator::new(runtime, "guest", None, Some(registry)).unwrap();

        let msg = MessageBuilder::new(b"World".to_vec())
            .header(header::CONTENT_TYPE, "text/plain")
            .header(header::REPLY_TO, "replies")
            .header(header::CORRELATION_ID, "corr-2")
            .build();

        let consumer = {
            let ch = replies.clone();
            tokio::spawn(async move { ch.consume("test").await })
        };
        tokio::task::yield_now().await;

        let result = activator.handle(msg).await;
        assert!(result.is_ok(), "handle failed: {:?}", result.err());

        let (reply, _) = consumer.await.unwrap().unwrap();
        assert_eq!(reply.headers().correlation_id(), Some("corr-2"));
        assert_eq!(reply.headers().content_type(), Some("text/plain"));
        // greet("World") -> "Hello, World", serialized as raw text bytes
        assert_eq!(reply.body(), b"Hello, World");
    }
}
