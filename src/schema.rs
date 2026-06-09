//! Schema derivation and structural alignment for surface-exposed operations.
//!
//! When a component-backed operation declares `param-mapping` or
//! `result-mapping`, the surface-facing input/output schema may diverge from
//! the raw WIT signature. This module derives the surface-facing schema from
//! the templates by resolving each reference against the WIT-side types.
//!
//! When the operation config also declares an explicit `input-schema` or
//! `output-schema`, structural alignment validation ensures the explicit
//! schema conforms to the same contract: shape (property names, nesting,
//! array/object/scalar kind) must match the derived schema. Type metadata
//! on the explicit side may enrich (description, constraints, etc.) and may be
//! looser than the derived type only when surface-side coercion can safely
//! bridge the difference (e.g. declaring `string` at a numeric position =>
//! coerce_value stringifies at serialization time).

use serde_json::{Map, Value, json};

use crate::mapping::{ContentTypeSpec, MappingConfig, PathSegment, ResultDecoding, parse_path};
use crate::types::Function;

// If a string is a path-only template (a single `{...}` token comprising the
// entire string), return the parsed path segments. None for any other shape
// or parse errors.
fn parse_path_only_template(s: &str) -> Option<Vec<PathSegment>> {
    let inner = s.strip_prefix('{')?.strip_suffix('}')?;
    if inner.contains('{') || inner.contains('}') {
        return None;
    }
    parse_token_path(inner)
}

// Extract every embedded `{...}` reference from a template string, returning
// the parsed path for each occurrence. Tokens whose contents don't parse as
// a path are skipped.
fn extract_template_refs(s: &str) -> Vec<Vec<PathSegment>> {
    let mut refs = Vec::new();
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'{'
            && let Some(end) = s[i + 1..].find('}')
        {
            let inner = &s[i + 1..i + 1 + end];
            if let Some(path) = parse_token_path(inner) {
                refs.push(path);
            }
            i = i + 1 + end + 1;
            continue;
        }
        i += 1;
    }
    refs
}

// Parse the inner content of a `{...}` token: strip any `| default` tail,
// then parse the path. Returns None on parse failure.
fn parse_token_path(inner: &str) -> Option<Vec<PathSegment>> {
    let trimmed = inner.trim();
    let head = trimmed.split('|').next().unwrap_or(trimmed).trim();
    parse_path(head).ok()
}

// Returns the sub-path under `body` when `path` is body-rooted. None when
// `path` is rooted elsewhere (e.g. `headers.*`) and so doesn't contribute
// to the body schema.
fn body_subpath(path: Vec<PathSegment>) -> Option<Vec<PathSegment>> {
    let mut iter = path.into_iter();
    match iter.next()? {
        PathSegment::Key(k) if k == "body" => Some(iter.collect()),
        _ => None,
    }
}

// Resolve a sequence of PathSegments into a WIT-derived schema. Returns the
// sub-schema at that path, or an error if the path cannot be resolved.
//
// Path semantics:
// - Empty path returns the schema unchanged.
// - Each segment descends one level:
//     * `Key(k)` on an object schema: descend via `properties.k`.
//     * `Index(i)` on an array schema with `items` (lists): return the items'
//       type (uniform across the list; index is discarded).
//     * `Index(i)` on an array schema with `prefixItems` (tuples): return the
//       type at position i.
//     * `oneOf` schemas: `result<T, E>` descends into the `ok` arm's value
//       type; `option<T>` descends into the non-null arm. Other oneOf shapes
//       (e.g. variant) fall through to a generic "cannot descend" error.
fn resolve_path<'a>(schema: &'a Value, path: &[PathSegment]) -> Result<&'a Value, String> {
    let mut current = unwrap_singular_oneof(schema)?;
    for segment in path {
        current = unwrap_singular_oneof(current)?;
        let schema_type = current.get("type").and_then(|t| t.as_str());
        match (segment, schema_type) {
            (PathSegment::Key(k), Some("object")) => {
                let properties = current
                    .get("properties")
                    .and_then(|p| p.as_object())
                    .ok_or_else(|| {
                        format!("cannot resolve key '{k}' in object schema with no properties")
                    })?;
                current = properties
                    .get(k)
                    .ok_or_else(|| format!("key '{k}' not found in object schema properties"))?;
            }
            (PathSegment::Index(idx), Some("array")) => {
                if let Some(prefix_items) = current.get("prefixItems").and_then(|p| p.as_array()) {
                    current = prefix_items.get(*idx).ok_or_else(|| {
                        format!(
                            "tuple index {idx} out of range (len {})",
                            prefix_items.len()
                        )
                    })?;
                } else if let Some(items) = current.get("items") {
                    // List: index is not relevant; the item type is uniform.
                    current = items;
                } else {
                    return Err(format!(
                        "cannot resolve index [{idx}] in array schema with no items or prefixItems"
                    ));
                }
            }
            (PathSegment::Key(k), _) => {
                return Err(format!(
                    "cannot descend key '{k}' into schema of kind {schema_type:?}"
                ));
            }
            (PathSegment::Index(idx), _) => {
                return Err(format!(
                    "cannot descend index [{idx}] into schema of kind {schema_type:?}"
                ));
            }
        }
    }
    unwrap_singular_oneof(current)
}

// If the schema is a `result<T, E>` or `option<T>` oneOf, return its
// "primary" arm: T for result, the non-null arm for option. Any other
// shape (variant oneOf, plain type, etc.) is returned unchanged.
fn unwrap_singular_oneof(schema: &Value) -> Result<&Value, String> {
    let Some(arms) = schema.get("oneOf").and_then(|a| a.as_array()) else {
        return Ok(schema);
    };
    if arms.len() != 2 {
        return Ok(schema); // variant or unfamiliar shape: leave as is
    }
    // option<T>: one arm is type:"null".
    let is_null = |s: &Value| s.get("type").and_then(|t| t.as_str()) == Some("null");
    if let Some(non_null) = arms.iter().find(|a| !is_null(a))
        && arms.iter().any(is_null)
    {
        return Ok(non_null);
    }
    // result<T, E>: each arm is { type: object, properties: { ok|error: ... } }.
    let mut ok_value: Option<&Value> = None;
    let mut has_error = false;
    for arm in arms {
        let Some(props) = arm.get("properties").and_then(|p| p.as_object()) else {
            return Ok(schema);
        };
        if props.len() != 1 {
            return Ok(schema);
        }
        let (key, value) = props.iter().next().unwrap();
        match key.as_str() {
            "ok" => ok_value = Some(value),
            "error" => has_error = true,
            _ => return Ok(schema),
        }
    }
    if let Some(v) = ok_value
        && has_error
    {
        return Ok(v);
    }
    Ok(schema)
}

