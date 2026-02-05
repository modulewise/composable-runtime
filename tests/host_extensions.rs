mod common;

use anyhow::Result;
use composable_runtime::{ComponentState, HostExtension, Runtime};
use serde::Deserialize;
use std::any::{Any, TypeId};
use wasmtime::component::Linker;

/// Test extension that provides a greeter interface
#[derive(Deserialize, Default)]
struct GreeterFeature;

impl HostExtension for GreeterFeature {
    fn interfaces(&self) -> Vec<String> {
        vec!["modulewise:test-host/greeter".to_string()]
    }

    fn link(&self, _linker: &mut Linker<ComponentState>) -> Result<()> {
        Ok(())
    }
}

fn component_importing_host_interface() -> common::TestFile {
    let wat = r#"
        (component
            (import "modulewise:test-host/greeter" (instance $greeter
                (export "greet" (func (param "name" string) (result string)))
            ))
            (core module $m
                (func (export "run"))
            )
            (core instance $i (instantiate $m))
            (func $run (canon lift (core func $i "run")))
            (export "run" (func $run))
        )
    "#;
    common::create_wasm_test_file(wat)
}

#[tokio::test]
async fn test_host_extension_provides_interface() {
    let component_wasm = component_importing_host_interface();

    let toml_content = format!(
        r#"
        [greeter]
        uri = "host:greeter"
        enables = "any"

        [guest]
        uri = "{}"
        expects = ["greeter"]
        exposed = true
        "#,
        component_wasm.display()
    );

    let toml_file = common::create_toml_test_file(&toml_content);
    let graph = common::load_graph_and_assert_ok(&[toml_file.to_path_buf()]);

    // Build runtime with the host extension
    let runtime = Runtime::builder(&graph)
        .with_host_extension::<GreeterFeature>("greeter")
        .build()
        .await;

    assert!(
        runtime.is_ok(),
        "Runtime::builder failed: {:?}",
        runtime.err()
    );

    let runtime = runtime.unwrap();

    // Verify the component was registered
    let components: Vec<_> = runtime.list_components();
    assert_eq!(components.len(), 1);
    assert_eq!(components[0].name, "guest");
}

#[tokio::test]
#[should_panic(expected = "Host extension 'missing' (URI: 'host:missing') not registered")]
async fn test_missing_host_extension_panics() {
    let component_wasm = component_importing_host_interface();

    let toml_content = format!(
        r#"
        [missing]
        uri = "host:missing"
        enables = "any"

        [guest]
        uri = "{}"
        expects = ["missing"]
        exposed = true
        "#,
        component_wasm.display()
    );

    let toml_file = common::create_toml_test_file(&toml_content);
    let graph = common::load_graph_and_assert_ok(&[toml_file.to_path_buf()]);

    // Build runtime without providing the host extension - should fail
    let _ = Runtime::builder(&graph).build().await.unwrap();
}

/// Test extension that provides a value-provider interface
#[derive(Deserialize, Default)]
struct ValueProviderFeature;

impl HostExtension for ValueProviderFeature {
    fn interfaces(&self) -> Vec<String> {
        vec!["modulewise:test-host/value-provider".to_string()]
    }

    fn link(&self, linker: &mut Linker<ComponentState>) -> Result<()> {
        let mut inst = linker.instance("modulewise:test-host/value-provider")?;
        inst.func_wrap("get-value", |_ctx, (): ()| -> Result<(u32,)> {
            Ok((42u32,))
        })?;
        Ok(())
    }
}

fn component_calling_host_get_value() -> common::TestFile {
    let wat = r#"
        (component
            (import "modulewise:test-host/value-provider" (instance $provider
                (export "get-value" (func (result u32)))
            ))
            (core func $host_get_value (canon lower (func $provider "get-value")))
            (core module $m
                (import "" "get-value" (func $imported_get_value (result i32)))
                (func (export "get-value") (result i32)
                    (call $imported_get_value)
                )
            )
            (core instance $i (instantiate $m
                (with "" (instance (export "get-value" (func $host_get_value))))
            ))
            (func $get_value (result u32) (canon lift (core func $i "get-value")))
            (export "get-value" (func $get_value))
        )
    "#;
    common::create_wasm_test_file(wat)
}

