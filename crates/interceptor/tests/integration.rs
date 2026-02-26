use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use tempfile::TempDir;
use wit_parser::{Resolve, WorldId, WorldItem};

// ============================================================
// Test helpers
// ============================================================

// Path to the real advice WIT from the project root.
fn project_advice_wit() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("wit/package.wit")
}

// Create a WIT directory structure with the real advice WIT as a dependency.
// Returns a TempDir (must be kept alive for the duration of the test).
fn wit_dir(package_wit: &str) -> TempDir {
    let dir = TempDir::new().unwrap();
    let wit = dir.path().join("wit");
    let deps = wit.join("deps").join("modulewise-interceptor-0.1.0");
    fs::create_dir_all(&deps).unwrap();

    fs::write(wit.join("package.wit"), package_wit).unwrap();
    fs::copy(project_advice_wit(), deps.join("package.wit")).unwrap();

    dir
}

fn wit_path(dir: &TempDir) -> PathBuf {
    dir.path().join("wit")
}

// Decoded structure of a generated interceptor component.
struct DecodedInterceptor {
    resolve: Resolve,
    world_id: WorldId,
}

impl DecodedInterceptor {
    fn from_bytes(bytes: &[u8]) -> Self {
        // Validate first
        wasmparser::Validator::new_with_features(wasmparser::WasmFeatures::all())
            .validate_all(bytes)
            .expect("generated component should be valid");

        let decoded = wit_parser::decoding::decode(bytes).expect("should decode as component");
        match decoded {
            wit_parser::decoding::DecodedWasm::Component(resolve, world_id) => {
                Self { resolve, world_id }
            }
            _ => panic!("expected a component, got a WIT package"),
        }
    }

    fn world(&self) -> &wit_parser::World {
        &self.resolve.worlds[self.world_id]
    }

    // Collect all import names.
    fn import_names(&self) -> HashSet<String> {
        self.world()
            .imports
            .iter()
            .map(|(key, _)| self.world_key_name(key))
            .collect()
    }

    // Collect all export names.
    fn export_names(&self) -> HashSet<String> {
        self.world()
            .exports
            .iter()
            .map(|(key, _)| self.world_key_name(key))
            .collect()
    }

    // Collect exported function names within an exported interface.
    fn exported_interface_funcs(&self, interface_export_name: &str) -> HashSet<String> {
        for (key, item) in &self.world().exports {
            if self.world_key_name(key) == interface_export_name {
                if let WorldItem::Interface { id, .. } = item {
                    return self.resolve.interfaces[*id]
                        .functions
                        .keys()
                        .cloned()
                        .collect();
                }
            }
        }
        panic!("no exported interface named '{interface_export_name}'");
    }

    // Collect type names within an exported interface.
    fn exported_interface_types(&self, interface_export_name: &str) -> HashSet<String> {
        for (key, item) in &self.world().exports {
            if self.world_key_name(key) == interface_export_name {
                if let WorldItem::Interface { id, .. } = item {
                    return self.resolve.interfaces[*id].types.keys().cloned().collect();
                }
            }
        }
        panic!("no exported interface named '{interface_export_name}'");
    }

    // Collect directly exported function names (not in an interface).
    fn direct_export_func_names(&self) -> HashSet<String> {
        self.world()
            .exports
            .iter()
            .filter_map(|(_key, item)| match item {
                WorldItem::Function(f) => Some(f.name.clone()),
                _ => None,
            })
            .collect()
    }

    fn world_key_name(&self, key: &wit_parser::WorldKey) -> String {
        match key {
            wit_parser::WorldKey::Name(n) => n.clone(),
            wit_parser::WorldKey::Interface(id) => {
                let iface = &self.resolve.interfaces[*id];
                let pkg = iface.package.map(|p| &self.resolve.packages[p].name);
                match (pkg, &iface.name) {
                    (Some(pkg), Some(name)) => {
                        let version = pkg
                            .version
                            .as_ref()
                            .map(|v| format!("@{v}"))
                            .unwrap_or_default();
                        format!("{}:{}/{}{}", pkg.namespace, pkg.name, name, version)
                    }
                    (None, Some(name)) => name.clone(),
                    _ => format!("{:?}", id),
                }
            }
        }
    }
}

// Build an interceptor from inline WIT and decode the result.
fn build_and_decode(package_wit: &str, world: &str, patterns: &[&str]) -> DecodedInterceptor {
    let dir = wit_dir(package_wit);
    let bytes = interceptor::create_from_wit(&wit_path(&dir), world, patterns).unwrap();
    DecodedInterceptor::from_bytes(&bytes)
}

