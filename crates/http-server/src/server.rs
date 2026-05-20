use std::collections::HashMap;
use std::sync::Arc;

use anyhow::Result;
use http_body_util::{BodyExt, Full};
use hyper::body::{Bytes, Incoming};
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{HeaderMap, Method, Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use hyper_util::server::graceful::GracefulShutdown;
use opentelemetry::trace::{Span, SpanKind, Tracer, TracerProvider};
use opentelemetry_otlp::{SpanExporter, WithExportConfig};
use opentelemetry_sdk::{
    runtime::Tokio,
    trace::{SdkTracerProvider, span_processor_with_async_runtime::BatchSpanProcessor},
};
use serde_json::{Map, Value};
use tokio::net::TcpListener;
use tokio::sync::watch;

use composable_runtime::{
    ComponentInvoker, Message, MessageBuilder, MessageHeaders, MessageMapper, MessagePublisher,
    PROPAGATED_HEADERS, PROPAGATION_CONTEXT, PropagatedHeader, PropagationContext, schema,
};

use crate::config::{
    ContentType, QueryParamKind, QueryParamSpec, RouteConfig, RouteTarget, ServerConfig,
};

// Schema for the assembled Message body. Used to validate inbound requests.
struct RequestSchema {
    validator: jsonschema::Validator,
}

impl RequestSchema {
    fn new(spec: &Value) -> Result<Self> {
        let validator = jsonschema::validator_for(spec)
            .map_err(|e| anyhow::anyhow!("failed to build request-schema validator: {e}"))?;
        Ok(Self { validator })
    }

    fn validate(&self, value: &Value) -> Result<(), String> {
        self.validator.validate(value).map_err(|e| e.to_string())
    }
}

// Bundles a JSON Schema value with its compiled validator. Used to coerce and
// validate response bodies.
struct ResponseSchema {
    spec: Value,
    validator: jsonschema::Validator,
}

impl ResponseSchema {
    fn new(spec: Value) -> Result<Self> {
        let validator = jsonschema::validator_for(&spec)
            .map_err(|e| anyhow::anyhow!("failed to build response-schema validator: {e}"))?;
        Ok(Self { spec, validator })
    }

    fn coerce(&self, value: &mut Value) -> Result<(), String> {
        schema::coerce_value(value, &self.spec)
    }

    fn validate(&self, value: &Value) -> Result<(), String> {
        self.validator.validate(value).map_err(|e| e.to_string())
    }
}

struct Route {
    name: String,
    method: Method,
    path_segments: Vec<Segment>,
    query_params: Vec<QueryParamSpec>,
    content_type: Option<ContentType>,
    target: CompiledTarget,
    request_schema: Option<RequestSchema>,
    propagate_request_headers: Vec<PropagatedHeader>,
    response_schema: Option<ResponseSchema>,
    propagate_response_headers: Vec<PropagatedHeader>,
}

enum CompiledTarget {
    Component {
        component_name: String,
        mapper: Arc<MessageMapper>,
        function_name_for_span: String,
    },
    Channel {
        channel: String,
        reply_timeout_ms: Option<u64>,
    },
}

enum Segment {
    Literal(String),
    // Named capture, e.g. `{id}`.
    Param(String),
    // Anonymous placeholder, e.g. `{}` (matches segment without capturing).
    Anonymous,
}

impl Route {
    fn from_config(config: &RouteConfig, invoker: &Arc<dyn ComponentInvoker>) -> Result<Self> {
        let method = config.method.parse::<Method>().map_err(|e| {
            anyhow::anyhow!(
                "route '{}': invalid method '{}': {e}",
                config.name,
                config.method
            )
        })?;

        let path_segments: Vec<_> = config
            .path
            .split('/')
            .filter(|s| !s.is_empty())
            .map(|s| {
                if s.starts_with('{') && s.ends_with('}') {
                    let inner = &s[1..s.len() - 1];
                    if inner.is_empty() {
                        Segment::Anonymous
                    } else {
                        Segment::Param(inner.to_string())
                    }
                } else {
                    Segment::Literal(s.to_string())
                }
            })
            .collect();

        // Resolve target and the final response_schema. Component-routes have
        // a knowable WIT signature, so the schema can be derived from
        // result-mapping (if present) or from the WIT return type. When the
        // route also declares an explicit response-schema, its alignment is
        // validated. Channel-routes cannot have derived schemas (no WIT
        // available at the surface), so the explicit response-schema is used.
        let (target, request_schema_spec, response_schema_spec) = match &config.target {
            RouteTarget::Component {
                component,
                function,
                mapping,
            } => {
                let component_def = invoker.get_component(component).ok_or_else(|| {
                    anyhow::anyhow!(
                        "route '{}': component '{}' not found",
                        config.name,
                        component
                    )
                })?;
                let func = component_def.functions.get(function).ok_or_else(|| {
                    anyhow::anyhow!(
                        "route '{}': function '{}' not found in component '{}'",
                        config.name,
                        function,
                        component
                    )
                })?;

                let request_schema_spec: Value = schema::derive_input_schema(func, mapping)
                    .map_err(|e| {
                        anyhow::anyhow!(
                            "route '{}': request-schema derivation failed: {e}",
                            config.name
                        )
                    })?;

                let derived_response_schema: Option<Value> =
                    schema::derive_output_schema(func, mapping).map_err(|e| {
                        anyhow::anyhow!(
                            "route '{}': response-schema derivation failed: {e}",
                            config.name
                        )
                    })?;

                let resolved_response_schema = match (
                    config.response_schema.as_ref(),
                    derived_response_schema.as_ref(),
                ) {
                    (Some(declared), Some(derived)) => {
                        schema::validate_structural_alignment(declared, derived).map_err(|e| {
                            anyhow::anyhow!(
                                "route '{}': explicit response-schema does not align \
                                         with derived schema: {e}",
                                config.name
                            )
                        })?;
                        Some(declared.clone())
                    }
                    (Some(declared), None) => Some(declared.clone()),
                    (None, Some(derived)) => Some(derived.clone()),
                    (None, None) => None,
                };

                let mapper = MessageMapper::from_component(
                    component_def,
                    Some(function.clone()),
                    mapping.clone(),
                )
                .map_err(|e| anyhow::anyhow!("route '{}': {e}", config.name))?;

                let target = CompiledTarget::Component {
                    component_name: component.clone(),
                    mapper: Arc::new(mapper),
                    function_name_for_span: function.clone(),
                };
                (target, Some(request_schema_spec), resolved_response_schema)
            }
            RouteTarget::Channel {
                channel,
                reply_timeout_ms,
            } => {
                let target = CompiledTarget::Channel {
                    channel: channel.clone(),
                    reply_timeout_ms: *reply_timeout_ms,
                };
                (target, None, config.response_schema.clone())
            }
        };

        let request_schema = request_schema_spec
            .as_ref()
            .map(|spec| {
                RequestSchema::new(spec)
                    .map_err(|e| anyhow::anyhow!("route '{}': {e}", config.name))
            })
            .transpose()?;

        let response_schema = response_schema_spec
            .map(|spec| {
                ResponseSchema::new(spec)
                    .map_err(|e| anyhow::anyhow!("route '{}': {e}", config.name))
            })
            .transpose()?;

        Ok(Self {
            name: config.name.clone(),
            method,
            path_segments,
            query_params: config.query_params.clone(),
            content_type: config.content_type,
            target,
            request_schema,
            propagate_request_headers: config.propagate_request_headers.clone(),
            response_schema,
            propagate_response_headers: config.propagate_response_headers.clone(),
        })
    }

    // Match a request against this route. Returns Some(captures) if the route
    // applies, None if it does not (try the next route).
    fn match_request(
        &self,
        method: &Method,
        path: &str,
        query: &HashMap<String, String>,
    ) -> Option<Captures> {
        if *method != self.method {
            return None;
        }

        let parts: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
        if parts.len() != self.path_segments.len() {
            return None;
        }

        let mut path_captures = HashMap::new();
        for (part, segment) in parts.iter().zip(&self.path_segments) {
            match segment {
                Segment::Literal(expected) if part == expected => {}
                Segment::Param(name) => {
                    path_captures.insert(name.clone(), (*part).to_string());
                }
                Segment::Anonymous => {}
                _ => return None,
            }
        }

        let mut query_captures = HashMap::new();
        for spec in &self.query_params {
            match self.apply_query_spec(spec, query) {
                QueryOutcome::Match => {}
                QueryOutcome::NoMatch => return None,
                QueryOutcome::Capture(value) => {
                    query_captures.insert(spec.name.clone(), value);
                }
            }
        }

        Some(Captures {
            path: path_captures,
            query: query_captures,
        })
    }

    fn apply_query_spec(
        &self,
        spec: &QueryParamSpec,
        query: &HashMap<String, String>,
    ) -> QueryOutcome {
        let actual = query.get(&spec.name);
        match &spec.kind {
            QueryParamKind::Forbidden => {
                if actual.is_some() {
                    QueryOutcome::NoMatch
                } else {
                    QueryOutcome::Match
                }
            }
            QueryParamKind::Required { value, capture } => match actual {
                None => QueryOutcome::NoMatch,
                Some(v) => {
                    if let Some(expected) = value
                        && v != expected
                    {
                        return QueryOutcome::NoMatch;
                    }
                    if *capture {
                        QueryOutcome::Capture(v.clone())
                    } else {
                        QueryOutcome::Match
                    }
                }
            },
            QueryParamKind::Optional { value, capture } => match actual {
                None => QueryOutcome::Match,
                Some(v) => {
                    if let Some(expected) = value
                        && v != expected
                    {
                        return QueryOutcome::NoMatch;
                    }
                    if *capture {
                        QueryOutcome::Capture(v.clone())
                    } else {
                        QueryOutcome::Match
                    }
                }
            },
        }
    }
}

enum QueryOutcome {
    Match,
    NoMatch,
    Capture(String),
}

struct Captures {
    path: HashMap<String, String>,
    query: HashMap<String, String>,
}

impl Captures {
    // Merge path + query captures into one map. Config-time validation
    // rejects routes where a capturing query-param shares a name with a path
    // capture, so the two maps have disjoint keys.
    fn merged(&self) -> HashMap<String, String> {
        let mut out = self.query.clone();
        for (k, v) in &self.path {
            out.insert(k.clone(), v.clone());
        }
        out
    }
}

struct Router {
    routes: Vec<Route>,
    invoker: Arc<dyn ComponentInvoker>,
    publisher: Option<Arc<dyn MessagePublisher>>,
    tracer_provider: Option<SdkTracerProvider>,
}

impl Router {
    async fn handle(&self, req: Request<Incoming>) -> Result<Response<Full<Bytes>>, hyper::Error> {
        let method = req.method().clone();
        let path = req.uri().path().to_string();
        let query = parse_query_string(req.uri().query().unwrap_or(""));

        let request = match collect_request_content(req).await {
            Ok(r) => r,
            Err((status, msg)) => return Ok(error_response(status, msg)),
        };

        for route in &self.routes {
            if let Some(captures) = route.match_request(&method, &path, &query) {
                tracing::debug!(route = %route.name, %method, %path, "matched route");
                return Ok(self.dispatch(route, captures, &request).await);
            }
        }

        Ok(Response::builder()
            .status(StatusCode::NOT_FOUND)
            .body(Full::new(Bytes::from("Not Found")))
            .unwrap())
    }

    async fn dispatch(
        &self,
        route: &Route,
        captures: Captures,
        request: &RequestContent,
    ) -> Response<Full<Bytes>> {
        let message = match build_message_from_request(
            request,
            captures,
            route.content_type,
            route.request_schema.as_ref(),
            &route.propagate_request_headers,
        ) {
            Ok(m) => m,
            Err((status, msg)) => return error_response(status, msg),
        };

        match &route.target {
            CompiledTarget::Component {
                component_name,
                mapper,
                function_name_for_span,
            } => {
                self.invoke_component(
                    component_name,
                    function_name_for_span,
                    Arc::clone(mapper),
                    message,
                    route.response_schema.as_ref(),
                    &route.propagate_response_headers,
                )
                .await
            }
            CompiledTarget::Channel {
                channel,
                reply_timeout_ms,
            } => {
                self.publish_to_channel(
                    channel,
                    message,
                    *reply_timeout_ms,
                    route.response_schema.as_ref(),
                    &route.propagate_response_headers,
                )
                .await
            }
        }
    }

    async fn invoke_component(
        &self,
        component_name: &str,
        function_name: &str,
        mapper: Arc<MessageMapper>,
        message: Message,
        response_schema: Option<&ResponseSchema>,
        propagate_response_headers: &[PropagatedHeader],
    ) -> Response<Full<Bytes>> {
        // Extract propagated headers from the Message (placed there by
        // `build_message_from_request`). These travel both as invocation
        // context and as headers on the eventual reply Message.
        let mut propagated: HashMap<String, String> = HashMap::new();
        for key in PROPAGATED_HEADERS {
            if let Some(val) = message.headers().get::<&str>(key) {
                propagated.insert((*key).to_string(), val.to_string());
            }
        }

        // Set up host span around the invocation.
        let mut host_span = self.tracer_provider.as_ref().map(|tp| {
            use opentelemetry::propagation::TextMapPropagator;
            use opentelemetry_sdk::propagation::TraceContextPropagator;
            let propagator = TraceContextPropagator::new();
            let parent_cx = propagator.extract(&propagated);
            let tracer = tp.tracer("composable-http-server");
            tracer
                .span_builder(format!("{component_name}/{function_name}"))
                .with_kind(SpanKind::Server)
                .start_with_context(&tracer, &parent_cx)
        });

        // Update traceparent with the host span if newly created,
        // or generate one if not already provided by inbound request.
        if let Some(span) = &host_span {
            let sc = span.span_context();
            propagated.insert(
                "traceparent".to_string(),
                format!(
                    "00-{:032x}-{:016x}-{:02x}",
                    sc.trace_id(),
                    sc.span_id(),
                    sc.trace_flags()
                ),
            );
        } else if !propagated.contains_key("traceparent") {
            let trace_id = uuid::Uuid::new_v4().as_simple().to_string();
            let span_id = &uuid::Uuid::new_v4().as_simple().to_string()[..16];
            propagated.insert(
                "traceparent".to_string(),
                format!("00-{trace_id}-{span_id}-01"),
            );
        }

        let result = self
            .invoke_with_mapper(
                component_name,
                mapper,
                message,
                propagated.clone(),
                response_schema,
                propagate_response_headers,
            )
            .await;

        if let Some(span) = &mut host_span {
            match &result {
                Ok(_) => span.set_status(opentelemetry::trace::Status::Ok),
                Err((_, msg)) => span.set_status(opentelemetry::trace::Status::error(msg.clone())),
            }
            span.end();
        }

        match result {
            Ok(response) => response,
            Err((status, msg)) => error_response(status, msg),
        }
    }

    async fn invoke_with_mapper(
        &self,
        component_name: &str,
        mapper: Arc<MessageMapper>,
        message: Message,
        propagated: HashMap<String, String>,
        response_schema: Option<&ResponseSchema>,
        propagate_response_headers: &[PropagatedHeader],
    ) -> Result<Response<Full<Bytes>>, (StatusCode, String)> {
        let invocation = mapper
            .to_invocation(&message)
            .map_err(|e| (StatusCode::BAD_REQUEST, e))?;

        // Scope the propagation context for the invocation. Downstream code
        // (e.g. host imports for outbound HTTP) reads via the task-local.
        // The explicit scope here is the boundary.
        let invoke_fut = self.invoker.invoke(
            component_name,
            invocation.function_key.as_str(),
            invocation.args,
            None,
        );
        let wit_result = if propagated.is_empty() {
            invoke_fut.await
        } else {
            let ctx = PropagationContext {
                entries: propagated.clone(),
            };
            PROPAGATION_CONTEXT.scope(Some(ctx), invoke_fut).await
        }
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("invocation error: {e}"),
            )
        })?;

        let reply = mapper
            .from_invocation_result(&wit_result, propagated)
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;

        build_http_response_from_message(reply, response_schema, propagate_response_headers)
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))
    }

    async fn publish_to_channel(
        &self,
        channel: &str,
        message: Message,
        reply_timeout_ms: Option<u64>,
        response_schema: Option<&ResponseSchema>,
        propagate_response_headers: &[PropagatedHeader],
    ) -> Response<Full<Bytes>> {
        let publisher = match &self.publisher {
            Some(p) => p,
            None => {
                return error_response(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "channel routes require messaging to be enabled".to_string(),
                );
            }
        };

        match reply_timeout_ms {
            None => match publisher.publish(channel, message).await {
                Ok(()) => Response::builder()
                    .status(StatusCode::ACCEPTED)
                    .body(Full::new(Bytes::new()))
                    .unwrap(),
                Err(e) => error_response(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!("failed to publish to channel '{channel}': {e}"),
                ),
            },
            Some(timeout_ms) => {
                let handle = match publisher.publish_request(channel, message).await {
                    Ok(h) => h,
                    Err(e) => {
                        return error_response(
                            StatusCode::INTERNAL_SERVER_ERROR,
                            format!("failed to publish to channel '{channel}': {e}"),
                        );
                    }
                };

                let timeout = std::time::Duration::from_millis(timeout_ms);
                match tokio::time::timeout(timeout, handle.take()).await {
                    Ok(Ok(reply)) => match build_http_response_from_message(
                        reply,
                        response_schema,
                        propagate_response_headers,
                    ) {
                        Ok(r) => r,
                        Err(e) => error_response(StatusCode::INTERNAL_SERVER_ERROR, e),
                    },
                    Ok(Err(e)) => error_response(
                        StatusCode::INTERNAL_SERVER_ERROR,
                        format!("reply error: {e}"),
                    ),
                    Err(_) => error_response(
                        StatusCode::GATEWAY_TIMEOUT,
                        format!("no reply received within {timeout_ms}ms"),
                    ),
                }
            }
        }
    }
}

