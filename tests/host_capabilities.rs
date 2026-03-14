mod common;

use anyhow::Result;
use composable_runtime::{ComponentState, HostCapability, Runtime};
use serde::Deserialize;
use std::any::{Any, TypeId};
use wasmtime::component::Linker;

// Host capability that provides a greeter interface
#[derive(Deserialize, Default)]
struct GreeterCapability;

impl HostCapability for GreeterCapability {
    fn interfaces(&self) -> Vec<String> {
        vec!["modulewise:test-host/greeter".to_string()]
    }

    fn link(&self, _linker: &mut Linker<ComponentState>) -> wasmtime::Result<()> {
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
async fn test_host_capability_provides_interface() {
    let component_wasm = component_importing_host_interface();

    let toml_content = format!(
        r#"
        [capability.greeter]
        type = "greeter"

        [component.guest]
        uri = "{}"
        imports = ["greeter"]
        "#,
        component_wasm.display()
    );

    let toml_file = common::create_toml_test_file(&toml_content);

    // Build runtime with the host capability
    let runtime = Runtime::builder()
        .from_path(&*toml_file)
        .with_capability::<GreeterCapability>("greeter")
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
#[should_panic(expected = "Capability type 'missing' not registered")]
async fn test_missing_host_capability_panics() {
    let component_wasm = component_importing_host_interface();

    let toml_content = format!(
        r#"
        [capability.missing]
        type = "missing"

        [component.guest]
        uri = "{}"
        imports = ["missing"]
        "#,
        component_wasm.display()
    );

    let toml_file = common::create_toml_test_file(&toml_content);

    // Build runtime without providing the host capability - should fail
    let _ = Runtime::builder()
        .from_path(&*toml_file)
        .build()
        .await
        .unwrap();
}

// Host capability that provides a value-provider interface
#[derive(Deserialize, Default)]
struct ValueProviderCapability;

impl HostCapability for ValueProviderCapability {
    fn interfaces(&self) -> Vec<String> {
        vec!["modulewise:test-host/value-provider".to_string()]
    }

    fn link(&self, linker: &mut Linker<ComponentState>) -> wasmtime::Result<()> {
        let mut inst = linker.instance("modulewise:test-host/value-provider")?;
        inst.func_wrap("get-value", |_ctx, (): ()| -> wasmtime::Result<(u32,)> {
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
async fn test_host_capability_invoked() {
    let component_wasm = component_calling_host_get_value();

    let toml_content = format!(
        r#"
        [capability.value-provider]
        type = "value-provider"

        [component.guest]
        uri = "{}"
        imports = ["value-provider"]
        "#,
        component_wasm.display()
    );

    let toml_file = common::create_toml_test_file(&toml_content);

    let runtime = Runtime::builder()
        .from_path(&*toml_file)
        .with_capability::<ValueProviderCapability>("value-provider")
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

// Host Capability that reads config from TOML and uses it in host function
#[derive(Deserialize, Clone)]
struct MultiplierCapability {
    #[serde(default = "default_multiplier")]
    multiplier: u32,
}

fn default_multiplier() -> u32 {
    1
}

impl Default for MultiplierCapability {
    fn default() -> Self {
        Self {
            multiplier: default_multiplier(),
        }
    }
}

impl HostCapability for MultiplierCapability {
    fn interfaces(&self) -> Vec<String> {
        vec!["modulewise:test-host/multiplier".to_string()]
    }

    fn link(&self, linker: &mut Linker<ComponentState>) -> wasmtime::Result<()> {
        let multiplier = self.multiplier;
        let mut inst = linker.instance("modulewise:test-host/multiplier")?;
        inst.func_wrap(
            "multiply",
            move |_ctx, (value,): (u32,)| -> wasmtime::Result<(u32,)> { Ok((value * multiplier,)) },
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
async fn test_host_capability_with_config() {
    let component_wasm = component_calling_multiply();

    let toml_content = format!(
        r#"
        [capability.multiplier]
        type = "multiplier"
        multiplier = 5

        [component.guest]
        uri = "{}"
        imports = ["multiplier"]
        "#,
        component_wasm.display()
    );

    let toml_file = common::create_toml_test_file(&toml_content);

    let runtime = Runtime::builder()
        .from_path(&*toml_file)
        .with_capability::<MultiplierCapability>("multiplier")
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
async fn test_host_capability_with_default_config() {
    let component_wasm = component_calling_multiply();

    // No config.multiplier - should use default value of 1
    let toml_content = format!(
        r#"
        [capability.multiplier]
        type = "multiplier"

        [component.guest]
        uri = "{}"
        imports = ["multiplier"]
        "#,
        component_wasm.display()
    );

    let toml_file = common::create_toml_test_file(&toml_content);

    let runtime = Runtime::builder()
        .from_path(&*toml_file)
        .with_capability::<MultiplierCapability>("multiplier")
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

// --- Tests for capability state ---

struct CounterState {
    count: u32,
}

#[derive(Deserialize, Default)]
struct CounterCapability;

impl HostCapability for CounterCapability {
    fn interfaces(&self) -> Vec<String> {
        vec!["modulewise:test-host/counter".to_string()]
    }

    fn link(&self, linker: &mut Linker<ComponentState>) -> wasmtime::Result<()> {
        let mut inst = linker.instance("modulewise:test-host/counter")?;
        inst.func_wrap(
            "increment",
            |mut ctx: wasmtime::StoreContextMut<'_, ComponentState>,
             (): ()|
             -> wasmtime::Result<(u32,)> {
                let state = ctx
                    .data_mut()
                    .get_extension_mut::<CounterState>()
                    .ok_or_else(|| wasmtime::Error::msg("CounterState not found"))?;
                state.count += 1;
                Ok((state.count,))
            },
        )?;
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
async fn test_host_capability_with_state() {
    let component_wasm = component_calling_increment_twice();

    let toml_content = format!(
        r#"
        [capability.counter]
        type = "counter"

        [component.guest]
        uri = "{}"
        imports = ["counter"]
        "#,
        component_wasm.display()
    );

    let toml_file = common::create_toml_test_file(&toml_content);

    let runtime = Runtime::builder()
        .from_path(&*toml_file)
        .with_capability::<CounterCapability>("counter")
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
async fn test_host_capability_state_isolated_per_instance() {
    let component_wasm = component_calling_increment_twice();

    let toml_content = format!(
        r#"
        [capability.counter]
        type = "counter"

        [component.guest]
        uri = "{}"
        imports = ["counter"]
        "#,
        component_wasm.display()
    );

    let toml_file = common::create_toml_test_file(&toml_content);

    let runtime = Runtime::builder()
        .from_path(&*toml_file)
        .with_capability::<CounterCapability>("counter")
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

// Shared state type used by two different capabilities
#[allow(dead_code)]
struct SharedState {
    value: u32,
}

#[derive(Deserialize, Default)]
struct FirstCapabilityWithSharedState;

impl HostCapability for FirstCapabilityWithSharedState {
    fn interfaces(&self) -> Vec<String> {
        vec!["modulewise:test-host/first".to_string()]
    }

    fn link(&self, _linker: &mut Linker<ComponentState>) -> wasmtime::Result<()> {
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
struct SecondCapabilityWithSharedState;

impl HostCapability for SecondCapabilityWithSharedState {
    fn interfaces(&self) -> Vec<String> {
        vec!["modulewise:test-host/second".to_string()]
    }

    fn link(&self, _linker: &mut Linker<ComponentState>) -> wasmtime::Result<()> {
        Ok(())
    }

    fn create_state_boxed(&self) -> Result<Option<(TypeId, Box<dyn Any + Send>)>> {
        // Returns same TypeId as FirstCapabilityWithSharedState - should cause error
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
async fn test_duplicate_capability_state_type_fails() {
    let component_wasm = component_importing_two_host_interfaces();

    let toml_content = format!(
        r#"
        [capability.first]
        type = "first"

        [capability.second]
        type = "second"

        [component.guest]
        uri = "{}"
        imports = ["first", "second"]
        "#,
        component_wasm.display()
    );

    let toml_file = common::create_toml_test_file(&toml_content);

    let runtime = Runtime::builder()
        .from_path(&*toml_file)
        .with_capability::<FirstCapabilityWithSharedState>("first")
        .with_capability::<SecondCapabilityWithSharedState>("second")
        .build()
        .await
        .expect("Failed to create runtime");

    // State is created during instantiation, not during build
    let result = runtime.instantiate("guest").await;

    // Should fail because both capabilities try to register SharedState
    match result {
        Ok(_) => panic!("Expected error due to duplicate state type, but instantiation succeeded"),
        Err(e) => {
            let err = e.to_string();
            assert!(
                err.contains("Duplicate state type"),
                "Expected 'Duplicate state type' error, got: {}",
                err
            );
        }
    }
}
