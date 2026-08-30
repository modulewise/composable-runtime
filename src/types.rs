//! Core type definitions shared across the crate.

use anyhow::Result;
use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::fmt;
use std::future::Future;
use std::pin::Pin;

/// Base set of header keys that should be propagated across service boundaries.
pub const PROPAGATED_HEADERS: &[&str] = &["traceparent", "tracestate", "baggage"];

pub fn default_scope() -> String {
    "any".to_string()
}

/// Capability definition (built-in and custom capabilities).
#[derive(Debug, Clone)]
pub struct CapabilityDefinition {
    pub name: String,
    pub kind: String,
    pub scope: String,
    pub properties: HashMap<String, serde_json::Value>,
}

/// Component definition.
#[derive(Debug, Clone)]
pub struct ComponentDefinition {
    pub name: String,
    pub uri: String,
    pub scope: String,
    pub imports: Vec<String>,
    pub interceptors: Vec<String>,
    pub config: HashMap<String, serde_json::Value>,
    pub labels: HashMap<String, String>,
}

/// The name of a component referenced by a `factory:` uri.
pub fn factory_uri_target(uri: &str) -> Option<&str> {
    let name = uri.strip_prefix("factory:")?;
    Some(name.strip_prefix("//").unwrap_or(name))
}

/// Per-store `wasi:http` hooks, configured from the `wasi:http` capability
/// properties. The `WasiHttpHooks` trait impl lives in `runtime::host`.
#[derive(Default)]
pub(crate) struct HttpHooks {
    /// When set, outgoing `application/grpc` requests are sent over cleartext
    /// HTTP/2 (h2c, prior knowledge) instead of HTTP/1.1. Enabled by the
    /// `h2c-for-grpc` capability property.
    pub(crate) h2c_for_grpc: bool,
}

impl HttpHooks {
    pub(crate) fn from_properties(props: &HashMap<String, serde_json::Value>) -> Self {
        Self {
            h2c_for_grpc: props
                .get("h2c-for-grpc")
                .and_then(|v| v.as_bool())
                .unwrap_or(false),
        }
    }
}

/// State passed to Wasm components during execution.
pub struct ComponentState {
    pub wasi_ctx: wasmtime_wasi::WasiCtx,
    pub wasi_http_ctx: Option<wasmtime_wasi_http::WasiHttpCtx>,
    pub(crate) http_hooks: HttpHooks,
    pub resource_table: wasmtime_wasi::ResourceTable,
    pub(crate) extensions: HashMap<TypeId, Box<dyn Any + Send>>,
}

impl ComponentState {
    /// Get a reference to an extension by type.
    pub fn get_extension<T: 'static + Send>(&self) -> Option<&T> {
        self.extensions
            .get(&TypeId::of::<T>())
            .and_then(|boxed| boxed.downcast_ref())
    }

    /// Get a mutable reference to an extension by type.
    pub fn get_extension_mut<T: 'static + Send>(&mut self) -> Option<&mut T> {
        self.extensions
            .get_mut(&TypeId::of::<T>())
            .and_then(|boxed| boxed.downcast_mut())
    }

    /// Set an extension value by type.
    pub fn set_extension<T: 'static + Send>(&mut self, value: T) {
        self.extensions.insert(TypeId::of::<T>(), Box::new(value));
    }
}

/// A validated WebAssembly Interface Type (WIT) interface name.
/// Format: `namespace:package/interface[@version]`
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Interface {
    namespace: String,
    package: String,
    interface: String,
    version: Option<String>,
    full_name: String,
}

impl Interface {
    /// Parse and validate a WIT interface string.
    pub fn parse(s: &str) -> Result<Self> {
        if let Some((namespace, rest)) = s.split_once(':')
            && let Some((package, after_slash)) = rest.split_once('/')
        {
            let (interface, version) = if let Some((i, v)) = after_slash.split_once('@') {
                (i, Some(v.to_string()))
            } else {
                (after_slash, None)
            };

            return Ok(Self {
                namespace: namespace.to_string(),
                package: package.to_string(),
                interface: interface.to_string(),
                version,
                full_name: s.to_string(),
            });
        }

        Err(anyhow::anyhow!(
            "Invalid WIT interface format: expected namespace:package/interface[@version], got: {s}"
        ))
    }

    /// Get the full interface string.
    pub fn as_str(&self) -> &str {
        &self.full_name
    }