// Parse a URL query string into a map. Repeated keys take the last value.
fn parse_query_string(q: &str) -> HashMap<String, String> {
    form_urlencoded::parse(q.as_bytes())
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect()
}

// The headers + body bytes of an HTTP request, after the streaming body has
// been collected.
struct RequestContent {
    headers: HeaderMap,
    body: Bytes,
}

// Collect the streaming body of an HTTP request and split off the headers.
async fn collect_request_content(
    req: Request<Incoming>,
) -> Result<RequestContent, (StatusCode, String)> {
    let (parts, body) = req.into_parts();
    let body = body
        .collect()
        .await
        .map_err(|e| (StatusCode::BAD_REQUEST, format!("failed to read body: {e}")))?
        .to_bytes();
    Ok(RequestContent {
        headers: parts.headers,
        body,
    })
}

// Build a Message from collected HTTP request content, merging path + query
// captures into the parsed body and attaching declared headers.
//
// Rules for body construction:
// - If the request body is empty: produce `{}` (object to attach captures)
//   when captures exist, else empty bytes.
// - If the request body is JSON and parses as an object: merge captures into
//   its top level. Collision (capture name == body field name) = 400.
// - If the request body is JSON and parses as a non-object (array/scalar) AND
//   captures exist: 400 ("non-object body cannot be merged with captures").
// - If content-type is `text/plain`: the body is the raw text. Captures with
//   a text/plain body is a 400 (no place to merge).
// - If `request_schema` is provided and the assembled body is JSON, the body
//   must conform; non-conformance is a 400.
//
// Header rules:
// - PROPAGATED_HEADERS (traceparent, tracestate, baggage) are always copied.
// - Each entry in `propagate_request_headers` reads the named HTTP header
//   (case-insensitively) and writes a Message header under the entry's
//   target name (the source name if no rename).
// - All copied header values are stored as Message header strings.
fn build_message_from_request(
    request: &RequestContent,
    captures: Captures,
    route_content_type: Option<ContentType>,
    request_schema: Option<&RequestSchema>,
    propagate_request_headers: &[PropagatedHeader],
) -> Result<Message, (StatusCode, String)> {
    // Resolve the effective inbound content-type. When `route_content_type` is
    // None the route does not accept a body and the request's Content-Type
    // header is ignored. Otherwise the request's Content-Type, if present,
    // must match the declared type.
    let content_type = match route_content_type {
        None => None,
        Some(declared) => {
            let request_ct = request
                .headers
                .get("content-type")
                .and_then(|v| v.to_str().ok());
            match request_ct {
                Some(ct) if !ct.starts_with(declared.as_str()) => {
                    return Err((
                        StatusCode::UNSUPPORTED_MEDIA_TYPE,
                        format!(
                            "request content-type '{ct}' does not match declared '{}'",
                            declared.as_str()
                        ),
                    ));
                }
                Some(ct) => Some(ct.to_string()),
                None => Some(declared.as_str().to_string()),
            }
        }
    };

    let captures_map = captures.merged();

    // Construct the body Value. For routes that don't accept a body
    // (content_type is None) the request body bytes are ignored and the
    // Message body is built from captures only. Otherwise parse per the
    // effective content-type and merge captures per the rules above.
    let body_value = match content_type.as_deref() {
        None => {
            if captures_map.is_empty() {
                None
            } else {
                Some(captures_to_value(&captures_map))
            }
        }
        Some(_) if request.body.is_empty() => {
            if captures_map.is_empty() {
                None
            } else {
                Some(captures_to_value(&captures_map))
            }
        }
        Some(ct) if ct.starts_with("application/json") => {
            let parsed: Value = serde_json::from_slice(&request.body)
                .map_err(|e| (StatusCode::BAD_REQUEST, format!("invalid JSON body: {e}")))?;
            if captures_map.is_empty() {
                Some(parsed)
            } else {
                match parsed {
                    Value::Object(mut map) => {
                        for (k, v) in &captures_map {
                            if map.contains_key(k) {
                                return Err((
                                    StatusCode::BAD_REQUEST,
                                    format!(
                                        "path/query capture '{k}' collides with a body field of the same name"
                                    ),
                                ));
                            }
                            map.insert(k.clone(), Value::String(v.clone()));
                        }
                        Some(Value::Object(map))
                    }
                    _ => {
                        return Err((
                            StatusCode::BAD_REQUEST,
                            "non-object body cannot be merged with path/query captures".to_string(),
                        ));
                    }
                }
            }
        }
        Some(ct) if ct.starts_with("text/plain") => {
            if !captures_map.is_empty() {
                return Err((
                    StatusCode::BAD_REQUEST,
                    "text/plain body cannot be merged with path/query captures".to_string(),
                ));
            }
            let text = std::str::from_utf8(&request.body).map_err(|e| {
                (
                    StatusCode::BAD_REQUEST,
                    format!("body is not valid UTF-8: {e}"),
                )
            })?;
            Some(Value::String(text.to_string()))
        }
        Some(ct) => {
            return Err((
                StatusCode::UNSUPPORTED_MEDIA_TYPE,
                format!("unsupported content type: {ct}"),
            ));
        }
    };

    if let Some(request_schema) = request_schema {
        let value_to_validate = body_value
            .as_ref()
            .cloned()
            .unwrap_or(Value::Object(Default::default()));
        request_schema.validate(&value_to_validate).map_err(|e| {
            (
                StatusCode::BAD_REQUEST,
                format!("request body does not conform to request-schema: {e}"),
            )
        })?;
    }

    let serialized_body = match (&body_value, content_type.as_deref()) {
        (None, _) => Vec::new(),
        (Some(Value::String(s)), Some(ct)) if ct.starts_with("text/plain") => s.as_bytes().to_vec(),
        (Some(v), _) => serde_json::to_vec(v).map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("failed to serialize body: {e}"),
            )
        })?,
    };

    let message_content_type = content_type
        .as_deref()
        .unwrap_or(ContentType::Json.as_str());
    let mut builder = MessageBuilder::new(serialized_body)
        .header(MessageHeaders::CONTENT_TYPE, message_content_type);

    // Copy declared propagate-request-headers: read from the HTTP request
    // (case-insensitive lookup), write into the Message under the entry's
    // target name.
    for entry in propagate_request_headers {
        if let Some(val) = request
            .headers
            .get(entry.source())
            .and_then(|v| v.to_str().ok())
        {
            builder = builder.header(entry.target(), val);
        }
    }

    // PROPAGATED_HEADERS (traceparent etc.): read from the HTTP request and
    // place into the Message headers. MessageBuilder.build() will also auto-
    // attach them from task-local PROPAGATION_CONTEXT if set. The explicit
    // header here wins (it's already in the builder before .build()).
    for key in PROPAGATED_HEADERS {
        if let Some(val) = request.headers.get(*key).and_then(|v| v.to_str().ok()) {
            builder = builder.header(*key, val);
        }
    }

    Ok(builder.build())
}

