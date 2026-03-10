use anyhow::Result;
use serde::de::DeserializeOwned;
use std::collections::HashMap;
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;

use crate::composition::graph::ComponentGraph;
use crate::composition::registry::{HostCapability, HostCapabilityFactory, build_registries};
use crate::config::types::{ConfigHandler, DefinitionLoader};
#[cfg(feature = "messaging")]
use crate::types::MessagePublisher;
use crate::types::{Component, ComponentInvoker};

mod grpc;
pub(crate) mod host;

use host::ComponentHost;

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
pub trait RuntimeService: Send + Sync {
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

/// Composable Runtime for invoking Wasm Components
pub struct Runtime {
    host: ComponentHost,
    services: Vec<Box<dyn RuntimeService>>,
    #[cfg(feature = "messaging")]
    publisher: Arc<dyn MessagePublisher>,
}

impl Runtime {
    /// Create a RuntimeBuilder
    pub fn builder() -> RuntimeBuilder {
        RuntimeBuilder::new()
    }

    /// List all components
    pub fn list_components(&self) -> Vec<Component> {
        self.host
            .component_registry
            .get_components()
            .map(|spec| Component {
                name: spec.name.clone(),
                functions: spec.functions.clone(),
            })
            .collect()
    }

    /// Get a specific component by name
    pub fn get_component(&self, name: &str) -> Option<Component> {
        ComponentInvoker::get_component(&self.host, name)
    }

    /// Invoke a component function
    pub async fn invoke(
        &self,
        component_name: &str,
        function_name: &str,
        args: Vec<serde_json::Value>,
    ) -> Result<serde_json::Value> {
        ComponentInvoker::invoke(&self.host, component_name, function_name, args).await
    }

    /// Invoke a component function with environment variables
    pub async fn invoke_with_env(
        &self,
        component_name: &str,
        function_name: &str,
        args: Vec<serde_json::Value>,
        env_vars: &[(&str, &str)],
    ) -> Result<serde_json::Value> {
        self.host
            .invoke(component_name, function_name, args, env_vars)
            .await
    }

    /// Instantiate a component
    pub async fn instantiate(
        &self,
        component_name: &str,
    ) -> Result<(
        wasmtime::Store<crate::types::ComponentState>,
        wasmtime::component::Instance,
    )> {
        self.instantiate_with_env(component_name, &[]).await
    }

    /// Instantiate a component with environment variables
    pub async fn instantiate_with_env(
        &self,
        component_name: &str,
        env_vars: &[(&str, &str)],
    ) -> Result<(
        wasmtime::Store<crate::types::ComponentState>,
        wasmtime::component::Instance,
    )> {
        self.host.instantiate(component_name, env_vars).await
    }

    /// Get a component invoker for this runtime.
    pub fn invoker(&self) -> Arc<dyn ComponentInvoker> {
        Arc::new(self.host.clone())
    }

    /// Get a message publisher for this runtime (messaging feature only).
    #[cfg(feature = "messaging")]
    pub fn publisher(&self) -> Arc<dyn MessagePublisher> {
        Arc::clone(&self.publisher)
    }

    /// Start the runtime (services, in registration order).
    ///
    /// Injects dependencies (`set_invoker`, `set_publisher`) into each
    /// service before calling `start()`.
    pub fn start(&self) -> Result<()> {
        let invoker: Arc<dyn ComponentInvoker> = Arc::new(self.host.clone());
        for service in &self.services {
            service.set_invoker(invoker.clone());
            #[cfg(feature = "messaging")]
            service.set_publisher(Arc::clone(&self.publisher));
            service.start()?;
        }
        Ok(())
    }

    /// Shutdown all services in reverse registration order.
    pub async fn shutdown(&self) {
        for service in self.services.iter().rev() {
            service.shutdown().await;
        }
    }

    /// Start the runtime and block until a shutdown signal (SIGINT/SIGTERM).
    ///
    /// Intended for long-lived processes (`composable run`).
    /// For one-off invocations, use `start()` / `shutdown().await` directly.
    pub async fn run(&self) -> Result<()> {
        self.start()?;
        wait_for_shutdown().await?;
        self.shutdown().await;
        Ok(())
    }
}

/// Builder for configuring and creating a Runtime
pub struct RuntimeBuilder {
    paths: Vec<PathBuf>,
    loaders: Vec<Box<dyn DefinitionLoader>>,
    handlers: Vec<Box<dyn ConfigHandler>>,
    services: Vec<Box<dyn RuntimeService>>,
    factories: HashMap<&'static str, HostCapabilityFactory>,
    use_default_loaders: bool,
}

impl RuntimeBuilder {
    fn new() -> Self {
        Self {
            paths: Vec::new(),
            loaders: Vec::new(),
            handlers: Vec::new(),
            services: Vec::new(),
            factories: HashMap::new(),
            use_default_loaders: true,
        }
    }

