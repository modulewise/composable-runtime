use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use anyhow::Result;
use serde_json::Value;

use composable_runtime::{
    CategoryClaim, Condition, ConfigHandler, MappingConfig, Operator, ParamEncoding, ParamMapping,
    PropagatedHeader, PropertyMap, ResultDecoding, Selector,
};

/// Parsed route within an HTTP server.
#[derive(Debug, Clone)]
pub struct RouteConfig {
    pub name: String,
    pub method: String,
    pub path: String,
    /// Query parameter specifications for matching and capturing.
    pub query_params: Vec<QueryParamSpec>,
    /// Inbound body content-type. `None` for methods that do not accept a
    /// request body (GET, HEAD, OPTIONS, TRACE).
    pub content_type: Option<ContentType>,
    pub target: RouteTarget,
    /// HTTP request headers to lift into the inbound Message headers.
    /// Source side is an HTTP request header name; target side is the
    /// Message header name (defaults to the source when no rename).
    pub propagate_request_headers: Vec<PropagatedHeader>,
    /// Reply Message headers to emit on the HTTP response.
    /// Source side is the Message header name; target side is the HTTP
    /// response header name (defaults to the source when no rename).
    pub propagate_response_headers: Vec<PropagatedHeader>,
    /// JSON Schema applied to reply body before serializing to HTTP response.
    pub response_schema: Option<Value>,
}

/// Supported inbound content-types for a route's request body.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContentType {
    Json,
    TextPlain,
}

impl ContentType {
    pub fn as_str(&self) -> &'static str {
        match self {
            ContentType::Json => "application/json",
            ContentType::TextPlain => "text/plain",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "application/json" => Some(ContentType::Json),
            "text/plain" => Some(ContentType::TextPlain),
            _ => None,
        }
    }
}

/// What a route dispatches to.
#[derive(Debug, Clone)]
pub enum RouteTarget {
    /// Invoke a component function via the messaging mapper.
    Component {
        component: String,
        function: String,
        mapping: MappingConfig,
    },
    /// Publish the request to a channel.
    Channel {
        channel: String,
        /// If set, use request-reply, else fire-and-forget.
        reply_timeout_ms: Option<u64>,
    },
}

/// Specification for a single query parameter on a route.
///
/// Grammar in config (per entry in `query-params`):
/// - `key` => required, any value, captured.
/// - `key=value` => required, must equal `value`, captured.
/// - `key?` =>  optional, captured if present.
/// - `key?=value` => optional, must equal `value` if present, captured.
/// - `~key` => required, any value, NOT captured.
/// - `~key=value` => required, must equal `value`, NOT captured.
/// - `~key?=value` => optional, must equal `value` if present, NOT captured.
/// - `!key` => forbidden (must be absent).
#[derive(Debug, Clone)]
pub struct QueryParamSpec {
    pub name: String,
    pub kind: QueryParamKind,
}

#[derive(Debug, Clone)]
pub enum QueryParamKind {
    /// Must be present. If `value` is Some, must equal that value.
    Required {
        value: Option<String>,
        capture: bool,
    },
    /// May be present. If present and `value` is Some, must equal that value.
    Optional {
        value: Option<String>,
        capture: bool,
    },
    /// Must NOT be present.
    Forbidden,
}

/// Parsed HTTP server definition.
#[derive(Debug, Clone)]
pub struct ServerConfig {
    pub name: String,
    pub port: u16,
    pub routes: Vec<RouteConfig>,
    /// OTLP endpoint for host-side span export. If absent, no host spans are exported.
    pub otlp_endpoint: Option<String>,
    /// OTLP protocol: "grpc" (default) or "http/protobuf".
    pub otlp_protocol: String,
}

pub type SharedConfig = Arc<Mutex<Vec<ServerConfig>>>;

pub fn shared_config() -> SharedConfig {
    Arc::new(Mutex::new(Vec::new()))
}

/// Claims `[server.*]` definitions where `type = "http"`.
pub struct HttpServerConfigHandler {
    servers: SharedConfig,
}

impl HttpServerConfigHandler {
    pub fn new(servers: SharedConfig) -> Self {
        Self { servers }
    }
}

impl ConfigHandler for HttpServerConfigHandler {
    fn claimed_categories(&self) -> Vec<CategoryClaim> {
        vec![CategoryClaim::with_selector(
            "server",
            Selector {
                conditions: vec![Condition {
                    key: "type".to_string(),
                    operator: Operator::Equals("http".to_string()),
                }],
            },
        )]
    }

    fn claimed_properties(&self) -> HashMap<&str, &[&str]> {
        HashMap::from([(
            "server",
            ["type", "port", "route", "otlp-endpoint", "otlp-protocol"].as_slice(),
        )])
    }

    fn handle_category(
        &mut self,
        category: &str,
        name: &str,
        mut properties: PropertyMap,
    ) -> Result<()> {
        if category != "server" {
            return Err(anyhow::anyhow!(
                "HttpServerConfigHandler received unexpected category '{category}'"
            ));
        }

        // type is only used by the selector
        properties.remove("type");

        let port = match properties.remove("port") {
            Some(Value::Number(n)) => {
                n.as_u64()
                    .and_then(|p| u16::try_from(p).ok())
                    .ok_or_else(|| {
                        anyhow::anyhow!("Server '{name}': 'port' must be a valid port number")
                    })?
            }
            Some(got) => {
                return Err(anyhow::anyhow!(
                    "Server '{name}': 'port' must be a number, got {got}"
                ));
            }
            None => {
                return Err(anyhow::anyhow!(
                    "Server '{name}' missing required 'port' field"
                ));
            }
        };

        let otlp_endpoint = match properties.remove("otlp-endpoint") {
            Some(Value::String(s)) => Some(s),
            Some(got) => {
                return Err(anyhow::anyhow!(
                    "Server '{name}': 'otlp-endpoint' must be a string, got {got}"
                ));
            }
            None => None,
        };

        let otlp_protocol = match properties.remove("otlp-protocol") {
            Some(Value::String(s)) => s,
            Some(got) => {
                return Err(anyhow::anyhow!(
                    "Server '{name}': 'otlp-protocol' must be a string, got {got}"
                ));
            }
            None => "grpc".to_string(),
        };

        let routes = parse_routes(name, &mut properties)?;

        if !properties.is_empty() {
            let unknown: Vec<_> = properties.keys().collect();
            return Err(anyhow::anyhow!(
                "Server '{name}' has unknown properties: {unknown:?}"
            ));
        }

        self.servers.lock().unwrap().push(ServerConfig {
            name: name.to_string(),
            port,
            routes,
            otlp_endpoint,
            otlp_protocol,
        });
        Ok(())
    }
}