/// Build the surface-facing input schema from a `param_mapping`.
///
/// For each entry in the mapping, the key is a WIT param name. The value is
/// a template that produces the WIT-side value from the surface-facing input
/// body. References inside (`{body.<path>}`) describe what the consumer must
/// send. Refs that don't start with `body` (e.g. `{headers.<path>}`) refer to
/// Message headers and do not contribute to the input body schema.
///
/// The derived schema's properties are the union of every distinct body path
/// referenced by any template. The type at each input position is determined
/// by the template shape that references it:
///
/// - **Path-only template** (`template == "{body.<path>}"`): the WIT param's
///   schema describes the value at that path directly (the template hands
///   the value through verbatim to the WIT-typed arg).
/// - **Interpolating template** (or any template that renders as a string):
///   the position is `{ type: "string" }` (the template renders this slot
///   into a string interpolation).
/// - **Multiple distinct WIT params reference the same input path** (e.g.
///   two path-only templates `{body.id}` mapped into different WIT params):
///   the readers' WIT types must produce the same leaf schema. An error
///   is returned otherwise.
pub fn derive_input_schema(function: &Function, config: &MappingConfig) -> Result<Value, String> {
    let param_mapping = config.param_mapping.as_ref();
    let param_encoding = config.param_encoding.as_ref();

    // Resolve the schema to advertise at the surface for a given WIT param.
    // When param-encoding declares the param, advertise the encoding-rule
    // schema (the pre-encoding caller-facing shape) rather than the raw
    // `list<u8>` WIT schema.
    let resolve_param_surface_schema = |param_name: &str, wit_schema: &Value| -> Value {
        if let Some(encoding) = param_encoding
            && let Some(spec) = encoding.0.get(param_name)
        {
            schema_for_encoded_position(spec)
        } else {
            wit_schema.clone()
        }
    };

    // Without param-mapping: input shape is one entry per WIT param,
    // each carrying the param's surface-facing schema.
    let Some(param_mapping) = param_mapping else {
        let mut properties = Map::new();
        let mut required: Vec<String> = Vec::new();
        for param in function.params() {
            let schema = resolve_param_surface_schema(&param.name, &param.json_schema);
            properties.insert(param.name.clone(), schema);
            if !param.is_optional {
                required.push(param.name.clone());
            }
        }
        return Ok(json!({
            "type": "object",
            "properties": properties,
            "required": required,
            "additionalProperties": false,
        }));
    };

    // With param-mapping, iterate each entry and contribute to the input
    // schema according to the template shape:
    // - When the template is a single `{body.<path>}` token, insert the
    //   param's surface schema at <path>.
    // - Otherwise the embedded `{body.<path>}` references are substituted
    //   as strings into a surrounding template, so each one inserts a
    //   string schema at <path>.
    let mut properties = Map::new();
    let mut required: Vec<String> = Vec::new();

    for (arg_name, template) in param_mapping {
        let param = function
            .params()
            .iter()
            .find(|p| p.name == *arg_name)
            .ok_or_else(|| {
                format!(
                    "param-mapping entry '{arg_name}' does not match any WIT param of function '{}'",
                    function.function_name()
                )
            })?;

        // Path-only template: the template IS a single `{body.<path>}` token.
        // The input position at that path has the param's surface schema.
        if let Value::String(s) = template
            && let Some(path) = parse_path_only_template(s)
            && let Some(under_body) = body_subpath(path)
        {
            let schema = resolve_param_surface_schema(&param.name, &param.json_schema);
            insert_at_path(&mut properties, &mut required, &under_body, schema)?;
            continue;
        }

        // Otherwise: every `{body.<path>}` reference inside the template
        // renders as a string at its input position (the template renders
        // this slot into a string interpolation).
        let refs = collect_refs(template);
        for full_path in refs {
            if let Some(under_body) = body_subpath(full_path) {
                insert_at_path(
                    &mut properties,
                    &mut required,
                    &under_body,
                    json!({ "type": "string" }),
                )?;
            }
        }
    }

    Ok(json!({
        "type": "object",
        "properties": properties,
        "required": required,
        "additionalProperties": false,
    }))
}

// Surface-facing schema for a position the host will encode/decode at
// runtime. `text/plain` literal advertises a string; anything else
// (`application/json` literal or runtime-determined path) advertises any.
fn schema_for_encoded_position(spec: &ContentTypeSpec) -> Value {
    match spec {
        ContentTypeSpec::Literal(ct) if ct == "text/plain" => json!({ "type": "string" }),
        _ => json!({}),
    }
}

/// Build the surface-facing output schema from a `result_mapping`.
///
/// The result-mapping has structural top-level keys `body` and/or `headers`.
/// Only the `body` sub-tree contributes to the derived output schema. The
/// `headers` sub-tree (if present) is ignored (it describes reply Message
/// headers, governed by surface-level propagate config).
///
/// When `result_decoding` is present, the WIT result schema is modified
/// before result-mapping references are resolved: each decoded field's
/// schema is replaced with `{ "type": "string" }` for `text/plain` literals,
/// or `{}` (any) for `application/json` literals and runtime-determined
/// paths.
///
/// Returns `Ok(None)` when result-mapping declares no body content: body slot
/// absent, or body explicitly set to `""` or `null`. These all mean "no body"
/// (zero bytes at runtime; the surface should advertise no result schema).
///
/// Returns `Ok(Some(schema))` when result-mapping declares a body template.
/// The template's shape becomes the output schema's shape; each `{<path>}`
/// reference resolves to the WIT result's schema at that path, and literal
/// values get a schema matching their JSON type.
pub fn derive_output_schema(
    function: &Function,
    config: &MappingConfig,
) -> Result<Option<Value>, String> {
    let result_decoding = config.result_decoding.as_ref();
    let result_mapping = config.result_mapping.as_ref();

    // Without result-mapping: the surface schema is the WIT result schema
    // itself, with the decoding-rule applied to any fields named in
    // result-decoding. No WIT result means no schema to advertise.
    let Some(result_mapping) = result_mapping else {
        let Some(wit_result) = function.result() else {
            return Ok(None);
        };
        return Ok(Some(match result_decoding {
            None => wit_result.clone(),
            Some(decoding) => apply_decoding_to_schema(wit_result, decoding),
        }));
    };

    // With result-mapping: the `body` sub-tree drives the output schema.
    // An absent or explicitly-empty body slot means no body to advertise.
    let body_template = match result_mapping.get("body") {
        None | Some(Value::Null) => return Ok(None),
        Some(Value::String(s)) if s.is_empty() => return Ok(None),
        Some(t) => t,
    };
    let wit_result = function.result().ok_or_else(|| {
        format!(
            "function '{}' has no return type, but a result-mapping declares a body template",
            function.function_name()
        )
    })?;
    let source_schema: Value = match result_decoding {
        None => wit_result.clone(),
        Some(decoding) => apply_decoding_to_schema(wit_result, decoding),
    };
    Ok(Some(derive_schema_from_template(
        body_template,
        &source_schema,
    )?))
}

