mod common;

#[tokio::test]
#[should_panic(expected = "Component 'handler' has unsatisfied imports")]
async fn test_unsatisfied_import_for_component() {
    let handler_wasm = common::handler_wasm();

    let toml_content = format!(
        r#"
        [component.handler]
        uri = "{}"
        "#,
        handler_wasm.display()
    );

    let toml_file = common::create_toml_test_file(&toml_content);
    let graph = common::load_graph_and_assert_ok(&[toml_file.to_path_buf()]);
    let (_component_registry, _capability_registry) =
        common::build_registries_and_assert_ok(&graph).await;
}
