//! Modulewise Composable Runtime
//!
//! A runtime for Wasm Components that supports
//! composition, config, and capability management.

pub use loader::{ComponentDefinition, RuntimeFeatureDefinition, load_definitions};
pub use registry::{ComponentRegistry, ComponentSpec, RuntimeFeatureRegistry, build_registries};
pub use runtime::Invoker;
pub use wit::Function;

pub mod composer;
pub mod graph;
pub mod loader;
pub mod registry;
pub mod runtime;
pub mod wit;
