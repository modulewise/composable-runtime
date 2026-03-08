//! Modulewise Composable Runtime
//!
//! A runtime for Wasm Components that supports
//! composition, config, and capability management.

pub use composition::graph::{ComponentGraph, GraphBuilder};
pub use composition::registry::{CapabilityStateHasData, HostCapability};
pub use runtime::{Component, Runtime, RuntimeBuilder};
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
