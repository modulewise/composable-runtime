mod common;

#[tokio::test]
async fn test_simple_interceptor() {
    let client_wasm = common::client_wasm();
    let interceptor_wasm = common::interceptor_wasm();
    let handler_wasm = common::handler_wasm();

    let toml_content = format!(
        r#"
        [client]
        uri = "{}"
        enables = "unexposed"

        [interceptor]
        uri = "{}"
        intercepts = ["client"]
        enables = "exposed"

        [handler]
        uri = "{}"
        expects = ["client"]
        exposed = true
        "#,
        client_wasm.display(),
        interceptor_wasm.display(),
        handler_wasm.display()
    );

    let toml_file = common::create_toml_test_file(&toml_content);
    let graph = common::load_graph_and_assert_ok(&[toml_file.to_path_buf()]);

    let client_def = common::get_component_definition(&graph, "client");
    assert_eq!(client_def.uri, client_wasm.to_path_buf().to_string_lossy());
    assert_eq!(client_def.expects.len(), 0);
    assert_eq!(client_def.enables, "unexposed");
    assert_eq!(client_def.exposed, false);

    let interceptor_def = common::get_component_definition(&graph, "interceptor");
    assert_eq!(
        interceptor_def.uri,
        interceptor_wasm.to_path_buf().to_string_lossy()
    );
    assert_eq!(interceptor_def.expects.len(), 0);
    assert_eq!(interceptor_def.enables, "exposed");
    assert_eq!(client_def.exposed, false);

    let handler_def = common::get_component_definition(&graph, "handler");
    assert_eq!(
        handler_def.uri,
        handler_wasm.to_path_buf().to_string_lossy()
    );
    assert_eq!(handler_def.expects, vec!["client"]);
    assert_eq!(handler_def.enables, "none");
    assert!(handler_def.exposed);

    let (_runtime_feature_registry, component_registry) =
        common::build_registries_and_assert_ok(&graph).await;

    assert_eq!(component_registry.get_components().count(), 1);
    let component = component_registry.get_components().next().unwrap();
    assert_eq!(component.name, "handler");
    assert_eq!(component.imports.len(), 0);
    assert_eq!(component.exports, vec!["modulewise:test/handler@0.1.0"]);
    assert_eq!(component.runtime_features.len(), 0);
    let functions = component.functions.clone().unwrap();
    assert_eq!(functions.len(), 1);
    let function = functions.get("handle").unwrap();
    assert_eq!(
        function.params(),
        Vec::<composable_runtime::wit::FunctionParam>::new()
    );
    assert_eq!(function.result(), None);
    assert_eq!(
        function.interface().as_str(),
        "modulewise:test/handler@0.1.0"
    );
}

#[tokio::test]
async fn test_interceptor_with_enables_scope_mismatch() {
    let client_wasm = common::client_wasm();
    let interceptor_wasm = common::interceptor_wasm();
    let handler_wasm = common::handler_wasm();

    let toml_content = format!(
        r#"
        [client]
        uri = "{}"
        enables = "any"

        [interceptor]
        uri = "{}"
        intercepts = ["client"]
        enables = "unexposed"

        [handler]
        uri = "{}"
        expects = ["client"]
        exposed = true
        "#,
        client_wasm.display(),
        interceptor_wasm.display(),
        handler_wasm.display()
    );
    let toml_file = common::create_toml_test_file(&toml_content);
    let graph = common::load_graph_and_assert_ok(&[toml_file.to_path_buf()]);

    let handler_index = graph.get_node_index("handler").unwrap();
    let dependencies: Vec<_> = graph.get_dependencies(handler_index).collect();

    assert_eq!(dependencies.len(), 1, "Handler should have one dependency");

    let provider_index = dependencies[0];
    let provider_node = &graph[provider_index];
    let provider_name = if let composable_runtime::graph::Node::Component(def) = provider_node {
        &def.name
    } else {
        panic!("Dependency provider was not a component");
    };

    // Assert that the dependency is "client", NOT "interceptor".
    assert_eq!(
        provider_name, "client",
        "Handler should be connected to 'client', but was connected to '{}'",
        provider_name
    );

    let (_runtime_feature_registry, component_registry) =
        common::build_registries_and_assert_ok(&graph).await;
    assert_eq!(component_registry.get_components().count(), 1);
}

#[tokio::test]
async fn test_multiple_interceptors() {
    let client_wasm = common::client_wasm();
    let interceptor_wasm = common::interceptor_wasm();
    let handler_wasm = common::handler_wasm();

    let toml_content = format!(
        r#"
        [client]
        uri = "{}"
        enables = "unexposed"

        [outer-interceptor]
        uri = "{}"
        intercepts = ["client"]
        enables = "any"
        precedence = 99

        [inner-interceptor]
        uri = "{}"
        intercepts = ["client"]
        enables = "any"
        precedence = 1

        [handler]
        uri = "{}"
        expects = ["client"]
        exposed = true
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
        if let composable_runtime::graph::Node::Component(def) = &graph[dependencies[0]] {
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

    let outer_provider_name =
        if let composable_runtime::graph::Node::Component(def) = &graph[outer_dependencies[0]] {
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

    let inner_provider_name =
        if let composable_runtime::graph::Node::Component(def) = &graph[inner_dependencies[0]] {
            &def.name
        } else {
            unreachable!()
        };
    assert_eq!(inner_provider_name, "client");

    let (_runtime_feature_registry, component_registry) =
        common::build_registries_and_assert_ok(&graph).await;
    assert_eq!(component_registry.get_components().count(), 1);
}
