//! Modulewise Composable Runtime
//!
//! A runtime for Wasm Components that supports
//! composition, config, and capability management.

pub use loader::{ComponentDefinition, RuntimeFeatureDefinition, load_definitions};
pub use registry::{ComponentRegistry, ComponentSpec, RuntimeFeatureRegistry, build_registries};
pub use runtime::Invoker;
pub use wit::Function;

mod composer;
mod loader;
mod registry;
mod runtime;
mod wit;
