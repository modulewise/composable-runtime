mod common;
use composable_runtime::ComponentGraph;

fn build_graph(toml_content: &str) -> anyhow::Result<ComponentGraph> {
    let toml_file = common::create_toml_test_file(toml_content);
    ComponentGraph::builder()
        .from_path(toml_file.to_path_buf())
        .build()
}

// Unconnected components build in reverse alphabetical order, which would put
// `referencer` before `referencee`. Only the `${wit(...)}` edge reverses that.
#[tokio::test]
async fn component_is_built_before_a_component_that_references_it() {
    let client_wasm = common::client_wasm();
    let configurable_wasm = common::configurable_wasm();
    let toml_content = format!(
        r#"
        [component.referencer]
        uri = "{}"
        config.target = "${{wit(referencee)}}"

        [component.referencee]
        uri = "{}"
        "#,
        configurable_wasm.display(),
        client_wasm.display()
    );
    let toml_file = common::create_toml_test_file(&toml_content);
    let graph = common::load_graph_and_assert_ok(&[toml_file.to_path_buf()]);

    let order = graph.get_build_order();
    let position = |name: &str| {
        let index = graph.get_node_index(name).expect("node");
        order.iter().position(|i| *i == index).expect("ordered")
    };
    assert!(position("referencee") < position("referencer"));

    // The reference orders the build without composing or depending.
    let (component_registry, _capability_registry) =
        common::build_registries_and_assert_ok(&graph).await;
    let referencee = component_registry
        .get_component("referencee")
        .expect("referencee registered");
    assert!(
        referencee.dependents.is_empty(),
        "{:?}",
        referencee.dependents
    );
}

#[test]
fn an_undefined_component_is_rejected() {
    let configurable_wasm = common::configurable_wasm();
    let err = build_graph(&format!(
        r#"
        [component.referencer]
        uri = "{}"
        config.target = "${{wit(missing)}}"
        "#,
        configurable_wasm.display()
    ))
    .expect_err("undefined component must be rejected");
    assert!(err.to_string().contains("missing"), "{err}");
}

#[test]
fn wit_reference_in_capability_is_rejected() {
    let client_wasm = common::client_wasm();
    let err = build_graph(&format!(
        r#"
        [component.client]
        uri = "{}"

        [capability.invalid]
        type = "test"
        target = "${{wit(client)}}"
        "#,
        client_wasm.display()
    ))
    .expect_err("capability with wit reference must be rejected");
    assert!(
        err.to_string().contains("only valid in component config"),
        "{err}"
    );
}

#[test]
fn self_reference_is_rejected() {
    let configurable_wasm = common::configurable_wasm();
    let err = build_graph(&format!(
        r#"
        [component.foo]
        uri = "{}"
        config.target = "${{wit(foo)}}"
        "#,
        configurable_wasm.display()
    ))
    .expect_err("self-reference must be rejected");
    assert!(err.to_string().contains("Circular dependency"), "{err}");
}

#[test]
fn circular_reference_is_rejected() {
    let configurable_wasm = common::configurable_wasm();
    let err = build_graph(&format!(
        r#"
        [component.foo]
        uri = "{}"
        config.target = "${{wit(bar)}}"
        [component.bar]
        uri = "{}"
        imports = ["foo"]
        "#,
        configurable_wasm.display(),
        configurable_wasm.display()
    ))
    .expect_err("circular reference must be rejected");
    assert!(err.to_string().contains("Circular dependency"), "{err}");
}

// The interceptor's config is on the clone that wraps `service` and takes its
// name. Without the `${wit(...)}` edge, `schema` would build after that clone.
#[test]
fn component_is_built_before_an_interceptor_that_references_it() {
    let client_wasm = common::client_wasm();
    let interceptor_wasm = common::interceptor_wasm();
    let graph = build_graph(&format!(
        r#"
        [component.service]
        uri = "{}"
        interceptors = ["advice"]

        [component.advice]
        uri = "{}"
        config.target = "${{wit(schema)}}"

        [component.schema]
        uri = "{}"
        "#,
        client_wasm.display(),
        interceptor_wasm.display(),
        client_wasm.display()
    ))
    .expect("graph");

    let order = graph.get_build_order();
    let position = |name: &str| {
        let index = graph.get_node_index(name).expect("node");
        order.iter().position(|i| *i == index).expect("ordered")
    };
    assert!(position("schema") < position("service"));
}
