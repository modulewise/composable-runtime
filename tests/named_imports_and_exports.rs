mod common;

use composable_runtime::Runtime;

const VALUE: &str = "modulewise:test/value@0.1.0";

// Exports `modulewise:test/value` with `get` returning `value`.
fn value_wasm(value: u32) -> common::TestFile {
    common::create_wasm_test_file(&format!(
        r#"
        (component
            (core module $m (func (export "get") (result i32) i32.const {value}))
            (core instance $i (instantiate $m))
            (func $get (result u32) (canon lift (core func $i "get")))
            (instance $value (export "get" (func $get)))
            (export "{VALUE}" (instance $value))
        )
        "#
    ))
}

// Imports `first` and `second`, both `modulewise:test/value`, and exports
// `run` returning `first.get() * 10 + second.get()`.
fn named_imports_wasm() -> common::TestFile {
    common::create_wasm_test_file(&format!(
        r#"
        (component
            (import "first" (implements "{VALUE}") (instance $first
                (export "get" (func (result u32)))
            ))
            (import "second" (implements "{VALUE}") (instance $second
                (export "get" (func (result u32)))
            ))
            (core func $first-get (canon lower (func $first "get")))
            (core func $second-get (canon lower (func $second "get")))
            (core module $m
                (import "" "first" (func $first (result i32)))
                (import "" "second" (func $second (result i32)))
                (func (export "run") (result i32)
                    (i32.add (i32.mul (call $first) (i32.const 10)) (call $second))
                )
            )
            (core instance $i (instantiate $m
                (with "" (instance
                    (export "first" (func $first-get))
                    (export "second" (func $second-get))
                ))
            ))
            (func $run (result u32) (canon lift (core func $i "run")))
            (export "run" (func $run))
        )
        "#
    ))
}

// Exports `modulewise:test/value` twice, as `first` returning 1 and `second`
// returning 2.
fn named_exports_wasm() -> common::TestFile {
    common::create_wasm_test_file(&format!(
        r#"
        (component
            (core module $m
                (func (export "first") (result i32) i32.const 1)
                (func (export "second") (result i32) i32.const 2)
            )
            (core instance $i (instantiate $m))
            (func $first-get (result u32) (canon lift (core func $i "first")))
            (func $second-get (result u32) (canon lift (core func $i "second")))
            (instance $first (export "get" (func $first-get)))
            (instance $second (export "get" (func $second-get)))
            (export "first" (implements "{VALUE}") (instance $first))
            (export "second" (implements "{VALUE}") (instance $second))
        )
        "#
    ))
}

// Imports the direct function `get` and exports `run` returning `get() + 100`.
fn function_import_wasm() -> common::TestFile {
    common::create_wasm_test_file(
        r#"
        (component
            (import "get" (func $get (result u32)))
            (core func $get-lowered (canon lower (func $get)))
            (core module $m
                (import "" "get" (func $get (result i32)))
                (func (export "run") (result i32) (i32.add (call $get) (i32.const 100)))
            )
            (core instance $i (instantiate $m
                (with "" (instance (export "get" (func $get-lowered))))
            ))
            (func $run (result u32) (canon lift (core func $i "run")))
            (export "run" (func $run))
        )
        "#,
    )
}

// Exports the direct function `get` returning 7.
fn function_export_wasm() -> common::TestFile {
    common::create_wasm_test_file(
        r#"
        (component
            (core module $m (func (export "get") (result i32) i32.const 7))
            (core instance $i (instantiate $m))
            (func $get (result u32) (canon lift (core func $i "get")))
            (export "get" (func $get))
        )
        "#,
    )
}

// Imports the inline interface `helper` and exports `run` returning
// `helper.get() + 200`.
fn inline_import_wasm() -> common::TestFile {
    common::create_wasm_test_file(
        r#"
        (component
            (import "helper" (instance $helper
                (export "get" (func (result u32)))
            ))
            (core func $get (canon lower (func $helper "get")))
            (core module $m
                (import "" "get" (func $get (result i32)))
                (func (export "run") (result i32) (i32.add (call $get) (i32.const 200)))
            )
            (core instance $i (instantiate $m
                (with "" (instance (export "get" (func $get))))
            ))
            (func $run (result u32) (canon lift (core func $i "run")))
            (export "run" (func $run))
        )
        "#,
    )
}

// Exports the inline interface `helper` with `get` returning 9.
fn inline_export_wasm() -> common::TestFile {
    common::create_wasm_test_file(
        r#"
        (component
            (core module $m (func (export "get") (result i32) i32.const 9))
            (core instance $i (instantiate $m))
            (func $get (result u32) (canon lift (core func $i "get")))
            (instance $helper (export "get" (func $get)))
            (export "helper" (instance $helper))
        )
        "#,
    )
}

// Exports `modulewise:test/value` unnamed, returning 5, and as `first`,
// returning 1.
fn unnamed_and_named_export_wasm() -> common::TestFile {
    common::create_wasm_test_file(&format!(
        r#"
        (component
            (core module $m
                (func (export "unnamed") (result i32) i32.const 5)
                (func (export "first") (result i32) i32.const 1)
            )
            (core instance $i (instantiate $m))
            (func $unnamed-get (result u32) (canon lift (core func $i "unnamed")))
            (func $first-get (result u32) (canon lift (core func $i "first")))
            (instance $unnamed (export "get" (func $unnamed-get)))
            (instance $first (export "get" (func $first-get)))
            (export "{VALUE}" (instance $unnamed))
            (export "first" (implements "{VALUE}") (instance $first))
        )
        "#
    ))
}

