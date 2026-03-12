use std::collections::HashMap;
use std::sync::Arc;

use anyhow::Result;
use http_body_util::{BodyExt, Full};
use hyper::body::{Bytes, Incoming};
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{Method, Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use tokio::net::TcpListener;
use tokio::sync::watch;

use composable_runtime::{ComponentInvoker, MessagePublisher};

use crate::config::{RouteConfig, RouteTarget};

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
        let component = match self.invoker.get_component(component_name) {
            Some(c) => c,
            None => {
                return Response::builder()
                    .status(StatusCode::INTERNAL_SERVER_ERROR)
                    .body(Full::new(Bytes::from(format!(
                        "component '{component_name}' not found"
                    ))))
                    .unwrap();
            }
        };

        let function = match component.functions.get(function_name) {
            Some(f) => f,
            None => {
                return Response::builder()
                    .status(StatusCode::INTERNAL_SERVER_ERROR)
                    .body(Full::new(Bytes::from(format!(
                        "function '{function_name}' not found on '{component_name}'"
                    ))))
                    .unwrap();
            }
        };

        let body_value = if body_param.is_some() {
            let content_type = req
                .headers()
                .get("content-type")
                .and_then(|v| v.to_str().ok())
                .unwrap_or("application/json")
                .to_string();

            let body_bytes = match req.collect().await {
                Ok(collected) => collected.to_bytes(),
                Err(e) => {
                    return Response::builder()
                        .status(StatusCode::BAD_REQUEST)
                        .body(Full::new(Bytes::from(format!("failed to read body: {e}"))))
                        .unwrap();
                }
            };

            let value = if content_type.starts_with("text/plain") {
                match std::str::from_utf8(&body_bytes) {
                    Ok(text) => serde_json::Value::String(text.to_string()),
                    Err(e) => {
                        return Response::builder()
                            .status(StatusCode::BAD_REQUEST)
                            .body(Full::new(Bytes::from(format!(
                                "body is not valid UTF-8: {e}"
                            ))))
                            .unwrap();
                    }
                }
            } else if content_type.starts_with("application/json") {
                match serde_json::from_slice::<serde_json::Value>(&body_bytes) {
                    Ok(value) => value,
                    Err(e) => {
                        return Response::builder()
                            .status(StatusCode::BAD_REQUEST)
                            .body(Full::new(Bytes::from(format!("invalid JSON body: {e}"))))
                            .unwrap();
                    }
                }
            } else {
                return Response::builder()
                    .status(StatusCode::UNSUPPORTED_MEDIA_TYPE)
                    .body(Full::new(Bytes::from(format!(
                        "unsupported content type: {content_type}"
                    ))))
                    .unwrap();
            };
            Some(value)
        } else {
            None
        };

        // Build args: path params by name, body param if configured
        let mut args: Vec<serde_json::Value> = Vec::new();
        for param in function.params() {
            if let Some(value) = params.get(param.name.as_str()) {
                args.push(serde_json::Value::String(value.clone()));
            } else if body_param == Some(param.name.as_str()) {
                args.push(body_value.clone().unwrap());
            } else {
                return Response::builder()
                    .status(StatusCode::BAD_REQUEST)
                    .body(Full::new(Bytes::from(format!(
                        "missing parameter '{}'",
                        param.name.as_str()
                    ))))
                    .unwrap();
            }
        }

        match self
            .invoker
            .invoke(component_name, function_name, args)
            .await
        {
            Ok(result) => {
                let body = serde_json::to_vec(&result).unwrap_or_default();
                Response::builder()
                    .status(StatusCode::OK)
                    .header("content-type", "application/json")
                    .body(Full::new(Bytes::from(body)))
                    .unwrap()
            }
            Err(e) => Response::builder()
                .status(StatusCode::INTERNAL_SERVER_ERROR)
                .body(Full::new(Bytes::from(format!("invocation error: {e}"))))
                .unwrap(),
        }
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

/// Start an HTTP server on the given port with the given routes.
/// Returns when the shutdown signal is received.
pub async fn run(
    port: u16,
    routes: Vec<RouteConfig>,
    invoker: Arc<dyn ComponentInvoker>,
    publisher: Option<Arc<dyn MessagePublisher>>,
    mut shutdown: watch::Receiver<bool>,
) -> Result<()> {
    let routes: Vec<Route> = routes
        .iter()
        .map(Route::from_config)
        .collect::<Result<_>>()?;

    let router = Arc::new(Router {
        routes,
        invoker,
        publisher,
    });

    let listener = TcpListener::bind(("0.0.0.0", port)).await?;
    tracing::info!(port, "HTTP gateway listening");

    loop {
        tokio::select! {
            accept = listener.accept() => {
                let (stream, _addr) = accept?;
                let router = Arc::clone(&router);
                tokio::spawn(async move {
                    let io = TokioIo::new(stream);
                    let service = service_fn(move |req| {
                        let router = Arc::clone(&router);
                        async move { router.handle(req).await }
                    });
                    if let Err(e) = http1::Builder::new().serve_connection(io, service).await {
                        tracing::error!("connection error: {e}");
                    }
                });
            }
            _ = shutdown.changed() => {
                tracing::info!(port, "HTTP gateway shutting down");
                break;
            }
        }
    }

    Ok(())
}