fn captures_to_value(captures: &HashMap<String, String>) -> Value {
    let mut map = Map::with_capacity(captures.len());
    for (k, v) in captures {
        map.insert(k.clone(), Value::String(v.clone()));
    }
    Value::Object(map)
}

// Build an HTTP response from a reply Message. Applies the optional response
// schema as tolerant-reader coercion (stringify at string-typed positions).
// Emits HTTP response headers per `propagate_response_headers`: each entry
// reads a Message header by `source()` and writes it under `target()` on the
// HTTP response.
fn build_http_response_from_message(
    msg: Message,
    response_schema: Option<&ResponseSchema>,
    propagate_response_headers: &[PropagatedHeader],
) -> Result<Response<Full<Bytes>>, String> {
    let content_type = msg
        .headers()
        .content_type()
        .unwrap_or("application/json")
        .to_string();

    let body_bytes = if let Some(response_schema) = response_schema {
        // Parse, coerce, validate, re-serialize. Coercion and validation
        // only apply to JSON bodies.
        if content_type.starts_with("application/json") {
            let mut parsed: Value = if msg.body().is_empty() {
                Value::Null
            } else {
                serde_json::from_slice(msg.body())
                    .map_err(|e| format!("failed to parse reply body as JSON: {e}"))?
            };
            response_schema.coerce(&mut parsed)?;
            response_schema
                .validate(&parsed)
                .map_err(|e| format!("reply body does not conform to response-schema: {e}"))?;
            serde_json::to_vec(&parsed)
                .map_err(|e| format!("failed to serialize coerced reply body: {e}"))?
        } else {
            // No coercion for non-JSON content types: pass through.
            msg.body().to_vec()
        }
    } else {
        msg.body().to_vec()
    };

    let mut builder = Response::builder()
        .status(StatusCode::OK)
        .header("content-type", content_type);

    for entry in propagate_response_headers {
        if let Some(val) = msg.headers().get::<&str>(entry.source()) {
            builder = builder.header(entry.target(), val);
        }
    }

    let response = builder
        .body(Full::new(Bytes::from(body_bytes)))
        .map_err(|e| format!("failed to build response: {e}"))?;
    Ok(response)
}

