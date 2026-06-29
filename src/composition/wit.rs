use anyhow::Result;
use serde_json::json;
use std::collections::HashMap;
use wit_parser::{Resolve, Type};

use crate::types::{Function, FunctionParam, Interface};

#[derive(Debug, Clone)]
pub struct PackageMetadata {
    pub namespace: Option<String>,
    pub name: Option<String>,
}

pub struct Parser;

impl Parser {
    /// Parse component and return imports, exports, and functions
    pub fn parse(
        component_bytes: &[u8],
    ) -> Result<(
        PackageMetadata,
        Vec<String>,
        Vec<String>,
        HashMap<String, Function>,
    )> {
        let decoded = wit_parser::decoding::decode(component_bytes)?;
        let resolve = decoded.resolve().clone();

        if resolve.worlds.len() != 1 {
            return Err(anyhow::anyhow!("Expected exactly one world in component"));
        }

        let (_, world) = resolve.worlds.iter().next().unwrap();

        // Extract component's own package/namespace metadata
        let component_metadata = if let Some(package_id) = &world.package {
            let package = resolve.packages.get(*package_id).unwrap();
            let package_name = &package.name;
            PackageMetadata {
                namespace: Some(package_name.namespace.clone()),
                name: Some(package_name.name.clone()),
            }
        } else {
            PackageMetadata {
                namespace: None,
                name: None,
            }
        };

        // Extract imports
        let mut imports = Vec::new();
        for (_, item) in &world.imports {
            if let wit_parser::WorldItem::Interface { id, .. } = item {
                let interface = resolve.interfaces.get(*id).unwrap();
                // Skip type-only interfaces (no functions to satisfy at runtime)
                if interface.functions.is_empty() {
                    continue;
                }
                let interface_name = Self::build_full_interface_name(&resolve, *id)?;
                imports.push(interface_name);
            }
        }

        // Extract exports
        let mut exports = Vec::new();
        for (_, item) in &world.exports {
            if let wit_parser::WorldItem::Interface { id, .. } = item {
                let interface_name = Self::build_full_interface_name(&resolve, *id)?;
                exports.push(interface_name);
            }
        }

        let function_map = {
            let mut functions = Vec::new();
            for (_, item) in &world.exports {
                match item {
                    wit_parser::WorldItem::Interface { id, .. } => {
                        let interface_functions = Self::parse_interface(id, &resolve)?;
                        functions.extend(interface_functions);
                    }
                    wit_parser::WorldItem::Function(func) => {
                        let function = Self::parse_function(func, None, &resolve)?;
                        functions.push(function);
                    }
                    wit_parser::WorldItem::Type { .. } => {
                        // No functions
                    }
                }
            }
            Self::build_function_map(functions)?
        };

        Ok((component_metadata, imports, exports, function_map))
    }

    fn build_full_interface_name(
        resolve: &wit_parser::Resolve,
        interface_id: wit_parser::InterfaceId,
    ) -> Result<String> {
        let interface = resolve.interfaces.get(interface_id).unwrap();
        if let Some(interface_name) = &interface.name {
            if let Some(package_id) = &interface.package {
                let package = resolve.packages.get(*package_id).unwrap();
                let package_name = &package.name;
                let version_suffix = package_name
                    .version
                    .as_ref()
                    .map(|v| format!("@{v}"))
                    .unwrap_or_default();
                let full_interface_name = format!(
                    "{}:{}/{}{}",
                    package_name.namespace, package_name.name, interface_name, version_suffix
                );
                Ok(full_interface_name)
            } else {
                Err(anyhow::anyhow!(
                    "Interface '{interface_name}' missing required package metadata"
                ))
            }
        } else {
            Err(anyhow::anyhow!("Interface missing name"))
        }
    }

    fn parse_interface(
        interface_id: &wit_parser::InterfaceId,
        resolve: &Resolve,
    ) -> Result<Vec<Function>> {
        let interface = resolve.interfaces.get(*interface_id).unwrap();
        let full_interface_name = Self::build_full_interface_name(resolve, *interface_id)?;
        let interface_obj = Interface::parse(&full_interface_name)?;

        let mut functions = Vec::new();
        for (_, func) in &interface.functions {
            let function_obj = Self::parse_function(func, Some(interface_obj.clone()), resolve)?;
            functions.push(function_obj);
        }
        Ok(functions)
    }