fn set(items: &[&str]) -> HashSet<String> {
    items.iter().map(|s| s.to_string()).collect()
}

// ============================================================
// Single interface
// ============================================================

#[test]
fn single_interface_with_primitive_params() {
    let d = build_and_decode(
        r#"
        package test:single@0.1.0;
        interface math {
            add: func(a: s32, b: s32) -> s32;
        }
        world target {
            export math;
        }
        "#,
        "target",
        &[],
    );

    assert_eq!(d.export_names(), set(&["test:single/math@0.1.0"]));
    assert_eq!(
        d.exported_interface_funcs("test:single/math@0.1.0"),
        set(&["add"])
    );
    assert!(d.import_names().contains("test:single/math@0.1.0"));
    assert!(
        d.import_names()
            .contains("modulewise:interceptor/advice@0.1.0")
    );
    assert!(
        d.import_names()
            .contains("modulewise:interceptor/types@0.1.0")
    );
}

#[test]
fn interface_with_string_params() {
    let d = build_and_decode(
        r#"
        package test:strings@0.1.0;
        interface greeter {
            greet: func(name: string) -> string;
        }
        world target {
            export greeter;
        }
        "#,
        "target",
        &[],
    );

    assert_eq!(d.export_names(), set(&["test:strings/greeter@0.1.0"]));
    assert_eq!(
        d.exported_interface_funcs("test:strings/greeter@0.1.0"),
        set(&["greet"])
    );
}

#[test]
fn interface_with_multiple_functions() {
    let d = build_and_decode(
        r#"
        package test:multi@0.1.0;
        interface ops {
            add: func(a: s32, b: s32) -> s32;
            negate: func(x: s32) -> s32;
            name: func() -> string;
        }
        world target {
            export ops;
        }
        "#,
        "target",
        &[],
    );

    assert_eq!(
        d.exported_interface_funcs("test:multi/ops@0.1.0"),
        set(&["add", "negate", "name"])
    );
}

// ============================================================
// Direct function exports
// ============================================================

#[test]
fn direct_function_exports() {
    let d = build_and_decode(
        r#"
        package test:direct@0.1.0;
        world target {
            export add: func(a: s32, b: s32) -> s32;
            export subtract: func(a: s32, b: s32) -> s32;
        }
        "#,
        "target",
        &[],
    );

    assert_eq!(d.direct_export_func_names(), set(&["add", "subtract"]));
    assert!(d.import_names().contains("add"));
    assert!(d.import_names().contains("subtract"));
}

#[test]
fn mixed_interface_and_direct_exports() {
    let d = build_and_decode(
        r#"
        package test:mixed@0.1.0;
        interface math {
            add: func(a: s32, b: s32) -> s32;
        }
        world target {
            export math;
            export double: func(x: s32) -> s32;
        }
        "#,
        "target",
        &[],
    );

    assert!(d.export_names().contains("test:mixed/math@0.1.0"));
    assert_eq!(d.direct_export_func_names(), set(&["double"]));
}

// ============================================================
// Type imports
// ============================================================

#[test]
fn record_params_with_use_types() {
    let d = build_and_decode(
        r#"
        package test:records@0.1.0;
        interface types {
            record point {
                x: f32,
                y: f32,
            }
        }
        interface geometry {
            use types.{point};
            magnitude: func(p: point) -> f32;
            distance: func(a: point, b: point) -> f64;
        }
        world target {
            export geometry;
        }
        "#,
        "target",
        &[],
    );

    assert_eq!(d.export_names(), set(&["test:records/geometry@0.1.0"]));
    assert_eq!(
        d.exported_interface_funcs("test:records/geometry@0.1.0"),
        set(&["magnitude", "distance"])
    );
    // types interface must be imported to provide the `point` type
    assert!(d.import_names().contains("test:records/types@0.1.0"));
    // The decoded geometry interface must include `point` (via `use types.{point}`)
    // so that consumers importing geometry can reference it.
    assert!(
        d.exported_interface_types("test:records/geometry@0.1.0")
            .contains("point"),
        "geometry export must include the `point` type from `use types.{{point}}`"
    );
}

