use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use anyhow::Result;

use composable_runtime::{
    CategoryClaim, Condition, ConfigHandler, Operator, PropertyMap, Selector,
};

/// Parsed route within an HTTP server.
#[derive(Debug, Clone)]
pub struct RouteConfig {
    pub name: String,
    pub method: String,
    pub path: String,
    pub target: RouteTarget,
}

/// What a route dispatches to.
#[derive(Debug, Clone)]
pub enum RouteTarget {
    /// Invoke a component function. Path params and optional body param map to function args.
    Component {
        component: String,
        function: String,
        body: Option<String>,
    },
    /// Publish the request body to a channel.
    Channel {
        channel: String,
        /// If set, use request-reply, else fire-and-forget.
        reply_timeout_ms: Option<u64>,
    },
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
            [
                "type",
                "port",
                "route",
                "otlp-endpoint",
                "otlp-protocol",
                "reply-timeout-ms",
            ]
            .as_slice(),
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
            Some(serde_json::Value::Number(n)) => n
                .as_u64()
                .and_then(|p| u16::try_from(p).ok())
                .ok_or_else(|| {
                    anyhow::anyhow!("Server '{name}': 'port' must be a valid port number")
                })?,
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
            Some(serde_json::Value::String(s)) => Some(s),
            Some(got) => {
                return Err(anyhow::anyhow!(
                    "Server '{name}': 'otlp-endpoint' must be a string, got {got}"
                ));
            }
            None => None,
        };

        let otlp_protocol = match properties.remove("otlp-protocol") {
            Some(serde_json::Value::String(s)) => s,
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
    props: &serde_json::Map<String, serde_json::Value>,
    key: &str,
    ctx: &dyn Fn(&str, &str) -> anyhow::Error,
) -> Result<Option<String>> {
    match props.get(key) {
        Some(serde_json::Value::String(s)) => Ok(Some(s.clone())),
        Some(got) => Err(ctx(
            &format!("'{key}'"),
            &format!("must be a string, got {got}"),
        )),
        None => Ok(None),
    }
}