fn get_optional_string(
    props: &serde_json::Map<String, Value>,
    key: &str,
    ctx: &dyn Fn(&str, &str) -> anyhow::Error,
) -> Result<Option<String>> {
    match props.get(key) {
        Some(Value::String(s)) => Ok(Some(s.clone())),
        Some(got) => Err(ctx(
            &format!("'{key}'"),
            &format!("must be a string, got {got}"),
        )),
        None => Ok(None),
    }
}

fn parse_routes(server_name: &str, properties: &mut PropertyMap) -> Result<Vec<RouteConfig>> {
    let route_table = match properties.remove("route") {
        Some(Value::Object(map)) => map,
        Some(got) => {
            return Err(anyhow::anyhow!(
                "Server '{server_name}': 'route' must be a table, got {got}"
            ));
        }
        None => return Ok(Vec::new()),
    };

    let mut routes = Vec::new();
    for (route_name, route_value) in route_table {
        let route_props = match route_value {
            Value::Object(map) => map,
            got => {
                return Err(anyhow::anyhow!(
                    "Server '{server_name}': route '{route_name}' must be a table, got {got}"
                ));
            }
        };

        let ctx = |field: &str, msg: &str| -> anyhow::Error {
            anyhow::anyhow!("Server '{server_name}': route '{route_name}' {field} {msg}")
        };

        let path = match route_props.get("path") {
            Some(Value::String(s)) => s.clone(),
            Some(got) => return Err(ctx("'path'", &format!("must be a string, got {got}"))),
            None => return Err(ctx("", "missing required 'path' field")),
        };

        let component = get_optional_string(&route_props, "component", &ctx)?;
        let function = get_optional_string(&route_props, "function", &ctx)?;
        let channel = get_optional_string(&route_props, "channel", &ctx)?;
        let reply_timeout_ms = match route_props.get("reply-timeout-ms") {
            Some(Value::Number(n)) => Some(
                n.as_u64()
                    .ok_or_else(|| ctx("'reply-timeout-ms'", "must be a non-negative integer"))?,
            ),
            Some(got) => {
                return Err(ctx(
                    "'reply-timeout-ms'",
                    &format!("must be a number, got {got}"),
                ));
            }
            None => None,
        };

        let param_mapping = match route_props.get("param-mapping") {
            Some(Value::Object(map)) => Some(map.clone().into_iter().collect::<ParamMapping>()),
            Some(got) => {
                return Err(ctx(
                    "'param-mapping'",
                    &format!("must be an object, got {got}"),
                ));
            }
            None => None,
        };

        let param_encoding = match route_props.get("param-encoding") {
            Some(Value::Object(map)) => {
                Some(ParamEncoding::parse(map).map_err(|e| ctx("'param-encoding'", &e))?)
            }
            Some(got) => {
                return Err(ctx(
                    "'param-encoding'",
                    &format!("must be an object, got {got}"),
                ));
            }
            None => None,
        };

        // Accepts any JSON shape (object/array/string/literal), but
        // the underlying `map_result` validates at substitution time.
        let result_mapping = route_props.get("result-mapping").cloned();

        let result_decoding = match route_props.get("result-decoding") {
            Some(Value::Object(map)) => {
                Some(ResultDecoding::parse(map).map_err(|e| ctx("'result-decoding'", &e))?)
            }
            Some(got) => {
                return Err(ctx(
                    "'result-decoding'",
                    &format!("must be an object, got {got}"),
                ));
            }
            None => None,
        };

        let response_schema = match route_props.get("response-schema") {
            Some(v @ Value::Object(_)) => Some(v.clone()),
            Some(got) => {
                return Err(ctx(
                    "'response-schema'",
                    &format!("must be a JSON schema object, got {got}"),
                ));
            }
            None => None,
        };

        let propagate_request_headers = parse_propagated_headers_list(
            &route_props,
            "propagate-request-headers",
            PropagationDirection::Inbound,
            &ctx,
        )?;
        let propagate_response_headers = parse_propagated_headers_list(
            &route_props,
            "propagate-response-headers",
            PropagationDirection::Outbound,
            &ctx,
        )?;

        // Cross-config check: a `result-mapping.headers` target would be
        // silently overridden if any `propagate-response-headers` entry has
        // that target name with a different source.
        let mapped_header_targets: std::collections::HashSet<&str> =
            match result_mapping.as_ref().and_then(|m| m.get("headers")) {
                Some(Value::Object(map)) => map.keys().map(|s| s.as_str()).collect(),
                _ => std::collections::HashSet::new(),
            };
        for entry in &propagate_response_headers {
            if mapped_header_targets.contains(entry.target()) && entry.source() != entry.target() {
                return Err(ctx(
                    "",
                    &format!(
                        "'{}' is written by 'result-mapping.headers' but overridden by \
                         'propagate-response-headers' entry '{} as {}'",
                        entry.target(),
                        entry.source(),
                        entry.target(),
                    ),
                ));
            }
        }

        let query_params = match route_props.get("query-params") {
            Some(Value::Array(items)) => items
                .iter()
                .map(|v| match v {
                    Value::String(s) => parse_query_param_spec(s, &ctx),
                    other => Err(ctx(
                        "'query-params'",
                        &format!("entries must be strings, got {other}"),
                    )),
                })
                .collect::<Result<Vec<_>>>()?,
            Some(got) => {
                return Err(ctx(
                    "'query-params'",
                    &format!("must be an array of strings, got {got}"),
                ));
            }
            None => Vec::new(),
        };

        let method = match route_props.get("method") {
            Some(Value::String(s)) => s.to_uppercase(),
            Some(got) => return Err(ctx("'method'", &format!("must be a string, got {got}"))),
            None => return Err(ctx("", "missing required 'method' field")),
        };

        let accepts_body = method_accepts_body(&method);
        let content_type = match (accepts_body, route_props.get("content-type")) {
            (false, Some(_)) => {
                return Err(ctx(
                    "'content-type'",
                    &format!(
                        "must not be declared on method '{method}' (no request body to describe)"
                    ),
                ));
            }
            (false, None) => None,
            (true, Some(Value::String(s))) => match ContentType::parse(s) {
                Some(ct) => Some(ct),
                None => {
                    return Err(ctx(
                        "'content-type'",
                        &format!("'{s}' is not a supported content-type"),
                    ));
                }
            },
            (true, Some(got)) => {
                return Err(ctx(
                    "'content-type'",
                    &format!("must be a string, got {got}"),
                ));
            }
            (true, None) => Some(ContentType::Json),
        };

        if matches!(content_type, Some(ContentType::TextPlain)) {
            if param_mapping.is_some() {
                return Err(ctx(
                    "",
                    "'param-mapping' is not allowed when 'content-type' is 'text/plain' \
                     (the inbound body is a raw string, not a JSON shape to traverse)",
                ));
            }
            if param_encoding.is_some() {
                return Err(ctx(
                    "",
                    "'param-encoding' is not allowed when 'content-type' is 'text/plain' \
                     (encoding to the WIT arg is handled automatically)",
                ));
            }
            if route_has_captures(&path, &query_params) {
                return Err(ctx(
                    "",
                    "path/query captures are not allowed when 'content-type' is 'text/plain' \
                     (the inbound body is a raw string; no JSON object to merge captures into)",
                ));
            }
        }

        let has_component = component.is_some() || function.is_some();
        let has_channel = channel.is_some();

        if has_component && has_channel {
            return Err(ctx(
                "",
                "cannot have both 'component'/'function' and 'channel'",
            ));
        }

        let target = if let Some(channel) = channel {
            if param_mapping.is_some()
                || param_encoding.is_some()
                || result_decoding.is_some()
                || result_mapping.is_some()
            {
                return Err(ctx(
                    "",
                    "'param-mapping', 'param-encoding', 'result-decoding', and \
                     'result-mapping' apply to component routes only; for channel routes, \
                     downstream subscriptions own their mappings",
                ));
            }
            RouteTarget::Channel {
                channel,
                reply_timeout_ms,
            }
        } else {
            let component = component
                .ok_or_else(|| ctx("", "missing 'component' (or 'channel' for channel routes)"))?;
            let function = function
                .ok_or_else(|| ctx("", "missing 'function' (required for component routes)"))?;
            if reply_timeout_ms.is_some() {
                return Err(ctx("", "'reply-timeout-ms' applies to channel routes only"));
            }
            RouteTarget::Component {
                component,
                function,
                mapping: MappingConfig {
                    param_mapping,
                    param_encoding,
                    result_decoding,
                    result_mapping,
                },
            }
        };

        validate_route_captures(&path, &query_params, &ctx)?;

        routes.push(RouteConfig {
            name: route_name,
            method,
            path,
            query_params,
            content_type,
            target,
            propagate_request_headers,
            propagate_response_headers,
            response_schema,
        });
    }

    // Detect conflicting routes. Two routes conflict when they have the same
    // method AND the same path structure AND the same content-type AND their
    // query-param specs do not make them mutually exclusive. Two routes that
    // share method/path/query but differ in content-type are allowed.
    for i in 0..routes.len().saturating_sub(1) {
        for j in (i + 1)..routes.len() {
            if routes[i].method == routes[j].method
                && path_structure(&routes[i].path) == path_structure(&routes[j].path)
                && routes[i].content_type == routes[j].content_type
                && !query_specs_disjoint(&routes[i].query_params, &routes[j].query_params)
            {
                return Err(anyhow::anyhow!(
                    "Server '{}': routes '{}' and '{}' conflict (same method {}, path \
                     structure, and content-type, and their 'query-params' specs do not \
                     make them mutually exclusive)",
                    server_name,
                    routes[i].name,
                    routes[j].name,
                    routes[i].method,
                ));
            }
        }
    }

    Ok(routes)
}

