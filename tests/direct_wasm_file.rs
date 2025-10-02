use composable_runtime::graph::Node;

mod common;

#[tokio::test]
async fn test_direct_wasm_file() {
    let client_wasm = common::client_wasm();

    let graph = common::load_graph_and_assert_ok(&[client_wasm.to_path_buf()]);
    assert_eq!(graph.nodes().count(), 1);
    let node = graph.nodes().next().unwrap();

    if let Node::Component(def) = &node.weight {
        assert_eq!(def.uri, client_wasm.to_path_buf().to_string_lossy());
        assert_eq!(def.expects.len(), 0);
        assert_eq!(def.enables, "none");
        assert!(def.exposed);
    } else {
        panic!("Node was not a component");
    }

    let (_runtime_feature_registry, component_registry) =
        common::build_registries_and_assert_ok(&graph).await;

    assert_eq!(component_registry.get_components().count(), 1);
    let component = component_registry.get_components().next().unwrap();
    assert_eq!(
        component.name,
        client_wasm.file_stem().unwrap().to_string_lossy()
    );
    assert_eq!(component.imports.len(), 0);
    assert_eq!(component.exports, vec!["modulewise:test/client@0.1.0"]);
    assert_eq!(component.runtime_features.len(), 0);
    let functions = component.functions.clone().unwrap();
    assert_eq!(functions.len(), 1);
    let function = functions.get("query").unwrap();
    assert_eq!(
        function.params(),
        Vec::<composable_runtime::FunctionParam>::new()
    );
    assert_eq!(function.result(), None);
    assert_eq!(
        function.interface().as_str(),
        "modulewise:test/client@0.1.0"
    );
}
