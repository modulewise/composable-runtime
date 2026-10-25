use anyhow::Result;
use std::collections::HashMap;
use std::sync::Arc;

use super::engine::ComponentEngine;
use crate::component::ComponentInstance;
use crate::composition::registry::{
    BootstrapRegistry, CapabilityRegistry, ComponentRegistry, ComponentSpec,
};
use crate::composition::resolver::Resolver;
use crate::host::{ComponentHost, ComponentInvoker};
use crate::types::{Component, ComponentMetadata, Val};

/// The build phase: components are registered as the graph is traversed.
pub(crate) struct Bootstrapper {
    engine: ComponentEngine,
    pub(crate) component_registry: BootstrapRegistry,
    pub(crate) capability_registry: CapabilityRegistry,
}

impl Bootstrapper {
    pub(crate) fn new(engine: ComponentEngine, capability_registry: CapabilityRegistry) -> Self {
        Self {
            engine,
            component_registry: BootstrapRegistry::default(),
            capability_registry,
        }
    }

    /// Produce the host with a sealed component registry.
    pub(crate) fn into_host(self) -> Host {
        let component_registry = self.component_registry.seal();
        let components = component_registry
            .get_components()
            .map(|spec| {
                let component = Component {
                    metadata: ComponentMetadata {
                        name: spec.name.clone(),
                        namespace: spec.namespace.clone(),
                        package: spec.package.clone(),
                        labels: spec.labels.clone(),
                        dependents: Some(spec.dependents.clone()),
                        exports: spec.exports.clone(),
                    },
                    functions: spec.functions.clone(),
                };
                (spec.name.clone(), component)
            })
            .collect();
        Host {
            engine: self.engine,
            components,
            component_registry,
            capability_registry: self.capability_registry,
        }
    }
}

/// Hosts components, managing their lifecycle (instantiate, link, invoke).
pub(crate) struct Host {
    engine: ComponentEngine,
    components: HashMap<String, Component>,
    pub(crate) component_registry: ComponentRegistry,
    pub(crate) capability_registry: CapabilityRegistry,
}

impl Host {
    pub(crate) async fn invoke(
        &self,
        component_name: &str,
        function_name: &str,
        args: Vec<Val>,
        env_vars: &[(String, String)],
    ) -> Result<Option<Val>> {
        let spec = self
            .component_registry
            .get_component(component_name)
            .ok_or_else(|| anyhow::anyhow!("Component '{component_name}' not found"))?;

        invoke_with_spec(
            &self.engine,
            spec,
            &self.capability_registry,
            function_name,
            args,
            env_vars,
        )
        .await
    }

    pub(crate) async fn instantiate(
        &self,
        component_name: &str,
        env_vars: &[(String, String)],
    ) -> Result<ComponentInstance> {
        let spec = self
            .component_registry
            .get_component(component_name)
            .ok_or_else(|| anyhow::anyhow!("Component '{component_name}' not found"))?;

        self.engine
            .instantiate_from_bytes(
                &spec.bytes,
                &spec.capabilities,
                &self.capability_registry,
                env_vars,
            )
            .await
    }
}

async fn invoke_with_spec(
    engine: &ComponentEngine,
    spec: &ComponentSpec,
    capability_registry: &CapabilityRegistry,
    function_name: &str,
    args: Vec<Val>,
    env_vars: &[(String, String)],
) -> Result<Option<Val>> {
    let function = spec.functions.get(function_name).ok_or_else(|| {
        anyhow::anyhow!(
            "Function '{function_name}' not found in component '{}'",
            spec.name
        )
    })?;

    engine
        .invoke(
            &spec.bytes,
            &spec.capabilities,
            capability_registry,
            function.clone(),
            args,
            env_vars,
        )
        .await
}

impl ComponentHost for Host {
    fn get_component(&self, name: &str) -> Option<&Component> {
        self.components.get(name)
    }

    fn list_components(&self, selector: Option<&crate::selector::Selector>) -> Vec<&Component> {
        match selector {
            Some(selector) => self
                .components
                .values()
                .filter(|c| selector.matches(&c.metadata.to_selectable()))
                .collect(),
            None => self.components.values().collect(),
        }
    }

    fn instantiate<'a>(
        &'a self,
        component_name: &'a str,
        env: Option<HashMap<String, String>>,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = anyhow::Result<ComponentInstance>> + Send + 'a>,
    > {
        Box::pin(async move {
            let env_pairs: Vec<(String, String)> =
                env.map(|m| m.into_iter().collect()).unwrap_or_default();
            self.instantiate(component_name, &env_pairs).await
        })
    }
}

impl ComponentInvoker for Host {
    fn invoke<'a>(
        &'a self,
        component_name: &'a str,
        function_name: &'a str,
        args: Vec<Val>,
        env: Option<HashMap<String, String>>,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = anyhow::Result<Option<Val>>> + Send + 'a>>
    {
        Box::pin(async move {
            let env_pairs: Vec<(String, String)> =
                env.map(|m| m.into_iter().collect()).unwrap_or_default();
            self.invoke(component_name, function_name, args, &env_pairs)
                .await
        })
    }
}

impl ComponentInvoker for Bootstrapper {
    fn invoke<'a>(
        &'a self,
        component_name: &'a str,
        function_name: &'a str,
        args: Vec<Val>,
        env: Option<HashMap<String, String>>,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = anyhow::Result<Option<Val>>> + Send + 'a>>
    {
        Box::pin(async move {
            let env_pairs: Vec<(String, String)> =
                env.map(|m| m.into_iter().collect()).unwrap_or_default();
            let spec = self
                .component_registry
                .get_component(component_name)
                .ok_or_else(|| anyhow::anyhow!("Component '{component_name}' not found"))?;
            invoke_with_spec(
                &self.engine,
                &spec,
                &self.capability_registry,
                function_name,
                args,
                &env_pairs,
            )
            .await
        })
    }
}

/// Produces a component's bytes by invoking the factory its `uri` references.
/// Unlike the File and OCI resolvers, this one requires the runtime, since a
/// factory is an invoked component.
pub(crate) struct FactoryResolver {
    component_invoker: Arc<dyn ComponentInvoker>,
}

impl FactoryResolver {
    pub(crate) fn new(component_invoker: Arc<dyn ComponentInvoker>) -> Self {
        Self { component_invoker }
    }
}

impl Resolver for FactoryResolver {
    fn resolve<'a>(
        &'a self,
        uri: &'a str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Vec<u8>>> + Send + 'a>> {
        Box::pin(async move {
            let name = uri
                .strip_prefix("factory:")
                .ok_or_else(|| anyhow::anyhow!("not a factory uri: {uri}"))?;

            let result = self
                .component_invoker
                .invoke(name, FACTORY_BUILD, Vec::new(), None)
                .await
                .map_err(|e| anyhow::anyhow!("factory '{name}' failed to build: {e}"))?;

            match result {
                Some(Val::Bytes(bytes)) => Ok(bytes),
                Some(_) | None => Err(anyhow::anyhow!(
                    "factory '{name}' did not return component bytes"
                )),
            }
        })
    }
}

// The exported interface and function from `composable:factory`.
const FACTORY_BUILD: &str = "factory.build";