// Apply result-decoding to a WIT result schema: replace each decoded field's
// schema with the post-decode shape, using the same `text/plain => string,
// else => any` rule as the param-encoding side.
fn apply_decoding_to_schema(wit_result: &Value, decoding: &ResultDecoding) -> Value {
    let mut out = wit_result.clone();
    let Some(properties) = out
        .as_object_mut()
        .and_then(|o| o.get_mut("properties"))
        .and_then(|p| p.as_object_mut())
    else {
        return out;
    };
    for (field_name, spec) in &decoding.0 {
        properties.insert(field_name.clone(), schema_for_encoded_position(spec));
    }
    out
}

fn derive_schema_from_template(template: &Value, wit_source: &Value) -> Result<Value, String> {
    match template {
        Value::String(s) => {
            if let Some(path) = parse_path_only_template(s) {
                Ok(resolve_path(wit_source, &path)?.clone())
            } else {
                // Interpolating template (or string literal): renders as string.
                Ok(json!({ "type": "string" }))
            }
        }
        Value::Object(map) => {
            let mut properties = Map::new();
            let mut required = Vec::new();
            for (k, v) in map {
                let sub = derive_schema_from_template(v, wit_source)?;
                properties.insert(k.clone(), sub);
                required.push(k.clone());
            }
            Ok(json!({
                "type": "object",
                "properties": properties,
                "required": required,
                "additionalProperties": false,
            }))
        }
        Value::Array(items) => {
            let item_schemas: Result<Vec<_>, _> = items
                .iter()
                .map(|v| derive_schema_from_template(v, wit_source))
                .collect();
            let item_schemas = item_schemas?;
            let len = item_schemas.len();
            Ok(json!({
                "type": "array",
                "prefixItems": item_schemas,
                "minItems": len,
                "maxItems": len,
            }))
        }
        Value::Bool(_) => Ok(json!({ "type": "boolean" })),
        Value::Number(_) => Ok(json!({ "type": "number" })),
        Value::Null => Ok(json!({ "type": "null" })),
    }
}

// Collect all template references across a template value.
fn collect_refs(template: &Value) -> Vec<Vec<PathSegment>> {
    let mut refs = Vec::new();
    collect_refs_into(template, &mut refs);
    refs
}

fn collect_refs_into(template: &Value, refs: &mut Vec<Vec<PathSegment>>) {
    match template {
        Value::String(s) => {
            if let Some(path) = parse_path_only_template(s) {
                refs.push(path);
            } else {
                refs.extend(extract_template_refs(s));
            }
        }
        Value::Array(items) => {
            for v in items {
                collect_refs_into(v, refs);
            }
        }
        Value::Object(map) => {
            for v in map.values() {
                collect_refs_into(v, refs);
            }
        }
        _ => {}
    }
}

// Return the key name from a `Key` segment, or an error for `Index` segments.
// Used by the insert helpers, which can only construct object-tree structure
// (input-schema body is modeled as an object tree).
fn key_segment(seg: &PathSegment) -> Result<&str, String> {
    match seg {
        PathSegment::Key(k) => Ok(k.as_str()),
        PathSegment::Index(i) => Err(format!(
            "cannot insert array-index segment [{i}] into a body schema (body is modeled as an object)"
        )),
    }
}

// Insert a leaf schema at the given path into a top-level `properties` map.
// Intermediate object levels are created as needed; any segment that's
// required (i.e. visited at all) goes onto the `required` list at its level.
//
// The body schema produced here is always object-rooted: `{type: "object",
// properties: {...}, required: [...]}`. Each path segment names a property
// at some object level, so all segments must be `Key`. An `Index` segment
// at any position would require an array-rooted body schema, which this
// function does not construct.
fn insert_at_path(
    top_properties: &mut Map<String, Value>,
    top_required: &mut Vec<String>,
    path: &[PathSegment],
    leaf_schema: Value,
) -> Result<(), String> {
    if path.is_empty() {
        return Err("empty path cannot be inserted".to_string());
    }

    let head = key_segment(&path[0])?.to_string();

    if path.len() == 1 {
        if let Some(existing) = top_properties.get(&head) {
            if existing != &leaf_schema {
                return Err(format!(
                    "input path '{head}' is referenced by multiple params with conflicting WIT types"
                ));
            }
            return Ok(());
        }
        if !top_required.contains(&head) {
            top_required.push(head.clone());
        }
        top_properties.insert(head, leaf_schema);
        return Ok(());
    }

    if !top_required.contains(&head) {
        top_required.push(head.clone());
    }
    let entry = top_properties.entry(head).or_insert_with(|| {
        json!({
            "type": "object",
            "properties": {},
            "required": [],
            "additionalProperties": false,
        })
    });
    insert_recursive(entry, &path[1..], leaf_schema)
}

fn insert_recursive(
    schema_node: &mut Value,
    remaining: &[PathSegment],
    leaf_schema: Value,
) -> Result<(), String> {
    if remaining.is_empty() {
        return Err("insert_recursive called with empty remaining segments".to_string());
    }

    // Schema_node must be an object schema at this level (we only insert
    // through object structure).
    let obj = schema_node
        .as_object_mut()
        .ok_or_else(|| "expected object schema for nested insertion".to_string())?;

    let head = key_segment(&remaining[0])?.to_string();

    // Update `required` first, then drop the borrow before reborrowing for `properties`.
    {
        let required_arr = obj
            .entry("required".to_string())
            .or_insert_with(|| json!([]))
            .as_array_mut()
            .ok_or_else(|| "required is not an array".to_string())?;
        if !required_arr.iter().any(|v| v == head.as_str()) {
            required_arr.push(Value::String(head.clone()));
        }
    }

    let properties = obj
        .entry("properties".to_string())
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .ok_or_else(|| "properties is not an object".to_string())?;

    if remaining.len() == 1 {
        if let Some(existing) = properties.get(&head) {
            if existing != &leaf_schema {
                return Err(format!(
                    "input path leaf '{head}' is referenced by multiple params with conflicting WIT types"
                ));
            }
            return Ok(());
        }
        properties.insert(head, leaf_schema);
        return Ok(());
    }

    let nested = properties.entry(head).or_insert_with(|| {
        json!({
            "type": "object",
            "properties": {},
            "required": [],
            "additionalProperties": false,
        })
    });
    insert_recursive(nested, &remaining[1..], leaf_schema)
}

/// Validate that `declared` (an explicit schema from the surface config) is
/// structurally compatible with `derived` (the WIT- or template-derived
/// schema).
///
/// Rules:
/// - Both schemas must have the same `type` kind, OR `declared` may use a
///   type that surface-side coercion can safely bridge (currently:
///   declaring `string` at a non-string position is allowed because
///   `coerce_value` will stringify).
/// - For object schemas: the set of properties (names) must match exactly;
///   recurse into each property.
/// - For array schemas: recurse into `items` if both have it; for tuple
///   shapes (`prefixItems`), the lengths must match and each position must
///   align.
///
/// Metadata (description, title, examples, validation constraints like
/// minimum/maximum/pattern/etc.) is allowed in `declared` regardless of
/// `derived`; these enrich the contract without changing its shape.
pub fn validate_structural_alignment(declared: &Value, derived: &Value) -> Result<(), String> {
    align(declared, derived, "")
}