    fn parse_function(
        func: &wit_parser::Function,
        interface: Option<Interface>,
        resolve: &Resolve,
    ) -> Result<Function> {
        // Validate and resolve parameter types
        let mut params = Vec::new();
        for p in &func.params {
            Self::validate_wit_type_for_json_rpc(p.ty, resolve)?;
            let json_schema = Self::wit_type_to_json_schema(p.ty, resolve);
            let is_optional = Self::is_optional_type(p.ty, resolve);
            params.push(FunctionParam {
                name: p.name.clone(),
                is_optional,
                json_schema,
            });
        }

        // Validate and convert result type
        let result = match &func.result {
            Some(return_type) => {
                Self::validate_wit_type_for_json_rpc(*return_type, resolve)?;
                Some(Self::wit_type_to_json_schema(*return_type, resolve))
            }
            None => None,
        };

        Ok(Function::new(
            interface,
            func.name.clone(),
            func.docs.contents.as_deref().unwrap_or("").to_string(),
            params,
            result,
        ))
    }

    // Build a function map keyed by Function::key().
    // Returns an error if more than one interface with the same unqualified
    // name exports the same function name.
    fn build_function_map(functions: Vec<Function>) -> Result<HashMap<String, Function>> {
        let mut result: HashMap<String, Function> = HashMap::new();

        for func in functions {
            let key = func.key();

            if let Some(existing) = result.get(&key) {
                return Err(anyhow::anyhow!(
                    "Ambiguous function: interfaces '{}' and '{}' both export '{}'.",
                    existing.interface().unwrap().as_str(),
                    func.interface().unwrap().as_str(),
                    key
                ));
            }
            result.insert(key, func);
        }

        Ok(result)
    }

    fn validate_wit_type_for_json_rpc(wit_type: Type, resolve: &Resolve) -> Result<()> {
        match wit_type {
            // Primitives are all supported
            Type::Bool
            | Type::U8
            | Type::U16
            | Type::U32
            | Type::U64
            | Type::S8
            | Type::S16
            | Type::S32
            | Type::S64
            | Type::F32
            | Type::F64
            | Type::Char
            | Type::String
            | Type::ErrorContext => Ok(()),

            // Complex types need validation
            Type::Id(type_id) => {
                let type_def = resolve
                    .types
                    .get(type_id)
                    .expect("Type definition not found for type ID");
                match &type_def.kind {
                    wit_parser::TypeDefKind::Type(inner_type) => {
                        Self::validate_wit_type_for_json_rpc(*inner_type, resolve)
                    }
                    wit_parser::TypeDefKind::Record(record) => {
                        for field in &record.fields {
                            Self::validate_wit_type_for_json_rpc(field.ty, resolve)?;
                        }
                        Ok(())
                    }
                    wit_parser::TypeDefKind::Variant(variant) => {
                        for case in &variant.cases {
                            if let Some(case_type) = case.ty {
                                Self::validate_wit_type_for_json_rpc(case_type, resolve)?;
                            }
                        }
                        Ok(())
                    }
                    wit_parser::TypeDefKind::Enum(_) => Ok(()),
                    wit_parser::TypeDefKind::Option(option_type) => {
                        Self::validate_wit_type_for_json_rpc(*option_type, resolve)
                    }
                    wit_parser::TypeDefKind::Result(result_type) => {
                        if let Some(ok_type) = result_type.ok {
                            Self::validate_wit_type_for_json_rpc(ok_type, resolve)?;
                        }
                        if let Some(err_type) = result_type.err {
                            Self::validate_wit_type_for_json_rpc(err_type, resolve)?;
                        }
                        Ok(())
                    }
                    wit_parser::TypeDefKind::List(element_type) => {
                        Self::validate_wit_type_for_json_rpc(*element_type, resolve)
                    }
                    wit_parser::TypeDefKind::Map(key_type, value_type) => {
                        // WIT restricts map keys to primitives.
                        // Both key and value still need to be representable.
                        Self::validate_wit_type_for_json_rpc(*key_type, resolve)?;
                        Self::validate_wit_type_for_json_rpc(*value_type, resolve)
                    }
                    wit_parser::TypeDefKind::FixedLengthList(..) => {
                        // Rejected on a surfaced (JSON-RPC invoked) signature:
                        // wasmtime does not yet support fixed-length lists in
                        // its public value-conversion (Val) API.
                        // Inter-component use is ok.
                        // See https://github.com/bytecodealliance/wasmtime/issues/12279
                        Err(anyhow::anyhow!("fixed-length lists are not yet supported"))
                    }
                    wit_parser::TypeDefKind::Tuple(tuple) => {
                        for tuple_type in &tuple.types {
                            Self::validate_wit_type_for_json_rpc(*tuple_type, resolve)?;
                        }
                        Ok(())
                    }
                    wit_parser::TypeDefKind::Flags(_) => Ok(()),
                    // Resources get placeholders
                    wit_parser::TypeDefKind::Resource => Ok(()),
                    wit_parser::TypeDefKind::Handle(_) => Ok(()),
                    _ => Err(anyhow::anyhow!("Unsupported WIT type: {:?}", type_def.kind)),
                }
            }
        }
    }