async fn runtime(toml: &str) -> anyhow::Result<Runtime> {
    let toml_file = common::create_toml_test_file(toml);
    Runtime::builder().from_path(&*toml_file).build().await
}

async fn invoke(runtime: &Runtime, component: &str, function: &str) -> serde_json::Value {
    runtime
        .host()
        .invoke(component, function, vec![], None)
        .await
        .expect("invoke")
        .expect("a result")
        .into_json()
        .expect("JSON")
}

#[tokio::test]
async fn named_imports_are_satisfied_by_the_dependencies_with_their_names() {
    let first = value_wasm(1);
    let second = value_wasm(2);
    let consumer = named_imports_wasm();
    let runtime = runtime(&format!(
        r#"
        [component.first]
        uri = "{}"

        [component.second]
        uri = "{}"

        [component.consumer]
        uri = "{}"
        imports = ["second", "first"]
        "#,
        first.display(),
        second.display(),
        consumer.display()
    ))
    .await
    .expect("runtime");

    assert_eq!(invoke(&runtime, "consumer", "run").await, 12);
}

#[tokio::test]
async fn named_import_without_a_dependency_of_its_name_is_unsatisfied() {
    let first = value_wasm(1);
    let other = value_wasm(2);
    let consumer = named_imports_wasm();
    let err = runtime(&format!(
        r#"
        [component.first]
        uri = "{}"

        [component.other]
        uri = "{}"

        [component.consumer]
        uri = "{}"
        imports = ["first", "other"]
        "#,
        first.display(),
        other.display(),
        consumer.display()
    ))
    .await
    .err()
    .expect("a named import must not be satisfied by a dependency of another name");

    assert!(format!("{err:#}").contains("other"), "{err:#}");
}

#[tokio::test]
async fn named_exports_are_invoked_by_their_names() {
    let component = named_exports_wasm();
    let runtime = runtime(&format!(
        r#"
        [component.values]
        uri = "{}"
        "#,
        component.display()
    ))
    .await
    .expect("runtime");

    let exports: Vec<_> = runtime
        .get_component("values")
        .expect("registered")
        .metadata
        .exports
        .iter()
        .map(|export| export.to_string())
        .collect();
    assert_eq!(
        exports,
        [format!("first: {VALUE}"), format!("second: {VALUE}")]
    );

    assert_eq!(invoke(&runtime, "values", "first.get").await, 1);
    assert_eq!(invoke(&runtime, "values", "second.get").await, 2);
}

#[tokio::test]
async fn function_import_is_satisfied_by_a_function_export_of_its_name() {
    let provider = function_export_wasm();
    let consumer = function_import_wasm();
    let runtime = runtime(&format!(
        r#"
        [component.provider]
        uri = "{}"

        [component.consumer]
        uri = "{}"
        imports = ["provider"]
        "#,
        provider.display(),
        consumer.display()
    ))
    .await
    .expect("runtime");

    assert_eq!(invoke(&runtime, "consumer", "run").await, 107);
}

#[tokio::test]
async fn unsatisfied_function_import_is_rejected() {
    let consumer = function_import_wasm();
    let err = runtime(&format!(
        r#"
        [component.consumer]
        uri = "{}"
        "#,
        consumer.display()
    ))
    .await
    .err()
    .expect("an unsatisfied function import must be rejected");

    assert!(
        format!("{err:#}").contains("unsatisfied imports"),
        "{err:#}"
    );
}

#[tokio::test]
async fn inline_interface_import_is_satisfied_by_an_inline_interface_export_of_its_name() {
    let provider = inline_export_wasm();
    let consumer = inline_import_wasm();
    let runtime = runtime(&format!(
        r#"
        [component.provider]
        uri = "{}"

        [component.consumer]
        uri = "{}"
        imports = ["provider"]
        "#,
        provider.display(),
        consumer.display()
    ))
    .await
    .expect("runtime");

    assert_eq!(invoke(&runtime, "consumer", "run").await, 209);
}

#[tokio::test]
async fn named_imports_are_satisfied_by_named_exports_of_the_same_names() {
    let values = named_exports_wasm();
    let consumer = named_imports_wasm();
    let runtime = runtime(&format!(
        r#"
        [component.values]
        uri = "{}"

        [component.consumer]
        uri = "{}"
        imports = ["values"]
        "#,
        values.display(),
        consumer.display()
    ))
    .await
    .expect("runtime");

    assert_eq!(invoke(&runtime, "consumer", "run").await, 12);
}

#[tokio::test]
async fn named_export_takes_precedence_over_the_dependency_name() {
    let first = unnamed_and_named_export_wasm();
    let second = value_wasm(2);
    let consumer = named_imports_wasm();
    let runtime = runtime(&format!(
        r#"
        [component.first]
        uri = "{}"

        [component.second]
        uri = "{}"

        [component.consumer]
        uri = "{}"
        imports = ["first", "second"]
        "#,
        first.display(),
        second.display(),
        consumer.display()
    ))
    .await
    .expect("runtime");

    // `first` is the named export (1), not the unnamed one (5).
    assert_eq!(invoke(&runtime, "consumer", "run").await, 12);
}
