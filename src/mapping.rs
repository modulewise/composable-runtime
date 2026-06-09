//! Messaging Mapper: the boundary between messaging and WIT-typed components.
//!
//! `MessageMapper` translates an inbound [`Message`] into an [`Invocation`] of
//! a specific WIT function, and translates the function's return value back
//! into a reply [`Message`]. The component knows nothing about messages, and
//! the messaging layer knows nothing about WIT.
//!
//! ## Mapping pipeline
//!
//! The mapper applies up to four user-declared blocks in pipeline order,
//! bundled into a single [`MappingConfig`]:
//!
//! Inbound (Message => WIT call):
//!   1. [`ParamMapping`]: per-arg templates that build WIT args by reading
//!      paths into the inbound Message. Without an entry for a given WIT
//!      param, the arg is name-matched against the parsed Message body.
//!   2. [`ParamEncoding`]: per-arg content-type specs that encode the
//!      assembled value as bytes (for any WIT param typed as `list<u8>`).
//!
//! Outbound (WIT result => reply Message):
//!   3. [`ResultDecoding`]: per-field content-type specs that decode any
//!      `list<u8>` field on the WIT result. The decoded value replaces the
//!      bytes in the WIT result before result-mapping runs.
//!   4. result_mapping: a structural `body` / `headers` table that produces
//!      the reply Message. Each slot can be a single source-path string
//!      (bulk-lift) or a sub-table of target-name => source-path entries
//!      (cherry-pick).
//!
//! With none declared, name-match drives inbound and the WIT result becomes
//! the reply body verbatim.
//!
//! ## Templates
//!
//! Templates reference values via `{path}` syntax. Paths use a uniform
//! dotted grammar across every block:
//!
//! - `body.user.email` => dot-name segments for normal keys.
//! - `headers["foo.bar"]` => bracket-quoted-string for keys containing
//!   characters that are not `[A-Za-z0-9_-]`.
//! - `body.items[3].name` => bracket-integer for array indices.
//!
//! The first segment names the source root:
//!
//! - `param-mapping`: source is the inbound Message; first segment must be
//!   `body` or `headers`.
//! - `result-mapping` and `result-decoding` (path-form): source is the WIT
//!   result; first segment is a top-level field.
//! - `param-encoding` (path-form): source is the assembled WIT args; first
//!   segment is a WIT param name.
//!
//! Defaulting is supported via `{path | <literal>}`:
//!
//! - `{body.user.email | "anonymous"}` => use the literal value when the
//!   path is missing.
//!
//! ## `result-mapping` shape
//!
//! When result-mapping is declared, it takes over building the reply Message.
//! The block has two structural sub-keys: `body` and `headers`.
//!
//! - `body` absent (or `null` or `""`) => empty body (zero bytes).
//! - `body = "<path>"` => bulk-lift that source path as the body.
//! - `body = { <target> = "<path>", ... }` => cherry-pick.
//! - `headers` follows the same shape; entries become reply Message headers.
//!
//! ## `result-decoding` and `param-encoding` shape
//!
//! Each entry's value is a content-type spec, in one of two forms:
//!
//! - A literal content-type: `payload = "application/json"`.
//! - A path that resolves at runtime to a content-type string:
//!   `payload = "{headers.content-type}"`.
//!
//! Supported content-types: `application/json` and `text/plain`.

use std::collections::HashMap;

use serde_json::{Map, Value};

use crate::message::{Message, MessageBuilder, MessageHeaders};
use crate::types::{Component, FunctionParam};

/// User-declared per-arg template config: arg name -> template value.
///
/// A template value can be a literal, a string containing `{path}`
/// placeholders, or a structured value with embedded placeholders. A missing
/// entry for an arg means fall back to name-match against the Message body.
pub type ParamMapping = HashMap<String, Value>;

/// A resolved function call: function key and positional JSON arguments.
#[derive(Debug)]
pub struct Invocation {
    pub function_key: String,
    pub args: Vec<Value>,
}

// How to determine the content-type for one `result-decoding` or
// `param-encoding` entry.
#[derive(Debug, Clone)]
pub(crate) enum ContentTypeSpec {
    // A hardcoded content-type value (e.g. `"application/json"`).
    Literal(String),
    // A path into the source whose value at runtime supplies the
    // content-type string. The source is the WIT result for
    // result-decoding, or the assembled WIT args for param-encoding.
    Path(Vec<PathSegment>),
}

/// Per-field decoding config applied to a WIT result before result-mapping.
///
/// Each entry names a WIT-result field whose value is a byte array and a
/// content-type spec describing how to decode it. The decoded value replaces
/// the byte array in the WIT result.
#[derive(Debug, Clone)]
pub struct ResultDecoding(pub(crate) HashMap<String, ContentTypeSpec>);

impl ResultDecoding {
    /// Parse a `result-decoding` config block into a [`ResultDecoding`].
    ///
    /// If the value is path-based (`{...}`), it is treated as a reference into
    /// the WIT result. Otherwise it is treated as a literal content-type.
    pub fn parse(map: &Map<String, Value>) -> Result<Self, String> {
        let inner = parse_content_type_specs(map, "result-decoding")?;
        Ok(Self(inner))
    }
}

/// The four mapping-related configs that bridge a component invocation
/// to/from the WIT boundary. Listed in pipeline order: inbound side first
/// (`param_mapping` followed by `param_encoding`), then outbound side
/// (`result_decoding` followed by `result_mapping`).
#[derive(Debug, Clone, Default)]
pub struct MappingConfig {
    pub param_mapping: Option<ParamMapping>,
    pub param_encoding: Option<ParamEncoding>,
    pub result_decoding: Option<ResultDecoding>,
    pub result_mapping: Option<Value>,
}

/// Per-param encoding config applied to assembled WIT args after param-mapping.
///
/// Each entry names a WIT param whose assembled value should be encoded as
/// bytes per a content-type spec. The encoded bytes replace the structured
/// value at that arg's position.
#[derive(Debug, Clone)]
pub struct ParamEncoding(pub(crate) HashMap<String, ContentTypeSpec>);

impl ParamEncoding {
    /// Parse a `param-encoding` config block into a [`ParamEncoding`].
    ///
    /// Same value grammar as `result-decoding`: `{...}` is a path into the
    /// assembled WIT args; otherwise the value is a literal content-type.
    pub fn parse(map: &Map<String, Value>) -> Result<Self, String> {
        let inner = parse_content_type_specs(map, "param-encoding")?;
        Ok(Self(inner))
    }
}

// Shared parser for ResultDecoding and ParamEncoding entries.
fn parse_content_type_specs(
    map: &Map<String, Value>,
    block_name: &str,
) -> Result<HashMap<String, ContentTypeSpec>, String> {
    let mut out = HashMap::new();
    for (field, value) in map {
        let s = value
            .as_str()
            .ok_or_else(|| format!("{block_name} entry '{field}' must be a string, got {value}"))?;
        let spec = if let Some(inner) = path_only_inner(s) {
            let segments =
                parse_path(inner).map_err(|e| format!("{block_name} entry '{field}': {e}"))?;
            ContentTypeSpec::Path(segments)
        } else {
            ContentTypeSpec::Literal(s.to_string())
        };
        out.insert(field.clone(), spec);
    }
    Ok(out)
}

// Returns the inner content of a path-only `{...}` template string, or None.
fn path_only_inner(s: &str) -> Option<&str> {
    let inner = s.strip_prefix('{')?.strip_suffix('}')?;
    if inner.contains('{') || inner.contains('}') {
        return None;
    }
    Some(inner)
}

// Whether a content-type literal is one that result-decoding / param-encoding
// can apply.
fn is_supported_content_type(ct: &str) -> bool {
    matches!(ct, "application/json" | "text/plain")
}

// Check whether a WIT param's JSON Schema represents a byte array (list<u8>).
// A byte array is `{ type: "array", items: { type: "number", minimum: 0, maximum: 255 } }`.
fn is_byte_array_param_schema(schema: &Value) -> bool {
    let Some(obj) = schema.as_object() else {
        return false;
    };
    if obj.get("type").and_then(|t| t.as_str()) != Some("array") {
        return false;
    }
    let Some(items) = obj.get("items") else {
        return false;
    };
    items.get("type").and_then(|t| t.as_str()) == Some("number")
        && items.get("minimum").and_then(|m| m.as_u64()) == Some(0)
        && items.get("maximum").and_then(|m| m.as_u64()) == Some(255)
}