    fn wit_type_to_json_schema(wit_type: Type, resolve: &Resolve) -> serde_json::Value {
        match wit_type {
            // Primitives - direct mappings
            Type::Bool => json!({"type": "boolean"}),
            Type::U8 => json!({"type": "number", "minimum": 0, "maximum": 255}),
            Type::U16 => json!({"type": "number", "minimum": 0, "maximum": 65535}),
            Type::U32 => json!({"type": "number", "minimum": 0, "maximum": 4294967295_u64}),
            Type::U64 => json!({"type": "number", "minimum": 0}),
            Type::S8 => json!({"type": "number", "minimum": -128, "maximum": 127}),
            Type::S16 => json!({"type": "number", "minimum": -32768, "maximum": 32767}),
            Type::S32 => {
                json!({"type": "number", "minimum": -2147483648_i64, "maximum": 2147483647})
            }
            Type::S64 => json!({"type": "number"}),
            Type::F32 | Type::F64 => json!({"type": "number"}),
            Type::Char => json!({"type": "string", "minLength": 1, "maxLength": 1}),
            Type::String => json!({"type": "string"}),

            // Complex types
            Type::Id(type_id) => {
                let type_def = resolve
                    .types
                    .get(type_id)
                    .expect("Type definition not found for type ID");
                match &type_def.kind {
                    wit_parser::TypeDefKind::Type(inner_type) => {
                        Self::wit_type_to_json_schema(*inner_type, resolve)
                    }
                    wit_parser::TypeDefKind::Record(record) => {
                        let mut properties = serde_json::Map::new();
                        let mut required = Vec::new();

                        for field in &record.fields {
                            properties.insert(
                                field.name.clone(),
                                Self::wit_type_to_json_schema(field.ty, resolve),
                            );
                            if !Self::is_optional_type(field.ty, resolve) {
                                required.push(field.name.clone());
                            }
                        }

                        let mut schema = json!({
                            "type": "object",
                            "properties": properties,
                            "required": required,
                            "additionalProperties": false
                        });

                        // Add title if type has a name
                        if let Some(type_name) = &type_def.name {
                            schema["title"] = json!(type_name);
                        }

                        schema
                    }
                    wit_parser::TypeDefKind::Variant(variant) => {
                        let cases: Vec<serde_json::Value> = variant.cases.iter().map(|case| {
                                if let Some(case_type) = case.ty {
                                    json!({
                                        "type": "object",
                                        "properties": {
                                            "type": {"const": case.name},
                                            "value": Self::wit_type_to_json_schema(case_type, resolve)
                                        },
                                        "required": ["type", "value"],
                                        "additionalProperties": false
                                    })
                                } else {
                                    json!({
                                        "type": "object", 
                                        "properties": {
                                            "type": {"const": case.name}
                                        },
                                        "required": ["type"],
                                        "additionalProperties": false
                                    })
                                }
                            }).collect();

                        json!({
                            "oneOf": cases
                        })
                    }
                    wit_parser::TypeDefKind::Enum(enum_def) => {
                        let enum_values: Vec<&String> =
                            enum_def.cases.iter().map(|case| &case.name).collect();
                        json!({
                            "type": "string",
                            "enum": enum_values
                        })
                    }
                    wit_parser::TypeDefKind::Option(option_type) => {
                        json!({
                            "oneOf": [
                                Self::wit_type_to_json_schema(*option_type, resolve),
                                {"type": "null"}
                            ]
                        })
                    }
                    wit_parser::TypeDefKind::Result(result_type) => {
                        let mut ok_schema = json!({"type": "null"});
                        let mut err_schema = json!({"type": "null"});

                        if let Some(ok_type) = result_type.ok {
                            ok_schema = Self::wit_type_to_json_schema(ok_type, resolve);
                        }
                        if let Some(err_type) = result_type.err {
                            err_schema = Self::wit_type_to_json_schema(err_type, resolve);
                        }

                        json!({
                            "oneOf": [
                                {
                                    "type": "object",
                                    "properties": {
                                        "ok": ok_schema
                                    },
                                    "required": ["ok"],
                                    "additionalProperties": false
                                },
                                {
                                    "type": "object",
                                    "properties": {
                                        "error": err_schema
                                    },
                                    "required": ["error"],
                                    "additionalProperties": false
                                }
                            ]
                        })
                    }
                    wit_parser::TypeDefKind::List(element_type) => {
                        json!({
                            "type": "array",
                            "items": Self::wit_type_to_json_schema(*element_type, resolve)
                        })
                    }
                    wit_parser::TypeDefKind::Map(key_type, value_type) => {
                        let value_schema = Self::wit_type_to_json_schema(*value_type, resolve);
                        if matches!(key_type, Type::String) {
                            // map<string, V> -> JSON object keyed by string.
                            json!({
                                "type": "object",
                                "additionalProperties": value_schema
                            })
                        } else {
                            // map<non-string, V> -> array of [key, val] pairs.
                            let key_schema = Self::wit_type_to_json_schema(*key_type, resolve);
                            json!({
                                "type": "array",
                                "items": {
                                    "type": "array",
                                    "prefixItems": [key_schema, value_schema],
                                    "minItems": 2,
                                    "maxItems": 2
                                }
                            })
                        }
                    }
                    wit_parser::TypeDefKind::Tuple(tuple) => {
                        let item_schemas: Vec<serde_json::Value> = tuple
                            .types
                            .iter()
                            .map(|t| Self::wit_type_to_json_schema(*t, resolve))
                            .collect();
                        let len = item_schemas.len();
                        json!({
                            "type": "array",
                            "prefixItems": item_schemas,
                            "minItems": len,
                            "maxItems": len
                        })
                    }
                    wit_parser::TypeDefKind::Flags(flags) => {
                        json!({
                            "type": "array",
                            "items": {
                                "type": "string",
                                "enum": flags.flags.iter().map(|f| &f.name).collect::<Vec<_>>()
                            },
                            "uniqueItems": true
                        })
                    }
                    wit_parser::TypeDefKind::Resource => {
                        json!({"type": "resource", "description": "Resource handle (not representable in JSON-RPC)"})
                    }
                    wit_parser::TypeDefKind::Handle(_) => {
                        json!({"type": "resource", "description": "Resource handle (not representable in JSON-RPC)"})
                    }
                    _ => {
                        unreachable!("Unsupported types should be caught by validation")
                    }
                }
            }
            Type::ErrorContext => json!({"type": "string"}),
        }
    }