#[test]
fn multiple_interfaces_with_shared_types() {
    let d = build_and_decode(
        r#"
        package test:multi-iface@0.1.0;
        interface types {
            record point {
                x: f32,
                y: f32,
            }
        }
        interface geometry {
            use types.{point};
            magnitude: func(p: point) -> f32;
            distance: func(a: point, b: point) -> f64;
        }
        interface transform {
            use types.{point};
            translate: func(p: point, dx: f32, dy: f32) -> point;
            scale: func(p: point, factor: f64) -> point;
        }
        world target {
            export geometry;
            export transform;
        }
        "#,
        "target",
        &[],
    );

    assert_eq!(
        d.export_names(),
        set(&[
            "test:multi-iface/geometry@0.1.0",
            "test:multi-iface/transform@0.1.0"
        ])
    );
    assert_eq!(
        d.exported_interface_funcs("test:multi-iface/geometry@0.1.0"),
        set(&["magnitude", "distance"])
    );
    assert_eq!(
        d.exported_interface_funcs("test:multi-iface/transform@0.1.0"),
        set(&["translate", "scale"])
    );
    // Both interfaces must include `point` from their `use types.{point}`
    assert!(
        d.exported_interface_types("test:multi-iface/geometry@0.1.0")
            .contains("point")
    );
    assert!(
        d.exported_interface_types("test:multi-iface/transform@0.1.0")
            .contains("point")
    );
    // types interface imported once to provide the shared type
    assert!(d.import_names().contains("test:multi-iface/types@0.1.0"));
}

#[test]
fn direct_function_using_foreign_type() {
    let d = build_and_decode(
        r#"
        package test:direct-types@0.1.0;
        interface types {
            record point {
                x: f32,
                y: f32,
            }
        }
        world target {
            use types.{point};
            export translate: func(p: point, dx: f32, dy: f32) -> point;
        }
        "#,
        "target",
        &[],
    );

    assert_eq!(d.direct_export_func_names(), set(&["translate"]));
    assert!(d.import_names().contains("test:direct-types/types@0.1.0"));
}

// ============================================================
// Void and list params
// ============================================================

#[test]
fn void_function() {
    let d = build_and_decode(
        r#"
        package test:void@0.1.0;
        interface actions {
            reset: func();
            set-value: func(v: u32);
        }
        world target {
            export actions;
        }
        "#,
        "target",
        &[],
    );

    assert_eq!(
        d.exported_interface_funcs("test:void/actions@0.1.0"),
        set(&["reset", "set-value"])
    );
}

#[test]
fn list_params() {
    let d = build_and_decode(
        r#"
        package test:lists@0.1.0;
        interface data {
            sum: func(values: list<s32>) -> s64;
        }
        world target {
            export data;
        }
        "#,
        "target",
        &[],
    );

    assert_eq!(
        d.exported_interface_funcs("test:lists/data@0.1.0"),
        set(&["sum"])
    );
}

// ============================================================
// Pattern matching: selective interception
// ============================================================

#[test]
fn pattern_intercepts_one_function() {
    let d = build_and_decode(
        r#"
        package test:pattern@0.1.0;
        interface ops {
            add: func(a: s32, b: s32) -> s32;
            sub: func(a: s32, b: s32) -> s32;
        }
        world target {
            export ops;
        }
        "#,
        "target",
        &["add"],
    );

    // Both functions must still be exported
    assert_eq!(
        d.exported_interface_funcs("test:pattern/ops@0.1.0"),
        set(&["add", "sub"])
    );
    // Advice must still be imported (add is intercepted)
    assert!(
        d.import_names()
            .contains("modulewise:interceptor/advice@0.1.0")
    );
}

#[test]
fn pattern_intercepts_by_interface() {
    let d = build_and_decode(
        r#"
        package test:iface-pattern@0.1.0;
        interface alpha {
            run: func() -> u32;
        }
        interface beta {
            run: func() -> u32;
        }
        world target {
            export alpha;
            export beta;
        }
        "#,
        "target",
        &["test:iface-pattern/alpha"],
    );

    // Both interfaces must be exported
    assert!(d.export_names().contains("test:iface-pattern/alpha@0.1.0"));
    assert!(d.export_names().contains("test:iface-pattern/beta@0.1.0"));
    // Both must still have their functions
    assert_eq!(
        d.exported_interface_funcs("test:iface-pattern/alpha@0.1.0"),
        set(&["run"])
    );
    assert_eq!(
        d.exported_interface_funcs("test:iface-pattern/beta@0.1.0"),
        set(&["run"])
    );
}

#[test]
fn pattern_glob_on_function_name() {
    let d = build_and_decode(
        r#"
        package test:glob@0.1.0;
        interface svc {
            get-name: func() -> string;
            get-age: func() -> u32;
            set-name: func(n: string);
        }
        world target {
            export svc;
        }
        "#,
        "target",
        &["get-*"],
    );

    // All three functions must be exported
    assert_eq!(
        d.exported_interface_funcs("test:glob/svc@0.1.0"),
        set(&["get-name", "get-age", "set-name"])
    );
}

