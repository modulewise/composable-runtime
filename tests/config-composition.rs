mod common;

#[tokio::test]
async fn empty_config() {
    let configurable_wasm = common::configurable_wasm();
    let graph = common::load_graph_and_assert_ok(&[configurable_wasm.to_path_buf()]);
    let (_runtime_registry, component_registry) =
        common::build_registries_and_assert_ok(&graph).await;

    assert_eq!(component_registry.get_components().count(), 1);
    let component = component_registry.get_components().next().unwrap();
    assert_eq!(component.imports.len(), 0);
}

#[tokio::test]
async fn config_value() {
    let configurable_wasm = common::configurable_wasm();
    let toml_content = format!(
        r#"
        [test-component]
        uri = "{}"
        exposed = true
        config.foo = "42"
        "#,
        configurable_wasm.to_string_lossy()
    );
    let toml_file = common::create_toml_test_file(&toml_content);
    let graph = common::load_graph_and_assert_ok(&[toml_file.to_path_buf()]);
    let (_runtime_feature_registry, component_registry) =
        common::build_registries_and_assert_ok(&graph).await;

    assert_eq!(component_registry.get_components().count(), 1);
    let component = component_registry.get_components().next().unwrap();
    assert_eq!(component.imports.len(), 0);
}
