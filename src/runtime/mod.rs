use anyhow::Result;
use serde::de::DeserializeOwned;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use crate::composition::graph::ComponentGraph;
use crate::composition::registry::{HostCapability, HostCapabilityFactory, build_registries};
use crate::config::types::{ConfigHandler, DefinitionLoader};
#[cfg(feature = "messaging")]
use crate::message::MessagePublisher;
use crate::service::Service;
use crate::types::{Component, ComponentInvoker};

mod grpc;
pub(crate) mod host;

use host::ComponentHost;

/// Composable Runtime for invoking Wasm Components
pub struct Runtime {
    host: ComponentHost,
    services: Vec<Box<dyn Service>>,
    #[cfg(feature = "messaging")]
    publisher: Arc<dyn MessagePublisher>,
}

impl Runtime {
    /// Create a RuntimeBuilder
    pub fn builder() -> RuntimeBuilder {
        RuntimeBuilder::new()
    }

    /// List all components, optionally filtered by a selector
    pub fn list_components(
        &self,
        selector: Option<&crate::config::types::Selector>,
    ) -> Vec<&Component> {
        self.host.list_components(selector)
    }

    /// Get a specific component by name
    pub fn get_component(&self, name: &str) -> Option<&Component> {
        self.host.get_component(name)
    }

    /// Instantiate a component
    pub async fn instantiate(
        &self,
        component_name: &str,
        env: Option<HashMap<String, String>>,
    ) -> Result<(
        wasmtime::Store<crate::types::ComponentState>,
        wasmtime::component::Instance,
    )> {
        let env_pairs: Vec<(String, String)> =
            env.map(|m| m.into_iter().collect()).unwrap_or_default();
        self.host.instantiate(component_name, &env_pairs).await
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
    services: Vec<Box<dyn Service>>,
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
    pub fn with_service<T: Service + Default + 'static>(mut self) -> Self {
        self.services.push(Box::new(T::default()));
        self
    }

    /// Register a host capability type for the given name.
    ///
    /// The name corresponds to the `type` value in `[capability.*]` TOML blocks.
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