// Validate that a param-encoding path exists in the assembled-args view:
// first segment is a WIT param name; subsequent segments traverse that
// param's schema.
fn validate_param_encoding_path(
    function: &crate::types::Function,
    segments: &[PathSegment],
) -> Result<(), String> {
    let first = segments.first().ok_or_else(|| "empty path".to_string())?;
    let param_name = match first {
        PathSegment::Key(k) => k.as_str(),
        PathSegment::Index(i) => {
            return Err(format!(
                "first path segment must be a WIT param name, got index [{i}]"
            ));
        }
    };
    let param = function
        .params()
        .iter()
        .find(|p| p.name == param_name)
        .ok_or_else(|| format!("no such WIT param '{param_name}'"))?;
    crate::schema::validate_path_exists(&param.json_schema, &segments[1..])
}

// Apply result-decoding to a WIT result. First resolves every content-type
// spec against the original wit_result, then decodes each named field and
// replaces its byte-array value with the decoded value.
//
// Returns a new `Value` with the decoded fields in place. Runtime errors:
//   - content-type path is missing or null at runtime
//   - content-type value is not a string
//   - content-type value is not supported
//   - field's value is not a byte array
//   - byte payload is malformed for the declared content-type
fn apply_result_decoding(wit_result: &Value, decoding: &ResultDecoding) -> Result<Value, String> {
    // Phase 1: resolve all content-types against the original wit_result.
    let mut resolved: HashMap<&str, String> = HashMap::with_capacity(decoding.0.len());
    for (field_name, spec) in &decoding.0 {
        let ct = match spec {
            ContentTypeSpec::Literal(s) => s.clone(),
            ContentTypeSpec::Path(segments) => {
                let mut current = wit_result;
                for seg in segments {
                    let next = match seg {
                        PathSegment::Key(k) => current.get(k),
                        PathSegment::Index(i) => current.get(*i),
                    };
                    current = next.ok_or_else(|| {
                        format!("result-decoding '{field_name}': content-type path did not resolve")
                    })?;
                }
                match current {
                    Value::String(s) => s.clone(),
                    Value::Null => {
                        return Err(format!(
                            "result-decoding '{field_name}': content-type path resolved to null"
                        ));
                    }
                    other => {
                        return Err(format!(
                            "result-decoding '{field_name}': content-type path must resolve to a string, got {other}"
                        ));
                    }
                }
            }
        };
        if !is_supported_content_type(&ct) {
            return Err(format!(
                "result-decoding '{field_name}': content-type '{ct}' is not supported"
            ));
        }
        resolved.insert(field_name.as_str(), ct);
    }

    // Phase 2: decode each field and swap in the decoded value.
    let mut out = wit_result.clone();
    let out_obj = out
        .as_object_mut()
        .ok_or_else(|| "result-decoding requires the WIT result to be an object".to_string())?;
    for (field_name, ct) in resolved {
        let bytes = value_to_bytes(out_obj.get(field_name).ok_or_else(|| {
            format!("result-decoding '{field_name}': field not present at runtime")
        })?)
        .ok_or_else(|| {
            format!("result-decoding '{field_name}': field value is not a byte array")
        })?;
        let decoded = match ct.as_str() {
            "application/json" => serde_json::from_slice::<Value>(&bytes).map_err(|e| {
                format!("result-decoding '{field_name}': malformed application/json bytes: {e}")
            })?,
            "text/plain" => {
                let s = std::str::from_utf8(&bytes).map_err(|e| {
                    format!("result-decoding '{field_name}': malformed text/plain bytes (not UTF-8): {e}")
                })?;
                Value::String(s.to_string())
            }
            _ => unreachable!("content-type already validated above"),
        };
        out_obj.insert(field_name.to_string(), decoded);
    }
    Ok(out)
}

// Convert a JSON array of integers into a Vec<u8> if all are in 0..=255.
fn value_to_bytes(v: &Value) -> Option<Vec<u8>> {
    let arr = v.as_array()?;
    let mut out = Vec::with_capacity(arr.len());
    for item in arr {
        let n = item.as_u64()?;
        if n > 255 {
            return None;
        }
        out.push(n as u8);
    }
    Some(out)
}

// Apply param-encoding to assembled WIT args. First resolves every
// content-type spec against the original assembled args, then encodes each
// named arg and replaces its value with the byte array.
//
// The `args` parameter is the positional args list (one per WIT param);
// `param_names` is the WIT param names in the same order, used to resolve
// path segments whose first segment names a param.
fn apply_param_encoding(
    args: &mut [Value],
    param_names: &[String],
    encoding: &ParamEncoding,
) -> Result<(), String> {
    // Phase 1: resolve all content-types against the original args view.
    let mut resolved: HashMap<&str, String> = HashMap::with_capacity(encoding.0.len());
    for (param_name, spec) in &encoding.0 {
        let ct = match spec {
            ContentTypeSpec::Literal(s) => s.clone(),
            ContentTypeSpec::Path(segments) => resolve_param_path(args, param_names, segments)
                .map_err(|e| format!("param-encoding '{param_name}': {e}"))?,
        };
        if !is_supported_content_type(&ct) {
            return Err(format!(
                "param-encoding '{param_name}': content-type '{ct}' is not supported"
            ));
        }
        resolved.insert(param_name.as_str(), ct);
    }

    // Phase 2: encode each named arg and replace its value.
    for (param_name, ct) in resolved {
        let idx = param_names
            .iter()
            .position(|n| n == param_name)
            .ok_or_else(|| format!("param-encoding '{param_name}': no such param at runtime"))?;
        let encoded = match ct.as_str() {
            "application/json" => serde_json::to_vec(&args[idx]).map_err(|e| {
                format!("param-encoding '{param_name}': failed to encode as application/json: {e}")
            })?,
            "text/plain" => match &args[idx] {
                Value::String(s) => s.as_bytes().to_vec(),
                Value::Null => {
                    return Err(format!(
                        "param-encoding '{param_name}': value is null; cannot encode as text/plain"
                    ));
                }
                other => {
                    return Err(format!(
                        "param-encoding '{param_name}': cannot encode {other} as text/plain (expected string)"
                    ));
                }
            },
            _ => unreachable!("content-type already validated above"),
        };
        args[idx] = Value::Array(encoded.into_iter().map(|b| Value::from(b as u64)).collect());
    }
    Ok(())
}

// Resolve a param-encoding path against the assembled args. The first segment
// names a WIT param; subsequent segments traverse that arg's value.
fn resolve_param_path(
    args: &[Value],
    param_names: &[String],
    segments: &[PathSegment],
) -> Result<String, String> {
    let first = segments
        .first()
        .ok_or_else(|| "empty content-type path".to_string())?;
    let param_name = match first {
        PathSegment::Key(k) => k.as_str(),
        PathSegment::Index(i) => {
            return Err(format!(
                "first path segment must be a WIT param name, got index [{i}]"
            ));
        }
    };
    let idx = param_names
        .iter()
        .position(|n| n == param_name)
        .ok_or_else(|| format!("content-type path references unknown param '{param_name}'"))?;
    let mut current = &args[idx];
    for seg in &segments[1..] {
        let next = match seg {
            PathSegment::Key(k) => current.get(k),
            PathSegment::Index(i) => current.get(*i),
        };
        current = next.ok_or_else(|| "content-type path did not resolve".to_string())?;
    }
    match current {
        Value::String(s) => Ok(s.clone()),
        Value::Null => Err("content-type path resolved to null".to_string()),
        other => Err(format!(
            "content-type path must resolve to a string, got {other}"
        )),
    }
}

/// Messaging Mapper that mediates between the messaging layer and a
/// component's typed WIT function.
///
/// One mapper instance is bound to one WIT function. The component knows
/// nothing about messages, and the messaging layer knows nothing about WIT.
pub struct MessageMapper {
    function_key: String,
    params: Vec<FunctionParam>,
    param_mapping: Option<ParamMapping>,
    param_encoding: Option<ParamEncoding>,
    result_decoding: Option<ResultDecoding>,
    result_mapping: Option<Value>,
}

