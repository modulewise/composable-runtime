//! Modulewise Composable Runtime
//!
//! An inversion of control runtime for Wasm Components.
//! Supports composition, config, and capability management.

pub use composition::graph::{ComponentGraph, GraphBuilder};
pub use composition::registry::{CapabilityStateHasData, HostCapability, HostCapabilityFactory};
pub use config::types::{ConfigHandler, DefinitionLoader, PropertyMap};
pub use runtime::{Component, Runtime, RuntimeBuilder, RuntimeService};
pub use types::{ComponentState, Function, FunctionParam};

// exposed for testing, hidden from docs
#[doc(hidden)]
pub mod composition;
#[doc(hidden)]
pub mod types;

pub(crate) mod config;
#[cfg(feature = "messaging")]
mod messaging;
mod runtime;
