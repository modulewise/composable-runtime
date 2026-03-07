//! Core type definitions shared across the crate.

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::fmt;

/// Base definition with URI and scope
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct DefinitionBase {
    pub uri: String,
    #[serde(default = "default_scope")]
    pub scope: String, // "any"|"package"|"namespace"
}

pub fn default_scope() -> String {
    "any".to_string()
}

/// Component definition base with additional fields
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct ComponentDefinitionBase {
    #[serde(flatten)]
    pub base: DefinitionBase,
    #[serde(default)]
    pub imports: Vec<String>, // Named components this imports
    #[serde(default)]
    pub intercepts: Vec<String>, // Components this intercepts
    #[serde(default)]
    pub precedence: i32, // Lower values have higher precedence
    #[serde(default)]
    pub config: HashMap<String, serde_json::Value>,
}

impl std::ops::Deref for ComponentDefinitionBase {
    type Target = DefinitionBase;
    fn deref(&self) -> &Self::Target {
        &self.base
    }
}

/// Capability definition (host capabilities and wasmtime capabilities)
#[derive(Deserialize, Serialize, Clone)]
pub struct CapabilityDefinition {
    pub name: String,
    #[serde(flatten)]
    pub base: DefinitionBase,
    /// Configuration from `config.[key]` entries in TOML
    #[serde(default)]
    pub config: HashMap<String, serde_json::Value>,
}

impl std::ops::Deref for CapabilityDefinition {
    type Target = DefinitionBase;
    fn deref(&self) -> &Self::Target {
        &self.base
    }
}

impl std::fmt::Debug for CapabilityDefinition {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CapabilityDefinition")
            .field("name", &self.name)
            .field("uri", &self.uri)
            .field("scope", &self.scope)
            .field("config", &self.config)
            .finish()
    }
}

/// Component definition
#[derive(Deserialize, Serialize, Clone)]
pub struct ComponentDefinition {
    pub name: String,
    #[serde(flatten)]
    pub base: ComponentDefinitionBase,
}

impl std::ops::Deref for ComponentDefinition {
    type Target = ComponentDefinitionBase;
    fn deref(&self) -> &Self::Target {
        &self.base
    }
}

impl std::fmt::Debug for ComponentDefinition {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ComponentDefinition")
            .field("name", &self.name)
            .field("uri", &self.uri)
            .field("scope", &self.scope)
            .field("imports", &self.imports)
            .field("intercepts", &self.intercepts)
            .field("precedence", &self.precedence)
            .field("config", &self.config)
            .finish()
    }
}

impl AsRef<DefinitionBase> for ComponentDefinition {
    fn as_ref(&self) -> &DefinitionBase {
        &self.base.base
    }
}

/// State passed to Wasm components during execution.
pub struct ComponentState {
    pub wasi_ctx: wasmtime_wasi::WasiCtx,
    pub wasi_http_ctx: Option<wasmtime_wasi_http::WasiHttpCtx>,
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

/// A validated WebAssembly Interface Type (WIT) interface name
/// Format: namespace:package/interface[@version]
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Interface {
    namespace: String,
    package: String,
    interface: String,
    version: Option<String>,
    full_name: String,
}

impl Interface {
    /// Parse and validate a WIT interface string
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

    /// Get the full interface string
    pub fn as_str(&self) -> &str {
        &self.full_name
    }

    /// Get the namespace (e.g., "wasi" from "wasi:http/outgoing-handler@0.2.3")
    pub fn namespace(&self) -> &str {
        &self.namespace
    }

    /// Get the package (e.g., "http" from "wasi:http/outgoing-handler@0.2.3")
    pub fn package(&self) -> &str {
        &self.package
    }

    /// Get the interface name (e.g., "outgoing-handler" from "wasi:http/outgoing-handler@0.2.3")
    pub fn interface_name(&self) -> &str {
        &self.interface
    }

    /// Get the version (e.g., Some("0.2.3") from "wasi:http/outgoing-handler@0.2.3")
    pub fn version(&self) -> Option<&str> {
        self.version.as_deref()
    }
}

impl fmt::Display for Interface {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.full_name)
    }
}

/// A function specification
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Function {
    interface: Option<Interface>,
    function_name: String,
    docs: String,
    params: Vec<FunctionParam>,
    result: Option<serde_json::Value>,
}

impl Function {
    /// Create a new function specification
    pub fn new(
        interface: Option<Interface>,
        function_name: String,
        docs: String,
        params: Vec<FunctionParam>,
        result: Option<serde_json::Value>,
    ) -> Self {
        Self {
            interface,
            function_name,
            docs,
            params,
            result,
        }
    }

    /// Get the interface (None for direct function exports)
    pub fn interface(&self) -> Option<&Interface> {
        self.interface.as_ref()
    }

    /// Get the function name
    pub fn function_name(&self) -> &str {
        &self.function_name
    }

    /// Get the function documentation
    pub fn docs(&self) -> &str {
        &self.docs
    }

    /// Get the function parameters
    pub fn params(&self) -> &[FunctionParam] {
        &self.params
    }

    /// Get the function result type
    pub fn result(&self) -> Option<&serde_json::Value> {
        self.result.as_ref()
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

/// A function parameter specification
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct FunctionParam {
    pub name: String,
    pub is_optional: bool,
    pub json_schema: serde_json::Value,
}