/// Verify that the top-level field `field_name` exists on the given result
/// schema and is typed as a byte array (`list<u8>` in WIT).
///
/// Unwraps `result<T, E>` and `option<T>` shapes to reach the structured
/// type before checking. Returns Ok(()) when the field is a byte array;
/// otherwise an error describing what was wrong.
pub fn validate_byte_array_field(result_schema: &Value, field_name: &str) -> Result<(), String> {
    let schema = unwrap_singular_oneof(result_schema)?;
    let schema = unwrap_singular_oneof(schema)?;
    let properties = schema
        .get("properties")
        .and_then(|p| p.as_object())
        .ok_or_else(|| {
            format!("result schema has no 'properties' to look up field '{field_name}'")
        })?;
    let field_schema = properties
        .get(field_name)
        .ok_or_else(|| format!("result schema has no field '{field_name}'"))?;
    let field_schema = unwrap_singular_oneof(field_schema)?;
    if field_schema.get("type").and_then(|t| t.as_str()) != Some("array") {
        return Err(format!(
            "result field '{field_name}' must be a byte array (list<u8>); not an array type"
        ));
    }
    let items = field_schema.get("items").ok_or_else(|| {
        format!("result field '{field_name}' must be a byte array (list<u8>); array has no 'items'")
    })?;
    let items = unwrap_singular_oneof(items)?;
    let is_byte = items.get("type").and_then(|t| t.as_str()) == Some("number")
        && items.get("minimum").and_then(|m| m.as_u64()) == Some(0)
        && items.get("maximum").and_then(|m| m.as_u64()) == Some(255);
    if !is_byte {
        return Err(format!(
            "result field '{field_name}' must be a byte array (list<u8>); item type is not u8"
        ));
    }
    Ok(())
}

// Verify that a path exists in the given result schema. Returns Ok(())
// regardless of whether the leaf is nullable.
pub(crate) fn validate_path_exists(
    result_schema: &Value,
    path: &[PathSegment],
) -> Result<(), String> {
    let _ = resolve_path(result_schema, path)?;
    Ok(())
}

/// Coerce a [`Value`] to fit a JSON Schema.
///
/// Surface-side tolerant-reader coercion called at the boundary between an
/// inbound value and a protocol response shape, used to reconcile minor type
/// mismatches without rejecting the value outright.
///
/// Currently only one rule is applied: at any position where the schema
/// declares `type: "string"`, a non-string non-null value is stringified
/// (via `serde_json::to_string`). Recurses into objects (via `properties`)
/// and arrays (via `items`).
pub fn coerce_value(value: &mut Value, schema: &Value) -> Result<(), String> {
    let schema_type = schema.get("type").and_then(|t| t.as_str());

    if schema_type == Some("string") && !matches!(value, Value::String(_) | Value::Null) {
        *value = Value::String(
            serde_json::to_string(value)
                .map_err(|e| format!("failed to stringify value for string-typed output: {e}"))?,
        );
        return Ok(());
    }

    if schema_type == Some("object")
        && let Value::Object(map) = value
        && let Some(properties) = schema.get("properties").and_then(|p| p.as_object())
    {
        for (key, sub_value) in map.iter_mut() {
            if let Some(sub_schema) = properties.get(key) {
                coerce_value(sub_value, sub_schema)?;
            }
        }
    }

    if schema_type == Some("array")
        && let Value::Array(items) = value
        && let Some(item_schema) = schema.get("items")
    {
        for item in items.iter_mut() {
            coerce_value(item, item_schema)?;
        }
    }

    Ok(())
}

fn align(declared: &Value, derived: &Value, path: &str) -> Result<(), String> {
    let declared = unwrap_singular_oneof(declared)?;
    let derived = unwrap_singular_oneof(derived)?;

    let declared_type = declared.get("type").and_then(|t| t.as_str());
    let derived_type = derived.get("type").and_then(|t| t.as_str());

    match (declared_type, derived_type) {
        (Some(d), Some(s)) if d == s => {}
        // Looser-declared coercion: declaring "string" at a non-string
        // position is bridged by coerce_value. The reverse is unsafe.
        (Some("string"), Some(_)) => {}
        // Mismatch.
        (Some(d), Some(s)) => {
            return Err(format!(
                "type mismatch at {}: declared '{d}' but derived '{s}'",
                display_path(path)
            ));
        }
        (None, _) | (_, None) => {
            // Either side without an explicit `type` is accepted. This is
            // needed for positions whose schema is `{}` (any) because the
            // content-type is determined at runtime; the user-declared schema
            // at such a position cannot be checked structurally.
        }
    }

    if derived_type == Some("object") && declared_type == Some("object") {
        let derived_props = derived
            .get("properties")
            .and_then(|p| p.as_object())
            .cloned()
            .unwrap_or_default();
        let declared_props = declared
            .get("properties")
            .and_then(|p| p.as_object())
            .cloned()
            .unwrap_or_default();

        let derived_keys: std::collections::BTreeSet<&str> =
            derived_props.keys().map(|s| s.as_str()).collect();
        let declared_keys: std::collections::BTreeSet<&str> =
            declared_props.keys().map(|s| s.as_str()).collect();

        if derived_keys != declared_keys {
            let extra: Vec<&&str> = declared_keys.difference(&derived_keys).collect();
            let missing: Vec<&&str> = derived_keys.difference(&declared_keys).collect();
            return Err(format!(
                "property set mismatch at {}: declared has extra {:?}, missing {:?}",
                display_path(path),
                extra,
                missing
            ));
        }

        for (name, derived_sub) in &derived_props {
            let declared_sub = declared_props.get(name).unwrap();
            let sub_path = if path.is_empty() {
                name.clone()
            } else {
                format!("{path}/{name}")
            };
            align(declared_sub, derived_sub, &sub_path)?;
        }
    }

    if derived_type == Some("array") && declared_type == Some("array") {
        let derived_prefix = derived.get("prefixItems").and_then(|p| p.as_array());
        let declared_prefix = declared.get("prefixItems").and_then(|p| p.as_array());

        if let (Some(d), Some(s)) = (declared_prefix, derived_prefix) {
            if d.len() != s.len() {
                return Err(format!(
                    "tuple length mismatch at {}: declared {} but derived {}",
                    display_path(path),
                    d.len(),
                    s.len()
                ));
            }
            for (i, (dv, sv)) in d.iter().zip(s.iter()).enumerate() {
                let sub_path = format!("{}/{i}", display_path(path));
                align(dv, sv, &sub_path)?;
            }
        } else if let (Some(d_items), Some(s_items)) = (declared.get("items"), derived.get("items"))
        {
            let sub_path = format!("{}/items", display_path(path));
            align(d_items, s_items, &sub_path)?;
        }
    }

    Ok(())
}

