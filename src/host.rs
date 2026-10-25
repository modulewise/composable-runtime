//! The surfaced traits for interacting with the components registered in a
//! runtime instance. Defines functions for listing and retrieving available
//! components, for instantiating components when needing to own an instance,
//! or for invoking a function directly on a single-use component instance.

use anyhow::Result;
use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;

use crate::component::ComponentInstance;
use crate::selector::Selector;
use crate::types::{Component, Val};

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

/// Instantiation, invocation, lookup, and discovery of available components.
pub trait ComponentHost: ComponentInvoker {
    fn get_component(&self, name: &str) -> Option<&Component>;

    fn list_components(&self, selector: Option<&Selector>) -> Vec<&Component>;

    /// Instantiate a component, for a caller that will own the instance and
    /// call it one or more times via [`ComponentInstance::call`] or within
    /// a closure via [`ComponentInstance::run_concurrent`]. Any resources it
    /// produces are no longer valid once the instance is dropped.
    ///
    /// Propagation context is read, as with [`ComponentInvoker::invoke`], from
    /// the ambient task-local when the component makes an outgoing request, so
    /// a caller that needs propagation must scope each of its calls.
    fn instantiate<'a>(
        &'a self,
        component_name: &'a str,
        env: Option<HashMap<String, String>>,
    ) -> Pin<Box<dyn Future<Output = Result<ComponentInstance>> + Send + 'a>>;
}
