//! Core type definitions shared across the crate.

use serde::{Deserialize, Serialize};
use std::any::{Any, TypeId};
use std::collections::HashMap;

/// Base definition with URI and enables scope
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct DefinitionBase {
    pub uri: String,
    #[serde(default = "default_enables")]
    pub enables: String, // "none"|"package"|"namespace"|"unexposed"|"exposed"|"any"
}

pub fn default_enables() -> String {
    "none".to_string()
}

/// Component definition base with additional fields
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct ComponentDefinitionBase {
    #[serde(flatten)]
    pub base: DefinitionBase,
    #[serde(default)]
    pub expects: Vec<String>, // Named components this expects to be available
    #[serde(default)]
    pub intercepts: Vec<String>, // Components this intercepts
    #[serde(default)]
    pub precedence: i32, // Lower values have higher precedence
    #[serde(default)]
    pub exposed: bool,
    pub config: Option<HashMap<String, serde_json::Value>>,
}

impl std::ops::Deref for ComponentDefinitionBase {
    type Target = DefinitionBase;
    fn deref(&self) -> &Self::Target {
        &self.base
    }
}

/// Runtime feature definition
#[derive(Deserialize, Serialize, Clone)]
pub struct RuntimeFeatureDefinition {
    pub name: String,
    #[serde(flatten)]
    pub base: DefinitionBase,
    /// Configuration from `config.[key]` entries in TOML
    #[serde(default)]
    pub config: HashMap<String, serde_json::Value>,
}

impl std::ops::Deref for RuntimeFeatureDefinition {
    type Target = DefinitionBase;
    fn deref(&self) -> &Self::Target {
        &self.base
    }
}

impl std::fmt::Debug for RuntimeFeatureDefinition {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RuntimeFeatureDefinition")
            .field("name", &self.name)
            .field("uri", &self.uri)
            .field("enables", &self.enables)
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
            .field("enables", &self.enables)
            .field("expects", &self.expects)
            .field("intercepts", &self.intercepts)
            .field("precedence", &self.precedence)
            .field("exposed", &self.exposed)
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
