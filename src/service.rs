use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use anyhow::Result;

use crate::composition::registry::HostCapabilityFactory;
use crate::config::types::ConfigHandler;
#[cfg(feature = "messaging")]
use crate::message::MessagePublisher;
use crate::types::ComponentInvoker;

/// Lifecycle-managed service that participates in config parsing and runtime.
///
/// A service optionally provides a `ConfigHandler` for parsing its own config
/// categories during the build phase, `HostCapability` implementations for
/// component linking, and `start`/`shutdown` lifecycle hooks.
///
/// The `config_handler()` method returns a separate handler object that can
/// write parsed config into shared state (e.g. `Arc<Mutex<...>>`). After
/// config processing, the handler is dropped and the service can read the
/// accumulated state in its `capabilities()` and `start()` implementations.
///
/// Dependencies are injected via `set_*` methods before `start()` is called.
/// Override only the ones your service needs; all have default no-ops.
pub trait Service: Send + Sync {
    /// Provide a config handler for parsing this service's config categories.
    /// Returns `None` if the service has no config (default).
    fn config_handler(&self) -> Option<Box<dyn ConfigHandler>> {
        None
    }

    /// Provide any HostCapability factories to register (default is empty).
    /// Called after config parsing and before registry build.
    /// Each factory creates capability instances from `config.*` values,
    /// closing over any service-internal state needed by the capability.
    fn capabilities(&self) -> Vec<(&'static str, HostCapabilityFactory)> {
        vec![]
    }

    /// Inject the component invoker. Called before `start()`.
    /// Override to stash the invoker for use during the service lifecycle.
    fn set_invoker(&self, _invoker: Arc<dyn ComponentInvoker>) {}

    /// Inject the message publisher. Called before `start()`.
    /// Override to stash the publisher for use during the service lifecycle.
    #[cfg(feature = "messaging")]
    fn set_publisher(&self, _publisher: Arc<dyn MessagePublisher>) {}

    /// Start the service. Called after all dependencies are injected.
    /// Implementations should spawn background tasks and return immediately.
    fn start(&self) -> Result<()> {
        Ok(())
    }

    /// Shutdown the service, cancelling background tasks.
    fn shutdown(&self) -> Pin<Box<dyn Future<Output = ()> + Send + '_>> {
        Box::pin(async {})
    }
}