    /// Get the namespace (e.g., "wasi" from "wasi:http/outgoing-handler@0.2.12").
    pub fn namespace(&self) -> &str {
        &self.namespace
    }

    /// Get the package (e.g., "http" from "wasi:http/outgoing-handler@0.2.12").
    pub fn package(&self) -> &str {
        &self.package
    }

    /// Get the interface name (e.g., "outgoing-handler" from "wasi:http/outgoing-handler@0.2.12").
    pub fn interface_name(&self) -> &str {
        &self.interface
    }

    /// Get the version (e.g., Some("0.2.12") from "wasi:http/outgoing-handler@0.2.12").
    pub fn version(&self) -> Option<&str> {
        self.version.as_deref()
    }
}

impl fmt::Display for Interface {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.full_name)
    }
}

/// A function specification.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Function {
    interface: Option<Interface>,
    function_name: String,
    docs: String,
    params: Vec<FunctionParam>,
    result: Option<serde_json::Value>,
    is_invokable: bool,
}

impl Function {
    /// Create a new function specification.
    pub fn new(
        interface: Option<Interface>,
        function_name: String,
        docs: String,
        params: Vec<FunctionParam>,
        result: Option<serde_json::Value>,
        is_invokable: bool,
    ) -> Self {
        Self {
            interface,
            function_name,
            docs,
            params,
            result,
            is_invokable,
        }
    }

    /// Whether this function can be invoked directly host-side (JSON-RPC).
    pub fn is_invokable(&self) -> bool {
        self.is_invokable
    }

    /// Get the interface (None for direct function exports)
    pub fn interface(&self) -> Option<&Interface> {
        self.interface.as_ref()
    }

    /// Get the function name.
    pub fn function_name(&self) -> &str {
        &self.function_name
    }

    /// Get the function documentation.
    pub fn docs(&self) -> &str {
        &self.docs
    }

    /// Get the function parameters.
    pub fn params(&self) -> &[FunctionParam] {
        &self.params
    }

    /// Get the function result type.
    pub fn result(&self) -> Option<&serde_json::Value> {
        self.result.as_ref()
    }

    /// Whether this function's result is a `list<u8>`.
    ///
    /// The generated schema for such a result is an array of numbers with
    /// `minimum: 0` and `maximum: 255`.
    pub fn returns_bytes(&self) -> bool {
        let Some(schema) = self.result.as_ref() else {
            return false;
        };
        // `result<T, E>` is encoded as a two-arm `oneOf`; take the `ok` arm.
        let schema = schema
            .get("oneOf")
            .and_then(|arms| arms.as_array())
            .and_then(|arms| arms.iter().find_map(|arm| arm.pointer("/properties/ok")))
            .unwrap_or(schema);

        if schema.get("type").and_then(|t| t.as_str()) != Some("array") {
            return false;
        }
        let Some(items) = schema.get("items") else {
            return false;
        };
        items.get("type").and_then(|t| t.as_str()) == Some("number")
            && items.get("minimum").and_then(|m| m.as_i64()) == Some(0)
            && items.get("maximum").and_then(|m| m.as_i64()) == Some(255)
    }

    /// Get the function key used in maps and invoke calls.
    /// - Direct function exports: `function_name`
    /// - Interface function exports: `unqualified_interface.function_name`
    pub fn key(&self) -> String {
        match &self.interface {
            Some(iface) => format!("{}.{}", iface.interface_name(), self.function_name),
            None => self.function_name.clone(),
        }
    }
}

impl fmt::Display for Function {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.interface {
            Some(iface) => write!(f, "{}#{}", iface, self.function_name),
            None => write!(f, "{}", self.function_name),
        }
    }
}

/// A function parameter specification.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct FunctionParam {
    pub name: String,
    pub is_optional: bool,
    pub json_schema: serde_json::Value,
}

/// Metadata about a component, available for selector evaluation.
/// `dependents` is `None` before registry building (e.g. at scope evaluation time).
#[derive(Debug, Clone)]
pub struct ComponentMetadata {
    pub name: String,
    pub namespace: Option<String>,
    pub package: Option<String>,
    pub labels: HashMap<String, String>,
    pub dependents: Option<Vec<String>>,
    pub exports: Vec<String>,
}

