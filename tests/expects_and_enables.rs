mod common;

#[tokio::test]
async fn test_imports_and_scope() {
    let client_wasm = common::client_wasm();
    let handler_wasm = common::handler_wasm();

    let toml_content = format!(
        r#"
        [capability.infra]
        type = "wasi:io"
        scope = "any"

        [component.client]
        uri = "{}"
        imports = ["infra"]
        scope = "any"

        [component.handler]
        uri = "{}"
        imports = ["client"]
        "#,
        client_wasm.display(),
        handler_wasm.display()
    );

    let toml_file = common::create_toml_test_file(&toml_content);
    let graph = common::load_graph_and_assert_ok(&[toml_file.to_path_buf()]);

    let infra_def = common::get_capability_definition(&graph, "infra");
    assert_eq!(infra_def.kind, "wasi:io");
    assert_eq!(infra_def.scope, "any");

    let client_def = common::get_component_definition(&graph, "client");
    assert_eq!(client_def.uri, client_wasm.to_path_buf().to_string_lossy());
    assert_eq!(client_def.imports, vec!["infra"]);
    assert_eq!(client_def.scope, "any");

    let handler_def = common::get_component_definition(&graph, "handler");
    assert_eq!(
        handler_def.uri,
        handler_wasm.to_path_buf().to_string_lossy()
    );
    assert_eq!(handler_def.imports, vec!["client"]);
    assert_eq!(handler_def.scope, "any");

    let (component_registry, capability_registry) =
        common::build_registries_and_assert_ok(&graph).await;
    assert_eq!(
        capability_registry.get_capability("infra").unwrap().kind,
        "wasi:io"
    );
    assert_eq!(component_registry.get_components().count(), 2);

    let handler = component_registry.get_component("handler").unwrap();
    assert_eq!(handler.name, "handler");
    assert_eq!(handler.imports.len(), 0);
    assert_eq!(handler.exports, vec!["modulewise:test/handler@0.1.0"]);
    assert_eq!(handler.capabilities, ["infra"]);
    let functions = handler.functions.clone();
    assert_eq!(functions.len(), 1);
    let function = functions.get("handler.handle").unwrap();
    assert!(function.params().is_empty());
    assert_eq!(function.result(), None);
    assert_eq!(
        function.interface().map(|i| i.as_str()),
        Some("modulewise:test/handler@0.1.0")
    );
}