#[tokio::test]
async fn test_host_extension_invoked() {
    let component_wasm = component_calling_host_get_value();

    let toml_content = format!(
        r#"
        [value-provider]
        uri = "host:value-provider"
        enables = "any"

        [guest]
        uri = "{}"
        expects = ["value-provider"]
        exposed = true
        "#,
        component_wasm.display()
    );

    let toml_file = common::create_toml_test_file(&toml_content);
    let graph = common::load_graph_and_assert_ok(&[toml_file.to_path_buf()]);

    let runtime = Runtime::builder(&graph)
        .with_host_extension::<ValueProviderFeature>("value-provider")
        .build()
        .await
        .expect("Failed to create runtime");

    let result = runtime
        .invoke("guest", "get-value", vec![])
        .await
        .expect("Failed to invoke");

    assert_eq!(result, serde_json::json!(42));
}

// --- Tests for TOML config ---

/// Extension that reads config from TOML and uses it in host function
#[derive(Deserialize, Clone)]
struct MultiplierFeature {
    #[serde(default = "default_multiplier")]
    multiplier: u32,
}

fn default_multiplier() -> u32 {
    1
}

impl Default for MultiplierFeature {
    fn default() -> Self {
        Self {
            multiplier: default_multiplier(),
        }
    }
}

impl HostExtension for MultiplierFeature {
    fn interfaces(&self) -> Vec<String> {
        vec!["modulewise:test-host/multiplier".to_string()]
    }

    fn link(&self, linker: &mut Linker<ComponentState>) -> Result<()> {
        let multiplier = self.multiplier;
        let mut inst = linker.instance("modulewise:test-host/multiplier")?;
        inst.func_wrap(
            "multiply",
            move |_ctx, (value,): (u32,)| -> Result<(u32,)> { Ok((value * multiplier,)) },
        )?;
        Ok(())
    }
}

fn component_calling_multiply() -> common::TestFile {
    let wat = r#"
        (component
            (import "modulewise:test-host/multiplier" (instance $mult
                (export "multiply" (func (param "value" u32) (result u32)))
            ))
            (core func $host_multiply (canon lower (func $mult "multiply")))
            (core module $m
                (import "" "multiply" (func $imported_multiply (param i32) (result i32)))
                (func (export "calc") (result i32)
                    (call $imported_multiply (i32.const 10))
                )
            )
            (core instance $i (instantiate $m
                (with "" (instance (export "multiply" (func $host_multiply))))
            ))
            (func $calc (result u32) (canon lift (core func $i "calc")))
            (export "calc" (func $calc))
        )
    "#;
    common::create_wasm_test_file(wat)
}

#[tokio::test]
async fn test_host_extension_with_config() {
    let component_wasm = component_calling_multiply();

    let toml_content = format!(
        r#"
        [multiplier]
        uri = "host:multiplier"
        enables = "any"
        config.multiplier = 5

        [guest]
        uri = "{}"
        expects = ["multiplier"]
        exposed = true
        "#,
        component_wasm.display()
    );

    let toml_file = common::create_toml_test_file(&toml_content);
    let graph = common::load_graph_and_assert_ok(&[toml_file.to_path_buf()]);

    let runtime = Runtime::builder(&graph)
        .with_host_extension::<MultiplierFeature>("multiplier")
        .build()
        .await
        .expect("Failed to create runtime");

    let result = runtime
        .invoke("guest", "calc", vec![])
        .await
        .expect("Failed to invoke");

    // 10 * 5 = 50
    assert_eq!(result, serde_json::json!(50));
}