impl MessageMapper {
    /// Create a mapper for a specific function on a component.
    ///
    /// If `function_key` is `None`, the component must export exactly one
    /// function. `config` carries the four optional mapping-related blocks.
    /// See [`MappingConfig`].
    pub fn from_component(
        component: &Component,
        function_key: Option<String>,
        config: MappingConfig,
    ) -> Result<Self, String> {
        let MappingConfig {
            param_mapping,
            param_encoding,
            result_decoding,
            result_mapping,
        } = config;
        let function_key = match function_key {
            Some(key) => key,
            None => {
                let functions = &component.functions;
                if functions.len() != 1 {
                    return Err(format!(
                        "mapper must specify a 'function' when component exports more than one: \
                         '{}' has {}",
                        component.metadata.name,
                        functions.len()
                    ));
                }
                functions.keys().next().unwrap().clone()
            }
        };

        let function = component.functions.get(&function_key).ok_or_else(|| {
            format!(
                "function '{}' not found in '{}'",
                function_key, component.metadata.name
            )
        })?;

        // Resolution-time validation for result-decoding:
        // - Each key must reference a `list<u8>` field on the WIT result.
        // - Each Path spec must reference an existing path on the WIT result.
        // - Each Literal spec must be a supported content-type.
        if let Some(decoding) = &result_decoding {
            let result_schema = function.result().ok_or_else(|| {
                format!(
                    "function '{}' has no return type; result-decoding requires one",
                    function.function_name()
                )
            })?;
            for (field_name, spec) in &decoding.0 {
                crate::schema::validate_byte_array_field(result_schema, field_name)?;
                match spec {
                    ContentTypeSpec::Path(segments) => {
                        crate::schema::validate_path_exists(result_schema, segments).map_err(
                            |e| {
                                format!(
                                    "result-decoding entry '{field_name}': content-type path invalid: {e}"
                                )
                            },
                        )?;
                    }
                    ContentTypeSpec::Literal(ct) => {
                        if !is_supported_content_type(ct) {
                            return Err(format!(
                                "result-decoding entry '{field_name}': content-type '{ct}' is not supported (supported: application/json, text/plain)"
                            ));
                        }
                    }
                }
            }
        }

        // Resolution-time validation for param-encoding:
        // - Each key must reference a WIT param whose type is `list<u8>`.
        // - Each Path spec must reference an existing path in the WIT args.
        // - Each Literal spec must be a supported content-type.
        if let Some(encoding) = &param_encoding {
            for (param_name, spec) in &encoding.0 {
                let param = function
                    .params()
                    .iter()
                    .find(|p| p.name == *param_name)
                    .ok_or_else(|| {
                        format!(
                            "param-encoding entry '{param_name}': no such WIT param on function '{}'",
                            function.function_name()
                        )
                    })?;
                if !is_byte_array_param_schema(&param.json_schema) {
                    return Err(format!(
                        "param-encoding entry '{param_name}': WIT param must be a byte array (list<u8>)"
                    ));
                }
                match spec {
                    ContentTypeSpec::Path(segments) => {
                        validate_param_encoding_path(function, segments).map_err(|e| {
                            format!(
                                "param-encoding entry '{param_name}': content-type path invalid: {e}"
                            )
                        })?;
                    }
                    ContentTypeSpec::Literal(ct) => {
                        if !is_supported_content_type(ct) {
                            return Err(format!(
                                "param-encoding entry '{param_name}': content-type '{ct}' is not supported (supported: application/json, text/plain)"
                            ));
                        }
                    }
                }
            }
        }

        Ok(Self {
            function_key,
            params: function.params().to_vec(),
            param_mapping,
            param_encoding,
            result_decoding,
            result_mapping,
        })
    }

    /// The function key this mapper is bound to.
    pub fn function_key(&self) -> &str {
        &self.function_key
    }

    /// Translate an inbound [`Message`] into an [`Invocation`].
    ///
    /// The Message body is parsed per its content-type and is reachable from
    /// templates via paths starting with `body`. Message headers are reachable
    /// via paths starting with `headers`. Without templates, each WIT param
    /// resolves by name-match against the parsed body.
    pub fn to_invocation(&self, msg: &Message) -> Result<Invocation, String> {
        if self.params.is_empty() {
            return Ok(Invocation {
                function_key: self.function_key.clone(),
                args: vec![],
            });
        }

        let parsed_body = parse_body(msg)?;
        let headers = headers_as_value(msg);

        // Build a source object with `body` and `headers` for template paths.
        let template_source = Value::Object(
            [
                ("body".to_string(), parsed_body.clone()),
                ("headers".to_string(), headers),
            ]
            .into_iter()
            .collect(),
        );

        // Name-match uses the (possibly wrapped) body directly.
        let name_match_body = self.normalize_body(parsed_body)?;

        let mut args = Vec::with_capacity(self.params.len());
        for param in &self.params {
            let value = match self.param_mapping.as_ref().and_then(|m| m.get(&param.name)) {
                Some(template) => substitute_value(template, &template_source)?,
                None => name_match(param, &name_match_body)?,
            };
            args.push(value);
        }

        if let Some(encoding) = &self.param_encoding {
            let param_names: Vec<String> = self.params.iter().map(|p| p.name.clone()).collect();
            apply_param_encoding(&mut args, &param_names, encoding)?;
        }

        for (param, arg) in self.params.iter().zip(args.iter_mut()) {
            let expects_string =
                param.json_schema.get("type").and_then(|t| t.as_str()) == Some("string");
            // Null represents an absent optional arg and must not be stringified.
            if expects_string && !matches!(arg, Value::String(_) | Value::Null) {
                *arg = Value::String(serde_json::to_string(arg).map_err(|e| {
                    format!(
                        "failed to stringify value for string arg '{}': {e}",
                        param.name
                    )
                })?);
            }
        }

        Ok(Invocation {
            function_key: self.function_key.clone(),
            args,
        })
    }

    /// Translate a WIT-result [`Value`] into a reply [`Message`].
    ///
    /// When `result_mapping` is absent, the WIT result becomes the reply body
    /// as-is (serialized per content-type).
    ///
    /// When `result_mapping` is present, it takes over: the `body` and
    /// `headers` slots define the reply Message:
    ///   - `body` slot absent, `null`, or `""` => empty body (zero bytes).
    ///   - `body` slot containing a template => substitute against the WIT
    ///     result and serialize.
    ///   - `headers` slot containing a template => substitute against the WIT
    ///     result; the resulting object's key-value pairs become Message
    ///     headers.
    ///
    /// Mapped headers are applied first, then `propagated` headers. A
    /// `propagated` header with the same name as a mapped header overwrites
    /// the mapped value.
    pub fn from_invocation_result(
        &self,
        wit_result: &Value,
        propagated: HashMap<String, String>,
    ) -> Result<Message, String> {
        let content_type = propagated
            .get(MessageHeaders::CONTENT_TYPE)
            .map(String::as_str)
            .unwrap_or("application/json");

        // Apply result-decoding (if any) before mapping. Each declared field
        // gets its byte-array value replaced with the decoded value, so
        // downstream result-mapping templates traverse the decoded shape.
        let decoded = match &self.result_decoding {
            None => None,
            Some(decoding) => Some(apply_result_decoding(wit_result, decoding)?),
        };
        let source = decoded.as_ref().unwrap_or(wit_result);

        let (body_bytes, mapped_headers) = match &self.result_mapping {
            None => (serialize_body(source, content_type)?, Map::new()),
            Some(mapping) => {
                let body_bytes = match mapping.get("body") {
                    None | Some(Value::Null) => Vec::new(),
                    Some(Value::String(s)) if s.is_empty() => Vec::new(),
                    Some(template) => {
                        let body_value = substitute_value(template, source)?;
                        serialize_body(&body_value, content_type)?
                    }
                };
                let mapped_headers = match mapping.get("headers") {
                    None | Some(Value::Null) => Map::new(),
                    Some(template) => {
                        let headers_value = substitute_value(template, source)?;
                        headers_value.as_object().cloned().ok_or_else(|| {
                            format!(
                                "result-mapping 'headers' must produce an object, got {headers_value}"
                            )
                        })?
                    }
                };
                (body_bytes, mapped_headers)
            }
        };

        let mut builder = MessageBuilder::new(body_bytes);
        for (key, value) in mapped_headers {
            let value_str = match value {
                Value::String(s) => s,
                other => other.to_string(),
            };
            builder = builder.header(key, value_str);
        }
        for (key, value) in propagated {
            builder = builder.header(key, value);
        }
        Ok(builder.build())
    }