impl ComponentMetadata {
    /// Flatten metadata into a selectable map for selector evaluation.
    /// Top-level fields become direct keys. Labels are prefixed with `labels.`.
    /// `None` fields are omitted (enabling existence checks like `!dependents`).
    pub fn to_selectable(&self) -> HashMap<String, Option<String>> {
        let mut map = HashMap::new();
        map.insert("name".to_string(), Some(self.name.clone()));
        if let Some(ns) = &self.namespace {
            map.insert("namespace".to_string(), Some(ns.clone()));
        }
        if let Some(pkg) = &self.package {
            map.insert("package".to_string(), Some(pkg.clone()));
        }
        if let Some(dependents) = &self.dependents
            && !dependents.is_empty()
        {
            map.insert(
                "dependents".to_string(),
                Some(format!("[{}]", dependents.join(","))),
            );
        }
        if !self.exports.is_empty() {
            map.insert(
                "exports".to_string(),
                Some(format!("[{}]", self.exports.join(","))),
            );
        }
        for (k, v) in &self.labels {
            map.insert(format!("labels.{k}"), Some(v.clone()));
        }
        map
    }
}

/// A named Wasm Component and its exported functions.
#[derive(Debug, Clone)]
pub struct Component {
    pub metadata: ComponentMetadata,
    pub functions: HashMap<String, Function>,
}

/// A value crossing the call boundary.
pub enum Val {
    /// Data passed by value.
    Json(serde_json::Value),
    /// A `list<u8>`, kept as bytes rather than a JSON array of numbers.
    Bytes(Vec<u8>),
    /// Reference to a resource owned by a component instance.
    Resource(ComponentResource),
}

impl Val {
    /// The JSON value, if this is data.
    pub fn as_json(&self) -> Option<&serde_json::Value> {
        match self {
            Self::Json(value) => Some(value),
            _ => None,
        }
    }

    /// The bytes, if this is a byte list.
    pub fn as_bytes(&self) -> Option<&[u8]> {
        match self {
            Self::Bytes(bytes) => Some(bytes),
            _ => None,
        }
    }

    /// The reference handle, if this is a resource.
    pub fn as_resource(&self) -> Option<&ComponentResource> {
        match self {
            Self::Resource(resource) => Some(resource),
            _ => None,
        }
    }

    /// The JSON value, for callers whose payload model is inherently
    /// JSON-based. Bytes convert to an array of numbers. A Resource has no
    /// JSON representation, and therefore errors.
    pub fn into_json(self) -> Result<serde_json::Value> {
        match self {
            Self::Json(value) => Ok(value),
            Self::Bytes(bytes) => Ok(serde_json::Value::Array(
                bytes.into_iter().map(serde_json::Value::from).collect(),
            )),
            Self::Resource(_) => Err(anyhow::anyhow!(
                "a resource has no JSON value representation"
            )),
        }
    }
}

impl From<serde_json::Value> for Val {
    fn from(value: serde_json::Value) -> Self {
        Self::Json(value)
    }
}

impl From<Vec<u8>> for Val {
    fn from(bytes: Vec<u8>) -> Self {
        Self::Bytes(bytes)
    }
}

impl From<ComponentResource> for Val {
    fn from(resource: ComponentResource) -> Self {
        Self::Resource(resource)
    }
}

/// A reference handle to a resource owned by a component instance.
///
/// Can only be used with its owning instance, as the "receiver" (first arg) of
/// a call or as an arg passed to another call. It is no longer valid once the
/// owning instance drops.
#[derive(Clone, Copy)]
pub struct ComponentResource {
    pub(crate) resource: wasmtime::component::ResourceAny,
}

/// Invoke components by name.
pub trait ComponentInvoker: Send + Sync {
    /// Invoke a component function.
    ///
    /// Propagation context (e.g. W3C tracecontext) is read from the ambient
    /// task-local [`PROPAGATION_CONTEXT`](crate::PROPAGATION_CONTEXT). Callers
    /// that need to establish or extend propagation must wrap the call with
    /// `PROPAGATION_CONTEXT.scope(...).await` at their boundary.
    fn invoke<'a>(
        &'a self,
        component_name: &'a str,
        function_name: &'a str,
        args: Vec<Val>,
        env: Option<HashMap<String, String>>,
    ) -> Pin<Box<dyn Future<Output = Result<Option<Val>>> + Send + 'a>>;
}

/// Invocation plus lookup and discovery of the components available to invoke.
pub trait ComponentHost: ComponentInvoker {
    fn get_component(&self, name: &str) -> Option<&Component>;

    fn list_components(&self, selector: Option<&crate::selector::Selector>) -> Vec<&Component>;
}