fn display_path(p: &str) -> String {
    if p.is_empty() {
        "(root)".to_string()
    } else {
        p.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mapping::{ParamEncoding, ParamMapping};
    use crate::types::FunctionParam;

    fn param(name: &str, json_schema: Value) -> FunctionParam {
        FunctionParam {
            name: name.into(),
            is_optional: false,
            json_schema,
        }
    }

    fn fn_with(params: Vec<FunctionParam>, result: Option<Value>) -> Function {
        Function::new(None, "test".into(), String::new(), params, result)
    }

    // ----- Path resolution -----

    #[test]
    fn resolve_object_property() {
        let schema = json!({
            "type": "object",
            "properties": {
                "name": { "type": "string" },
                "age": { "type": "number" }
            }
        });
        let r = resolve_path(&schema, &[PathSegment::Key("name".into())]).unwrap();
        assert_eq!(r["type"], "string");
    }

    #[test]
    fn resolve_nested_object() {
        let schema = json!({
            "type": "object",
            "properties": {
                "user": {
                    "type": "object",
                    "properties": { "email": { "type": "string" } }
                }
            }
        });
        let r = resolve_path(
            &schema,
            &[
                PathSegment::Key("user".into()),
                PathSegment::Key("email".into()),
            ],
        )
        .unwrap();
        assert_eq!(r["type"], "string");
    }

    #[test]
    fn resolve_list_index_returns_items_type() {
        let schema = json!({
            "type": "array",
            "items": { "type": "number" }
        });
        let r = resolve_path(&schema, &[PathSegment::Index(5)]).unwrap();
        assert_eq!(r["type"], "number");
    }

    #[test]
    fn resolve_tuple_index() {
        let schema = json!({
            "type": "array",
            "prefixItems": [{ "type": "string" }, { "type": "number" }],
            "minItems": 2,
            "maxItems": 2
        });
        let r = resolve_path(&schema, &[PathSegment::Index(1)]).unwrap();
        assert_eq!(r["type"], "number");
    }

    #[test]
    fn resolve_through_result_ok_arm() {
        let schema = json!({
            "oneOf": [
                {
                    "type": "object",
                    "properties": {
                        "ok": {
                            "type": "object",
                            "properties": { "id": { "type": "string" } }
                        }
                    },
                    "required": ["ok"]
                },
                {
                    "type": "object",
                    "properties": { "error": { "type": "string" } },
                    "required": ["error"]
                }
            ]
        });
        let r = resolve_path(&schema, &[PathSegment::Key("id".into())]).unwrap();
        assert_eq!(r["type"], "string");
    }

    #[test]
    fn resolve_through_option_non_null_arm() {
        let schema = json!({
            "oneOf": [
                {
                    "type": "object",
                    "properties": { "name": { "type": "string" } }
                },
                { "type": "null" }
            ]
        });
        let r = resolve_path(&schema, &[PathSegment::Key("name".into())]).unwrap();
        assert_eq!(r["type"], "string");
    }

    #[test]
    fn resolve_missing_property_errors() {
        let schema = json!({
            "type": "object",
            "properties": { "a": { "type": "string" } }
        });
        let err = resolve_path(&schema, &[PathSegment::Key("b".into())]).unwrap_err();
        assert!(err.contains("not found"), "unexpected: {err}");
    }

    #[test]
    fn resolve_tuple_index_out_of_range_errors() {
        let schema = json!({
            "type": "array",
            "prefixItems": [{ "type": "string" }]
        });
        let err = resolve_path(&schema, &[PathSegment::Index(5)]).unwrap_err();
        assert!(err.contains("out of range"), "unexpected: {err}");
    }

    // Wrap a ParamMapping as a MappingConfig with everything else default.
    fn cfg_with_param_mapping(mapping: ParamMapping) -> MappingConfig {
        MappingConfig {
            param_mapping: Some(mapping),
            ..Default::default()
        }
    }

    // Wrap a result-mapping value as a MappingConfig with everything else
    // default.
    fn cfg_with_result_mapping(result_mapping: Value) -> MappingConfig {
        MappingConfig {
            result_mapping: Some(result_mapping),
            ..Default::default()
        }
    }

    // Wrap a result-mapping value and a result-decoding into a MappingConfig.
    fn cfg_with_result_mapping_and_decoding(
        result_mapping: Value,
        decoding: ResultDecoding,
    ) -> MappingConfig {
        MappingConfig {
            result_mapping: Some(result_mapping),
            result_decoding: Some(decoding),
            ..Default::default()
        }
    }

    // ----- Input schema derivation -----

    #[test]
    fn derive_input_path_only_template_preserves_type() {
        // param 'count' is u32; template '{body.count}' preserves number type.
        let function = fn_with(
            vec![param(
                "count",
                json!({ "type": "number", "minimum": 0, "maximum": 4294967295u64 }),
            )],
            None,
        );
        let mut mapping = ParamMapping::new();
        mapping.insert("count".into(), json!("{body.count}"));
        let schema = derive_input_schema(&function, &cfg_with_param_mapping(mapping)).unwrap();
        assert_eq!(schema["properties"]["count"]["type"], "number");
        assert_eq!(schema["properties"]["count"]["minimum"], 0);
        assert_eq!(schema["required"][0], "count");
    }

    #[test]
    fn derive_input_header_ref_does_not_contribute_to_input_schema() {
        // `{headers.X}` describes a Message header, not part of the input body
        // schema. The derived schema should not include `headers` as a property.
        let function = fn_with(vec![param("ct", json!({ "type": "string" }))], None);
        let mut mapping = ParamMapping::new();
        mapping.insert("ct".into(), json!("{headers.content-type}"));
        let schema = derive_input_schema(&function, &cfg_with_param_mapping(mapping)).unwrap();
        assert!(
            schema["properties"].as_object().unwrap().is_empty(),
            "headers ref should not produce input-body properties; got {schema}"
        );
    }

    #[test]
    fn derive_input_interpolating_template_is_string() {
        // A template like "https://example.com/{body.id}" renders as string
        // regardless of the source field's WIT type.
        let function = fn_with(vec![param("url", json!({ "type": "string" }))], None);
        let mut mapping = ParamMapping::new();
        mapping.insert("url".into(), json!("https://example.com/{body.id}"));
        let schema = derive_input_schema(&function, &cfg_with_param_mapping(mapping)).unwrap();
        assert_eq!(schema["properties"]["id"]["type"], "string");
    }

    #[test]
    fn derive_input_nested_path_creates_nested_input_structure() {
        // Template `{body.user.email}` references a nested input path.
        // The derived schema mirrors that nesting on the surface side.
        // Because the template is path-only, the WIT param's type (string)
        // is used at the leaf position.
        let function = fn_with(vec![param("email", json!({ "type": "string" }))], None);
        let mut mapping = ParamMapping::new();
        mapping.insert("email".into(), json!("{body.user.email}"));
        let schema = derive_input_schema(&function, &cfg_with_param_mapping(mapping)).unwrap();
        assert_eq!(schema["properties"]["user"]["type"], "object");
        assert_eq!(
            schema["properties"]["user"]["properties"]["email"]["type"],
            "string"
        );
    }

    #[test]
    fn derive_input_path_only_template_uses_full_wit_param_schema() {
        // When the template is `{body.user}` and the WIT param `user` is a
        // record, the input position `user` carries the full record schema
        // (not just a string).
        let user_record = json!({
            "type": "object",
            "properties": {
                "name": { "type": "string" },
                "age": { "type": "number" }
            },
            "required": ["name", "age"]
        });
        let function = fn_with(vec![param("user", user_record.clone())], None);
        let mut mapping = ParamMapping::new();
        mapping.insert("user".into(), json!("{body.user}"));
        let schema = derive_input_schema(&function, &cfg_with_param_mapping(mapping)).unwrap();
        assert_eq!(schema["properties"]["user"], user_record);
    }

    #[test]
    fn derive_input_unknown_arg_errors() {
        let function = fn_with(vec![param("x", json!({ "type": "string" }))], None);
        let mut mapping = ParamMapping::new();
        mapping.insert("nope".into(), json!("{body.nope}"));
        let err = derive_input_schema(&function, &cfg_with_param_mapping(mapping)).unwrap_err();
        assert!(
            err.contains("does not match any WIT param"),
            "unexpected: {err}"
        );
    }

    #[test]
    fn derive_input_shared_path_same_type_is_ok() {
        // Two params reference `body.id` and both expect a string. The body
        // position `id` is contributed twice with the same leaf schema, which
        // is allowed (no conflict).
        let function = fn_with(
            vec![
                param("a", json!({ "type": "string" })),
                param("b", json!({ "type": "string" })),
            ],
            None,
        );
        let mut mapping = ParamMapping::new();
        mapping.insert("a".into(), json!("{body.id}"));
        mapping.insert("b".into(), json!("{body.id}"));
        let schema = derive_input_schema(&function, &cfg_with_param_mapping(mapping)).unwrap();
        assert_eq!(schema["properties"]["id"]["type"], "string");
    }

    #[test]
    fn derive_input_shared_path_conflicting_types_is_error() {
        // Two params reference `body.id`, one expecting a string and one a
        // number. The body position `id` can't satisfy both at once.
        let function = fn_with(
            vec![
                param("a", json!({ "type": "string" })),
                param("b", json!({ "type": "number" })),
            ],
            None,
        );
        let mut mapping = ParamMapping::new();
        mapping.insert("a".into(), json!("{body.id}"));
        mapping.insert("b".into(), json!("{body.id}"));
        let err = derive_input_schema(&function, &cfg_with_param_mapping(mapping)).unwrap_err();
        assert!(
            err.contains("conflicting WIT types") && err.contains("id"),
            "unexpected: {err}"
        );
    }

    // ----- Input schema derivation with param-encoding -----

    fn byte_array_schema() -> Value {
        json!({
            "type": "array",
            "items": { "type": "number", "minimum": 0, "maximum": 255 }
        })
    }

    // Build a config carrying a param-mapping and a param-encoding.
    fn cfg_with_param_mapping_and_encoding(
        mapping: ParamMapping,
        encoding: ParamEncoding,
    ) -> MappingConfig {
        MappingConfig {
            param_mapping: Some(mapping),
            param_encoding: Some(encoding),
            ..Default::default()
        }
    }

    // Build a config with only param-encoding (no param-mapping).
    fn cfg_with_param_encoding(encoding: ParamEncoding) -> MappingConfig {
        MappingConfig {
            param_encoding: Some(encoding),
            ..Default::default()
        }
    }

    #[test]
    fn derive_input_text_plain_param_encoding_yields_string_at_input_position() {
        // WIT param `body` is a byte array; param-encoding says text/plain.
        // The surface input position carries `string`, not the raw byte schema.
        let function = fn_with(vec![param("body", byte_array_schema())], None);
        let mut mapping = ParamMapping::new();
        mapping.insert("body".into(), json!("{body.payload}"));
        let encoding =
            ParamEncoding::parse(json!({ "body": "text/plain" }).as_object().unwrap()).unwrap();
        let schema = derive_input_schema(
            &function,
            &cfg_with_param_mapping_and_encoding(mapping, encoding),
        )
        .unwrap();
        assert_eq!(schema["properties"]["payload"]["type"], "string");
    }

    #[test]
    fn derive_input_application_json_param_encoding_yields_any_at_input_position() {
        let function = fn_with(vec![param("body", byte_array_schema())], None);
        let mut mapping = ParamMapping::new();
        mapping.insert("body".into(), json!("{body.payload}"));
        let encoding =
            ParamEncoding::parse(json!({ "body": "application/json" }).as_object().unwrap())
                .unwrap();
        let schema = derive_input_schema(
            &function,
            &cfg_with_param_mapping_and_encoding(mapping, encoding),
        )
        .unwrap();
        assert!(
            schema["properties"]["payload"]
                .as_object()
                .map(|o| o.is_empty())
                .unwrap_or(false),
            "expected empty (any), got {schema}"
        );
    }

    #[test]
    fn derive_input_path_param_encoding_yields_any_at_input_position() {
        // Path-resolved content-type: surface schema is any (not statically
        // determined which content-type will resolve).
        let function = fn_with(
            vec![
                param("headers", json!({ "type": "object" })),
                param("body", byte_array_schema()),
            ],
            None,
        );
        let mut mapping = ParamMapping::new();
        mapping.insert("body".into(), json!("{body.payload}"));
        let encoding =
            ParamEncoding::parse(json!({ "body": "{headers.ct}" }).as_object().unwrap()).unwrap();
        let schema = derive_input_schema(
            &function,
            &cfg_with_param_mapping_and_encoding(mapping, encoding),
        )
        .unwrap();
        assert!(
            schema["properties"]["payload"]
                .as_object()
                .map(|o| o.is_empty())
                .unwrap_or(false),
            "expected empty (any), got {schema}"
        );
    }

    #[test]
    fn derive_input_param_encoding_without_param_mapping_yields_wit_shape_with_encoded_swap() {
        // No param-mapping: input shape has one property per WIT param. For
        // params named in param-encoding, the schema is the encoding-rule
        // result (here: string for text/plain).
        let function = fn_with(
            vec![
                param("url", json!({ "type": "string" })),
                param("body", byte_array_schema()),
            ],
            None,
        );
        let encoding =
            ParamEncoding::parse(json!({ "body": "text/plain" }).as_object().unwrap()).unwrap();
        let schema = derive_input_schema(&function, &cfg_with_param_encoding(encoding)).unwrap();
        assert_eq!(schema["properties"]["url"]["type"], "string");
        assert_eq!(schema["properties"]["body"]["type"], "string");
    }

    #[test]
    fn derive_input_param_encoding_without_param_mapping_non_encoded_params_keep_wit_schema() {
        let function = fn_with(
            vec![
                param("url", json!({ "type": "string" })),
                param(
                    "count",
                    json!({ "type": "number", "minimum": 0, "maximum": 4294967295u64 }),
                ),
                param("body", byte_array_schema()),
            ],
            None,
        );
        let encoding =
            ParamEncoding::parse(json!({ "body": "application/json" }).as_object().unwrap())
                .unwrap();
        let schema = derive_input_schema(&function, &cfg_with_param_encoding(encoding)).unwrap();
        // url and count keep their WIT types; body becomes any.
        assert_eq!(schema["properties"]["url"]["type"], "string");
        assert_eq!(schema["properties"]["count"]["type"], "number");
        assert_eq!(schema["properties"]["count"]["minimum"], 0);
        assert!(
            schema["properties"]["body"]
                .as_object()
                .map(|o| o.is_empty())
                .unwrap_or(false)
        );
    }

    // ----- Output schema derivation -----

    #[test]
    fn derive_output_structured_template_preserves_inner_types() {
        let wit_result = json!({
            "type": "object",
            "properties": {
                "name": { "type": "string" },
                "age": { "type": "number" }
            }
        });
        let function = fn_with(vec![], Some(wit_result));
        let result_mapping = json!({
            "body": {
                "label": "{name}",
                "year": "{age}"
            }
        });
        let schema = derive_output_schema(&function, &cfg_with_result_mapping(result_mapping))
            .unwrap()
            .expect("body slot present");
        assert_eq!(schema["type"], "object");
        assert_eq!(schema["properties"]["label"]["type"], "string");
        assert_eq!(schema["properties"]["year"]["type"], "number");
    }

    #[test]
    fn derive_output_unwraps_result_ok_arm() {
        // function returns result<T, string>; template references into T.
        let wit_result = json!({
            "oneOf": [
                {
                    "type": "object",
                    "properties": {
                        "ok": {
                            "type": "object",
                            "properties": { "id": { "type": "string" } },
                            "required": ["id"]
                        }
                    },
                    "required": ["ok"]
                },
                {
                    "type": "object",
                    "properties": { "error": { "type": "string" } },
                    "required": ["error"]
                }
            ]
        });
        let function = fn_with(vec![], Some(wit_result));
        let result_mapping = json!({ "body": { "id": "{id}" } });
        let schema = derive_output_schema(&function, &cfg_with_result_mapping(result_mapping))
            .unwrap()
            .expect("body slot present");
        assert_eq!(schema["properties"]["id"]["type"], "string");
    }

    #[test]
    fn derive_output_interpolating_template_is_string() {
        let wit_result = json!({
            "type": "object",
            "properties": { "id": { "type": "number" } }
        });
        let function = fn_with(vec![], Some(wit_result));
        let result_mapping = json!({ "body": { "label": "id={id}" } });
        let schema = derive_output_schema(&function, &cfg_with_result_mapping(result_mapping))
            .unwrap()
            .expect("body slot present");
        assert_eq!(schema["properties"]["label"]["type"], "string");
    }

    #[test]
    fn derive_output_body_slot_absent_returns_none() {
        let wit_result = json!({
            "type": "object",
            "properties": { "id": { "type": "string" } }
        });
        let function = fn_with(vec![], Some(wit_result));
        let result_mapping = json!({ "headers": { "x-trace": "{id}" } });
        let derived =
            derive_output_schema(&function, &cfg_with_result_mapping(result_mapping)).unwrap();
        assert!(derived.is_none(), "no body slot means no output schema");
    }

    #[test]
    fn derive_output_body_null_returns_none() {
        let wit_result = json!({
            "type": "object",
            "properties": { "id": { "type": "string" } }
        });
        let function = fn_with(vec![], Some(wit_result));
        let result_mapping = json!({ "body": null });
        let derived =
            derive_output_schema(&function, &cfg_with_result_mapping(result_mapping)).unwrap();
        assert!(derived.is_none(), "body: null means no output schema");
    }

    #[test]
    fn derive_output_body_empty_string_returns_none() {
        let wit_result = json!({
            "type": "object",
            "properties": { "id": { "type": "string" } }
        });
        let function = fn_with(vec![], Some(wit_result));
        let result_mapping = json!({ "body": "" });
        let derived =
            derive_output_schema(&function, &cfg_with_result_mapping(result_mapping)).unwrap();
        assert!(derived.is_none(), "body: \"\" means no output schema");
    }

    #[test]
    fn derive_output_no_wit_result_with_body_template_errors() {
        let function = fn_with(vec![], None);
        let result_mapping = json!({ "body": { "x": "y" } });
        let err =
            derive_output_schema(&function, &cfg_with_result_mapping(result_mapping)).unwrap_err();
        assert!(err.contains("no return type"), "unexpected: {err}");
    }

    #[test]
    fn derive_output_no_wit_result_with_no_body_is_ok() {
        // Empty body is meaningful even when the WIT function has no return.
        let function = fn_with(vec![], None);
        let result_mapping = json!({ "headers": { "x-trace": "static" } });
        let derived =
            derive_output_schema(&function, &cfg_with_result_mapping(result_mapping)).unwrap();
        assert!(derived.is_none());
    }

    // ----- Output schema derivation with result-decoding -----

    // A WIT result schema describing { status: number, payload: list<u8> }.
    fn envelope_schema() -> Value {
        json!({
            "type": "object",
            "properties": {
                "status": { "type": "number" },
                "payload": {
                    "type": "array",
                    "items": { "type": "number", "minimum": 0, "maximum": 255 }
                }
            }
        })
    }

    #[test]
    fn derive_output_with_text_plain_decoding_makes_field_string() {
        let function = fn_with(vec![], Some(envelope_schema()));
        let decoding = crate::mapping::ResultDecoding::parse(
            json!({ "payload": "text/plain" }).as_object().unwrap(),
        )
        .unwrap();
        let result_mapping = json!({ "body": "{payload}" });
        let derived = derive_output_schema(
            &function,
            &cfg_with_result_mapping_and_decoding(result_mapping, decoding),
        )
        .unwrap()
        .expect("body present");
        assert_eq!(derived["type"], "string");
    }

    #[test]
    fn derive_output_with_application_json_decoding_makes_field_any() {
        let function = fn_with(vec![], Some(envelope_schema()));
        let decoding = crate::mapping::ResultDecoding::parse(
            json!({ "payload": "application/json" })
                .as_object()
                .unwrap(),
        )
        .unwrap();
        // Lift the decoded payload into the body via a path-only template.
        // The derived schema is `{}` (any), because the JSON shape isn't
        // statically known.
        let result_mapping = json!({ "body": "{payload}" });
        let derived = derive_output_schema(
            &function,
            &cfg_with_result_mapping_and_decoding(result_mapping, decoding),
        )
        .unwrap()
        .expect("body present");
        assert!(
            derived.as_object().map(|o| o.is_empty()).unwrap_or(false),
            "expected empty (any) schema, got {derived}"
        );
    }

    #[test]
    fn derive_output_with_path_decoding_makes_field_any() {
        // Schema includes a ct field that holds the content-type at runtime.
        let schema = json!({
            "type": "object",
            "properties": {
                "ct": { "type": "string" },
                "payload": {
                    "type": "array",
                    "items": { "type": "number", "minimum": 0, "maximum": 255 }
                }
            }
        });
        let function = fn_with(vec![], Some(schema));
        let decoding = crate::mapping::ResultDecoding::parse(
            json!({ "payload": "{ct}" }).as_object().unwrap(),
        )
        .unwrap();
        let result_mapping = json!({ "body": "{payload}" });
        let derived = derive_output_schema(
            &function,
            &cfg_with_result_mapping_and_decoding(result_mapping, decoding),
        )
        .unwrap()
        .expect("body present");
        assert!(
            derived.as_object().map(|o| o.is_empty()).unwrap_or(false),
            "expected empty (any) schema, got {derived}"
        );
    }

    // ----- Output schema derivation without result-mapping -----

    fn cfg_with_result_decoding(decoding: ResultDecoding) -> MappingConfig {
        MappingConfig {
            result_decoding: Some(decoding),
            ..Default::default()
        }
    }

    #[test]
    fn derive_output_no_mapping_no_decoding_returns_wit_result_schema() {
        let function = fn_with(vec![], Some(envelope_schema()));
        let derived = derive_output_schema(&function, &MappingConfig::default()).unwrap();
        assert_eq!(derived, Some(envelope_schema()));
    }

    #[test]
    fn derive_output_no_mapping_with_text_plain_decoding_swaps_field_to_string() {
        // No result-mapping; result-decoding alone modifies the advertised
        // output schema by replacing the decoded field's WIT type with the
        // encoding-rule schema.
        let function = fn_with(vec![], Some(envelope_schema()));
        let decoding =
            ResultDecoding::parse(json!({ "payload": "text/plain" }).as_object().unwrap()).unwrap();
        let derived = derive_output_schema(&function, &cfg_with_result_decoding(decoding))
            .unwrap()
            .expect("output schema");
        assert_eq!(derived["type"], "object");
        assert_eq!(derived["properties"]["payload"]["type"], "string");
        // status field keeps its WIT type.
        assert_eq!(derived["properties"]["status"]["type"], "number");
    }

    #[test]
    fn derive_output_no_mapping_with_application_json_decoding_swaps_field_to_any() {
        let function = fn_with(vec![], Some(envelope_schema()));
        let decoding = ResultDecoding::parse(
            json!({ "payload": "application/json" })
                .as_object()
                .unwrap(),
        )
        .unwrap();
        let derived = derive_output_schema(&function, &cfg_with_result_decoding(decoding))
            .unwrap()
            .expect("output schema");
        assert!(
            derived["properties"]["payload"]
                .as_object()
                .map(|o| o.is_empty())
                .unwrap_or(false),
            "expected empty (any), got {derived}"
        );
    }

    #[test]
    fn derive_output_no_mapping_no_wit_result_returns_none() {
        let function = fn_with(vec![], None);
        let derived = derive_output_schema(&function, &MappingConfig::default()).unwrap();
        assert!(derived.is_none());
    }

    // ----- Alignment -----

    #[test]
    fn align_same_object_shapes_passes() {
        let declared = json!({
            "type": "object",
            "properties": {
                "name": { "type": "string", "description": "the name" }
            }
        });
        let derived = json!({
            "type": "object",
            "properties": { "name": { "type": "string" } }
        });
        validate_structural_alignment(&declared, &derived).unwrap();
    }

    #[test]
    fn align_extra_property_errors() {
        let declared = json!({
            "type": "object",
            "properties": {
                "name": { "type": "string" },
                "extra": { "type": "string" }
            }
        });
        let derived = json!({
            "type": "object",
            "properties": { "name": { "type": "string" } }
        });
        let err = validate_structural_alignment(&declared, &derived).unwrap_err();
        assert!(err.contains("property set mismatch"), "unexpected: {err}");
    }

    #[test]
    fn align_missing_property_errors() {
        let declared = json!({
            "type": "object",
            "properties": { "name": { "type": "string" } }
        });
        let derived = json!({
            "type": "object",
            "properties": {
                "name": { "type": "string" },
                "age": { "type": "number" }
            }
        });
        let err = validate_structural_alignment(&declared, &derived).unwrap_err();
        assert!(err.contains("property set mismatch"), "unexpected: {err}");
    }

    #[test]
    fn align_type_mismatch_errors() {
        let declared = json!({
            "type": "object",
            "properties": { "age": { "type": "boolean" } }
        });
        let derived = json!({
            "type": "object",
            "properties": { "age": { "type": "number" } }
        });
        let err = validate_structural_alignment(&declared, &derived).unwrap_err();
        assert!(err.contains("type mismatch"), "unexpected: {err}");
    }

    #[test]
    fn align_declared_string_at_number_position_passes() {
        // Surface-side `coerce_value` will stringify the value at
        // serialization time. The looser-declared-string case is allowed.
        let declared = json!({
            "type": "object",
            "properties": { "id": { "type": "string" } }
        });
        let derived = json!({
            "type": "object",
            "properties": { "id": { "type": "number" } }
        });
        validate_structural_alignment(&declared, &derived).unwrap();
    }

    #[test]
    fn align_tuple_length_mismatch_errors() {
        let declared = json!({
            "type": "array",
            "prefixItems": [{ "type": "string" }]
        });
        let derived = json!({
            "type": "array",
            "prefixItems": [{ "type": "string" }, { "type": "number" }]
        });
        let err = validate_structural_alignment(&declared, &derived).unwrap_err();
        assert!(err.contains("tuple length mismatch"), "unexpected: {err}");
    }

    #[test]
    fn align_nested_alignment_passes() {
        let declared = json!({
            "type": "object",
            "properties": {
                "user": {
                    "type": "object",
                    "properties": {
                        "email": { "type": "string", "format": "email" }
                    }
                }
            }
        });
        let derived = json!({
            "type": "object",
            "properties": {
                "user": {
                    "type": "object",
                    "properties": { "email": { "type": "string" } }
                }
            }
        });
        validate_structural_alignment(&declared, &derived).unwrap();
    }
}
