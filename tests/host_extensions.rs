mod common;

use anyhow::Result;
use composable_runtime::Runtime;
use composable_runtime::registry::{HostExtension, build_registries};

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

    let greeter_extension = HostExtension::new(
        "greeter",
        vec!["modulewise:test-host/greeter".to_string()],
        |_linker| -> Result<()> { Ok(()) },
    );

    // Build registries with the host extension
    let result = build_registries(&graph, vec![greeter_extension]).await;
    assert!(
        result.is_ok(),
        "build_registries failed: {:?}",
        result.err()
    );

    let (runtime_feature_registry, component_registry) = result.unwrap();

    // Verify the runtime feature was registered
    let feature = runtime_feature_registry.get_runtime_feature("greeter");
    assert!(feature.is_some(), "greeter feature should be registered");
    assert_eq!(feature.unwrap().uri, "host:greeter");

    // Verify the component was registered
    assert_eq!(component_registry.get_components().count(), 1);
    let component = component_registry.get_components().next().unwrap();
    assert_eq!(component.name, "guest");
    assert!(component.runtime_features.contains(&"greeter".to_string()));
}

#[tokio::test]
#[should_panic(expected = "Host extension 'missing' (URI: 'host:missing') not provided")]
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

    // Build registries without providing the host extension - should fail
    let _ = build_registries(&graph, vec![]).await.unwrap();
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

    let value_extension = HostExtension::new(
        "value-provider",
        vec!["modulewise:test-host/value-provider".to_string()],
        |linker| -> Result<()> {
            let mut inst = linker.instance("modulewise:test-host/value-provider")?;
            inst.func_wrap("get-value", |_ctx, (): ()| -> Result<(u32,)> {
                Ok((42u32,))
            })?;
            Ok(())
        },
    );

    let runtime = Runtime::from_graph_with_host_extensions(&graph, vec![value_extension])
        .await
        .expect("Failed to create runtime");

    let result = runtime
        .invoke("guest", "get-value", vec![])
        .await
        .expect("Failed to invoke");

    assert_eq!(result, serde_json::json!(42));
}
