mod common;

#[tokio::test]
async fn test_expects_and_enables() {
    let client_wasm = common::client_wasm();
    let handler_wasm = common::handler_wasm();

    let toml_content = format!(
        r#"
        [infra]
        uri = "wasmtime:some-infra"
        enables = "unexposed"

        [client]
        uri = "{}"
        expects = ["infra"]
        enables = "exposed"

        [handler]
        uri = "{}"
        expects = ["client"]
        exposed = true
        "#,
        client_wasm.display(),
        handler_wasm.display()
    );

    let toml_file = common::create_toml_test_file(&toml_content);
    let graph = common::load_graph_and_assert_ok(&[toml_file.to_path_buf()]);

    let infra_def = common::get_runtime_feature_definition(&graph, "infra");
    assert_eq!(infra_def.uri, "wasmtime:some-infra");
    assert_eq!(infra_def.enables, "unexposed");

    let client_def = common::get_component_definition(&graph, "client");
    assert_eq!(client_def.uri, client_wasm.to_path_buf().to_string_lossy());
    assert_eq!(client_def.expects, vec!["infra"]);
    assert_eq!(client_def.enables, "exposed");
    assert_eq!(client_def.exposed, false);

    let handler_def = common::get_component_definition(&graph, "handler");
    assert_eq!(
        handler_def.uri,
        handler_wasm.to_path_buf().to_string_lossy()
    );
    assert_eq!(handler_def.expects, vec!["client"]);
    assert_eq!(handler_def.enables, "none");
    assert!(handler_def.exposed);

    let (component_registry, runtime_feature_registry) =
        common::build_registries_and_assert_ok(&graph).await;
    assert_eq!(
        runtime_feature_registry
            .get_runtime_feature("infra")
            .unwrap()
            .uri,
        "wasmtime:some-infra"
    );
    assert_eq!(component_registry.get_components().count(), 1);

    let component = component_registry.get_components().next().unwrap();
    assert_eq!(component.name, "handler");
    assert_eq!(component.imports.len(), 0);
    assert_eq!(component.exports, vec!["modulewise:test/handler@0.1.0"]);
    assert_eq!(component.runtime_features, ["infra"]);
    let functions = component.functions.clone().unwrap();
    assert_eq!(functions.len(), 1);
    let function = functions.get("handler.handle").unwrap();
    assert!(function.params().is_empty());
    assert_eq!(function.result(), None);
    assert_eq!(
        function.interface().map(|i| i.as_str()),
        Some("modulewise:test/handler@0.1.0")
    );
}
