mod common;

#[tokio::test]
async fn test_simple_interceptor() {
    let client_wasm = common::client_wasm();
    let interceptor_wasm = common::interceptor_wasm();
    let handler_wasm = common::handler_wasm();

    let toml_content = format!(
        r#"
        [component.client]
        uri = "{}"

        [component.interceptor]
        uri = "{}"
        intercepts = ["client"]

        [component.handler]
        uri = "{}"
        imports = ["client"]
        "#,
        client_wasm.display(),
        interceptor_wasm.display(),
        handler_wasm.display()
    );

    let toml_file = common::create_toml_test_file(&toml_content);
    let graph = common::load_graph_and_assert_ok(&[toml_file.to_path_buf()]);

    let client_def = common::get_component_definition(&graph, "client");
    assert_eq!(client_def.uri, client_wasm.to_path_buf().to_string_lossy());
    assert_eq!(client_def.imports.len(), 0);

    let interceptor_def = common::get_component_definition(&graph, "interceptor");
    assert_eq!(
        interceptor_def.uri,
        interceptor_wasm.to_path_buf().to_string_lossy()
    );
    assert_eq!(interceptor_def.imports.len(), 0);

    let handler_def = common::get_component_definition(&graph, "handler");
    assert_eq!(
        handler_def.uri,
        handler_wasm.to_path_buf().to_string_lossy()
    );
    assert_eq!(handler_def.imports, vec!["client"]);

    let (component_registry, _capability_registry) =
        common::build_registries_and_assert_ok(&graph).await;

    assert_eq!(component_registry.get_components().count(), 3);
    let handler = component_registry.get_component("handler").unwrap();
    assert_eq!(handler.name, "handler");
    assert_eq!(handler.imports.len(), 0);
    assert_eq!(handler.exports, vec!["modulewise:test/handler@0.1.0"]);
    assert_eq!(handler.capabilities.len(), 0);
    let functions = handler.functions.clone();
    assert_eq!(functions.len(), 1);
    let function = functions.get("handler.handle").unwrap();
    assert_eq!(
        function.params(),
        Vec::<composable_runtime::FunctionParam>::new()
    );
    assert_eq!(function.result(), None);
    assert_eq!(
        function.interface().map(|i| i.as_str()),
        Some("modulewise:test/handler@0.1.0")
    );
}

#[tokio::test]
async fn test_multiple_interceptors() {
    let client_wasm = common::client_wasm();
    let interceptor_wasm = common::interceptor_wasm();
    let handler_wasm = common::handler_wasm();

    let toml_content = format!(
        r#"
        [component.client]
        uri = "{}"

        [component.outer-interceptor]
        uri = "{}"
        intercepts = ["client"]
        precedence = 99

        [component.inner-interceptor]
        uri = "{}"
        intercepts = ["client"]
        precedence = 1

        [component.handler]
        uri = "{}"
        imports = ["client"]
        "#,
        client_wasm.display(),
        interceptor_wasm.display(),
        interceptor_wasm.display(),
        handler_wasm.display()
    );
    let toml_file = common::create_toml_test_file(&toml_content);
    let graph = common::load_graph_and_assert_ok(&[toml_file.to_path_buf()]);

    let handler_index = graph.get_node_index("handler").unwrap();
    let dependencies: Vec<_> = graph.get_dependencies(handler_index).collect();

    assert_eq!(
        dependencies.len(),
        1,
        "The handler should have one dependency"
    );

    let provider_name =
        if let composable_runtime::composition::graph::Node::Component(def) = &graph[dependencies[0]] {
            &def.name
        } else {
            unreachable!()
        };

    // Assert the lower precedence (higher number) interceptor is on the "outside"
    assert_eq!(
        provider_name, "outer-interceptor",
        "Handler should be connected to 'outer-interceptor', but was connected to '{}'",
        provider_name
    );

    // Assert outer-interceptor is connected to inner-interceptor
    let outer_interceptor_index = graph.get_node_index("outer-interceptor").unwrap();
    let outer_dependencies: Vec<_> = graph.get_dependencies(outer_interceptor_index).collect();
    assert_eq!(
        outer_dependencies.len(),
        1,
        "The outer-interceptor should have one dependency"
    );

    let outer_provider_name = if let composable_runtime::composition::graph::Node::Component(def) =
        &graph[outer_dependencies[0]]
    {
        &def.name
    } else {
        unreachable!()
    };
    assert_eq!(outer_provider_name, "inner-interceptor");

    // Assert inner-interceptor is connected to client
    let inner_interceptor_index = graph.get_node_index("inner-interceptor").unwrap();
    let inner_dependencies: Vec<_> = graph.get_dependencies(inner_interceptor_index).collect();
    assert_eq!(
        inner_dependencies.len(),
        1,
        "The inner-interceptor should have one dependency"
    );

    let inner_provider_name = if let composable_runtime::composition::graph::Node::Component(def) =
        &graph[inner_dependencies[0]]
    {
        &def.name
    } else {
        unreachable!()
    };
    assert_eq!(inner_provider_name, "client");

    let (component_registry, _capability_registry) =
        common::build_registries_and_assert_ok(&graph).await;
    assert_eq!(component_registry.get_components().count(), 4);
}
