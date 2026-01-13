//! Modulewise Composable Runtime
//!
//! A runtime for Wasm Components that supports
//! composition, config, and capability management.

pub use graph::ComponentGraph;
pub use loader::load_definitions;
pub use registry::HostExtension;
pub use runtime::{Component, ComponentState, Runtime};
pub use wit::{Function, FunctionParam};

// exposed for testing, hidden from docs
#[doc(hidden)]
pub mod graph;
#[doc(hidden)]
pub mod registry;

mod composer;
mod loader;
mod runtime;
mod wit;
