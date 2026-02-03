#![allow(dead_code)]

use composable_runtime::graph::{ComponentDefinition, Node, RuntimeFeatureDefinition};
use composable_runtime::registry::{ComponentRegistry, RuntimeFeatureRegistry, build_registries};
use composable_runtime::{ComponentGraph, load_definitions};
use std::collections::HashMap;
use std::io::Write;
use std::ops::Deref;
use std::path::Path;
use std::path::PathBuf;
use tempfile::{Builder, NamedTempFile};

pub struct TestFile(NamedTempFile);

impl Deref for TestFile {
    type Target = Path;

    fn deref(&self) -> &Self::Target {
        self.0.path()
    }
}

pub fn create_toml_test_file(content: &str) -> TestFile {
    let mut temp_file = Builder::new().suffix(".toml").tempfile().unwrap();
    write!(temp_file, "{}", content).unwrap();
    TestFile(temp_file)
}

pub fn create_wasm_test_file(content: &str) -> TestFile {
    let component_bytes = wat::parse_str(content).unwrap();
    let mut temp_file = Builder::new().suffix(".wasm").tempfile().unwrap();
    temp_file.write_all(&component_bytes).unwrap();
    TestFile(temp_file)
}

pub fn client_wasm() -> TestFile {
    let wat = r#"
        (component
            (core module $m
                (func (export "query"))
            )
            (core instance $i (instantiate $m))
            (func $f (canon lift (core func $i "query")))
            (instance $client (export "query" (func $f)))
            (export "modulewise:test/client@0.1.0" (instance $client))
        )
    "#;
    create_wasm_test_file(wat)
}

pub fn handler_wasm() -> TestFile {
    let wat = r#"
        (component
            (import "modulewise:test/client@0.1.0" (instance $client
                (export "query" (func))
            ))
            (core func $query (canon lower (func $client "query")))
            (core module $handler_module
                (import "" "query" (func $client_import))
                (func (export "handle") (call $client_import))
            )
            (core instance $handler_instance (instantiate $handler_module
                (with "" (instance (export "query" (func $query))))
            ))
            (func $handle_lifted (canon lift (core func $handler_instance "handle")))
            (instance $handler (export "handle" (func $handle_lifted)))
            (export "modulewise:test/handler@0.1.0" (instance $handler))
        )
    "#;
    create_wasm_test_file(wat)
}

pub fn interceptor_wasm() -> TestFile {
    let wat = r#"
        (component
            (import "modulewise:test/client@0.1.0" (instance $client
                (export "query" (func))
            ))
            (core func $query (canon lower (func $client "query")))
            (core module $interceptor_module
                (import "" "query" (func $client_import))
                (func (export "query") (call $client_import))
            )
            (core instance $interceptor_instance (instantiate $interceptor_module
                (with "" (instance (export "query" (func $query))))
            ))
            (func $query_lifted (canon lift (core func $interceptor_instance "query")))
            (instance $interceptor (export "query" (func $query_lifted)))
            (export "modulewise:test/client@0.1.0" (instance $interceptor))
        )
    "#;
    create_wasm_test_file(wat)
}

pub fn configurable_wasm() -> TestFile {
    let wat = r#"
        (component
            (import "wasi:config/store@0.2.0-rc.1" (instance))
        )
    "#;
    create_wasm_test_file(wat)
}

pub fn get_component_definition<'a>(
    graph: &'a ComponentGraph,
    name: &str,
) -> &'a ComponentDefinition {
    let index = graph
        .get_node_index(name)
        .unwrap_or_else(|| panic!("Node '{}' not found in graph", name));

    if let Node::Component(def) = &graph[index] {
        def
    } else {
        panic!("Node '{}' is not a ComponentDefinition", name);
    }
}

pub fn get_runtime_feature_definition<'a>(
    graph: &'a ComponentGraph,
    name: &str,
) -> &'a RuntimeFeatureDefinition {
    let index = graph
        .get_node_index(name)
        .unwrap_or_else(|| panic!("Node '{}' not found in graph", name));

    if let Node::RuntimeFeature(def) = &graph[index] {
        def
    } else {
        panic!("Node '{}' is not a RuntimeFeatureDefinition", name);
    }
}

pub async fn build_registries_and_assert_ok(
    graph: &ComponentGraph,
) -> (RuntimeFeatureRegistry, ComponentRegistry) {
    let registries_result = build_registries(graph, HashMap::new()).await;
    assert!(
        registries_result.is_ok(),
        "build_registries failed with: {:?}",
        registries_result.err()
    );
    registries_result.unwrap()
}

pub fn load_graph_and_assert_ok(paths: &[PathBuf]) -> ComponentGraph {
    let graph_result = load_definitions(paths);
    assert!(
        graph_result.is_ok(),
        "load_definitions failed with: {:?}",
        graph_result.err()
    );
    graph_result.unwrap()
}