    // Without a mapping, normalize the body to support name-match semantics:
    //   - Single-param function with a non-object body, OR an object body that
    //     does NOT contain the param's name: wrap as `{ <param.name>: body }`.
    //     This lets name-match still pick up the body as the single arg.
    //   - Multi-param function: body must be an object (so per-param
    //     name-match can find each field). A non-object body for a multi-param
    //     function will return an error.
    //   - With a mapping configured: pass body through unchanged.
    fn normalize_body(&self, body: Value) -> Result<Value, String> {
        if self.param_mapping.is_some() {
            return Ok(body);
        }
        if self.params.len() == 1 {
            let first = &self.params[0];
            return Ok(match &body {
                Value::Object(obj) if obj.contains_key(&first.name) => body,
                _ => Value::Object([(first.name.clone(), body)].into_iter().collect()),
            });
        }
        if !body.is_object() {
            return Err(format!(
                "non-object body cannot be mapped to {} parameters",
                self.params.len()
            ));
        }
        Ok(body)
    }
}

// Serialize a JSON Value into reply-body bytes per content-type. Used by
// `from_invocation_result`.
fn serialize_body(value: &Value, content_type: &str) -> Result<Vec<u8>, String> {
    match content_type {
        "text/plain" => match value {
            Value::String(s) => Ok(s.as_bytes().to_vec()),
            other => Ok(other.to_string().into_bytes()),
        },
        _ => serde_json::to_vec(value).map_err(|e| format!("failed to serialize result body: {e}")),
    }
}

fn parse_body(msg: &Message) -> Result<Value, String> {
    let content_type = msg.headers().content_type().unwrap_or("application/json");
    match content_type {
        "application/json" => {
            if msg.body().is_empty() {
                Ok(Value::Null)
            } else {
                serde_json::from_slice(msg.body())
                    .map_err(|e| format!("failed to parse body as JSON: {e}"))
            }
        }
        "text/plain" => {
            let text = std::str::from_utf8(msg.body())
                .map_err(|e| format!("body is not valid UTF-8: {e}"))?;
            Ok(Value::String(text.to_string()))
        }
        other => Err(format!("unsupported content-type: {other}")),
    }
}

fn headers_as_value(msg: &Message) -> Value {
    let mut map = Map::new();
    for (key, val) in msg.headers().iter() {
        map.insert(key.to_string(), Value::String(val.to_string()));
    }
    Value::Object(map)
}

fn name_match(param: &FunctionParam, body: &Value) -> Result<Value, String> {
    match body.get(&param.name) {
        Some(v) => Ok(v.clone()),
        None if param.is_optional => Ok(Value::Null),
        None => Err(format!(
            "missing required arg '{}' (no template and no name match in body)",
            param.name
        )),
    }
}

// Substitute placeholders in a template Value against a single source.
// Path-only templates like `{path}` preserve the looked-up value's native
// JSON type. Interpolating string templates render as strings. Objects and
// arrays recurse.
fn substitute_value(template: &Value, source: &Value) -> Result<Value, String> {
    match template {
        Value::String(s) => {
            if let Some(spec) = parse_path_only_template(s) {
                resolve(&spec, source)
            } else {
                Ok(Value::String(substitute_string(s, source)?))
            }
        }
        Value::Array(items) => items
            .iter()
            .map(|v| substitute_value(v, source))
            .collect::<Result<Vec<_>, _>>()
            .map(Value::Array),
        Value::Object(map) => {
            let mut out = Map::with_capacity(map.len());
            for (k, v) in map {
                out.insert(k.clone(), substitute_value(v, source)?);
            }
            Ok(Value::Object(out))
        }
        other => Ok(other.clone()),
    }
}

// Parse a `{...}` token into a template spec. Returns None unless the entire
// string is a single `{...}` token (path-only template). Returns the inner
// spec parsed.
fn parse_path_only_template(s: &str) -> Option<TemplateSpec> {
    let bytes = s.as_bytes();
    if bytes.len() < 3 || bytes[0] != b'{' || bytes[bytes.len() - 1] != b'}' {
        return None;
    }
    let inner = &s[1..s.len() - 1];
    if inner.contains('{') || inner.contains('}') {
        return None;
    }
    parse_spec(inner).ok()
}

// A path segment for traversing a JSON value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PathSegment {
    Key(String),
    Index(usize),
}

// A parsed `{path | "default"}` spec.
struct TemplateSpec {
    path: Vec<PathSegment>,
    raw_path: String,
    default: Option<Value>,
}

fn parse_spec(inner: &str) -> Result<TemplateSpec, String> {
    let trimmed = inner.trim();
    if trimmed.is_empty() {
        return Err("empty template reference '{}'".to_string());
    }

    // Split off `| "default"` first.
    let (head, default) = match trimmed.split_once('|') {
        Some((h, d)) => (h.trim(), Some(parse_default(d.trim())?)),
        None => (trimmed, None),
    };

    if head.is_empty() {
        return Err(format!("empty path in template reference '{{{inner}}}'"));
    }

    let path = parse_path(head)?;

    Ok(TemplateSpec {
        path,
        raw_path: head.to_string(),
        default,
    })
}

// Parse a path string into segments. Grammar:
//   - First segment: bare name (matches `[A-Za-z0-9_-]+`).
//   - Subsequent segments: `.name`, `["string"]`, or `[integer]`.
pub(crate) fn parse_path(s: &str) -> Result<Vec<PathSegment>, String> {
    let mut segments = Vec::new();
    let bytes = s.as_bytes();
    let mut i = 0;

    // First segment: bare name.
    let start = i;
    while i < bytes.len() && is_name_char(bytes[i]) {
        i += 1;
    }
    if i == start {
        return Err(format!("path '{s}' must start with a name segment"));
    }
    segments.push(PathSegment::Key(s[start..i].to_string()));

    // Subsequent segments.
    while i < bytes.len() {
        match bytes[i] {
            b'.' => {
                i += 1;
                let start = i;
                while i < bytes.len() && is_name_char(bytes[i]) {
                    i += 1;
                }
                if i == start {
                    return Err(format!("path '{s}' has empty segment after '.'"));
                }
                segments.push(PathSegment::Key(s[start..i].to_string()));
            }
            b'[' => {
                i += 1;
                if i >= bytes.len() {
                    return Err(format!("path '{s}' has unclosed '['"));
                }
                if bytes[i] == b'"' {
                    i += 1;
                    let start = i;
                    while i < bytes.len() && bytes[i] != b'"' {
                        i += 1;
                    }
                    if i >= bytes.len() {
                        return Err(format!("path '{s}' has unterminated quoted segment"));
                    }
                    let key = s[start..i].to_string();
                    i += 1; // closing quote
                    if i >= bytes.len() || bytes[i] != b']' {
                        return Err(format!("path '{s}' missing ']' after quoted segment"));
                    }
                    i += 1; // closing bracket
                    segments.push(PathSegment::Key(key));
                } else {
                    let start = i;
                    while i < bytes.len() && bytes[i].is_ascii_digit() {
                        i += 1;
                    }
                    if i == start {
                        return Err(format!(
                            "path '{s}' has invalid bracket segment (expected integer or quoted string)"
                        ));
                    }
                    let idx: usize = s[start..i].parse().map_err(|_| {
                        format!("path '{s}' has bracket-integer that is too large or invalid")
                    })?;
                    if i >= bytes.len() || bytes[i] != b']' {
                        return Err(format!("path '{s}' missing ']' after index"));
                    }
                    i += 1; // closing bracket
                    segments.push(PathSegment::Index(idx));
                }
            }
            other => {
                return Err(format!(
                    "path '{s}' has unexpected character '{}' at position {i}",
                    other as char
                ));
            }
        }
    }

    Ok(segments)
}