// Are two routes' query-param specs mutually exclusive?
//
// Returns true iff there exists at least one parameter name `k` whose specs
// on the two routes guarantee no single HTTP request can match both:
//   - `Required value=X` vs `Required value=Y` with X != Y
//   - `Required` (any form) vs `Forbidden`
//
// All other combinations admit at least one request that matches both. For
// example, `Optional value=X` vs `Optional value=Y` both match a request
// that omits the parameter entirely.
fn query_specs_disjoint(a: &[QueryParamSpec], b: &[QueryParamSpec]) -> bool {
    let a_map: HashMap<&str, &QueryParamKind> =
        a.iter().map(|s| (s.name.as_str(), &s.kind)).collect();
    for spec_b in b {
        if let Some(kind_a) = a_map.get(spec_b.name.as_str())
            && kinds_are_exclusive(kind_a, &spec_b.kind)
        {
            return true;
        }
    }
    false
}

fn kinds_are_exclusive(a: &QueryParamKind, b: &QueryParamKind) -> bool {
    match (a, b) {
        // Forbidden vs any Required form: requests can satisfy at most one.
        (QueryParamKind::Forbidden, QueryParamKind::Required { .. })
        | (QueryParamKind::Required { .. }, QueryParamKind::Forbidden) => true,

        // Required with different fixed values: no request can match both.
        (
            QueryParamKind::Required {
                value: Some(va), ..
            },
            QueryParamKind::Required {
                value: Some(vb), ..
            },
        ) if va != vb => true,

        // All other combinations admit overlap. In particular:
        //   - Required-any-value vs Required-fixed-value: overlap on fixed value.
        //   - Optional vs anything: overlap on absence, and possibly on value.
        //   - Forbidden vs Optional: overlap on absence.
        _ => false,
    }
}

fn parse_string_list(
    props: &serde_json::Map<String, Value>,
    key: &str,
    ctx: &dyn Fn(&str, &str) -> anyhow::Error,
) -> Result<Vec<String>> {
    match props.get(key) {
        Some(Value::Array(items)) => items
            .iter()
            .map(|v| match v {
                Value::String(s) => Ok(s.clone()),
                other => Err(ctx(
                    &format!("'{key}'"),
                    &format!("entries must be strings, got {other}"),
                )),
            })
            .collect(),
        Some(got) => Err(ctx(
            &format!("'{key}'"),
            &format!("must be an array of strings, got {got}"),
        )),
        None => Ok(Vec::new()),
    }
}

// The direction of propagation. Determines which side of each entry
// (source / target) must be an HTTP header token.
#[derive(Debug, Clone, Copy)]
enum PropagationDirection {
    // Inbound: source is an HTTP header; target is a Message header.
    Inbound,
    // Outbound: source is a Message header; target is an HTTP header.
    Outbound,
}

fn parse_propagated_headers_list(
    props: &serde_json::Map<String, Value>,
    key: &str,
    direction: PropagationDirection,
    ctx: &dyn Fn(&str, &str) -> anyhow::Error,
) -> Result<Vec<PropagatedHeader>> {
    let raw = parse_string_list(props, key, ctx)?;
    raw.into_iter()
        .map(|s| {
            let entry = PropagatedHeader::parse(&s)
                .map_err(|e| ctx(&format!("'{key}'"), &format!("entry '{s}': {e}")))?;
            // Validate the side that must be an HTTP header token.
            let http_name = match direction {
                PropagationDirection::Inbound => entry.source(),
                PropagationDirection::Outbound => entry.target(),
            };
            if hyper::header::HeaderName::from_bytes(http_name.as_bytes()).is_err() {
                return Err(ctx(
                    &format!("'{key}'"),
                    &format!("entry '{s}': '{http_name}' is not a valid HTTP header name"),
                ));
            }
            Ok(entry)
        })
        .collect()
}