fn error_response(status: StatusCode, msg: String) -> Response<Full<Bytes>> {
    Response::builder()
        .status(status)
        .body(Full::new(Bytes::from(msg)))
        .unwrap()
}

pub struct HttpServer {
    port: u16,
    router: Arc<Router>,
    tracer_provider: Option<SdkTracerProvider>,
}

impl HttpServer {
    pub fn new(
        config: ServerConfig,
        invoker: Arc<dyn ComponentInvoker>,
        publisher: Option<Arc<dyn MessagePublisher>>,
    ) -> Result<Self> {
        let routes: Vec<Route> = config
            .routes
            .iter()
            .map(|c| Route::from_config(c, &invoker))
            .collect::<Result<_>>()?;

        let tracer_provider = if let Some(endpoint) = &config.otlp_endpoint {
            match build_tracer_provider(endpoint, &config.otlp_protocol, &config.name) {
                Ok(tp) => Some(tp),
                Err(e) => {
                    tracing::warn!(server = %config.name, "failed to build OTLP span exporter: {e}");
                    None
                }
            }
        } else {
            None
        };

        let router = Arc::new(Router {
            routes,
            invoker,
            publisher,
            tracer_provider: tracer_provider.clone(),
        });

        Ok(Self {
            port: config.port,
            router,
            tracer_provider,
        })
    }