fn parse_routes(server_name: &str, properties: &mut PropertyMap) -> Result<Vec<RouteConfig>> {
    let route_table = match properties.remove("route") {
        Some(serde_json::Value::Object(map)) => map,
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
            serde_json::Value::Object(map) => map,
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
            Some(serde_json::Value::String(s)) => s.clone(),
            Some(got) => return Err(ctx("'path'", &format!("must be a string, got {got}"))),
            None => return Err(ctx("", "missing required 'path' field")),
        };

        let component = get_optional_string(&route_props, "component", &ctx)?;
        let function = get_optional_string(&route_props, "function", &ctx)?;
        let channel = get_optional_string(&route_props, "channel", &ctx)?;
        let body = get_optional_string(&route_props, "body", &ctx)?;
        let reply_timeout_ms = match route_props.get("reply-timeout-ms") {
            Some(serde_json::Value::Number(n)) => Some(
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

        let has_component = component.is_some() || function.is_some();
        let has_channel = channel.is_some();

        if has_component && has_channel {
            return Err(ctx(
                "",
                "cannot have both 'component'/'function' and 'channel'",
            ));
        }

        let target = if let Some(channel) = channel {
            if body.is_some() {
                return Err(ctx("", "'body' is not valid on channel routes"));
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
            RouteTarget::Component {
                component,
                function,
                body,
            }
        };

        // Infer method from target type if necessary
        let method = match route_props.get("method") {
            Some(serde_json::Value::String(s)) => s.to_uppercase(),
            Some(got) => return Err(ctx("'method'", &format!("must be a string, got {got}"))),
            None => match &target {
                RouteTarget::Channel { .. } => "POST".to_string(),
                RouteTarget::Component { body, .. } => {
                    if body.is_some() {
                        "POST".to_string()
                    } else {
                        "GET".to_string()
                    }
                }
            },
        };

        // Validate method + target combinations
        if method == "GET" {
            if let RouteTarget::Component { body: Some(_), .. } = &target {
                return Err(ctx("", "cannot have 'body' with method GET"));
            }
            if matches!(&target, RouteTarget::Channel { .. }) {
                return Err(ctx(
                    "",
                    "cannot use method GET with channel route (requires a body)",
                ));
            }
        }

        routes.push(RouteConfig {
            name: route_name,
            method,
            path,
            target,
        });
    }

    // Detect conflicting routes (same method + same path structure)
    for i in 0..routes.len().saturating_sub(1) {
        for j in (i + 1)..routes.len() {
            if routes[i].method == routes[j].method
                && path_structure(&routes[i].path) == path_structure(&routes[j].path)
            {
                return Err(anyhow::anyhow!(
                    "Server '{}': routes '{}' and '{}' conflict \
                     (same method {} and path structure)",
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

// Normalize a path for structural comparison: replace `{param}` segments with `{}`.
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

    fn make_handler() -> (HttpServerConfigHandler, SharedConfig) {
        let config = shared_config();
        let handler = HttpServerConfigHandler::new(Arc::clone(&config));
        (handler, config)
    }

    fn props(pairs: Vec<(&str, serde_json::Value)>) -> PropertyMap {
        pairs.into_iter().map(|(k, v)| (k.to_string(), v)).collect()
    }

    #[test]
    fn parse_basic_server() {
        let (mut handler, config) = make_handler();
        let properties = props(vec![
            ("type", serde_json::json!("http")),
            ("port", serde_json::json!(8080)),
            (
                "route",
                serde_json::json!({
                    "hello": {
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
        assert_eq!(servers[0].name, "api");
        assert_eq!(servers[0].port, 8080);
        assert_eq!(servers[0].routes.len(), 1);
        assert_eq!(servers[0].routes[0].name, "hello");
        assert_eq!(servers[0].routes[0].method, "GET");
        assert_eq!(servers[0].routes[0].path, "/hello/{name}");
        match &servers[0].routes[0].target {
            RouteTarget::Component {
                component,
                function,
                body,
            } => {
                assert_eq!(component, "greeter");
                assert_eq!(function, "greet");
                assert!(body.is_none());
            }
            other => panic!("expected Component target, got {other:?}"),
        }
    }

    #[test]
    fn body_implies_post() {
        let (mut handler, config) = make_handler();
        let properties = props(vec![
            ("type", serde_json::json!("http")),
            ("port", serde_json::json!(3000)),
            (
                "route",
                serde_json::json!({
                    "create": {
                        "path": "/users",
                        "component": "user-service",
                        "function": "create",
                        "body": "user"
                    }
                }),
            ),
        ]);

        handler
            .handle_category("server", "api", properties)
            .unwrap();

        let servers = config.lock().unwrap();
        assert_eq!(servers[0].routes[0].method, "POST");
        match &servers[0].routes[0].target {
            RouteTarget::Component { body, .. } => {
                assert_eq!(body.as_deref(), Some("user"));
            }
            other => panic!("expected Component target, got {other:?}"),
        }
    }

    #[test]
    fn explicit_method_overrides_default() {
        let (mut handler, config) = make_handler();
        let properties = props(vec![
            ("type", serde_json::json!("http")),
            ("port", serde_json::json!(3000)),
            (
                "route",
                serde_json::json!({
                    "update": {
                        "method": "PUT",
                        "path": "/users/{id}",
                        "component": "user-service",
                        "function": "update",
                        "body": "user"
                    }
                }),
            ),
        ]);

        handler
            .handle_category("server", "api", properties)
            .unwrap();

        let servers = config.lock().unwrap();
        assert_eq!(servers[0].routes[0].method, "PUT");
    }

    #[test]
    fn body_with_get_is_error() {
        let (mut handler, _) = make_handler();
        let properties = props(vec![
            ("type", serde_json::json!("http")),
            ("port", serde_json::json!(3000)),
            (
                "route",
                serde_json::json!({
                    "bad": {
                        "method": "GET",
                        "path": "/bad",
                        "component": "test",
                        "function": "test",
                        "body": "data"
                    }
                }),
            ),
        ]);

        let result = handler.handle_category("server", "api", properties);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("cannot have 'body' with method GET")
        );
    }

    #[test]
    fn missing_port_is_error() {
        let (mut handler, _) = make_handler();
        let properties = props(vec![("type", serde_json::json!("http"))]);

        let result = handler.handle_category("server", "api", properties);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("missing required 'port'")
        );
    }

    #[test]
    fn missing_route_path_is_error() {
        let (mut handler, _) = make_handler();
        let properties = props(vec![
            ("type", serde_json::json!("http")),
            ("port", serde_json::json!(8080)),
            (
                "route",
                serde_json::json!({
                    "bad": {
                        "component": "test",
                        "function": "test"
                    }
                }),
            ),
        ]);

        let result = handler.handle_category("server", "api", properties);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("missing required 'path'")
        );
    }

    #[test]
    fn no_routes_is_valid() {
        let (mut handler, config) = make_handler();
        let properties = props(vec![
            ("type", serde_json::json!("http")),
            ("port", serde_json::json!(8080)),
        ]);

        handler
            .handle_category("server", "api", properties)
            .unwrap();

        let servers = config.lock().unwrap();
        assert_eq!(servers[0].routes.len(), 0);
    }

    #[test]
    fn channel_route() {
        let (mut handler, config) = make_handler();
        let properties = props(vec![
            ("type", serde_json::json!("http")),
            ("port", serde_json::json!(8080)),
            (
                "route",
                serde_json::json!({
                    "events": {
                        "path": "/events",
                        "channel": "incoming-events"
                    }
                }),
            ),
        ]);

        handler
            .handle_category("server", "api", properties)
            .unwrap();

        let servers = config.lock().unwrap();
        assert_eq!(servers[0].routes[0].method, "POST");
        match &servers[0].routes[0].target {
            RouteTarget::Channel {
                channel,
                reply_timeout_ms,
            } => {
                assert_eq!(channel, "incoming-events");
                assert!(reply_timeout_ms.is_none());
            }
            other => panic!("expected Channel target, got {other:?}"),
        }
    }

    #[test]
    fn channel_and_component_is_error() {
        let (mut handler, _) = make_handler();
        let properties = props(vec![
            ("type", serde_json::json!("http")),
            ("port", serde_json::json!(8080)),
            (
                "route",
                serde_json::json!({
                    "bad": {
                        "path": "/bad",
                        "component": "mutually",
                        "function": "test",
                        "channel": "exclusive"
                    }
                }),
            ),
        ]);

        let result = handler.handle_category("server", "api", properties);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("cannot have both"));
    }

    #[test]
    fn channel_with_body_is_error() {
        let (mut handler, _) = make_handler();
        let properties = props(vec![
            ("type", serde_json::json!("http")),
            ("port", serde_json::json!(8080)),
            (
                "route",
                serde_json::json!({
                    "bad": {
                        "path": "/bad",
                        "channel": "payloads",
                        "body": "data"
                    }
                }),
            ),
        ]);

        let result = handler.handle_category("server", "api", properties);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("'body' is not valid on channel routes")
        );
    }

    #[test]
    fn channel_with_get_is_error() {
        let (mut handler, _) = make_handler();
        let properties = props(vec![
            ("type", serde_json::json!("http")),
            ("port", serde_json::json!(8080)),
            (
                "route",
                serde_json::json!({
                    "bad": {
                        "method": "GET",
                        "path": "/bad",
                        "channel": "posters"
                    }
                }),
            ),
        ]);

        let result = handler.handle_category("server", "api", properties);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("cannot use method GET with channel route")
        );
    }

    #[test]
    fn conflicting_routes_same_method_and_path() {
        let (mut handler, _) = make_handler();
        let properties = props(vec![
            ("type", serde_json::json!("http")),
            ("port", serde_json::json!(8080)),
            (
                "route",
                serde_json::json!({
                    "get-user": {
                        "path": "/users/{id}",
                        "component": "user-service",
                        "function": "get"
                    },
                    "get-profile": {
                        "path": "/users/{uid}",
                        "component": "user-service",
                        "function": "get-profile"
                    }
                }),
            ),
        ]);

        let result = handler.handle_category("server", "api", properties);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("conflict"));
    }

    #[test]
    fn same_path_different_method_is_ok() {
        let (mut handler, config) = make_handler();
        let properties = props(vec![
            ("type", serde_json::json!("http")),
            ("port", serde_json::json!(8080)),
            (
                "route",
                serde_json::json!({
                    "get-user": {
                        "method": "GET",
                        "path": "/users/{id}",
                        "component": "user-service",
                        "function": "get"
                    },
                    "update-user": {
                        "method": "PUT",
                        "path": "/users/{id}",
                        "component": "user-service",
                        "function": "update",
                        "body": "user"
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
    fn selector_matches_http_type() {
        let handler = HttpServerConfigHandler::new(shared_config());
        let claims = handler.claimed_categories();
        assert_eq!(claims.len(), 1);
        assert_eq!(claims[0].category, "server");
        assert!(claims[0].selector.is_some());

        let selector = claims[0].selector.as_ref().unwrap();
        let mut props = HashMap::new();
        props.insert("type".to_string(), Some("http".to_string()));
        assert!(selector.matches(&props));

        let mut props = HashMap::new();
        props.insert("type".to_string(), Some("grpc".to_string()));
        assert!(!selector.matches(&props));
    }
}
