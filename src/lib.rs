//! Modulewise Composable Runtime
//!
//! A runtime for Wasm Components that supports
//! composition, config, and capability management.

pub use graph::{ComponentGraph, GraphBuilder};
pub use registry::HostExtension;
pub use runtime::{Component, Runtime, RuntimeBuilder};
pub use types::ComponentState;
pub use wit::{Function, FunctionParam};

// exposed for testing, hidden from docs
#[doc(hidden)]
pub mod graph;
#[doc(hidden)]
pub mod registry;
#[doc(hidden)]
pub mod types;

mod composer;
mod loader;
mod runtime;
mod wit;
