mod common;

use composable_runtime::Runtime;

const VALUE: &str = "modulewise:test/value@0.1.0";
const COORDINATE: &str = "modulewise:test/coordinate@0.1.0";

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

// Exports `modulewise:test/coordinate`, an interface with a type: `get` returns
// the record `coordinate` with `x` set to `x`.
fn coordinate_wasm(x: u32) -> common::TestFile {
    common::create_wasm_test_file(&format!(
        r#"
        (component
            (type $coordinate (record (field "x" u32)))
            (core module $m (func (export "get") (result i32) i32.const {x}))
            (core instance $i (instantiate $m))
            (func $get (result $coordinate) (canon lift (core func $i "get")))
            (instance $instance (export "coordinate" (type $coordinate)) (export "get" (func $get)))
            (export "{COORDINATE}" (instance $instance))
        )
        "#
    ))
}

// Imports `modulewise:test/coordinate` as `a`, `b` and `c`, and exports `run`
// returning `a.get().x * 100 + b.get().x * 10 + c.get().x`.
fn coordinate_imports_wasm() -> common::TestFile {
    let import = |name: &str| {
        format!(
            r#"(import "{name}" (implements "{COORDINATE}") (instance ${name}
                (type $coordinate (record (field "x" u32)))
                (export "coordinate" (type $exported (eq $coordinate)))
                (export "get" (func (result $exported)))
            ))"#
        )
    };
    common::create_wasm_test_file(&format!(
        r#"
        (component
            {a}
            {b}
            {c}
            (core func $a-get (canon lower (func $a "get")))
            (core func $b-get (canon lower (func $b "get")))
            (core func $c-get (canon lower (func $c "get")))
            (core module $m
                (import "" "a" (func $a (result i32)))
                (import "" "b" (func $b (result i32)))
                (import "" "c" (func $c (result i32)))
                (func (export "run") (result i32)
                    (i32.add
                        (i32.add (i32.mul (call $a) (i32.const 100)) (i32.mul (call $b) (i32.const 10)))
                        (call $c)
                    )
                )
            )
            (core instance $i (instantiate $m
                (with "" (instance
                    (export "a" (func $a-get))
                    (export "b" (func $b-get))
                    (export "c" (func $c-get))
                ))
            ))
            (func $run (result u32) (canon lift (core func $i "run")))
            (export "run" (func $run))
        )
        "#,
        a = import("a"),
        b = import("b"),
        c = import("c"),
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

// Exports `modulewise:test/value` unnamed, returning 5, and as `name`,
// returning `value`.
fn unnamed_and_named_export_wasm(name: &str, value: u32) -> common::TestFile {
    common::create_wasm_test_file(&format!(
        r#"
        (component
            (core module $m
                (func (export "unnamed") (result i32) i32.const 5)
                (func (export "named") (result i32) i32.const {value})
            )
            (core instance $i (instantiate $m))
            (func $unnamed-get (result u32) (canon lift (core func $i "unnamed")))
            (func $named-get (result u32) (canon lift (core func $i "named")))
            (instance $unnamed (export "get" (func $unnamed-get)))
            (instance $named (export "get" (func $named-get)))
            (export "{VALUE}" (instance $unnamed))
            (export "{name}" (implements "{VALUE}") (instance $named))
        )
        "#
    ))
}

// Imports `modulewise:test/value` unnamed and exports `run` returning `get()`.
fn value_import_wasm() -> common::TestFile {
    common::create_wasm_test_file(&format!(
        r#"
        (component
            (import "{VALUE}" (instance $value
                (export "get" (func (result u32)))
            ))
            (core func $get (canon lower (func $value "get")))
            (core module $m
                (import "" "get" (func $get (result i32)))
                (func (export "run") (result i32) (call $get))
            )
            (core instance $i (instantiate $m
                (with "" (instance (export "get" (func $get))))
            ))
            (func $run (result u32) (canon lift (core func $i "run")))
            (export "run" (func $run))
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
async fn named_imports_of_an_interface_with_a_type_are_each_satisfied_by_their_own_dependency() {
    let (a, b, c) = (coordinate_wasm(1), coordinate_wasm(2), coordinate_wasm(3));
    let consumer = coordinate_imports_wasm();
    let runtime = runtime(&format!(
        r#"
        [component.a]
        uri = "{}"

        [component.b]
        uri = "{}"

        [component.c]
        uri = "{}"

        [component.consumer]
        uri = "{}"
        imports = ["a", "b", "c"]
        "#,
        a.display(),
        b.display(),
        c.display(),
        consumer.display()
    ))
    .await
    .expect("runtime");

    assert_eq!(invoke(&runtime, "consumer", "run").await, 123);
}

#[tokio::test]
async fn named_import_without_a_dependency_of_its_name_is_unsatisfied() {
    let first = value_wasm(1);
    let consumer = named_imports_wasm();
    let err = runtime(&format!(
        r#"
        [component.first]
        uri = "{}"

        [component.consumer]
        uri = "{}"
        imports = ["first"]
        "#,
        first.display(),
        consumer.display()
    ))
    .await
    .err()
    .expect("a named import with no dependency of its name must be unsatisfied");

    let message = format!("{err:#}");
    assert!(message.contains("unsatisfied imports"), "{message}");
    assert!(message.contains(&format!("second: {VALUE}")), "{message}");
}

#[tokio::test]
async fn dependency_of_another_name_does_not_satisfy_a_named_import() {
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

    let message = format!("{err:#}");
    assert!(message.contains("dependency 'other'"), "{message}");
    assert!(message.contains("no import is satisfied"), "{message}");
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
async fn named_export_takes_precedence_over_the_component_name_fallback() {
    let first = unnamed_and_named_export_wasm("first", 1);
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

#[tokio::test]
async fn named_export_takes_precedence_over_the_dependency_of_the_import_name() {
    // `exporter` satisfies `first` with its named export, and `first` could
    // too, as the dependency of that name. `first` also satisfies `second`
    // with its named export, so neither is left over.
    let exporter = unnamed_and_named_export_wasm("first", 1);
    let first = unnamed_and_named_export_wasm("second", 2);
    let consumer = named_imports_wasm();
    for imports in [r#"["exporter", "first"]"#, r#"["first", "exporter"]"#] {
        let runtime = runtime(&format!(
            r#"
            [component.exporter]
            uri = "{}"

            [component.first]
            uri = "{}"

            [component.consumer]
            uri = "{}"
            imports = {imports}
            "#,
            exporter.display(),
            first.display(),
            consumer.display()
        ))
        .await
        .expect("runtime");

        // `first` is the named export of `exporter` (1), not the unnamed
        // export of `first` (5), whichever order the imports are listed in.
        assert_eq!(invoke(&runtime, "consumer", "run").await, 12, "{imports}");
    }
}

#[tokio::test]
async fn import_satisfied_by_more_than_one_dependency_is_rejected() {
    let first = value_wasm(1);
    let second = value_wasm(2);
    let consumer = value_import_wasm();
    let err = runtime(&format!(
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
    .err()
    .expect("an import that two dependencies satisfy must be rejected");

    let message = format!("{err:#}");
    assert!(message.contains("more than one dependency"), "{message}");
    assert!(message.contains("\"first\""), "{message}");
    assert!(message.contains("\"second\""), "{message}");
}

#[tokio::test]
async fn named_import_satisfied_by_named_exports_of_more_than_one_dependency_is_rejected() {
    let one = named_exports_wasm();
    let other = named_exports_wasm();
    let consumer = named_imports_wasm();
    let err = runtime(&format!(
        r#"
        [component.one]
        uri = "{}"

        [component.other]
        uri = "{}"

        [component.consumer]
        uri = "{}"
        imports = ["one", "other"]
        "#,
        one.display(),
        other.display(),
        consumer.display()
    ))
    .await
    .err()
    .expect("a named import that two named exports satisfy must be rejected");

    let message = format!("{err:#}");
    assert!(message.contains("more than one dependency"), "{message}");
    assert!(message.contains("\"one\""), "{message}");
    assert!(message.contains("\"other\""), "{message}");
}