#[tokio::test]
async fn test_host_extension_with_default_config() {
    let component_wasm = component_calling_multiply();

    // No config.multiplier - should use default value of 1
    let toml_content = format!(
        r#"
        [multiplier]
        uri = "host:multiplier"
        enables = "any"

        [guest]
        uri = "{}"
        expects = ["multiplier"]
        exposed = true
        "#,
        component_wasm.display()
    );

    let toml_file = common::create_toml_test_file(&toml_content);
    let graph = common::load_graph_and_assert_ok(&[toml_file.to_path_buf()]);

    let runtime = Runtime::builder(&graph)
        .with_host_extension::<MultiplierFeature>("multiplier")
        .build()
        .await
        .expect("Failed to create runtime");

    let result = runtime
        .invoke("guest", "calc", vec![])
        .await
        .expect("Failed to invoke");

    // 10 * 1 = 10 (default multiplier)
    assert_eq!(result, serde_json::json!(10));
}

// --- Tests for extension state ---

struct CounterState {
    count: u32,
}

#[derive(Deserialize, Default)]
struct CounterFeature;

impl HostExtension for CounterFeature {
    fn interfaces(&self) -> Vec<String> {
        vec!["modulewise:test-host/counter".to_string()]
    }

    fn link(&self, linker: &mut Linker<ComponentState>) -> Result<()> {
        let mut inst = linker.instance("modulewise:test-host/counter")?;
        inst.func_wrap("increment", |mut ctx, (): ()| -> Result<(u32,)> {
            let state = ctx
                .data_mut()
                .get_extension_mut::<CounterState>()
                .ok_or_else(|| anyhow::anyhow!("CounterState not found"))?;
            state.count += 1;
            Ok((state.count,))
        })?;
        Ok(())
    }

    fn create_state_boxed(&self) -> Result<Option<(TypeId, Box<dyn Any + Send>)>> {
        Ok(Some((
            TypeId::of::<CounterState>(),
            Box::new(CounterState { count: 0 }),
        )))
    }
}

fn component_calling_increment_twice() -> common::TestFile {
    let wat = r#"
        (component
            (import "modulewise:test-host/counter" (instance $counter
                (export "increment" (func (result u32)))
            ))
            (core func $host_increment (canon lower (func $counter "increment")))
            (core module $m
                (import "" "increment" (func $imported_increment (result i32)))
                (func (export "count-twice") (result i32)
                    (drop (call $imported_increment))
                    (call $imported_increment)
                )
            )
            (core instance $i (instantiate $m
                (with "" (instance (export "increment" (func $host_increment))))
            ))
            (func $count_twice (result u32) (canon lift (core func $i "count-twice")))
            (export "count-twice" (func $count_twice))
        )
    "#;
    common::create_wasm_test_file(wat)
}

#[tokio::test]
async fn test_host_extension_with_state() {
    let component_wasm = component_calling_increment_twice();

    let toml_content = format!(
        r#"
        [counter]
        uri = "host:counter"
        enables = "any"

        [guest]
        uri = "{}"
        expects = ["counter"]
        exposed = true
        "#,
        component_wasm.display()
    );

    let toml_file = common::create_toml_test_file(&toml_content);
    let graph = common::load_graph_and_assert_ok(&[toml_file.to_path_buf()]);

    let runtime = Runtime::builder(&graph)
        .with_host_extension::<CounterFeature>("counter")
        .build()
        .await
        .expect("Failed to create runtime");

    let result = runtime
        .invoke("guest", "count-twice", vec![])
        .await
        .expect("Failed to invoke");

    // increment() called twice
    assert_eq!(result, serde_json::json!(2));
}