fn is_name_char(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b == b'-'
}

// Parse the default literal that follows `|`. Supports quoted strings,
// numeric literals, true/false, and null.
fn parse_default(s: &str) -> Result<Value, String> {
    let s = s.trim();
    if s.is_empty() {
        return Err("empty default literal after '|'".to_string());
    }
    serde_json::from_str(s).map_err(|e| format!("invalid default literal '{s}' after '|': {e}"))
}

fn resolve(spec: &TemplateSpec, source: &Value) -> Result<Value, String> {
    let mut current = source;
    for segment in &spec.path {
        match segment {
            PathSegment::Key(k) => match current.get(k) {
                Some(v) => current = v,
                None => {
                    return match &spec.default {
                        Some(default) => Ok(default.clone()),
                        None => Err(format!(
                            "template references unknown path: '{}'",
                            spec.raw_path
                        )),
                    };
                }
            },
            PathSegment::Index(idx) => match current.get(*idx) {
                Some(v) => current = v,
                None => {
                    return match &spec.default {
                        Some(default) => Ok(default.clone()),
                        None => Err(format!(
                            "template references unknown path: '{}'",
                            spec.raw_path
                        )),
                    };
                }
            },
        }
    }
    Ok(current.clone())
}

fn substitute_string(template: &str, source: &Value) -> Result<String, String> {
    let mut out = String::with_capacity(template.len());
    let bytes = template.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'{'
            && let Some(end) = template[i + 1..].find('}')
        {
            let inner = &template[i + 1..i + 1 + end];
            let spec = parse_spec(inner)?;
            out.push_str(&render_scalar(&resolve(&spec, source)?));
            i += 1 + end + 1;
            continue;
        }
        out.push(bytes[i] as char);
        i += 1;
    }
    Ok(out)
}