// Parse a single `query-params` entry per the grammar.
fn parse_query_param_spec(
    raw: &str,
    ctx: &dyn Fn(&str, &str) -> anyhow::Error,
) -> Result<QueryParamSpec> {
    let entry = raw.trim();
    if entry.is_empty() {
        return Err(ctx("'query-params'", "entry must not be empty"));
    }

    let (forbidden, capture, rest) = if let Some(r) = entry.strip_prefix('!') {
        (true, false, r)
    } else if let Some(r) = entry.strip_prefix('~') {
        (false, false, r)
    } else {
        (false, true, entry)
    };

    if forbidden {
        if rest.contains('=') || rest.contains('?') {
            return Err(ctx(
                "'query-params'",
                &format!("forbidden entry '{entry}' cannot have '=' or '?'"),
            ));
        }
        let name = validate_name(rest, entry, ctx)?;
        return Ok(QueryParamSpec {
            name,
            kind: QueryParamKind::Forbidden,
        });
    }

    // Split optional trailing `?` (must come immediately after the name).
    let (name_and_value, optional) = if let Some(idx) = rest.find('?') {
        let (head, tail) = rest.split_at(idx);
        let after_q = &tail[1..]; // skip the `?`
        // After `?` only `=value` is allowed.
        if !after_q.is_empty() && !after_q.starts_with('=') {
            return Err(ctx(
                "'query-params'",
                &format!("unexpected characters after '?' in '{entry}'"),
            ));
        }
        // `~key?` (don't-capture + optional + no value constraint) does not
        // change matching whether `key` is present or absent, making it
        // meaningless to include the entry.
        if !capture && after_q.is_empty() {
            return Err(ctx(
                "'query-params'",
                &format!(
                    "entry '{entry}' has no effect (don't-capture + optional with no value constraint)"
                ),
            ));
        }
        (format!("{head}{after_q}"), true)
    } else {
        (rest.to_string(), false)
    };

    // Split name and =value.
    let (name, value) = match name_and_value.split_once('=') {
        Some((n, v)) => (n, Some(v.to_string())),
        None => (name_and_value.as_str(), None),
    };
    let name = validate_name(name, entry, ctx)?;

    let kind = if optional {
        QueryParamKind::Optional { value, capture }
    } else {
        QueryParamKind::Required { value, capture }
    };

    Ok(QueryParamSpec { name, kind })
}

fn validate_name(
    name: &str,
    entry: &str,
    ctx: &dyn Fn(&str, &str) -> anyhow::Error,
) -> Result<String> {
    if name.is_empty() {
        return Err(ctx(
            "'query-params'",
            &format!("entry '{entry}' has empty name"),
        ));
    }
    if name.chars().any(|c| matches!(c, '?' | '=' | '!' | '~')) {
        return Err(ctx(
            "'query-params'",
            &format!("entry '{entry}' has invalid characters in name"),
        ));
    }
    Ok(name.to_string())
}

// Validate the route's captures:
// - Each `{...}` path segment is either empty (anonymous) or a non-empty
//   name containing no `{` or `}`.
// - Path capture names within the same path must be unique.
// - No capturing query-param may share a name with a path capture (the
//   runtime merges both into a single map, so a shared name would silently
//   drop the query value).
fn validate_route_captures(
    path: &str,
    query_params: &[QueryParamSpec],
    ctx: &dyn Fn(&str, &str) -> anyhow::Error,
) -> Result<()> {
    let mut path_captures = std::collections::HashSet::new();
    for segment in path.split('/').filter(|s| !s.is_empty()) {
        if segment.starts_with('{') && segment.ends_with('}') {
            let inner = &segment[1..segment.len() - 1];
            if inner.contains('{') || inner.contains('}') {
                return Err(ctx(
                    "'path'",
                    &format!("invalid placeholder segment '{segment}'"),
                ));
            }
            if !inner.is_empty() && !path_captures.insert(inner.to_string()) {
                return Err(ctx(
                    "'path'",
                    &format!("duplicate path capture name '{inner}'"),
                ));
            }
        }
    }

    for spec in query_params {
        let captures = match &spec.kind {
            QueryParamKind::Required { capture, .. } => *capture,
            QueryParamKind::Optional { capture, .. } => *capture,
            QueryParamKind::Forbidden => false,
        };
        if captures && path_captures.contains(&spec.name) {
            return Err(ctx(
                "",
                &format!(
                    "query-param '{}' collides with path capture of the same name",
                    spec.name
                ),
            ));
        }
    }

    Ok(())
}

// Whether this HTTP method's request body is accepted. GET, HEAD, OPTIONS, and
// TRACE ignore any body present on the wire.
fn method_accepts_body(method: &str) -> bool {
    !matches!(method, "GET" | "HEAD" | "OPTIONS" | "TRACE")
}

// Returns true when the route declares any capturing path/query placeholder.
fn route_has_captures(path: &str, query_params: &[QueryParamSpec]) -> bool {
    for segment in path.split('/').filter(|s| !s.is_empty()) {
        if segment.starts_with('{') && segment.ends_with('}') {
            let inner = &segment[1..segment.len() - 1];
            if !inner.is_empty() {
                return true;
            }
        }
    }
    query_params.iter().any(|spec| match &spec.kind {
        QueryParamKind::Required { capture, .. } => *capture,
        QueryParamKind::Optional { capture, .. } => *capture,
        QueryParamKind::Forbidden => false,
    })
}

