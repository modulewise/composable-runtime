use std::collections::HashMap;
use std::sync::Arc;

use anyhow::Result;
use http_body_util::{BodyExt, Full};
use hyper::body::{Bytes, Incoming};
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{Method, Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use hyper_util::server::graceful::GracefulShutdown;
use opentelemetry::trace::{Span, SpanKind, Tracer, TracerProvider};
use opentelemetry_otlp::{SpanExporter, WithExportConfig};
use opentelemetry_sdk::{
    runtime::Tokio,
    trace::{SdkTracerProvider, span_processor_with_async_runtime::BatchSpanProcessor},
};
use tokio::net::TcpListener;
use tokio::sync::watch;

use composable_runtime::{ComponentInvoker, MessagePublisher, PROPAGATED_HEADERS};

use crate::config::{RouteConfig, RouteTarget, ServerConfig};

struct Route {
    name: String,
    method: Method,
    segments: Vec<Segment>,
    target: Target,
}

enum Target {
    Component {
        component: String,
        function: String,
        body: Option<String>,
    },
    Channel {
        channel: String,
    },
}

enum Segment {
    Literal(String),
    Param(String),
}

impl Route {
    fn from_config(config: &RouteConfig) -> Result<Self> {
        let method = config
            .method
            .parse::<Method>()
            .map_err(|e| anyhow::anyhow!("invalid method '{}': {e}", config.method))?;

        let segments: Vec<_> = config
            .path
            .split('/')
            .filter(|s| !s.is_empty())
            .map(|s| {
                if s.starts_with('{') && s.ends_with('}') {
                    Segment::Param(s[1..s.len() - 1].to_string())
                } else {
                    Segment::Literal(s.to_string())
                }
            })
            .collect();

        let target = match &config.target {
            RouteTarget::Component {
                component,
                function,
                body,
            } => {
                if let Some(body_name) = body {
                    let conflicts = segments
                        .iter()
                        .any(|s| matches!(s, Segment::Param(p) if p == body_name.as_str()));
                    if conflicts {
                        return Err(anyhow::anyhow!(
                            "route '{}': body param '{}' conflicts with a path param of the same name",
                            config.name,
                            body_name
                        ));
                    }
                }
                Target::Component {
                    component: component.clone(),
                    function: function.clone(),
                    body: body.clone(),
                }
            }
            RouteTarget::Channel { channel } => Target::Channel {
                channel: channel.clone(),
            },
        };

        Ok(Self {
            name: config.name.clone(),
            method,
            segments,
            target,
        })
    }

    fn matches(&self, method: &Method, path: &str) -> Option<HashMap<String, String>> {
        if *method != self.method {
            return None;
        }

        let parts: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
        if parts.len() != self.segments.len() {
            return None;
        }

        let mut params = HashMap::new();
        for (part, segment) in parts.iter().zip(&self.segments) {
            match segment {
                Segment::Literal(expected) if *part == expected.as_str() => {}
                Segment::Param(name) => {
                    params.insert(name.clone(), (*part).to_string());
                }
                _ => return None,
            }
        }

        Some(params)
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

        for route in &self.routes {
            if let Some(params) = route.matches(&method, &path) {
                tracing::debug!(route = %route.name, %method, %path, "matched route");
                return Ok(self.dispatch(route, params, req).await);
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
        params: HashMap<String, String>,
        req: Request<Incoming>,
    ) -> Response<Full<Bytes>> {
        match &route.target {
            Target::Component {
                component,
                function,
                body,
            } => {
                self.invoke_component(component, function, body.as_deref(), params, req)
                    .await
            }
            Target::Channel { channel } => self.publish_to_channel(channel, req).await,
        }
    }

    async fn invoke_component(
        &self,
        component_name: &str,
        function_name: &str,
        body_param: Option<&str>,
        params: HashMap<String, String>,
        req: Request<Incoming>,
    ) -> Response<Full<Bytes>> {
        // Extract propagated headers from the inbound request.
        let mut context: HashMap<String, String> = HashMap::new();
        for key in PROPAGATED_HEADERS {
            if let Some(val) = req.headers().get(*key).and_then(|v| v.to_str().ok()) {
                context.insert(key.to_string(), val.to_string());
            }
        }

        let mut host_span = self.tracer_provider.as_ref().map(|tp| {
            use opentelemetry::propagation::TextMapPropagator;
            use opentelemetry_sdk::propagation::TraceContextPropagator;
            let propagator = TraceContextPropagator::new();
            let parent_cx = propagator.extract(&context);
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
            context.insert(
                "traceparent".to_string(),
                format!(
                    "00-{:032x}-{:016x}-{:02x}",
                    sc.trace_id(),
                    sc.span_id(),
                    sc.trace_flags()
                ),
            );
        } else if !context.contains_key("traceparent") {
            let trace_id = uuid::Uuid::new_v4().as_simple().to_string();
            let span_id = &uuid::Uuid::new_v4().as_simple().to_string()[..16];
            context.insert(
                "traceparent".to_string(),
                format!("00-{trace_id}-{span_id}-01"),
            );
        };

        let result = self
            .invoke_inner(
                component_name,
                function_name,
                body_param,
                params,
                req,
                context,
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
            Err((status, msg)) => Response::builder()
                .status(status)
                .body(Full::new(Bytes::from(msg)))
                .unwrap(),
        }
    }

    async fn invoke_inner(
        &self,
        component_name: &str,
        function_name: &str,
        body_param: Option<&str>,
        params: HashMap<String, String>,
        req: Request<Incoming>,
        context: HashMap<String, String>,
    ) -> Result<Response<Full<Bytes>>, (StatusCode, String)> {
        let component = self.invoker.get_component(component_name).ok_or_else(|| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("component '{component_name}' not found"),
            )
        })?;

        let function = component.functions.get(function_name).ok_or_else(|| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("function '{function_name}' not found on '{component_name}'"),
            )
        })?;

        let body_value = if body_param.is_some() {
            let content_type = req
                .headers()
                .get("content-type")
                .and_then(|v| v.to_str().ok())
                .unwrap_or("application/json")
                .to_string();

            let body_bytes = req
                .collect()
                .await
                .map_err(|e| (StatusCode::BAD_REQUEST, format!("failed to read body: {e}")))?
                .to_bytes();

            let value = if content_type.starts_with("text/plain") {
                std::str::from_utf8(&body_bytes)
                    .map(|text| serde_json::Value::String(text.to_string()))
                    .map_err(|e| {
                        (
                            StatusCode::BAD_REQUEST,
                            format!("body is not valid UTF-8: {e}"),
                        )
                    })?
            } else if content_type.starts_with("application/json") {
                serde_json::from_slice::<serde_json::Value>(&body_bytes)
                    .map_err(|e| (StatusCode::BAD_REQUEST, format!("invalid JSON body: {e}")))?
            } else {
                return Err((
                    StatusCode::UNSUPPORTED_MEDIA_TYPE,
                    format!("unsupported content type: {content_type}"),
                ));
            };
            Some(value)
        } else {
            None
        };

        let mut args: Vec<serde_json::Value> = Vec::new();
        for param in function.params() {
            if let Some(value) = params.get(param.name.as_str()) {
                args.push(serde_json::Value::String(value.clone()));
            } else if body_param == Some(param.name.as_str()) {
                args.push(body_value.clone().unwrap());
            } else {
                return Err((
                    StatusCode::BAD_REQUEST,
                    format!("missing parameter '{}'", param.name.as_str()),
                ));
            }
        }

        let invoke_result = self
            .invoker
            .invoke(component_name, function_name, args, Some(context), None)
            .await
            .map_err(|e| {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!("invocation error: {e}"),
                )
            })?;

        let body = serde_json::to_vec(&invoke_result).unwrap_or_default();
        Ok(Response::builder()
            .status(StatusCode::OK)
            .header("content-type", "application/json")
            .body(Full::new(Bytes::from(body)))
            .unwrap())
    }

    async fn publish_to_channel(
        &self,
        channel: &str,
        req: Request<Incoming>,
    ) -> Response<Full<Bytes>> {
        let publisher = match &self.publisher {
            Some(p) => p,
            None => {
                return Response::builder()
                    .status(StatusCode::INTERNAL_SERVER_ERROR)
                    .body(Full::new(Bytes::from(
                        "channel routes require messaging to be enabled",
                    )))
                    .unwrap();
            }
        };

        let content_type = req
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("application/json")
            .to_string();

        let body_bytes = match req.collect().await {
            Ok(collected) => collected.to_bytes().to_vec(),
            Err(e) => {
                return Response::builder()
                    .status(StatusCode::BAD_REQUEST)
                    .body(Full::new(Bytes::from(format!("failed to read body: {e}"))))
                    .unwrap();
            }
        };

        let mut headers = HashMap::new();
        headers.insert("content-type".to_string(), content_type);

        match publisher.publish(channel, body_bytes, headers).await {
            Ok(()) => Response::builder()
                .status(StatusCode::ACCEPTED)
                .body(Full::new(Bytes::new()))
                .unwrap(),
            Err(e) => Response::builder()
                .status(StatusCode::INTERNAL_SERVER_ERROR)
                .body(Full::new(Bytes::from(format!(
                    "failed to publish to channel '{channel}': {e}"
                ))))
                .unwrap(),
        }
    }
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
            .map(Route::from_config)
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
