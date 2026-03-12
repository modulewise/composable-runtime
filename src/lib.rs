//! Modulewise Composable Runtime
//!
//! An inversion of control runtime for Wasm Components.
//! Supports composition, config, and capability management.

pub use composition::graph::{ComponentGraph, GraphBuilder};
pub use composition::registry::{CapabilityStateHasData, HostCapability, HostCapabilityFactory};
pub use config::types::{
    CategoryClaim, Condition, ConfigHandler, DefinitionLoader, Operator, PropertyMap, Selector,
};
pub use runtime::{Runtime, RuntimeBuilder};
pub use service::Service;
pub use types::{
    Component, ComponentInvoker, ComponentState, Function, FunctionParam, MessagePublisher,
};

// exposed for testing, hidden from docs
#[doc(hidden)]
pub mod composition;
#[doc(hidden)]
pub mod types;

pub(crate) mod config;
#[cfg(feature = "messaging")]
mod messaging;
mod runtime;
pub(crate) mod service;

#[cfg(feature = "messaging")]
pub use messaging::{Channel, Message, MessageBuilder, header};