// Normalize a path for structural comparison: replace `{name}` segments with
// `{}` so that `/users/{id}` and `/users/{uid}` are recognized as conflicts,
// and so anonymous `{}` segments also compare equal.
fn path_structure(path: &str) -> Vec<&str> {
    path.split('/')
        .filter(|s| !s.is_empty())
        .map(|s| {
            if s.starts_with('{') && s.ends_with('}') {
                "{}"
            } else {
                s
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn make_handler() -> (HttpServerConfigHandler, SharedConfig) {
        let config = shared_config();
        let handler = HttpServerConfigHandler::new(Arc::clone(&config));
        (handler, config)
    }

    fn props(pairs: Vec<(&str, Value)>) -> PropertyMap {
        pairs.into_iter().map(|(k, v)| (k.to_string(), v)).collect()
    }

    #[test]
    fn parse_basic_component_route() {
        let (mut handler, config) = make_handler();
        let properties = props(vec![
            ("type", json!("http")),
            ("port", json!(8080)),
            (
                "route",
                json!({
                    "hello": {
                        "method": "GET",
                        "path": "/hello/{name}",
                        "component": "greeter",
                        "function": "greet"
                    }
                }),
            ),
        ]);

        handler
            .handle_category("server", "api", properties)
            .unwrap();

        let servers = config.lock().unwrap();
        assert_eq!(servers.len(), 1);
        assert_eq!(servers[0].routes[0].method, "GET");
        match &servers[0].routes[0].target {
            RouteTarget::Component {
                component,
                function,
                mapping,
            } => {
                assert_eq!(component, "greeter");
                assert_eq!(function, "greet");
                assert!(mapping.param_mapping.is_none());
                assert!(mapping.param_encoding.is_none());
                assert!(mapping.result_decoding.is_none());
                assert!(mapping.result_mapping.is_none());
            }
            other => panic!("expected Component target, got {other:?}"),
        }
    }

    #[test]
    fn parse_component_route_with_mappings() {
        let (mut handler, config) = make_handler();
        let properties = props(vec![
            ("type", json!("http")),
            ("port", json!(8080)),
            (
                "route",
                json!({
                    "create": {
                        "method": "POST",
                        "path": "/users",
                        "component": "user-service",
                        "function": "create",
                        "param-mapping": { "user": "{body}" },
                        "result-mapping": { "id": "{id}" },
                        "response-schema": { "type": "object" }
                    }
                }),
            ),
        ]);

        handler
            .handle_category("server", "api", properties)
            .unwrap();

        let servers = config.lock().unwrap();
        match &servers[0].routes[0].target {
            RouteTarget::Component { mapping, .. } => {
                assert!(mapping.param_mapping.is_some());
                assert!(mapping.result_mapping.is_some());
            }
            other => panic!("expected Component target, got {other:?}"),
        }
        assert!(servers[0].routes[0].response_schema.is_some());
    }

    #[test]
    fn channel_route_with_param_mapping_is_error() {
        let (mut handler, _) = make_handler();
        let properties = props(vec![
            ("type", json!("http")),
            ("port", json!(8080)),
            (
                "route",
                json!({
                    "bad": {
                        "method": "POST",
                        "path": "/events",
                        "channel": "events",
                        "param-mapping": { "x": "{body}" }
                    }
                }),
            ),
        ]);

        let err = handler
            .handle_category("server", "api", properties)
            .unwrap_err();
        assert!(
            err.to_string().contains("apply to component routes only"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn channel_route_accepts_response_schema_and_propagate_request_headers() {
        let (mut handler, config) = make_handler();
        let properties = props(vec![
            ("type", json!("http")),
            ("port", json!(8080)),
            (
                "route",
                json!({
                    "events": {
                        "method": "POST",
                        "path": "/events",
                        "channel": "events",
                        "response-schema": { "type": "object" },
                        "propagate-request-headers": [
                            "Authorization",
                            "X-Request-Id as request-id"
                        ]
                    }
                }),
            ),
        ]);

        handler
            .handle_category("server", "api", properties)
            .unwrap();
        let servers = config.lock().unwrap();
        assert!(servers[0].routes[0].response_schema.is_some());
        let entries = &servers[0].routes[0].propagate_request_headers;
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].source(), "Authorization");
        assert_eq!(entries[0].target(), "Authorization");
        assert_eq!(entries[1].source(), "X-Request-Id");
        assert_eq!(entries[1].target(), "request-id");
    }

    #[test]
    fn route_parses_propagate_response_headers() {
        let (mut handler, config) = make_handler();
        let properties = props(vec![
            ("type", json!("http")),
            ("port", json!(8080)),
            (
                "route",
                json!({
                    "get-thing": {
                        "method": "GET",
                        "path": "/thing",
                        "component": "test",
                        "function": "get",
                        "propagate-response-headers": [
                            "x-rate-limit",
                            "internal-tag as X-Tag"
                        ]
                    }
                }),
            ),
        ]);

        handler
            .handle_category("server", "api", properties)
            .unwrap();
        let servers = config.lock().unwrap();
        let entries = &servers[0].routes[0].propagate_response_headers;
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].source(), "x-rate-limit");
        assert_eq!(entries[0].target(), "x-rate-limit");
        assert_eq!(entries[1].source(), "internal-tag");
        assert_eq!(entries[1].target(), "X-Tag");
    }

    #[test]
    fn propagate_request_headers_rejects_invalid_http_token_source() {
        let (mut handler, _) = make_handler();
        let properties = props(vec![
            ("type", json!("http")),
            ("port", json!(8080)),
            (
                "route",
                json!({
                    "bad": {
                        "method": "GET",
                        "path": "/x",
                        "component": "test",
                        "function": "test",
                        "propagate-request-headers": ["bad name as request-id"]
                    }
                }),
            ),
        ]);
        let err = handler
            .handle_category("server", "api", properties)
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("valid HTTP header name"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn parse_component_route_with_result_decoding() {
        let (mut handler, config) = make_handler();
        let properties = props(vec![
            ("type", json!("http")),
            ("port", json!(8080)),
            (
                "route",
                json!({
                    "fetch": {
                        "method": "GET",
                        "path": "/fetch",
                        "component": "transport",
                        "function": "get",
                        "result-decoding": {
                            "body": "{headers.content-type}"
                        }
                    }
                }),
            ),
        ]);
        handler
            .handle_category("server", "api", properties)
            .unwrap();
        let servers = config.lock().unwrap();
        match &servers[0].routes[0].target {
            RouteTarget::Component { mapping, .. } => {
                assert!(mapping.result_decoding.is_some());
            }
            other => panic!("expected Component target, got {other:?}"),
        }
    }

    #[test]
    fn channel_route_with_result_decoding_is_error() {
        let (mut handler, _) = make_handler();
        let properties = props(vec![
            ("type", json!("http")),
            ("port", json!(8080)),
            (
                "route",
                json!({
                    "bad": {
                        "method": "POST",
                        "path": "/events",
                        "channel": "events",
                        "result-decoding": { "body": "application/json" }
                    }
                }),
            ),
        ]);
        let err = handler
            .handle_category("server", "api", properties)
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("apply to component routes only"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn parse_component_route_with_param_encoding() {
        let (mut handler, config) = make_handler();
        let properties = props(vec![
            ("type", json!("http")),
            ("port", json!(8080)),
            (
                "route",
                json!({
                    "publish": {
                        "method": "POST",
                        "path": "/publish",
                        "component": "transport",
                        "function": "post",
                        "param-encoding": {
                            "body": "{headers.content-type}"
                        }
                    }
                }),
            ),
        ]);
        handler
            .handle_category("server", "api", properties)
            .unwrap();
        let servers = config.lock().unwrap();
        match &servers[0].routes[0].target {
            RouteTarget::Component { mapping, .. } => {
                assert!(mapping.param_encoding.is_some());
            }
            other => panic!("expected Component target, got {other:?}"),
        }
    }

    #[test]
    fn channel_route_with_param_encoding_is_error() {
        let (mut handler, _) = make_handler();
        let properties = props(vec![
            ("type", json!("http")),
            ("port", json!(8080)),
            (
                "route",
                json!({
                    "bad": {
                        "method": "POST",
                        "path": "/events",
                        "channel": "events",
                        "param-encoding": { "body": "application/json" }
                    }
                }),
            ),
        ]);
        let err = handler
            .handle_category("server", "api", properties)
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("apply to component routes only"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn result_mapping_header_overridden_by_propagate_response_is_error() {
        let (mut handler, _) = make_handler();
        let properties = props(vec![
            ("type", json!("http")),
            ("port", json!(8080)),
            (
                "route",
                json!({
                    "bad": {
                        "method": "GET",
                        "path": "/x",
                        "component": "test",
                        "function": "test",
                        "result-mapping": {
                            "headers": { "x-tag": "{label}" }
                        },
                        "propagate-response-headers": ["other-source as x-tag"]
                    }
                }),
            ),
        ]);
        let err = handler
            .handle_category("server", "api", properties)
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("written by 'result-mapping.headers'") && err.contains("overridden"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn result_mapping_header_with_matching_propagate_response_identity_is_ok() {
        let (mut handler, _) = make_handler();
        let properties = props(vec![
            ("type", json!("http")),
            ("port", json!(8080)),
            (
                "route",
                json!({
                    "ok": {
                        "method": "GET",
                        "path": "/x",
                        "component": "test",
                        "function": "test",
                        "result-mapping": {
                            "headers": { "x-tag": "{label}" }
                        },
                        "propagate-response-headers": ["x-tag"]
                    }
                }),
            ),
        ]);
        handler
            .handle_category("server", "api", properties)
            .unwrap();
    }

    #[test]
    fn propagate_response_headers_rejects_invalid_http_token_target() {
        let (mut handler, _) = make_handler();
        let properties = props(vec![
            ("type", json!("http")),
            ("port", json!(8080)),
            (
                "route",
                json!({
                    "bad": {
                        "method": "GET",
                        "path": "/x",
                        "component": "test",
                        "function": "test",
                        "propagate-response-headers": ["internal as bad name"]
                    }
                }),
            ),
        ]);
        let err = handler
            .handle_category("server", "api", properties)
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("valid HTTP header name"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn missing_port_is_error() {
        let (mut handler, _) = make_handler();
        let properties = props(vec![("type", json!("http"))]);
        let err = handler
            .handle_category("server", "api", properties)
            .unwrap_err();
        assert!(err.to_string().contains("missing required 'port'"));
    }

    #[test]
    fn missing_route_path_is_error() {
        let (mut handler, _) = make_handler();
        let properties = props(vec![
            ("type", json!("http")),
            ("port", json!(8080)),
            (
                "route",
                json!({ "bad": { "method": "GET", "component": "test", "function": "test" } }),
            ),
        ]);
        let err = handler
            .handle_category("server", "api", properties)
            .unwrap_err();
        assert!(err.to_string().contains("missing required 'path'"));
    }

    #[test]
    fn missing_route_method_is_error() {
        let (mut handler, _) = make_handler();
        let properties = props(vec![
            ("type", json!("http")),
            ("port", json!(8080)),
            (
                "route",
                json!({ "bad": { "path": "/x", "component": "test", "function": "test" } }),
            ),
        ]);
        let err = handler
            .handle_category("server", "api", properties)
            .unwrap_err();
        assert!(
            err.to_string().contains("missing required 'method'"),
            "unexpected: {err}"
        );
    }

    #[test]
    fn channel_and_component_is_error() {
        let (mut handler, _) = make_handler();
        let properties = props(vec![
            ("type", json!("http")),
            ("port", json!(8080)),
            (
                "route",
                json!({
                    "bad": {
                        "method": "GET",
                        "path": "/bad",
                        "component": "mutually",
                        "function": "test",
                        "channel": "exclusive"
                    }
                }),
            ),
        ]);
        let err = handler
            .handle_category("server", "api", properties)
            .unwrap_err();
        assert!(err.to_string().contains("cannot have both"));
    }

    #[test]
    fn conflicting_routes_anonymous_vs_named_capture() {
        let (mut handler, _) = make_handler();
        let properties = props(vec![
            ("type", json!("http")),
            ("port", json!(8080)),
            (
                "route",
                json!({
                    "a": {
                        "method": "GET",
                        "path": "/users/{id}",
                        "component": "user-service",
                        "function": "get-by-id"
                    },
                    "b": {
                        "method": "GET",
                        "path": "/users/{}",
                        "component": "user-service",
                        "function": "get"
                    }
                }),
            ),
        ]);
        let err = handler
            .handle_category("server", "api", properties)
            .unwrap_err();
        assert!(err.to_string().contains("conflict"));
    }

    #[test]
    fn duplicate_capture_name_is_error() {
        let (mut handler, _) = make_handler();
        let properties = props(vec![
            ("type", json!("http")),
            ("port", json!(8080)),
            (
                "route",
                json!({
                    "bad": {
                        "method": "GET",
                        "path": "/a/{id}/b/{id}",
                        "component": "test",
                        "function": "x"
                    }
                }),
            ),
        ]);
        let err = handler
            .handle_category("server", "api", properties)
            .unwrap_err();
        assert!(err.to_string().contains("duplicate path capture name"));
    }

    #[test]
    fn capturing_query_param_collides_with_path_capture_is_error() {
        let (mut handler, _) = make_handler();
        let properties = props(vec![
            ("type", json!("http")),
            ("port", json!(8080)),
            (
                "route",
                json!({
                    "bad": {
                        "method": "GET",
                        "path": "/users/{id}",
                        "query-params": ["id"],
                        "component": "test",
                        "function": "x"
                    }
                }),
            ),
        ]);
        let err = handler
            .handle_category("server", "api", properties)
            .unwrap_err();
        assert!(
            err.to_string()
                .contains("collides with path capture of the same name"),
            "unexpected: {err}"
        );
    }

    #[test]
    fn non_capturing_query_param_named_like_path_capture_is_ok() {
        let (mut handler, _) = make_handler();
        let properties = props(vec![
            ("type", json!("http")),
            ("port", json!(8080)),
            (
                "route",
                json!({
                    "ok": {
                        "method": "GET",
                        "path": "/users/{id}",
                        "query-params": ["~id=admin"],
                        "component": "test",
                        "function": "x"
                    }
                }),
            ),
        ]);
        handler
            .handle_category("server", "api", properties)
            .unwrap();
    }

    #[test]
    fn body_accepting_method_defaults_content_type_to_json() {
        let (mut handler, config) = make_handler();
        let properties = props(vec![
            ("type", json!("http")),
            ("port", json!(8080)),
            (
                "route",
                json!({
                    "r": {
                        "method": "POST",
                        "path": "/x",
                        "component": "c",
                        "function": "f"
                    }
                }),
            ),
        ]);
        handler
            .handle_category("server", "api", properties)
            .unwrap();
        let servers = config.lock().unwrap();
        assert_eq!(servers[0].routes[0].content_type, Some(ContentType::Json));
    }

    #[test]
    fn non_body_accepting_method_has_no_content_type() {
        let (mut handler, config) = make_handler();
        let properties = props(vec![
            ("type", json!("http")),
            ("port", json!(8080)),
            (
                "route",
                json!({
                    "r": {
                        "method": "GET",
                        "path": "/x",
                        "component": "c",
                        "function": "f"
                    }
                }),
            ),
        ]);
        handler
            .handle_category("server", "api", properties)
            .unwrap();
        let servers = config.lock().unwrap();
        assert_eq!(servers[0].routes[0].content_type, None);
    }

    #[test]
    fn explicit_text_plain_content_type_accepted_on_body_accepting_method() {
        let (mut handler, config) = make_handler();
        let properties = props(vec![
            ("type", json!("http")),
            ("port", json!(8080)),
            (
                "route",
                json!({
                    "r": {
                        "method": "POST",
                        "path": "/x",
                        "content-type": "text/plain",
                        "component": "c",
                        "function": "f"
                    }
                }),
            ),
        ]);
        handler
            .handle_category("server", "api", properties)
            .unwrap();
        let servers = config.lock().unwrap();
        assert_eq!(
            servers[0].routes[0].content_type,
            Some(ContentType::TextPlain)
        );
    }

    #[test]
    fn unknown_content_type_value_is_error() {
        let (mut handler, _) = make_handler();
        let properties = props(vec![
            ("type", json!("http")),
            ("port", json!(8080)),
            (
                "route",
                json!({
                    "r": {
                        "method": "POST",
                        "path": "/x",
                        "content-type": "application/xml",
                        "component": "c",
                        "function": "f"
                    }
                }),
            ),
        ]);
        let err = handler
            .handle_category("server", "api", properties)
            .unwrap_err();
        assert!(
            err.to_string().contains("not a supported content-type"),
            "unexpected: {err}"
        );
    }

    #[test]
    fn content_type_on_non_body_accepting_method_is_error() {
        let (mut handler, _) = make_handler();
        let properties = props(vec![
            ("type", json!("http")),
            ("port", json!(8080)),
            (
                "route",
                json!({
                    "r": {
                        "method": "GET",
                        "path": "/x",
                        "content-type": "application/json",
                        "component": "c",
                        "function": "f"
                    }
                }),
            ),
        ]);
        let err = handler
            .handle_category("server", "api", properties)
            .unwrap_err();
        assert!(
            err.to_string()
                .contains("must not be declared on method 'GET'"),
            "unexpected: {err}"
        );
    }

    #[test]
    fn text_plain_with_param_mapping_is_error() {
        let (mut handler, _) = make_handler();
        let properties = props(vec![
            ("type", json!("http")),
            ("port", json!(8080)),
            (
                "route",
                json!({
                    "r": {
                        "method": "POST",
                        "path": "/x",
                        "content-type": "text/plain",
                        "component": "c",
                        "function": "f",
                        "param-mapping": { "a": "{body}" }
                    }
                }),
            ),
        ]);
        let err = handler
            .handle_category("server", "api", properties)
            .unwrap_err();
        assert!(
            err.to_string().contains("'param-mapping' is not allowed"),
            "unexpected: {err}"
        );
    }

    #[test]
    fn text_plain_with_param_encoding_is_error() {
        let (mut handler, _) = make_handler();
        let properties = props(vec![
            ("type", json!("http")),
            ("port", json!(8080)),
            (
                "route",
                json!({
                    "r": {
                        "method": "POST",
                        "path": "/x",
                        "content-type": "text/plain",
                        "component": "c",
                        "function": "f",
                        "param-encoding": { "a": "application/json" }
                    }
                }),
            ),
        ]);
        let err = handler
            .handle_category("server", "api", properties)
            .unwrap_err();
        assert!(
            err.to_string().contains("'param-encoding' is not allowed"),
            "unexpected: {err}"
        );
    }

    #[test]
    fn text_plain_with_path_capture_is_error() {
        let (mut handler, _) = make_handler();
        let properties = props(vec![
            ("type", json!("http")),
            ("port", json!(8080)),
            (
                "route",
                json!({
                    "r": {
                        "method": "POST",
                        "path": "/users/{id}",
                        "content-type": "text/plain",
                        "component": "c",
                        "function": "f"
                    }
                }),
            ),
        ]);
        let err = handler
            .handle_category("server", "api", properties)
            .unwrap_err();
        assert!(
            err.to_string()
                .contains("path/query captures are not allowed"),
            "unexpected: {err}"
        );
    }

    #[test]
    fn text_plain_with_capturing_query_param_is_error() {
        let (mut handler, _) = make_handler();
        let properties = props(vec![
            ("type", json!("http")),
            ("port", json!(8080)),
            (
                "route",
                json!({
                    "r": {
                        "method": "POST",
                        "path": "/x",
                        "content-type": "text/plain",
                        "query-params": ["filter"],
                        "component": "c",
                        "function": "f"
                    }
                }),
            ),
        ]);
        let err = handler
            .handle_category("server", "api", properties)
            .unwrap_err();
        assert!(
            err.to_string()
                .contains("path/query captures are not allowed"),
            "unexpected: {err}"
        );
    }

    #[test]
    fn same_method_path_query_different_content_types_coexist() {
        let (mut handler, config) = make_handler();
        let properties = props(vec![
            ("type", json!("http")),
            ("port", json!(8080)),
            (
                "route",
                json!({
                    "json": {
                        "method": "POST",
                        "path": "/x",
                        "content-type": "application/json",
                        "component": "c",
                        "function": "f"
                    },
                    "text": {
                        "method": "POST",
                        "path": "/x",
                        "content-type": "text/plain",
                        "component": "c",
                        "function": "f"
                    }
                }),
            ),
        ]);
        handler
            .handle_category("server", "api", properties)
            .unwrap();
        let servers = config.lock().unwrap();
        assert_eq!(servers[0].routes.len(), 2);
    }

    #[test]
    fn same_method_path_query_same_content_type_is_conflict() {
        let (mut handler, _) = make_handler();
        let properties = props(vec![
            ("type", json!("http")),
            ("port", json!(8080)),
            (
                "route",
                json!({
                    "a": {
                        "method": "POST",
                        "path": "/x",
                        "content-type": "application/json",
                        "component": "c",
                        "function": "f"
                    },
                    "b": {
                        "method": "POST",
                        "path": "/x",
                        "component": "c",
                        "function": "f"
                    }
                }),
            ),
        ]);
        let err = handler
            .handle_category("server", "api", properties)
            .unwrap_err();
        assert!(err.to_string().contains("conflict"), "unexpected: {err}");
    }

    // Routes at the same path+method but with mutually-exclusive query specs
    // (different required values on the same key) are accepted.
    #[test]
    fn same_path_disjoint_required_values_is_ok() {
        let (mut handler, config) = make_handler();
        let properties = props(vec![
            ("type", json!("http")),
            ("port", json!(8080)),
            (
                "route",
                json!({
                    "list-admins": {
                        "method": "GET",
                        "path": "/users",
                        "component": "user-service",
                        "function": "list-admins",
                        "query-params": ["type=admin"]
                    },
                    "list-users": {
                        "method": "GET",
                        "path": "/users",
                        "component": "user-service",
                        "function": "list-users",
                        "query-params": ["type=user"]
                    }
                }),
            ),
        ]);

        handler
            .handle_category("server", "api", properties)
            .unwrap();
        let servers = config.lock().unwrap();
        assert_eq!(servers[0].routes.len(), 2);
    }

    // Required vs Forbidden on the same key is mutually exclusive.
    #[test]
    fn same_path_required_vs_forbidden_is_ok() {
        let (mut handler, _) = make_handler();
        let properties = props(vec![
            ("type", json!("http")),
            ("port", json!(8080)),
            (
                "route",
                json!({
                    "with-debug": {
                        "method": "GET",
                        "path": "/x",
                        "component": "y",
                        "function": "debug",
                        "query-params": ["debug"]
                    },
                    "no-debug": {
                        "method": "GET",
                        "path": "/x",
                        "component": "y",
                        "function": "normal",
                        "query-params": ["!debug"]
                    }
                }),
            ),
        ]);
        handler
            .handle_category("server", "api", properties)
            .unwrap();
    }

    // Two Optional specs are NOT mutually exclusive (both match a request that
    // omits the param entirely). Routes should be rejected as conflicting.
    #[test]
    fn same_path_both_optional_is_conflict() {
        let (mut handler, _) = make_handler();
        let properties = props(vec![
            ("type", json!("http")),
            ("port", json!(8080)),
            (
                "route",
                json!({
                    "a": {
                        "method": "GET",
                        "path": "/x",
                        "component": "y",
                        "function": "a",
                        "query-params": ["type?=admin"]
                    },
                    "b": {
                        "method": "GET",
                        "path": "/x",
                        "component": "y",
                        "function": "b",
                        "query-params": ["type?=user"]
                    }
                }),
            ),
        ]);
        let err = handler
            .handle_category("server", "api", properties)
            .unwrap_err();
        assert!(
            err.to_string().contains("conflict"),
            "expected conflict, got: {err}"
        );
    }

    // Required-any-value vs Required-fixed-value overlap on the fixed value
    // (a request with `?type=admin` matches both). Conflict.
    #[test]
    fn same_path_required_vs_required_fixed_is_conflict() {
        let (mut handler, _) = make_handler();
        let properties = props(vec![
            ("type", json!("http")),
            ("port", json!(8080)),
            (
                "route",
                json!({
                    "a": {
                        "method": "GET",
                        "path": "/x",
                        "component": "y",
                        "function": "a",
                        "query-params": ["type"]
                    },
                    "b": {
                        "method": "GET",
                        "path": "/x",
                        "component": "y",
                        "function": "b",
                        "query-params": ["type=admin"]
                    }
                }),
            ),
        ]);
        let err = handler
            .handle_category("server", "api", properties)
            .unwrap_err();
        assert!(err.to_string().contains("conflict"));
    }

    // Disjoint via one key, plus an overlapping spec on another key, still
    // accepted: a single disjoint key is sufficient.
    #[test]
    fn same_path_one_disjoint_key_suffices() {
        let (mut handler, _) = make_handler();
        let properties = props(vec![
            ("type", json!("http")),
            ("port", json!(8080)),
            (
                "route",
                json!({
                    "a": {
                        "method": "GET",
                        "path": "/x",
                        "component": "y",
                        "function": "a",
                        "query-params": ["type=admin", "verbose?"]
                    },
                    "b": {
                        "method": "GET",
                        "path": "/x",
                        "component": "y",
                        "function": "b",
                        "query-params": ["type=user", "verbose?"]
                    }
                }),
            ),
        ]);
        handler
            .handle_category("server", "api", properties)
            .unwrap();
    }

    fn parse_qp(s: &str) -> QueryParamSpec {
        let ctx = |_field: &str, msg: &str| -> anyhow::Error { anyhow::anyhow!("{msg}") };
        parse_query_param_spec(s, &ctx).unwrap()
    }

    #[test]
    fn query_param_required_bare() {
        let spec = parse_qp("key");
        assert_eq!(spec.name, "key");
        match spec.kind {
            QueryParamKind::Required {
                value: None,
                capture: true,
            } => {}
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn query_param_required_with_value() {
        let spec = parse_qp("type=user");
        assert_eq!(spec.name, "type");
        match spec.kind {
            QueryParamKind::Required {
                value: Some(v),
                capture: true,
            } => assert_eq!(v, "user"),
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn query_param_optional() {
        let spec = parse_qp("limit?");
        match spec.kind {
            QueryParamKind::Optional {
                value: None,
                capture: true,
            } => {}
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn query_param_optional_with_value() {
        let spec = parse_qp("scope?=public");
        match spec.kind {
            QueryParamKind::Optional {
                value: Some(v),
                capture: true,
            } => assert_eq!(v, "public"),
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn query_param_no_capture() {
        let spec = parse_qp("~active");
        match spec.kind {
            QueryParamKind::Required { capture: false, .. } => {}
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn query_param_no_capture_optional_with_value() {
        let spec = parse_qp("~feature?=detail");
        match spec.kind {
            QueryParamKind::Optional {
                value: Some(v),
                capture: false,
            } => assert_eq!(v, "detail"),
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn query_param_no_capture_optional_no_value_is_error() {
        let ctx = |_field: &str, msg: &str| -> anyhow::Error { anyhow::anyhow!("{msg}") };
        let err = parse_query_param_spec("~feature?", &ctx).unwrap_err();
        assert!(
            err.to_string().contains("has no effect"),
            "unexpected: {err}"
        );
    }

    #[test]
    fn query_param_forbidden() {
        let spec = parse_qp("!debug");
        assert!(matches!(spec.kind, QueryParamKind::Forbidden));
    }

    #[test]
    fn query_param_forbidden_with_value_is_error() {
        let ctx = |_field: &str, msg: &str| -> anyhow::Error { anyhow::anyhow!("{msg}") };
        let err = parse_query_param_spec("!debug=true", &ctx).unwrap_err();
        assert!(err.to_string().contains("cannot have '='"));
    }

    #[test]
    fn query_param_empty_name_is_error() {
        let ctx = |_field: &str, msg: &str| -> anyhow::Error { anyhow::anyhow!("{msg}") };
        let err = parse_query_param_spec("?", &ctx).unwrap_err();
        assert!(err.to_string().contains("empty name"));
    }

    #[test]
    fn selector_matches_http_type() {
        let handler = HttpServerConfigHandler::new(shared_config());
        let claims = handler.claimed_categories();
        assert_eq!(claims.len(), 1);
        let selector = claims[0].selector.as_ref().unwrap();
        let mut p = HashMap::new();
        p.insert("type".to_string(), Some("http".to_string()));
        assert!(selector.matches(&p));
        let mut p2 = HashMap::new();
        p2.insert("type".to_string(), Some("grpc".to_string()));
        assert!(!selector.matches(&p2));
    }
}
