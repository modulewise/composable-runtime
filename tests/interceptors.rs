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
        interceptors = ["interceptor"]

        [component.interceptor]
        uri = "{}"

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

    // The original client is renamed to _client$0.
    let original_def = common::get_component_definition(&graph, "_client$0");
    assert_eq!(
        original_def.uri,
        client_wasm.to_path_buf().to_string_lossy()
    );

    // The outermost (only) interceptor takes over the "client" name.
    let interceptor_def = common::get_component_definition(&graph, "client");
    assert_eq!(
        interceptor_def.uri,
        interceptor_wasm.to_path_buf().to_string_lossy()
    );

    let handler_def = common::get_component_definition(&graph, "handler");
    assert_eq!(handler_def.imports, vec!["client"]);

    // Handler's dependency resolves to the outermost interceptor (named "client").
    let handler_index = graph.get_node_index("handler").unwrap();
    let dependencies: Vec<_> = graph.get_dependencies(handler_index).collect();
    assert_eq!(dependencies.len(), 1);

    let (dep_index, _) = dependencies[0];
    let dep_name =
        if let composable_runtime::composition::graph::Node::Component(def) = &graph[dep_index] {
            &def.name
        } else {
            unreachable!()
        };
    assert_eq!(dep_name, "client");

    let (component_registry, _capability_registry) =
        common::build_registries_and_assert_ok(&graph).await;

    // Public components: client (outermost interceptor), handler.
    // _client$0 is internal, interceptor template is excluded.
    assert_eq!(component_registry.get_components().count(), 2);
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

    // Call order: outer first, inner second (inner wraps client directly).
    let toml_content = format!(
        r#"
        [component.client]
        uri = "{}"
        interceptors = ["outer-interceptor", "inner-interceptor"]

        [component.outer-interceptor]
        uri = "{}"

        [component.inner-interceptor]
        uri = "{}"

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

    // Handler depends on the outermost interceptor, which has taken over "client".
    let handler_index = graph.get_node_index("handler").unwrap();
    let dependencies: Vec<_> = graph.get_dependencies(handler_index).collect();

    assert_eq!(
        dependencies.len(),
        1,
        "The handler should have one dependency"
    );

    let (dep_index, _) = dependencies[0];
    let provider_name =
        if let composable_runtime::composition::graph::Node::Component(def) = &graph[dep_index] {
            &def.name
        } else {
            unreachable!()
        };

    assert_eq!(
        provider_name, "client",
        "Handler should be connected to 'client' (outermost interceptor), but was connected to '{}'",
        provider_name
    );

    // Outermost interceptor ("client") depends on inner interceptor ("_client$1").
    let outer_index = graph.get_node_index("client").unwrap();
    let outer_deps: Vec<_> = graph.get_dependencies(outer_index).collect();
    assert_eq!(outer_deps.len(), 1);

    let (outer_dep_index, _) = outer_deps[0];
    let outer_dep_name = if let composable_runtime::composition::graph::Node::Component(def) =
        &graph[outer_dep_index]
    {
        &def.name
    } else {
        unreachable!()
    };
    assert_eq!(outer_dep_name, "_client$1");

    // Inner interceptor ("_client$1") depends on original ("_client$0").
    let inner_index = graph.get_node_index("_client$1").unwrap();
    let inner_deps: Vec<_> = graph.get_dependencies(inner_index).collect();
    assert_eq!(inner_deps.len(), 1);

    let (inner_dep_index, _) = inner_deps[0];
    let inner_dep_name = if let composable_runtime::composition::graph::Node::Component(def) =
        &graph[inner_dep_index]
    {
        &def.name
    } else {
        unreachable!()
    };
    assert_eq!(inner_dep_name, "_client$0");

    let (component_registry, _capability_registry) =
        common::build_registries_and_assert_ok(&graph).await;
    // Public components: client (outermost interceptor), handler.
    // _client$0 and _client$1 are internal, two templates excluded.
    assert_eq!(component_registry.get_components().count(), 2);
}