    pub async fn run(self, mut shutdown: watch::Receiver<bool>) -> Result<()> {
        let HttpServer {
            port,
            router,
            tracer_provider,
        } = self;

        let listener = TcpListener::bind(("0.0.0.0", port)).await?;
        tracing::info!(port, "HTTP server listening");

        let graceful = GracefulShutdown::new();

        loop {
            tokio::select! {
                accept = listener.accept() => {
                    let (stream, _addr) = accept?;
                    let router = Arc::clone(&router);
                    let conn = http1::Builder::new().serve_connection(
                        TokioIo::new(stream),
                        service_fn(move |req| {
                            let router = Arc::clone(&router);
                            async move { router.handle(req).await }
                        }),
                    );
                    let conn = graceful.watch(conn);
                    tokio::spawn(async move {
                        if let Err(e) = conn.await {
                            tracing::error!("connection error: {e}");
                        }
                    });
                }
                _ = shutdown.changed() => {
                    tracing::info!(port, "HTTP server shutting down");
                    break;
                }
            }
        }

        tokio::select! {
            _ = graceful.shutdown() => {}
            _ = tokio::time::sleep(std::time::Duration::from_secs(5)) => {
                tracing::warn!(port, "timed out waiting for connections to close");
            }
        }

        // Drop router (and its provider clone) before shutting down the provider.
        drop(router);

        // spawn_blocking since BatchSpanProcessor::shutdown() calls block_on internally.
        if let Some(provider) = tracer_provider {
            let _ = tokio::task::spawn_blocking(move || provider.shutdown()).await;
        }

        Ok(())
    }
}

