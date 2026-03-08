use anyhow::Result;
use serde::de::DeserializeOwned;
use std::collections::HashMap;
use std::path::PathBuf;

use crate::composition::graph::ComponentGraph;
use crate::composition::registry::{HostCapability, HostCapabilityFactory, build_registries};
use crate::config::types::{ConfigHandler, DefinitionLoader};
use crate::types::Function;

mod grpc;
pub(crate) mod host;

use host::ComponentHost;

/// Wasm Component whose functions can be invoked
#[derive(Debug, Clone)]
pub struct Component {
    pub name: String,
    pub functions: HashMap<String, Function>,
}

/// Lifecycle-managed service that participates in config parsing and runtime.
///
/// A service optionally provides a `ConfigHandler` for parsing its own config
/// categories during the build phase, `HostCapability` implementations for
/// component linking, and `start`/`stop` lifecycle hooks.
///
/// The `config_handler()` method returns a separate handler object that can
/// write parsed config into shared state (e.g. `Arc<Mutex<...>>`). After
/// config processing, the handler is dropped and the service can read the
/// accumulated state in its `capabilities()` and `start()` implementations.
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

    /// Start the service. Called after all registries are built.
    /// Implementations should spawn background tasks and return immediately.
    fn start(&self) -> Result<()> {
        Ok(())
    }

    /// Signal the service to shut down gracefully.
    /// Implementations should cancel background tasks spawned by `start()`.
    fn stop(&self) -> Result<()> {
        Ok(())
    }
}

/// Composable Runtime for invoking Wasm Components
pub struct Runtime {
    host: ComponentHost,
    services: Vec<Box<dyn RuntimeService>>,
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
        self.host
            .component_registry
            .get_component(name)
            .map(|spec| Component {
                name: spec.name.clone(),
                functions: spec.functions.clone(),
            })
    }

    /// Invoke a component function
    pub async fn invoke(
        &self,
        component_name: &str,
        function_name: &str,
        args: Vec<serde_json::Value>,
    ) -> Result<serde_json::Value> {
        self.invoke_with_env(component_name, function_name, args, &[])
            .await
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

    /// Start the runtime (services, in registration order).
    pub fn start(&self) -> Result<()> {
        for service in &self.services {
            service.start()?;
        }
        Ok(())
    }

    /// Stop the runtime (services, in reverse registration order).
    pub fn stop(&self) {
        for service in self.services.iter().rev() {
            if let Err(e) = service.stop() {
                tracing::error!("Error stopping service: {e}");
            }
        }
    }

    /// Start the runtime and block until a shutdown signal (SIGINT/SIGTERM).
    ///
    /// Intended for long-lived processes (`composable run`).
    /// For one-off invocations, use `start()` / `stop()` directly.
    pub async fn run(&self) -> Result<()> {
        self.start()?;
        wait_for_shutdown().await?;
        self.stop();
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
    /// and `stop()` are called during the runtime lifecycle.
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
    pub async fn build(self) -> Result<Runtime> {
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