#[tokio::test]
async fn test_host_extension_state_isolated_per_instance() {
    let component_wasm = component_calling_increment_twice();

    let toml_content = format!(
        r#"
        [counter]
        uri = "host:counter"
        enables = "any"

        [guest]
        uri = "{}"
        expects = ["counter"]
        exposed = true
        "#,
        component_wasm.display()
    );

    let toml_file = common::create_toml_test_file(&toml_content);
    let graph = common::load_graph_and_assert_ok(&[toml_file.to_path_buf()]);

    let runtime = Runtime::builder(&graph)
        .with_host_extension::<CounterFeature>("counter")
        .build()
        .await
        .expect("Failed to create runtime");

    // First invocation
    let result1 = runtime
        .invoke("guest", "count-twice", vec![])
        .await
        .expect("Failed to invoke");
    assert_eq!(result1, serde_json::json!(2));

    // Second invocation - should start fresh (new instance, new state)
    let result2 = runtime
        .invoke("guest", "count-twice", vec![])
        .await
        .expect("Failed to invoke");
    assert_eq!(result2, serde_json::json!(2));
}

// --- Tests for duplicate state type detection ---

// Shared state type used by two different extensions
#[allow(dead_code)]
struct SharedState {
    value: u32,
}

#[derive(Deserialize, Default)]
struct FirstFeatureWithSharedState;

impl HostExtension for FirstFeatureWithSharedState {
    fn interfaces(&self) -> Vec<String> {
        vec!["modulewise:test-host/first".to_string()]
    }

    fn link(&self, _linker: &mut Linker<ComponentState>) -> Result<()> {
        Ok(())
    }

    fn create_state_boxed(&self) -> Result<Option<(TypeId, Box<dyn Any + Send>)>> {
        Ok(Some((
            TypeId::of::<SharedState>(),
            Box::new(SharedState { value: 1 }),
        )))
    }
}

#[derive(Deserialize, Default)]
struct SecondFeatureWithSharedState;

impl HostExtension for SecondFeatureWithSharedState {
    fn interfaces(&self) -> Vec<String> {
        vec!["modulewise:test-host/second".to_string()]
    }

    fn link(&self, _linker: &mut Linker<ComponentState>) -> Result<()> {
        Ok(())
    }

    fn create_state_boxed(&self) -> Result<Option<(TypeId, Box<dyn Any + Send>)>> {
        // Returns same TypeId as FirstFeatureWithSharedState - should cause error
        Ok(Some((
            TypeId::of::<SharedState>(),
            Box::new(SharedState { value: 2 }),
        )))
    }
}

fn component_importing_two_host_interfaces() -> common::TestFile {
    let wat = r#"
        (component
            (import "modulewise:test-host/first" (instance))
            (import "modulewise:test-host/second" (instance))
            (core module $m
                (func (export "run"))
            )
            (core instance $i (instantiate $m))
            (func $run (canon lift (core func $i "run")))
            (export "run" (func $run))
        )
    "#;
    common::create_wasm_test_file(wat)
}

#[tokio::test]
async fn test_duplicate_extension_state_type_fails() {
    let component_wasm = component_importing_two_host_interfaces();

    let toml_content = format!(
        r#"
        [first]
        uri = "host:first"
        enables = "any"

        [second]
        uri = "host:second"
        enables = "any"

        [guest]
        uri = "{}"
        expects = ["first", "second"]
        exposed = true
        "#,
        component_wasm.display()
    );

    let toml_file = common::create_toml_test_file(&toml_content);
    let graph = common::load_graph_and_assert_ok(&[toml_file.to_path_buf()]);

    let runtime = Runtime::builder(&graph)
        .with_host_extension::<FirstFeatureWithSharedState>("first")
        .with_host_extension::<SecondFeatureWithSharedState>("second")
        .build()
        .await
        .expect("Failed to create runtime");

    // State is created during instantiation, not during build
    let result = runtime.instantiate("guest").await;

    // Should fail because both extensions try to register SharedState
    match result {
        Ok(_) => panic!("Expected error due to duplicate state type, but instantiation succeeded"),
        Err(e) => {
            let err = e.to_string();
            assert!(
                err.contains("Duplicate extension state type"),
                "Expected 'Duplicate extension state type' error, got: {}",
                err
            );
        }
    }
}