fn render_scalar(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::message::MessageBuilder;
    use crate::types::FunctionParam;
    use serde_json::json;

    fn param(name: &str, schema: serde_json::Value, optional: bool) -> FunctionParam {
        FunctionParam {
            name: name.into(),
            is_optional: optional,
            json_schema: schema,
        }
    }

    // WIT `list<u8>` schema, produced by composition::wit for byte-array params.
    fn byte_array_schema() -> serde_json::Value {
        json!({
            "type": "array",
            "items": { "type": "number", "minimum": 0, "maximum": 255 }
        })
    }

    // Build a MessageMapper directly from params + optional mappings, bypassing
    // component construction. Use this when tests don't need a real WIT component.
    fn mapper(
        params: Vec<FunctionParam>,
        param_mapping: Option<ParamMapping>,
        result_mapping: Option<Value>,
    ) -> MessageMapper {
        MessageMapper {
            function_key: "test-fn".to_string(),
            params,
            param_mapping,
            param_encoding: None,
            result_decoding: None,
            result_mapping,
        }
    }

    // Variant of `mapper` that also accepts a param_encoding. Used by tests
    // for the inbound encoding behavior.
    fn mapper_with_encoding(
        params: Vec<FunctionParam>,
        param_mapping: Option<ParamMapping>,
        param_encoding: ParamEncoding,
    ) -> MessageMapper {
        MessageMapper {
            function_key: "test-fn".to_string(),
            params,
            param_mapping,
            param_encoding: Some(param_encoding),
            result_decoding: None,
            result_mapping: None,
        }
    }

    fn json_msg(body: Value) -> Message {
        MessageBuilder::new(serde_json::to_vec(&body).unwrap())
            .header(MessageHeaders::CONTENT_TYPE, "application/json")
            .build()
    }

    #[test]
    fn to_invocation_name_match_simple() {
        let m = mapper(
            vec![
                param("a", json!({"type": "number"}), false),
                param("b", json!({"type": "string"}), false),
            ],
            None,
            None,
        );
        let inv = m
            .to_invocation(&json_msg(json!({ "a": 1, "b": "x" })))
            .unwrap();
        assert_eq!(inv.args, vec![json!(1), json!("x")]);
    }

    #[test]
    fn to_invocation_missing_required_errors() {
        let m = mapper(
            vec![
                param("a", json!({"type": "number"}), false),
                param("b", json!({"type": "string"}), false),
            ],
            None,
            None,
        );
        let err = m.to_invocation(&json_msg(json!({ "a": 1 }))).unwrap_err();
        assert!(err.contains("missing required arg 'b'"));
    }

    #[test]
    fn to_invocation_missing_optional_is_null() {
        let m = mapper(
            vec![
                param("a", json!({"type": "number"}), false),
                param("b", json!({"type": "string"}), true),
            ],
            None,
            None,
        );
        let inv = m.to_invocation(&json_msg(json!({ "a": 1 }))).unwrap();
        assert_eq!(inv.args, vec![json!(1), Value::Null]);
    }

    #[test]
    fn to_invocation_templated_string_substitution() {
        let mapping: ParamMapping = [(
            "url".into(),
            json!("https://example.com/{body.id}?m={body.msg}"),
        )]
        .into_iter()
        .collect();
        let m = mapper(
            vec![param("url", json!({"type": "string"}), false)],
            Some(mapping),
            None,
        );
        let inv = m
            .to_invocation(&json_msg(json!({ "id": "abc", "msg": "hi" })))
            .unwrap();
        assert_eq!(inv.args, vec![json!("https://example.com/abc?m=hi")]);
    }

    #[test]
    fn to_invocation_path_only_template_preserves_type() {
        let mapping: ParamMapping = [
            ("age".into(), json!("{body.age}")),
            ("user".into(), json!("{body.user}")),
        ]
        .into_iter()
        .collect();
        let m = mapper(
            vec![
                param("age", json!({"type": "number"}), false),
                param("user", json!({"type": "object"}), false),
            ],
            Some(mapping),
            None,
        );
        let inv = m
            .to_invocation(&json_msg(json!({
                "age": 30,
                "user": { "name": "Alice" }
            })))
            .unwrap();
        assert_eq!(inv.args[0], json!(30));
        assert_eq!(inv.args[1], json!({ "name": "Alice" }));
    }

    #[test]
    fn to_invocation_body_nested_path() {
        let mapping: ParamMapping = [("ct".into(), json!("{body.headers.Content-Type}"))]
            .into_iter()
            .collect();
        let m = mapper(
            vec![param("ct", json!({"type": "string"}), false)],
            Some(mapping),
            None,
        );
        let inv = m
            .to_invocation(&json_msg(json!({
                "headers": { "Content-Type": "application/json" }
            })))
            .unwrap();
        assert_eq!(inv.args, vec![json!("application/json")]);
    }

    #[test]
    fn to_invocation_headers_path() {
        let mapping: ParamMapping = [("correlator".into(), json!("{headers.correlation-id}"))]
            .into_iter()
            .collect();
        let m = mapper(
            vec![param("correlator", json!({"type": "string"}), false)],
            Some(mapping),
            None,
        );
        let msg = MessageBuilder::new(b"{}".to_vec())
            .header(MessageHeaders::CONTENT_TYPE, "application/json")
            .header(MessageHeaders::CORRELATION_ID, "cid-42")
            .build();
        let inv = m.to_invocation(&msg).unwrap();
        assert_eq!(inv.args, vec![json!("cid-42")]);
    }

    #[test]
    fn to_invocation_default_when_path_missing() {
        let mapping: ParamMapping = [("name".into(), json!("{body.nope | \"anonymous\"}"))]
            .into_iter()
            .collect();
        let m = mapper(
            vec![param("name", json!({"type": "string"}), false)],
            Some(mapping),
            None,
        );
        let inv = m.to_invocation(&json_msg(json!({ "a": 1 }))).unwrap();
        assert_eq!(inv.args, vec![json!("anonymous")]);
    }

    #[test]
    fn to_invocation_default_for_number() {
        let mapping: ParamMapping = [("count".into(), json!("{body.nope | 0}"))]
            .into_iter()
            .collect();
        let m = mapper(
            vec![param("count", json!({"type": "number"}), false)],
            Some(mapping),
            None,
        );
        let inv = m.to_invocation(&json_msg(json!({}))).unwrap();
        assert_eq!(inv.args, vec![json!(0)]);
    }

    #[test]
    fn to_invocation_nested_and_indexed_paths() {
        let mapping: ParamMapping = [
            ("ct".into(), json!("{body.headers.Content-Type}")),
            ("id".into(), json!("{body.items[1].id}")),
        ]
        .into_iter()
        .collect();
        let m = mapper(
            vec![
                param("ct", json!({"type": "string"}), false),
                param("id", json!({"type": "string"}), false),
            ],
            Some(mapping),
            None,
        );
        let inv = m
            .to_invocation(&json_msg(json!({
                "headers": { "Content-Type": "application/json" },
                "items": [{ "id": "first" }, { "id": "second" }]
            })))
            .unwrap();
        assert_eq!(inv.args, vec![json!("application/json"), json!("second")]);
    }

    #[test]
    fn to_invocation_partial_mapping_falls_back_to_name_match() {
        let mapping: ParamMapping = [("url".into(), json!("https://example.com/{body.id}"))]
            .into_iter()
            .collect();
        let m = mapper(
            vec![
                param("url", json!({"type": "string"}), false),
                param("verbose", json!({"type": "boolean"}), false),
            ],
            Some(mapping),
            None,
        );
        let inv = m
            .to_invocation(&json_msg(json!({ "id": "abc", "verbose": true })))
            .unwrap();
        assert_eq!(
            inv.args,
            vec![json!("https://example.com/abc"), json!(true)]
        );
    }

    #[test]
    fn to_invocation_string_param_stringifies_non_string() {
        let m = mapper(
            vec![param("payload", json!({"type": "string"}), false)],
            None,
            None,
        );
        let inv = m
            .to_invocation(&json_msg(json!({ "payload": { "k": 1 } })))
            .unwrap();
        assert_eq!(inv.args, vec![json!("{\"k\":1}")]);
    }

    #[test]
    fn to_invocation_structured_template_preserves_inner_types() {
        let mapping: ParamMapping = [(
            "body".into(),
            json!({ "user": { "name": "{body.name}", "age": "{body.age}" } }),
        )]
        .into_iter()
        .collect();
        let m = mapper(
            vec![param("body", json!({"type": "object"}), false)],
            Some(mapping),
            None,
        );
        let inv = m
            .to_invocation(&json_msg(json!({ "name": "Alice", "age": 30 })))
            .unwrap();
        assert_eq!(
            inv.args[0],
            json!({ "user": { "name": "Alice", "age": 30 } })
        );
    }

    #[test]
    fn to_invocation_missing_template_path_errors() {
        let mapping: ParamMapping = [("x".into(), json!("{body.nope}"))].into_iter().collect();
        let m = mapper(
            vec![param("x", json!({"type": "string"}), false)],
            Some(mapping),
            None,
        );
        let err = m.to_invocation(&json_msg(json!({ "a": 1 }))).unwrap_err();
        assert!(err.contains("unknown path"));
    }

    #[test]
    fn to_invocation_unknown_top_level_segment_errors() {
        let mapping: ParamMapping = [("x".into(), json!("{nosuch.x}"))].into_iter().collect();
        let m = mapper(
            vec![param("x", json!({"type": "string"}), false)],
            Some(mapping),
            None,
        );
        let err = m.to_invocation(&json_msg(json!({}))).unwrap_err();
        assert!(err.contains("unknown path"));
    }

    #[test]
    fn to_invocation_single_param_wraps_non_object_body() {
        let m = mapper(
            vec![param("value", json!({"type": "number"}), false)],
            None,
            None,
        );
        let msg = MessageBuilder::new(b"21".to_vec())
            .header(MessageHeaders::CONTENT_TYPE, "application/json")
            .build();
        let inv = m.to_invocation(&msg).unwrap();
        assert_eq!(inv.args, vec![json!(21)]);
    }

    #[test]
    fn to_invocation_no_params_returns_empty_args() {
        let m = mapper(vec![], None, None);
        let inv = m.to_invocation(&json_msg(json!({}))).unwrap();
        assert!(inv.args.is_empty());
    }

    #[test]
    fn from_invocation_result_passthrough() {
        let m = mapper(vec![], None, None);
        let result = json!({ "ok": true });
        let propagated = HashMap::new();
        let reply = m.from_invocation_result(&result, propagated).unwrap();
        assert_eq!(reply.body(), br#"{"ok":true}"#);
    }

    #[test]
    fn from_invocation_result_body_path_only_template() {
        // Result-mapping's body slot is a path-only template `{name}`: picks
        // the `name` field from the WIT result and uses it as the reply body.
        let mapping = json!({ "body": "{name}" });
        let m = mapper(vec![], None, Some(mapping));
        let result = json!({ "name": "Alice", "age": 30 });
        let propagated = HashMap::new();
        let reply = m.from_invocation_result(&result, propagated).unwrap();
        assert_eq!(reply.body(), br#""Alice""#);
    }

    #[test]
    fn from_invocation_result_body_structured_template() {
        let mapping = json!({
            "body": { "user": { "n": "{name}", "a": "{age}" } }
        });
        let m = mapper(vec![], None, Some(mapping));
        let result = json!({ "name": "Alice", "age": 30 });
        let propagated = HashMap::new();
        let reply = m.from_invocation_result(&result, propagated).unwrap();
        assert_eq!(reply.body(), br#"{"user":{"a":30,"n":"Alice"}}"#);
    }

    #[test]
    fn from_invocation_result_body_absent_means_empty() {
        let mapping = json!({ "headers": { "x-trace": "{name}" } });
        let m = mapper(vec![], None, Some(mapping));
        let result = json!({ "name": "Alice" });
        let reply = m.from_invocation_result(&result, HashMap::new()).unwrap();
        assert_eq!(reply.body(), b"");
        assert_eq!(reply.headers().get::<&str>("x-trace"), Some("Alice"));
    }

    #[test]
    fn from_invocation_result_body_null_means_empty() {
        let mapping = json!({ "body": null });
        let m = mapper(vec![], None, Some(mapping));
        let result = json!({ "name": "Alice" });
        let reply = m.from_invocation_result(&result, HashMap::new()).unwrap();
        assert_eq!(reply.body(), b"");
    }

    #[test]
    fn from_invocation_result_body_empty_string_means_empty() {
        let mapping = json!({ "body": "" });
        let m = mapper(vec![], None, Some(mapping));
        let result = json!({ "name": "Alice" });
        let reply = m.from_invocation_result(&result, HashMap::new()).unwrap();
        assert_eq!(reply.body(), b"");
    }

    #[test]
    fn from_invocation_result_mapped_headers_set_on_reply() {
        let mapping = json!({
            "body": { "ok": true },
            "headers": { "x-name": "{name}", "x-age": "{age}" }
        });
        let m = mapper(vec![], None, Some(mapping));
        let result = json!({ "name": "Alice", "age": 30 });
        let reply = m.from_invocation_result(&result, HashMap::new()).unwrap();
        assert_eq!(reply.headers().get::<&str>("x-name"), Some("Alice"));
        assert_eq!(reply.headers().get::<&str>("x-age"), Some("30"));
    }

    #[test]
    fn from_invocation_result_headers_non_object_template_errors() {
        let mapping = json!({ "body": null, "headers": "{name}" });
        let m = mapper(vec![], None, Some(mapping));
        let result = json!({ "name": "Alice" });
        let err = m
            .from_invocation_result(&result, HashMap::new())
            .unwrap_err();
        assert!(err.contains("must produce an object"), "unexpected: {err}");
    }

    // Build a mapper with optional result_decoding for the decode tests.
    fn mapper_with_decoding(
        result_mapping: Option<Value>,
        result_decoding: ResultDecoding,
    ) -> MessageMapper {
        MessageMapper {
            function_key: "test-fn".to_string(),
            params: vec![],
            param_mapping: None,
            param_encoding: None,
            result_decoding: Some(result_decoding),
            result_mapping,
        }
    }

    // Encode a string as a JSON array of u8 bytes (how list<u8> appears).
    fn bytes_value(s: &str) -> Value {
        Value::Array(s.bytes().map(|b| Value::from(b as u64)).collect())
    }

    #[test]
    fn result_decoding_literal_application_json_decodes_field() {
        let decoding = ResultDecoding::parse(
            json!({ "payload": "application/json" })
                .as_object()
                .unwrap(),
        )
        .unwrap();
        let mapping = json!({ "body": "{payload.uuid}" });
        let m = mapper_with_decoding(Some(mapping), decoding);
        let wit_result = json!({
            "status": 200,
            "payload": bytes_value(r#"{"uuid":"abc-123"}"#),
        });
        let reply = m
            .from_invocation_result(&wit_result, HashMap::new())
            .unwrap();
        assert_eq!(reply.body(), br#""abc-123""#);
    }

    #[test]
    fn result_decoding_literal_text_plain_decodes_to_string() {
        let decoding =
            ResultDecoding::parse(json!({ "body": "text/plain" }).as_object().unwrap()).unwrap();
        let mapping = json!({ "body": "{body}" });
        let m = mapper_with_decoding(Some(mapping), decoding);
        let wit_result = json!({
            "body": bytes_value("hello world"),
        });
        let reply = m
            .from_invocation_result(&wit_result, HashMap::new())
            .unwrap();
        assert_eq!(reply.body(), br#""hello world""#);
    }

    #[test]
    fn result_decoding_path_resolves_content_type_from_wit_result() {
        let decoding =
            ResultDecoding::parse(json!({ "payload": "{ct}" }).as_object().unwrap()).unwrap();
        let mapping = json!({ "body": "{payload.k}" });
        let m = mapper_with_decoding(Some(mapping), decoding);
        let wit_result = json!({
            "ct": "application/json",
            "payload": bytes_value(r#"{"k":"v"}"#),
        });
        let reply = m
            .from_invocation_result(&wit_result, HashMap::new())
            .unwrap();
        assert_eq!(reply.body(), br#""v""#);
    }

    #[test]
    fn result_decoding_malformed_json_errors() {
        let decoding = ResultDecoding::parse(
            json!({ "payload": "application/json" })
                .as_object()
                .unwrap(),
        )
        .unwrap();
        let m = mapper_with_decoding(Some(json!({ "body": "{payload}" })), decoding);
        let wit_result = json!({
            "payload": bytes_value("not json"),
        });
        let err = m
            .from_invocation_result(&wit_result, HashMap::new())
            .unwrap_err();
        assert!(
            err.contains("malformed application/json"),
            "unexpected: {err}"
        );
    }

    #[test]
    fn result_decoding_path_missing_at_runtime_errors() {
        let decoding =
            ResultDecoding::parse(json!({ "payload": "{ct}" }).as_object().unwrap()).unwrap();
        let m = mapper_with_decoding(Some(json!({ "body": "{payload}" })), decoding);
        let wit_result = json!({
            "payload": bytes_value("{}"),
        });
        let err = m
            .from_invocation_result(&wit_result, HashMap::new())
            .unwrap_err();
        assert!(err.contains("path did not resolve"), "unexpected: {err}");
    }

    #[test]
    fn result_decoding_path_null_at_runtime_errors() {
        let decoding =
            ResultDecoding::parse(json!({ "payload": "{ct}" }).as_object().unwrap()).unwrap();
        let m = mapper_with_decoding(Some(json!({ "body": "{payload}" })), decoding);
        let wit_result = json!({
            "ct": null,
            "payload": bytes_value("{}"),
        });
        let err = m
            .from_invocation_result(&wit_result, HashMap::new())
            .unwrap_err();
        assert!(err.contains("resolved to null"), "unexpected: {err}");
    }

    #[test]
    fn result_decoding_path_unsupported_content_type_errors() {
        let decoding =
            ResultDecoding::parse(json!({ "payload": "{ct}" }).as_object().unwrap()).unwrap();
        let m = mapper_with_decoding(Some(json!({ "body": "{payload}" })), decoding);
        let wit_result = json!({
            "ct": "application/xml",
            "payload": bytes_value("<x/>"),
        });
        let err = m
            .from_invocation_result(&wit_result, HashMap::new())
            .unwrap_err();
        assert!(err.contains("not supported"), "unexpected: {err}");
    }

    #[test]
    fn result_decoding_handles_multiple_fields() {
        // Two byte-array fields are each decoded using their own content-type
        // path. Both decoded values appear in the final result.
        let decoding = ResultDecoding::parse(
            json!({
                "a": "{ct_a}",
                "b": "{ct_b}"
            })
            .as_object()
            .unwrap(),
        )
        .unwrap();
        let m = mapper_with_decoding(Some(json!({ "body": "{a}" })), decoding);
        let wit_result = json!({
            "ct_a": "text/plain",
            "ct_b": "application/json",
            "a": bytes_value("hi"),
            "b": bytes_value(r#"{"x":1}"#),
        });
        let reply = m
            .from_invocation_result(&wit_result, HashMap::new())
            .unwrap();
        assert_eq!(reply.body(), br#""hi""#);
    }

    // ----- param-encoding runtime tests -----

    // Decode a JSON byte array back into its UTF-8 string for assertions.
    fn bytes_value_to_string(v: &Value) -> String {
        let bytes: Vec<u8> = v
            .as_array()
            .unwrap()
            .iter()
            .map(|n| n.as_u64().unwrap() as u8)
            .collect();
        String::from_utf8(bytes).unwrap()
    }

    #[test]
    fn param_encoding_literal_application_json_encodes_value() {
        let encoding =
            ParamEncoding::parse(json!({ "body": "application/json" }).as_object().unwrap())
                .unwrap();
        let mapping: ParamMapping = [
            ("url".into(), json!("https://example.com")),
            ("body".into(), json!({ "msg": "{body.msg}" })),
        ]
        .into_iter()
        .collect();
        let m = mapper_with_encoding(
            vec![
                param("url", json!({"type": "string"}), false),
                param("body", byte_array_schema(), false),
            ],
            Some(mapping),
            encoding,
        );
        let inv = m
            .to_invocation(&json_msg(json!({ "msg": "hello" })))
            .unwrap();
        // body arg was encoded as JSON bytes; decode to string for assertion.
        let body_bytes_str = bytes_value_to_string(&inv.args[1]);
        assert_eq!(body_bytes_str, r#"{"msg":"hello"}"#);
        assert_eq!(inv.args[0], json!("https://example.com"));
    }

    #[test]
    fn param_encoding_literal_text_plain_uses_string_directly() {
        let encoding =
            ParamEncoding::parse(json!({ "body": "text/plain" }).as_object().unwrap()).unwrap();
        let mapping: ParamMapping = [("body".into(), json!("{body.msg}"))].into_iter().collect();
        let m = mapper_with_encoding(
            vec![param("body", byte_array_schema(), false)],
            Some(mapping),
            encoding,
        );
        let inv = m
            .to_invocation(&json_msg(json!({ "msg": "hello world" })))
            .unwrap();
        let body_bytes_str = bytes_value_to_string(&inv.args[0]);
        assert_eq!(body_bytes_str, "hello world");
    }

    #[test]
    fn param_encoding_path_resolves_content_type_from_other_arg() {
        let encoding =
            ParamEncoding::parse(json!({ "body": "{headers.ct}" }).as_object().unwrap()).unwrap();
        let mapping: ParamMapping = [
            ("headers".into(), json!({ "ct": "{body.requested-ct}" })),
            ("body".into(), json!({ "x": "{body.x}" })),
        ]
        .into_iter()
        .collect();
        let m = mapper_with_encoding(
            vec![
                param("headers", json!({"type": "object"}), false),
                param("body", byte_array_schema(), false),
            ],
            Some(mapping),
            encoding,
        );
        let inv = m
            .to_invocation(&json_msg(json!({
                "requested-ct": "application/json",
                "x": "value"
            })))
            .unwrap();
        let body_bytes_str = bytes_value_to_string(&inv.args[1]);
        assert_eq!(body_bytes_str, r#"{"x":"value"}"#);
    }

    #[test]
    fn param_encoding_text_plain_non_string_value_errors() {
        let encoding =
            ParamEncoding::parse(json!({ "body": "text/plain" }).as_object().unwrap()).unwrap();
        let mapping: ParamMapping = [("body".into(), json!({ "x": "{body.x}" }))]
            .into_iter()
            .collect();
        let m = mapper_with_encoding(
            vec![param("body", byte_array_schema(), false)],
            Some(mapping),
            encoding,
        );
        let err = m
            .to_invocation(&json_msg(json!({ "x": "value" })))
            .unwrap_err();
        assert!(
            err.contains("cannot encode") && err.contains("text/plain"),
            "unexpected: {err}"
        );
    }

    #[test]
    fn param_encoding_path_missing_at_runtime_errors() {
        let encoding =
            ParamEncoding::parse(json!({ "body": "{headers.ct}" }).as_object().unwrap()).unwrap();
        let mapping: ParamMapping = [
            ("headers".into(), json!({})),
            ("body".into(), json!({ "x": "{body.x}" })),
        ]
        .into_iter()
        .collect();
        let m = mapper_with_encoding(
            vec![
                param("headers", json!({"type": "object"}), false),
                param("body", byte_array_schema(), false),
            ],
            Some(mapping),
            encoding,
        );
        let err = m
            .to_invocation(&json_msg(json!({ "x": "value" })))
            .unwrap_err();
        assert!(err.contains("path did not resolve"), "unexpected: {err}");
    }

    #[test]
    fn param_encoding_path_unsupported_content_type_errors() {
        let encoding =
            ParamEncoding::parse(json!({ "body": "{headers.ct}" }).as_object().unwrap()).unwrap();
        let mapping: ParamMapping = [
            ("headers".into(), json!({ "ct": "{body.requested-ct}" })),
            ("body".into(), json!({ "x": "{body.x}" })),
        ]
        .into_iter()
        .collect();
        let m = mapper_with_encoding(
            vec![
                param("headers", json!({"type": "object"}), false),
                param("body", byte_array_schema(), false),
            ],
            Some(mapping),
            encoding,
        );
        let err = m
            .to_invocation(&json_msg(json!({
                "requested-ct": "application/xml",
                "x": "value"
            })))
            .unwrap_err();
        assert!(err.contains("is not supported"), "unexpected: {err}");
    }

    #[test]
    fn from_invocation_result_text_plain_content_type() {
        let m = mapper(vec![], None, None);
        let result = json!("Hello, World");
        let mut propagated = HashMap::new();
        propagated.insert(
            MessageHeaders::CONTENT_TYPE.to_string(),
            "text/plain".to_string(),
        );
        let reply = m.from_invocation_result(&result, propagated).unwrap();
        assert_eq!(reply.body(), b"Hello, World");
        assert_eq!(reply.headers().content_type(), Some("text/plain"));
    }

    #[test]
    fn from_invocation_result_propagated_headers_attached() {
        let m = mapper(vec![], None, None);
        let result = json!(42);
        let mut propagated = HashMap::new();
        propagated.insert(
            MessageHeaders::CORRELATION_ID.to_string(),
            "cid-1".to_string(),
        );
        propagated.insert("traceparent".to_string(), "00-abc-def-01".to_string());
        let reply = m.from_invocation_result(&result, propagated).unwrap();
        assert_eq!(reply.headers().correlation_id(), Some("cid-1"));
        assert_eq!(
            reply.headers().get::<&str>("traceparent"),
            Some("00-abc-def-01")
        );
    }

    #[test]
    fn from_invocation_result_missing_template_path_errors() {
        let mapping = json!({ "body": "{nope}" });
        let m = mapper(vec![], None, Some(mapping));
        let err = m
            .from_invocation_result(&json!({}), HashMap::new())
            .unwrap_err();
        assert!(err.contains("unknown path"));
    }

    #[test]
    fn from_invocation_result_default_when_missing() {
        let mapping = json!({ "body": "{nope | \"fallback\"}" });
        let m = mapper(vec![], None, Some(mapping));
        let reply = m
            .from_invocation_result(&json!({}), HashMap::new())
            .unwrap();
        assert_eq!(reply.body(), br#""fallback""#);
    }

    fn test_component(functions: Vec<(&str, Vec<FunctionParam>)>) -> Component {
        use crate::types::{ComponentMetadata, Function};
        let mut function_map = std::collections::HashMap::new();
        for (name, params) in functions {
            function_map.insert(
                name.to_string(),
                Function::new(None, name.to_string(), String::new(), params, None),
            );
        }
        Component {
            metadata: ComponentMetadata {
                name: "test".to_string(),
                namespace: None,
                package: None,
                labels: std::collections::HashMap::new(),
                dependents: None,
                exports: vec![],
            },
            functions: function_map,
        }
    }

    #[test]
    fn from_component_picks_only_function() {
        let component = test_component(vec![(
            "greet",
            vec![param("name", json!({"type": "string"}), false)],
        )]);
        let m = MessageMapper::from_component(&component, None, MappingConfig::default()).unwrap();
        assert_eq!(m.function_key(), "greet");
    }

    #[test]
    fn from_component_multi_function_requires_key() {
        let component = test_component(vec![("a", vec![]), ("b", vec![])]);
        let result = MessageMapper::from_component(&component, None, MappingConfig::default());
        let err = match result {
            Ok(_) => panic!("expected error for multi-function component without key"),
            Err(e) => e,
        };
        assert!(err.contains("must specify a 'function'"));
    }

    #[test]
    fn from_component_unknown_function_errors() {
        let component = test_component(vec![("greet", vec![])]);
        let result = MessageMapper::from_component(
            &component,
            Some("nope".into()),
            MappingConfig::default(),
        );
        let err = match result {
            Ok(_) => panic!("expected error for unknown function"),
            Err(e) => e,
        };
        assert!(err.contains("function 'nope' not found"));
    }

    // Component with one function whose result schema matches an envelope
    // shape used in result-decoding tests.
    fn envelope_component() -> Component {
        use crate::types::{ComponentMetadata, Function};
        let result_schema = json!({
            "type": "object",
            "properties": {
                "ct": { "type": "string" },
                "payload": {
                    "type": "array",
                    "items": { "type": "number", "minimum": 0, "maximum": 255 }
                },
                "label": { "type": "string" }
            }
        });
        let mut functions = std::collections::HashMap::new();
        functions.insert(
            "fetch".to_string(),
            Function::new(
                None,
                "fetch".to_string(),
                String::new(),
                vec![],
                Some(result_schema),
            ),
        );
        Component {
            metadata: ComponentMetadata {
                name: "envelope".to_string(),
                namespace: None,
                package: None,
                labels: std::collections::HashMap::new(),
                dependents: None,
                exports: vec![],
            },
            functions,
        }
    }

    fn try_with_decoding(component: &Component, decoding: ResultDecoding) -> String {
        match MessageMapper::from_component(
            component,
            Some("fetch".into()),
            MappingConfig {
                result_decoding: Some(decoding),
                ..Default::default()
            },
        ) {
            Ok(_) => panic!("expected error"),
            Err(e) => e,
        }
    }

    #[test]
    fn from_component_result_decoding_unknown_field_errors() {
        let component = envelope_component();
        let decoding = ResultDecoding::parse(
            json!({ "missing": "application/json" })
                .as_object()
                .unwrap(),
        )
        .unwrap();
        let err = try_with_decoding(&component, decoding);
        assert!(err.contains("no field 'missing'"), "unexpected: {err}");
    }

    #[test]
    fn from_component_result_decoding_non_byte_field_errors() {
        let component = envelope_component();
        let decoding =
            ResultDecoding::parse(json!({ "label": "application/json" }).as_object().unwrap())
                .unwrap();
        let err = try_with_decoding(&component, decoding);
        assert!(err.contains("must be a byte array"), "unexpected: {err}");
    }

    #[test]
    fn from_component_result_decoding_unsupported_literal_errors() {
        let component = envelope_component();
        let decoding =
            ResultDecoding::parse(json!({ "payload": "application/xml" }).as_object().unwrap())
                .unwrap();
        let err = try_with_decoding(&component, decoding);
        assert!(err.contains("is not supported"), "unexpected: {err}");
    }

    #[test]
    fn from_component_result_decoding_unknown_path_errors() {
        let component = envelope_component();
        let decoding =
            ResultDecoding::parse(json!({ "payload": "{no_such_field}" }).as_object().unwrap())
                .unwrap();
        let err = try_with_decoding(&component, decoding);
        assert!(err.contains("content-type path"), "unexpected: {err}");
    }
}