#[test]
fn pattern_direct_function_selective() {
    let d = build_and_decode(
        r#"
        package test:direct-pat@0.1.0;
        world target {
            export add: func(a: s32, b: s32) -> s32;
            export sub: func(a: s32, b: s32) -> s32;
            export mul: func(a: s32, b: s32) -> s32;
        }
        "#,
        "target",
        &["add", "mul"],
    );

    // All three must be exported
    assert_eq!(d.direct_export_func_names(), set(&["add", "sub", "mul"]));
}

// ============================================================
// Error cases
// ============================================================

#[test]
fn no_exports_errors() {
    let dir = wit_dir(
        r#"
        package test:empty@0.1.0;
        world target {
        }
        "#,
    );
    let err = interceptor::create_from_wit(&wit_path(&dir), "target", &[]).unwrap_err();
    assert!(
        err.to_string().contains("No exports"),
        "expected 'No exports' error, got: {err}"
    );
}

#[test]
fn bad_world_name_errors() {
    let dir = wit_dir(
        r#"
        package test:badworld@0.1.0;
        world target {
            export add: func(a: s32, b: s32) -> s32;
        }
        "#,
    );
    let err = interceptor::create_from_wit(&wit_path(&dir), "nonexistent", &[]).unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("nonexistent") || msg.contains("world"),
        "expected error about missing world, got: {msg}"
    );
}

// ============================================================
// create_from_component (WAT => bytes path)
// ============================================================

#[test]
fn from_component_simple_interface() {
    let component_bytes = wat::parse_str(
        r#"
        (component
            (core module $m
                (func (export "add") (param i32 i32) (result i32)
                    local.get 0 local.get 1 i32.add
                )
            )
            (core instance $i (instantiate $m))
            (func $add (param "a" s32) (param "b" s32) (result s32)
                (canon lift (core func $i "add"))
            )
            (instance $math (export "add" (func $add)))
            (export "test:calc/math@0.1.0" (instance $math))
        )
        "#,
    )
    .unwrap();
    let bytes = interceptor::create_from_component(&component_bytes, &[]).unwrap();
    let d = DecodedInterceptor::from_bytes(&bytes);

    assert!(d.export_names().contains("test:calc/math@0.1.0"));
    assert_eq!(
        d.exported_interface_funcs("test:calc/math@0.1.0"),
        set(&["add"])
    );
    assert!(d.import_names().contains("test:calc/math@0.1.0"));
    assert!(
        d.import_names()
            .contains("modulewise:interceptor/advice@0.1.0")
    );
}

#[test]
fn from_component_direct_export() {
    let component_bytes = wat::parse_str(
        r#"
        (component
            (core module $m
                (func (export "add") (param i32 i32) (result i32)
                    local.get 0 local.get 1 i32.add
                )
            )
            (core instance $i (instantiate $m))
            (func $add (param "a" s32) (param "b" s32) (result s32)
                (canon lift (core func $i "add"))
            )
            (export "add" (func $add))
        )
        "#,
    )
    .unwrap();
    let bytes = interceptor::create_from_component(&component_bytes, &[]).unwrap();
    let d = DecodedInterceptor::from_bytes(&bytes);

    assert_eq!(d.direct_export_func_names(), set(&["add"]));
    assert!(d.import_names().contains("add"));
    assert!(
        d.import_names()
            .contains("modulewise:interceptor/advice@0.1.0")
    );
}

// ============================================================
// Chained use types
// ============================================================

#[test]
fn chained_use_types_across_type_providers() {
    let d = build_and_decode(
        r#"
        package test:chained@0.1.0;
        interface primitives {
            record point {
                x: f32,
                y: f32,
            }
        }
        interface shapes {
            use primitives.{point};
            record line {
                start: point,
                end: point,
            }
        }
        interface drawing {
            use shapes.{line};
            draw: func(l: line) -> bool;
        }
        world target {
            export drawing;
        }
        "#,
        "target",
        &[],
    );

    assert_eq!(d.export_names(), set(&["test:chained/drawing@0.1.0"]));
    assert_eq!(
        d.exported_interface_funcs("test:chained/drawing@0.1.0"),
        set(&["draw"])
    );
    // Both type-providing interfaces must be imported
    assert!(d.import_names().contains("test:chained/shapes@0.1.0"));
    assert!(d.import_names().contains("test:chained/primitives@0.1.0"));
}