    fn is_optional_type(wit_type: Type, resolve: &Resolve) -> bool {
        match wit_type {
            Type::Id(type_id) => {
                let type_def = resolve
                    .types
                    .get(type_id)
                    .expect("Type definition not found for type ID");
                match &type_def.kind {
                    wit_parser::TypeDefKind::Option(_) => true,
                    wit_parser::TypeDefKind::Type(inner_type) => {
                        Self::is_optional_type(*inner_type, resolve)
                    }
                    _ => false,
                }
            }
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wit_parser::{Resolve, UnresolvedPackageGroup};

    // Parse the WIT text, register it in a Resolve, and return that Resolve
    // along with the Type for the named alias `t` declared in the single
    // interface of the WIT.
    fn parse_named_type(wit_text: &str) -> (Resolve, Type) {
        let group = UnresolvedPackageGroup::parse("test.wit", wit_text).unwrap();
        let mut resolve = Resolve::default();
        resolve.push_group(group).unwrap();

        let (_iface_id, iface) = resolve
            .interfaces
            .iter()
            .next()
            .expect("expected at least one interface");
        let type_id = iface
            .types
            .get("t")
            .copied()
            .expect("expected type alias `t` in interface");
        let ty = Type::Id(type_id);
        (resolve, ty)
    }

    // Parse WIT, emit the schema for the named alias `t`, meta-validate, return it.
    fn emit_schema(wit_text: &str) -> serde_json::Value {
        let (resolve, ty) = parse_named_type(wit_text);
        let schema = Parser::wit_type_to_json_schema(ty, &resolve);
        assert_schema_passes_meta_validation(&schema);
        schema
    }

    // Ensure the emitted schema is valid against the JSON Schema meta-schema.
    fn assert_schema_passes_meta_validation(schema: &serde_json::Value) {
        if let Err(e) = jsonschema::validator_for(schema) {
            panic!("emitted schema rejected by jsonschema validator: {e}\nschema: {schema:#}");
        }
    }

    fn assert_accepts(schema: &serde_json::Value, value: &serde_json::Value) {
        let v = jsonschema::validator_for(schema).expect("failed to build validator");
        if let Err(e) = v.validate(value) {
            panic!("expected accept but got {e}\nschema: {schema:#}\nvalue: {value:#}");
        }
    }

    fn assert_rejects(schema: &serde_json::Value, value: &serde_json::Value) {
        let v = jsonschema::validator_for(schema).expect("failed to build validator");
        if v.validate(value).is_ok() {
            panic!("expected reject but got accept\nschema: {schema:#}\nvalue: {value:#}");
        }
    }

    // --- primitives ---

    #[test]
    fn primitives_emit_expected_schemas() {
        // string
        assert_eq!(
            emit_schema("package test:p; interface i { type t = string; }"),
            json!({"type": "string"})
        );
        // boolean
        assert_eq!(
            emit_schema("package test:p; interface i { type t = bool; }"),
            json!({"type": "boolean"})
        );
        // u32 carries minimum/maximum bounds.
        let u32_schema = emit_schema("package test:p; interface i { type t = u32; }");
        assert_eq!(u32_schema["type"], "number");
        assert_eq!(u32_schema["minimum"], 0);
        assert_eq!(u32_schema["maximum"], 4_294_967_295_u64);
        // s32 carries signed bounds.
        let s32_schema = emit_schema("package test:p; interface i { type t = s32; }");
        assert_eq!(s32_schema["minimum"], -2_147_483_648_i64);
        assert_eq!(s32_schema["maximum"], 2_147_483_647);
        // char is length-1 string.
        let char_schema = emit_schema("package test:p; interface i { type t = char; }");
        assert_eq!(char_schema["type"], "string");
        assert_eq!(char_schema["minLength"], 1);
        assert_eq!(char_schema["maxLength"], 1);
    }

    // --- list ---

    #[test]
    fn list_uses_singular_items_schema() {
        let schema = emit_schema("package test:p; interface i { type t = list<u32>; }");
        assert_eq!(schema["type"], "array");
        assert_eq!(schema["items"]["type"], "number");
        assert_accepts(&schema, &json!([1, 2, 3]));
        assert_rejects(&schema, &json!([1, "x"]));
    }

    // --- record ---

    #[test]
    fn record_has_properties_required_and_no_additional() {
        let schema = emit_schema(
            r#"
            package test:p;
            interface i {
                record point { x: s32, y: s32 }
                type t = point;
            }
            "#,
        );
        assert_eq!(schema["type"], "object");
        assert_eq!(schema["additionalProperties"], false);
        assert_eq!(schema["properties"]["x"]["type"], "number");
        assert_eq!(schema["properties"]["y"]["type"], "number");
        let required: Vec<&str> = schema["required"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        assert!(required.contains(&"x") && required.contains(&"y"));
        assert_accepts(&schema, &json!({"x": 1, "y": 2}));
        assert_rejects(&schema, &json!({"x": 1}));
        assert_rejects(&schema, &json!({"x": 1, "y": 2, "z": 3}));
    }

    // --- variant ---

    #[test]
    fn variant_emits_oneof_of_per_case() {
        let schema = emit_schema(
            r#"
            package test:p;
            interface i {
                variant shape {
                    circle(f64),
                    square,
                }
                type t = shape;
            }
            "#,
        );
        let cases = schema["oneOf"].as_array().unwrap();
        assert_eq!(cases.len(), 2);
        assert_accepts(&schema, &json!({"type": "circle", "value": 1.5}));
        assert_accepts(&schema, &json!({"type": "square"}));
        assert_rejects(&schema, &json!({"type": "triangle"}));
        assert_rejects(&schema, &json!({"type": "square", "value": 1}));
    }

    // --- enum ---

    #[test]
    fn enum_emits_string_with_enum_values() {
        let schema = emit_schema(
            r#"
            package test:p;
            interface i {
                enum color { red, green, blue }
                type t = color;
            }
            "#,
        );
        assert_eq!(schema["type"], "string");
        let values: Vec<&str> = schema["enum"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        assert_eq!(values, vec!["red", "green", "blue"]);
        assert_accepts(&schema, &json!("red"));
        assert_rejects(&schema, &json!("purple"));
    }

    // --- option ---

    #[test]
    fn option_emits_oneof_with_inner_and_null() {
        let schema = emit_schema("package test:p; interface i { type t = option<string>; }");
        let arms = schema["oneOf"].as_array().unwrap();
        assert_eq!(arms.len(), 2);
        assert_accepts(&schema, &json!("hello"));
        assert_accepts(&schema, &json!(null));
        assert_rejects(&schema, &json!(42));
    }

    // --- result ---

    #[test]
    fn result_emits_oneof_of_ok_and_error_objects() {
        let schema = emit_schema("package test:p; interface i { type t = result<u32, string>; }");
        let arms = schema["oneOf"].as_array().unwrap();
        assert_eq!(arms.len(), 2);
        assert_accepts(&schema, &json!({"ok": 5}));
        assert_accepts(&schema, &json!({"error": "oops"}));
        assert_rejects(&schema, &json!({"ok": 5, "error": "oops"}));
        assert_rejects(&schema, &json!({"value": 5}));
    }

    // --- flags ---

    #[test]
    fn flags_emit_array_of_enum_strings_unique() {
        let schema = emit_schema(
            r#"
            package test:p;
            interface i {
                flags perm { read, write, execute }
                type t = perm;
            }
            "#,
        );
        assert_eq!(schema["type"], "array");
        assert_eq!(schema["uniqueItems"], true);
        assert_eq!(schema["items"]["type"], "string");
        let values: Vec<&str> = schema["items"]["enum"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        assert_eq!(values, vec!["read", "write", "execute"]);
        assert_accepts(&schema, &json!(["read", "write"]));
        assert_rejects(&schema, &json!(["read", "read"]));
        assert_rejects(&schema, &json!(["delete"]));
    }

    // --- nested composition ---

    #[test]
    fn record_with_list_of_tuples_field() {
        let schema = emit_schema(
            r#"
            package test:p;
            interface i {
                record response { headers: list<tuple<string, string>> }
                type t = response;
            }
            "#,
        );
        assert_accepts(&schema, &json!({"headers": [["a", "1"], ["b", "2"]]}));
        assert_rejects(&schema, &json!({"headers": [["a"]]}));
        assert_rejects(&schema, &json!({"headers": [["a", 1]]}));
    }

    #[test]
    fn option_of_record() {
        let schema = emit_schema(
            r#"
            package test:p;
            interface i {
                record user { id: string, age: u32 }
                type t = option<user>;
            }
            "#,
        );
        assert_accepts(&schema, &json!(null));
        assert_accepts(&schema, &json!({"id": "abc", "age": 30}));
        assert_rejects(&schema, &json!({"id": "abc"}));
    }

    #[test]
    fn list_of_record_with_variant_field() {
        let schema = emit_schema(
            r#"
            package test:p;
            interface i {
                variant event { created, deleted(string) }
                record entry { name: string, evt: event }
                type t = list<entry>;
            }
            "#,
        );
        assert_accepts(
            &schema,
            &json!([
                {"name": "a", "evt": {"type": "created"}},
                {"name": "b", "evt": {"type": "deleted", "value": "x"}}
            ]),
        );
        assert_rejects(&schema, &json!([{"name": "a", "evt": {"type": "unknown"}}]));
    }

    #[test]
    fn tuple_inside_list_passes_meta_validation() {
        let wit = r#"
            package test:schemas;
            interface i {
                type t = list<tuple<string, string>>;
            }
        "#;
        let (resolve, ty) = parse_named_type(wit);
        let schema = Parser::wit_type_to_json_schema(ty, &resolve);
        assert_schema_passes_meta_validation(&schema);
    }

    #[test]
    fn tuple_schema_enforces_shape() {
        let wit = r#"
            package test:schemas;
            interface i {
                type t = tuple<string, string>;
            }
        "#;
        let (resolve, ty) = parse_named_type(wit);
        let schema = Parser::wit_type_to_json_schema(ty, &resolve);
        let validator = jsonschema::validator_for(&schema).expect("emitted schema should be valid");

        let good = json!(["a", "b"]);
        assert!(
            validator.validate(&good).is_ok(),
            "valid 2-string tuple rejected: schema={schema:#}"
        );

        let too_short = json!(["a"]);
        assert!(
            validator.validate(&too_short).is_err(),
            "1-element value should fail minItems"
        );

        let too_long = json!(["a", "b", "c"]);
        assert!(
            validator.validate(&too_long).is_err(),
            "3-element value should fail maxItems"
        );

        let wrong_type = json!(["a", 42]);
        assert!(
            validator.validate(&wrong_type).is_err(),
            "non-string in position 1 should fail prefixItems"
        );
    }
}
