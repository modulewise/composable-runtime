mod common;

use anyhow::Result;
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
    assert!(result.is_ok(), "build_registries failed: {:?}", result.err());

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