fn build_tracer_provider(
    endpoint: &str,
    protocol: &str,
    service_name: &str,
) -> Result<SdkTracerProvider> {
    let exporter = match protocol {
        "http/protobuf" => SpanExporter::builder()
            .with_http()
            .with_endpoint(endpoint)
            .build()
            .map_err(|e| anyhow::anyhow!("failed to build span exporter: {e}"))?,
        _ => {
            if protocol != "grpc" {
                tracing::warn!(protocol, "unrecognized OTLP protocol, defaulting to grpc");
            }
            SpanExporter::builder()
                .with_tonic()
                .with_endpoint(endpoint)
                .build()
                .map_err(|e| anyhow::anyhow!("failed to build span exporter: {e}"))?
        }
    };
    let resource = opentelemetry_sdk::Resource::builder()
        .with_attribute(opentelemetry::KeyValue::new(
            "service.name",
            service_name.to_string(),
        ))
        .build();
    let processor = BatchSpanProcessor::builder(exporter, Tokio).build();
    Ok(SdkTracerProvider::builder()
        .with_resource(resource)
        .with_span_processor(processor)
        .build())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // Construct a Route with a stub channel target for tests that exercise
    // path/query matching (not dispatch). The route's `name` is set from
    // `path` for clarity in failure messages.
    fn route(method: Method, path: &str, query_params: Vec<QueryParamSpec>) -> Route {
        let path_segments: Vec<Segment> = path
            .split('/')
            .filter(|s| !s.is_empty())
            .map(|s| {
                if s.starts_with('{') && s.ends_with('}') {
                    let inner = &s[1..s.len() - 1];
                    if inner.is_empty() {
                        Segment::Anonymous
                    } else {
                        Segment::Param(inner.to_string())
                    }
                } else {
                    Segment::Literal(s.to_string())
                }
            })
            .collect();
        Route {
            name: path.to_string(),
            method,
            path_segments,
            query_params,
            content_type: Some(ContentType::Json),
            target: CompiledTarget::Channel {
                channel: "stub".to_string(),
                reply_timeout_ms: None,
            },
            request_schema: None,
            propagate_request_headers: Vec::new(),
            response_schema: None,
            propagate_response_headers: Vec::new(),
        }
    }

    fn qp_required(name: &str) -> QueryParamSpec {
        QueryParamSpec {
            name: name.to_string(),
            kind: QueryParamKind::Required {
                value: None,
                capture: true,
            },
        }
    }

    fn qp_required_value(name: &str, value: &str) -> QueryParamSpec {
        QueryParamSpec {
            name: name.to_string(),
            kind: QueryParamKind::Required {
                value: Some(value.to_string()),
                capture: true,
            },
        }
    }

    fn qp_optional(name: &str) -> QueryParamSpec {
        QueryParamSpec {
            name: name.to_string(),
            kind: QueryParamKind::Optional {
                value: None,
                capture: true,
            },
        }
    }

    fn qp_forbidden(name: &str) -> QueryParamSpec {
        QueryParamSpec {
            name: name.to_string(),
            kind: QueryParamKind::Forbidden,
        }
    }

    fn captures(path: &[(&str, &str)], query: &[(&str, &str)]) -> Captures {
        Captures {
            path: path
                .iter()
                .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
                .collect(),
            query: query
                .iter()
                .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
                .collect(),
        }
    }

    fn request_content(headers: &[(&str, &str)], body: &[u8]) -> RequestContent {
        let mut map = HeaderMap::new();
        for (k, v) in headers {
            map.insert(
                hyper::header::HeaderName::from_bytes(k.as_bytes()).unwrap(),
                hyper::header::HeaderValue::from_str(v).unwrap(),
            );
        }
        RequestContent {
            headers: map,
            body: Bytes::copy_from_slice(body),
        }
    }

    fn empty_query() -> HashMap<String, String> {
        HashMap::new()
    }

    // ---------- Path matching ----------

    #[test]
    fn path_literal_segment_matches() {
        let r = route(Method::GET, "/users", vec![]);
        assert!(
            r.match_request(&Method::GET, "/users", &empty_query())
                .is_some()
        );
        assert!(
            r.match_request(&Method::GET, "/other", &empty_query())
                .is_none()
        );
    }

    #[test]
    fn path_named_capture_extracts_value() {
        let r = route(Method::GET, "/users/{id}", vec![]);
        let caps = r
            .match_request(&Method::GET, "/users/42", &empty_query())
            .expect("route should match");
        assert_eq!(caps.path.get("id"), Some(&"42".to_string()));
    }

    #[test]
    fn path_anonymous_placeholder_matches_without_capture() {
        let r = route(Method::GET, "/users/{}", vec![]);
        let caps = r
            .match_request(&Method::GET, "/users/42", &empty_query())
            .expect("route should match");
        assert!(caps.path.is_empty());
    }

    #[test]
    fn path_length_mismatch_returns_none() {
        let r = route(Method::GET, "/users/{id}", vec![]);
        assert!(
            r.match_request(&Method::GET, "/users", &empty_query())
                .is_none()
        );
        assert!(
            r.match_request(&Method::GET, "/users/42/extra", &empty_query())
                .is_none()
        );
    }

    #[test]
    fn path_wrong_method_returns_none() {
        let r = route(Method::POST, "/users", vec![]);
        assert!(
            r.match_request(&Method::GET, "/users", &empty_query())
                .is_none()
        );
    }

    // ---------- Query matching ----------

    fn query(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect()
    }

    #[test]
    fn query_forbidden_present_does_not_match() {
        let r = route(Method::GET, "/x", vec![qp_forbidden("debug")]);
        assert!(
            r.match_request(&Method::GET, "/x", &query(&[("debug", "1")]))
                .is_none()
        );
    }

    #[test]
    fn query_forbidden_absent_matches() {
        let r = route(Method::GET, "/x", vec![qp_forbidden("debug")]);
        let caps = r.match_request(&Method::GET, "/x", &empty_query()).unwrap();
        assert!(caps.query.is_empty());
    }

    #[test]
    fn query_required_missing_does_not_match() {
        let r = route(Method::GET, "/x", vec![qp_required("type")]);
        assert!(
            r.match_request(&Method::GET, "/x", &empty_query())
                .is_none()
        );
    }

    #[test]
    fn query_required_value_mismatch_does_not_match() {
        let r = route(Method::GET, "/x", vec![qp_required_value("type", "admin")]);
        assert!(
            r.match_request(&Method::GET, "/x", &query(&[("type", "user")]))
                .is_none()
        );
    }

    #[test]
    fn query_required_value_match_captures() {
        let r = route(Method::GET, "/x", vec![qp_required_value("type", "admin")]);
        let caps = r
            .match_request(&Method::GET, "/x", &query(&[("type", "admin")]))
            .unwrap();
        assert_eq!(caps.query.get("type"), Some(&"admin".to_string()));
    }

    #[test]
    fn query_optional_absent_matches_no_capture() {
        let r = route(Method::GET, "/x", vec![qp_optional("limit")]);
        let caps = r.match_request(&Method::GET, "/x", &empty_query()).unwrap();
        assert!(!caps.query.contains_key("limit"));
    }

    #[test]
    fn query_optional_present_captures() {
        let r = route(Method::GET, "/x", vec![qp_optional("limit")]);
        let caps = r
            .match_request(&Method::GET, "/x", &query(&[("limit", "10")]))
            .unwrap();
        assert_eq!(caps.query.get("limit"), Some(&"10".to_string()));
    }

    #[test]
    fn query_disjoint_required_values_select_correct_route() {
        let r_admin = route(
            Method::GET,
            "/users",
            vec![qp_required_value("type", "admin")],
        );
        let r_user = route(
            Method::GET,
            "/users",
            vec![qp_required_value("type", "user")],
        );

        let req = query(&[("type", "admin")]);
        assert!(
            r_admin
                .match_request(&Method::GET, "/users", &req)
                .is_some()
        );
        assert!(r_user.match_request(&Method::GET, "/users", &req).is_none());

        let req = query(&[("type", "user")]);
        assert!(
            r_admin
                .match_request(&Method::GET, "/users", &req)
                .is_none()
        );
        assert!(r_user.match_request(&Method::GET, "/users", &req).is_some());
    }

    // ---------- build_message_from_request ----------

    #[test]
    fn body_empty_no_captures_produces_empty_message_body() {
        let req = request_content(&[("content-type", "application/json")], b"");
        let msg = build_message_from_request(
            &req,
            captures(&[], &[]),
            Some(ContentType::Json),
            None,
            &[],
        )
        .unwrap();
        assert_eq!(msg.body(), b"");
    }

    #[test]
    fn body_empty_with_captures_produces_object_of_captures() {
        let req = request_content(&[("content-type", "application/json")], b"");
        let msg = build_message_from_request(
            &req,
            captures(&[("id", "42")], &[]),
            Some(ContentType::Json),
            None,
            &[],
        )
        .unwrap();
        let parsed: Value = serde_json::from_slice(msg.body()).unwrap();
        assert_eq!(parsed, json!({ "id": "42" }));
    }

    #[test]
    fn body_json_object_merged_with_captures() {
        let req = request_content(
            &[("content-type", "application/json")],
            br#"{"name":"Alice"}"#,
        );
        let msg = build_message_from_request(
            &req,
            captures(&[("id", "42")], &[]),
            Some(ContentType::Json),
            None,
            &[],
        )
        .unwrap();
        let parsed: Value = serde_json::from_slice(msg.body()).unwrap();
        assert_eq!(parsed, json!({ "id": "42", "name": "Alice" }));
    }

    #[test]
    fn body_capture_collides_with_body_field_returns_400() {
        let req = request_content(&[("content-type", "application/json")], br#"{"id":"99"}"#);
        let err = build_message_from_request(
            &req,
            captures(&[("id", "42")], &[]),
            Some(ContentType::Json),
            None,
            &[],
        )
        .expect_err("collision should be rejected");
        assert_eq!(err.0, StatusCode::BAD_REQUEST);
        assert!(
            err.1.contains("collides with a body field"),
            "unexpected: {}",
            err.1
        );
    }

    #[test]
    fn body_non_object_json_with_captures_returns_400() {
        let req = request_content(&[("content-type", "application/json")], b"42");
        let err = build_message_from_request(
            &req,
            captures(&[("id", "1")], &[]),
            Some(ContentType::Json),
            None,
            &[],
        )
        .expect_err("non-object body + captures should be rejected");
        assert_eq!(err.0, StatusCode::BAD_REQUEST);
        assert!(err.1.contains("non-object body"), "unexpected: {}", err.1);
    }

    #[test]
    fn text_plain_with_captures_returns_400() {
        let req = request_content(&[("content-type", "text/plain")], b"hello");
        let err = build_message_from_request(
            &req,
            captures(&[("id", "1")], &[]),
            Some(ContentType::TextPlain),
            None,
            &[],
        )
        .expect_err("text/plain + captures should be rejected");
        assert_eq!(err.0, StatusCode::BAD_REQUEST);
        assert!(err.1.contains("text/plain"), "unexpected: {}", err.1);
    }

    #[test]
    fn text_plain_without_captures_passes_through_as_raw_body() {
        let req = request_content(&[("content-type", "text/plain")], b"hello, world");
        let msg = build_message_from_request(
            &req,
            captures(&[], &[]),
            Some(ContentType::TextPlain),
            None,
            &[],
        )
        .unwrap();
        assert_eq!(msg.body(), b"hello, world");
        assert_eq!(msg.headers().content_type(), Some("text/plain"));
    }

    #[test]
    fn request_schema_accepts_conformant_body() {
        let schema = RequestSchema::new(&json!({
            "type": "object",
            "properties": {
                "id": { "type": "string" },
                "name": { "type": "string" }
            },
            "required": ["id", "name"]
        }))
        .unwrap();
        let req = request_content(
            &[("content-type", "application/json")],
            br#"{"name":"Alice"}"#,
        );
        let msg = build_message_from_request(
            &req,
            captures(&[("id", "42")], &[]),
            Some(ContentType::Json),
            Some(&schema),
            &[],
        )
        .unwrap();
        let parsed: Value = serde_json::from_slice(msg.body()).unwrap();
        assert_eq!(parsed, json!({ "id": "42", "name": "Alice" }));
    }

    #[test]
    fn request_schema_rejects_nonconformant_body() {
        // Schema requires `age` to be a number. Body sends a string.
        let schema = RequestSchema::new(&json!({
            "type": "object",
            "properties": {
                "age": { "type": "number" }
            },
            "required": ["age"]
        }))
        .unwrap();
        let req = request_content(
            &[("content-type", "application/json")],
            br#"{"age":"forty-two"}"#,
        );
        let err = build_message_from_request(
            &req,
            captures(&[], &[]),
            Some(ContentType::Json),
            Some(&schema),
            &[],
        )
        .expect_err("non-conformant body should be rejected");
        assert_eq!(err.0, StatusCode::BAD_REQUEST);
        assert!(
            err.1.contains("does not conform to request-schema"),
            "unexpected: {}",
            err.1
        );
    }

    #[test]
    fn mismatched_request_content_type_returns_415() {
        // Route declares application/json but the request arrives with
        // text/plain. The content-type mismatch is rejected with 415.
        let req = request_content(&[("content-type", "text/plain")], b"raw text");
        let err = build_message_from_request(
            &req,
            captures(&[], &[]),
            Some(ContentType::Json),
            None,
            &[],
        )
        .expect_err("content-type mismatch should be rejected");
        assert_eq!(err.0, StatusCode::UNSUPPORTED_MEDIA_TYPE);
        assert!(
            err.1.contains("does not match declared"),
            "unexpected: {}",
            err.1
        );
    }

    // ---------- Header propagation ----------

    #[test]
    fn declared_propagate_header_copied_lowercased() {
        let req = request_content(
            &[
                ("content-type", "application/json"),
                ("Authorization", "Bearer abc"),
            ],
            b"{}",
        );
        let propagate = vec![PropagatedHeader::parse("authorization").unwrap()];
        let msg = build_message_from_request(
            &req,
            captures(&[], &[]),
            Some(ContentType::Json),
            None,
            &propagate,
        )
        .unwrap();
        assert_eq!(
            msg.headers().get::<&str>("authorization"),
            Some("Bearer abc")
        );
    }

    #[test]
    fn propagated_tracing_header_copied_automatically() {
        let req = request_content(
            &[
                ("content-type", "application/json"),
                ("traceparent", "00-abc-def-01"),
            ],
            b"{}",
        );
        let msg = build_message_from_request(
            &req,
            captures(&[], &[]),
            Some(ContentType::Json),
            None,
            &[],
        )
        .unwrap();
        assert_eq!(
            msg.headers().get::<&str>("traceparent"),
            Some("00-abc-def-01")
        );
    }

    #[test]
    fn undeclared_header_not_copied() {
        let req = request_content(
            &[("content-type", "application/json"), ("X-Custom", "value")],
            b"{}",
        );
        let msg = build_message_from_request(
            &req,
            captures(&[], &[]),
            Some(ContentType::Json),
            None,
            &[],
        )
        .unwrap();
        assert!(msg.headers().get::<&str>("x-custom").is_none());
    }

    // ---------- Response schema coercion ----------

    fn json_reply(body: Value) -> Message {
        let bytes = serde_json::to_vec(&body).unwrap();
        MessageBuilder::new(bytes)
            .header(MessageHeaders::CONTENT_TYPE, "application/json")
            .build()
    }

    async fn response_body_bytes(resp: Response<Full<Bytes>>) -> Bytes {
        use http_body_util::BodyExt;
        resp.into_body().collect().await.unwrap().to_bytes()
    }

    #[tokio::test]
    async fn response_schema_stringifies_root_when_string() {
        let schema = ResponseSchema::new(json!({ "type": "string" })).unwrap();
        let msg = json_reply(json!(42));
        let resp = build_http_response_from_message(msg, Some(&schema), &[]).unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let parsed: Value = serde_json::from_slice(&response_body_bytes(resp).await).unwrap();
        assert_eq!(parsed, json!("42"));
    }

    #[tokio::test]
    async fn response_schema_recurses_into_object_properties() {
        let schema = ResponseSchema::new(json!({
            "type": "object",
            "properties": {
                "name": { "type": "string" },
                "meta": { "type": "string" }
            }
        }))
        .unwrap();
        let msg = json_reply(json!({ "name": "Alice", "meta": { "x": 1 } }));
        let resp = build_http_response_from_message(msg, Some(&schema), &[]).unwrap();
        let parsed: Value = serde_json::from_slice(&response_body_bytes(resp).await).unwrap();
        assert_eq!(parsed["name"], json!("Alice"));
        assert_eq!(parsed["meta"], json!("{\"x\":1}"));
    }

    #[tokio::test]
    async fn response_schema_skipped_for_text_plain() {
        let schema = ResponseSchema::new(json!({ "type": "string" })).unwrap();
        let msg = MessageBuilder::new(b"raw text".to_vec())
            .header(MessageHeaders::CONTENT_TYPE, "text/plain")
            .build();
        let resp = build_http_response_from_message(msg, Some(&schema), &[]).unwrap();
        // text/plain body passes through unchanged (coercion is JSON-only).
        assert_eq!(&response_body_bytes(resp).await[..], b"raw text");
    }

    #[tokio::test]
    async fn response_schema_validator_rejects_nonconformant_body() {
        // Schema requires `name` (string) and `age` (number). Reply body has
        // `name` only. Coercion does not invent the missing field, so
        // validation rejects.
        let schema = ResponseSchema::new(json!({
            "type": "object",
            "properties": {
                "name": { "type": "string" },
                "age": { "type": "number" }
            },
            "required": ["name", "age"]
        }))
        .unwrap();
        let msg = json_reply(json!({ "name": "Alice" }));
        let err = build_http_response_from_message(msg, Some(&schema), &[]).unwrap_err();
        assert!(
            err.contains("does not conform to response-schema"),
            "unexpected: {err}"
        );
    }

    #[tokio::test]
    async fn propagate_response_headers_emits_named_message_headers_to_response() {
        let msg = MessageBuilder::new(b"{}".to_vec())
            .header(MessageHeaders::CONTENT_TYPE, "application/json")
            .header("internal-tag", "abc")
            .header("internal-skip", "should-not-appear")
            .build();
        let propagate = vec![PropagatedHeader::parse("internal-tag as X-Tag").unwrap()];
        let resp = build_http_response_from_message(msg, None, &propagate).unwrap();
        let headers = resp.headers();
        assert_eq!(
            headers.get("X-Tag").and_then(|v| v.to_str().ok()),
            Some("abc")
        );
        assert!(headers.get("internal-skip").is_none());
    }
}
