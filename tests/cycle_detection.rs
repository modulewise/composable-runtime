mod common;
use composable_runtime::ComponentGraph;

#[test]
#[should_panic(expected = "Circular dependency detected")]
fn test_circular_dependency() {
    let simple_component = "(component)";
    let a_wasm = common::create_wasm_test_file(simple_component);
    let b_wasm = common::create_wasm_test_file(simple_component);

    let toml_content = format!(
        r#"
        [component-a]
        uri = "{}"
        expects = ["component-b"]

        [component-b]
        uri = "{}"
        expects = ["component-a"]
        "#,
        a_wasm.display(),
        b_wasm.display()
    );

    let toml_file = common::create_toml_test_file(&toml_content);
    ComponentGraph::builder()
        .load_file(toml_file.to_path_buf())
        .build()
        .unwrap();
}
