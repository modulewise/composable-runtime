mod common;

#[tokio::test]
async fn test_unsatisfied_import_for_exposed_component() {
    let handler_wasm = common::handler_wasm();

    let toml_content = format!(
        r#"
        [handler]
        uri = "{}"
        exposed = true
        "#,
        handler_wasm.display()
    );

    let toml_file = common::create_toml_test_file(&toml_content);
    let graph = common::load_graph_and_assert_ok(&[toml_file.to_path_buf()]);
    let (_runtime_feature_registry, component_registry) =
        common::build_registries_and_assert_ok(&graph).await;
    // Unsatisfied import should cause an exposed component to be skipped.
    assert_eq!(component_registry.get_components().count(), 0);
}

#[tokio::test]
#[should_panic(expected = "Component 'handler' has unsatisfied imports")]
async fn test_unsatisfied_import_for_enabling_component() {
    let handler_wasm = common::handler_wasm();

    let toml_content = format!(
        r#"
        [handler]
        uri = "{}"
        enables = "any"
        "#,
        handler_wasm.display()
    );

    let toml_file = common::create_toml_test_file(&toml_content);
    let graph = common::load_graph_and_assert_ok(&[toml_file.to_path_buf()]);
    let (_runtime_feature_registry, _component_registry) =
        common::build_registries_and_assert_ok(&graph).await;
}