    /// Add a definition source path (.toml, .wasm, oci://, etc.)
    pub fn from_path(mut self, path: impl Into<PathBuf>) -> Self {
        self.paths.push(path.into());
        self
    }

    /// Add multiple definition source paths
    pub fn from_paths(mut self, paths: &[PathBuf]) -> Self {
        self.paths.extend_from_slice(paths);
        self
    }

    /// Register a custom definition loader
    pub fn with_definition_loader(mut self, loader: Box<dyn DefinitionLoader>) -> Self {
        self.loaders.push(loader);
        self
    }

    /// Register a standalone config handler
    pub fn with_config_handler(mut self, handler: Box<dyn ConfigHandler>) -> Self {
        self.handlers.push(handler);
        self
    }

    /// Opt out of the default TomlLoader + WasmLoader
    pub fn no_default_loaders(mut self) -> Self {
        self.use_default_loaders = false;
        self
    }

    /// Register a lifecycle-managed service.
    ///
    /// The service's config handler (if any) participates in config parsing.
    /// Its capabilities are registered after config parsing. Its `start()`
    /// and `shutdown()` are called during the runtime lifecycle.
    pub fn with_service<T: RuntimeService + Default + 'static>(mut self) -> Self {
        self.services.push(Box::new(T::default()));
        self
    }

    /// Register a host capability type for the given name.
    ///
    /// The name corresponds to the suffix in `uri = "host:name"` in TOML.
    ///
    /// If the config is empty and deserialization fails,
    /// falls back to `Default::default()`.
    pub fn with_capability<T>(mut self, name: &'static str) -> Self
    where
        T: HostCapability + DeserializeOwned + Default + 'static,
    {
        self.factories.insert(
            name,
            Box::new(
                |config: serde_json::Value| -> Result<Box<dyn HostCapability>> {
                    match serde_json::from_value::<T>(config.clone()) {
                        Ok(instance) => Ok(Box::new(instance)),
                        Err(e) => {
                            if config == serde_json::json!({}) {
                                Ok(Box::new(T::default()))
                            } else {
                                Err(e.into())
                            }
                        }
                    }
                },
            ),
        );
        self
    }

    /// Build the Runtime: load config, build graph, build registries, create component host
    #[allow(unused_mut)]
    pub async fn build(mut self) -> Result<Runtime> {
        // Auto-register MessagingService when feature is enabled
        #[cfg(feature = "messaging")]
        let messaging_publisher: Arc<dyn MessagePublisher> = {
            let svc = crate::messaging::MessagingService::new();
            let publisher = svc.publisher();
            self.services.push(Box::new(svc));
            publisher
        };

        let mut graph_builder = ComponentGraph::builder().from_paths(&self.paths);
        if !self.use_default_loaders {
            graph_builder = graph_builder.no_default_loaders();
        }
        for loader in self.loaders {
            graph_builder = graph_builder.add_loader(loader);
        }
        for handler in self.handlers {
            graph_builder = graph_builder.add_handler(handler);
        }
        // Add config handlers from registered services
        for service in &self.services {
            if let Some(handler) = service.config_handler() {
                graph_builder = graph_builder.add_handler(handler);
            }
        }
        let graph = graph_builder.build()?;

        // Collect capability factories from both with_capability and service registrations
        let mut factories = self.factories;
        for service in &self.services {
            for (name, factory) in service.capabilities() {
                factories.insert(name, factory);
            }
        }

        // Build registries from graph
        let (component_registry, capability_registry) = build_registries(&graph, factories).await?;

        // Create component host
        let host = ComponentHost::new(component_registry, capability_registry)?;

        Ok(Runtime {
            host,
            services: self.services,
            #[cfg(feature = "messaging")]
            publisher: messaging_publisher,
        })
    }
}

async fn wait_for_shutdown() -> Result<()> {
    let ctrl_c = tokio::signal::ctrl_c();

    #[cfg(unix)]
    {
        let mut sigterm =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
        tokio::select! {
            result = ctrl_c => result?,
            _ = sigterm.recv() => {}
        }
    }

    #[cfg(not(unix))]
    ctrl_c.await?;

    Ok(())
}
