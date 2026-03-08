use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::any::{Any, TypeId};
use std::collections::{HashMap, HashSet};
use std::marker::PhantomData;
use std::path::PathBuf;
use std::sync::Arc;
use wasmtime::component::{HasData, Linker};

use super::composer::Composer;
use super::graph::{ComponentGraph, Node};
use super::wit::{ComponentMetadata, Parser};
use crate::types::{CapabilityDefinition, ComponentDefinition, ComponentState, Function};

/// Trait implemented by host capability instances.
///
/// An instance represents a configured capability (from one TOML block).
/// Multiple TOML blocks with the same `uri = "host:X"` create multiple instances.
pub trait HostCapability: Send + Sync {
    /// Fully qualified interfaces this capability provides (namespace:package/interface@version)
    fn interfaces(&self) -> Vec<String>;

    /// Add bindings to the linker. Called once per component instantiation.
    fn link(&self, linker: &mut Linker<ComponentState>) -> Result<()>;

    /// Create per-component-instance state. Called once per component instantiation.
    /// Returns None if capability needs no per-instance state.
    fn create_state_boxed(&self) -> Result<Option<(TypeId, Box<dyn Any + Send>)>> {
        Ok(None)
    }
}

/// `HasData` implementation that projects `ComponentState` to a capability's state type.
///
/// Use this in `link()` when adding bindings so host impls receive only their own state
/// (the type created by `create_state_boxed`), not full `ComponentState`.
///
/// # Example
///
/// ```ignore
/// fn link(&self, linker: &mut Linker<ComponentState>) -> Result<()> {
///     my_capability::add_to_linker::<_, CapabilityStateHasData<MyState>>(
///         linker,
///         |state| state.get_extension_mut::<MyState>().expect("MyState not initialized"),
///     )?;
///     Ok(())
/// }
/// ```
///
/// The host impl must then be `my_capability::Host for MyState` (not `ComponentState`).
pub struct CapabilityStateHasData<T>(PhantomData<T>);

impl<T: Send + 'static> HasData for CapabilityStateHasData<T> {
    type Data<'a> = &'a mut T;
}

/// Factory function that creates a HostCapability instance from TOML config.
pub type HostCapabilityFactory =
    Box<dyn Fn(serde_json::Value) -> Result<Box<dyn HostCapability>> + Send + Sync>;

/// Macro for implementing `create_state_boxed()` with automatic TypeId inference.
///
/// The `$body` expression receives the capability instance via `$self` and can use `?`
/// for fallible operations.
///
/// # Example
///
/// ```ignore
/// impl HostCapability for MyCapability {
///     // ...
///     create_state!(this, MyState, {
///         MyState {
///             shared_resource: this.get_resource(),
///             counter: 0,
///         }
///     });
/// }
/// ```
#[macro_export]
macro_rules! create_state {
    ($self:ident, $type:ty, $body:expr) => {
        fn create_state_boxed(
            &self,
        ) -> anyhow::Result<Option<(std::any::TypeId, Box<dyn std::any::Any + Send>)>> {
            let $self = self;
            let state: $type = $body;
            Ok(Some((std::any::TypeId::of::<$type>(), Box::new(state))))
        }
    };
}

/// Macro for creating a `(&str, HostCapabilityFactory)` tuple with reduced boilerplate.
///
/// # Examples
///
/// ```ignore
/// // Without config — capability is constructed directly
/// create_capability!("greeting", GreetingCapability {
///     message: self.message.clone(),
/// })
///
/// // With config — closure receives the capability's config value
/// create_capability!("greeting", |config| {
///     let suffix = config.get("suffix").and_then(|v| v.as_str()).unwrap_or("!");
///     GreetingCapability { message: self.message.clone(), suffix: suffix.to_string() }
/// })
/// ```
#[macro_export]
macro_rules! create_capability {
    ($name:expr, |$config:ident| $body:expr) => {
        (
            $name,
            Box::new(
                move |$config: serde_json::Value| -> anyhow::Result<Box<dyn $crate::HostCapability>> {
                    Ok(Box::new($body))
                },
            ) as $crate::HostCapabilityFactory,
        )
    };
    ($name:expr, $body:expr) => {
        (
            $name,
            Box::new(
                move |_config: serde_json::Value| -> anyhow::Result<Box<dyn $crate::HostCapability>> {
                    Ok(Box::new($body))
                },
            ) as $crate::HostCapabilityFactory,
        )
    };
}

#[derive(Serialize, Deserialize)]
pub struct Capability {
    pub uri: String,
    pub scope: String,
    pub interfaces: Vec<String>,
    /// The host capability instance (for `host:` URIs)
    #[serde(skip)]
    pub instance: Option<Box<dyn HostCapability>>,
}

impl std::fmt::Debug for Capability {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Capability")
            .field("uri", &self.uri)
            .field("scope", &self.scope)
            .field("interfaces", &self.interfaces)
            .field(
                "instance",
                &self.instance.as_ref().map(|_| "<dyn HostCapability>"),
            )
            .finish()
    }
}

#[derive(Debug, Clone)]
pub struct ComponentSpec {
    pub name: String,
    pub namespace: Option<String>,
    pub package: Option<String>,
    pub bytes: Arc<[u8]>,
    pub imports: Vec<String>,
    pub exports: Vec<String>,
    pub capabilities: Vec<String>,
    pub functions: HashMap<String, Function>,
}

#[derive(Debug, Clone)]
pub struct CapabilityRegistry {
    pub capabilities: Arc<HashMap<String, Capability>>,
}

#[derive(Debug, Clone)]
pub struct ComponentRegistry {
    pub components: Arc<HashMap<String, ComponentSpec>>,
}

impl CapabilityRegistry {
    pub fn new(capabilities: HashMap<String, Capability>) -> Self {
        Self {
            capabilities: Arc::new(capabilities),
        }
    }

    pub fn get_capability(&self, name: &str) -> Option<&Capability> {
        self.capabilities.get(name)
    }

    // TODO: replace hardcoded "any" with label selector evaluation
    pub fn verify_importable(
        &self,
        candidate: &CapabilityDefinition,
        requester: &ComponentDefinition,
    ) -> Result<()> {
        match candidate.scope.as_str() {
            "any" => Ok(()),
            scope => Err(anyhow::anyhow!(
                "Component '{}' cannot import capability '{}' (scope: '{scope}')",
                requester.name,
                candidate.name
            )),
        }
    }
}

impl ComponentRegistry {
    pub fn empty() -> Self {
        Self {
            components: Arc::new(HashMap::new()),
        }
    }

    pub fn get_components(&self) -> impl Iterator<Item = &ComponentSpec> {
        self.components.values()
    }

    pub fn get_component(&self, name: &str) -> Option<&ComponentSpec> {
        self.components.get(name)
    }

    // TODO: replace hardcoded "any" with label selector evaluation
    pub fn get_required_import(
        &self,
        candidate: &ComponentDefinition,
        requester: &ComponentDefinition,
        _requester_metadata: &ComponentMetadata,
    ) -> Result<&ComponentSpec> {
        let component = self
            .components
            .get(&candidate.name)
            .expect("component must exist in registry");
        match candidate.scope.as_str() {
            "any" => Ok(component),
            scope => Err(anyhow::anyhow!(
                "Component '{}' cannot import dependency '{}' (scope: '{scope}')",
                requester.name,
                candidate.name
            )),
        }
    }
}

impl Default for ComponentRegistry {
    fn default() -> Self {
        Self::empty()
    }
}

/// Build registries from definitions
pub async fn build_registries(
    component_graph: &ComponentGraph,
    factories: HashMap<&'static str, HostCapabilityFactory>,
) -> Result<(ComponentRegistry, CapabilityRegistry)> {
    let mut capability_definitions = Vec::new();
    for node in component_graph.nodes() {
        if let Node::Capability(def) = &node.weight {
            capability_definitions.push(def.clone());
        }
    }

    let capability_registry = create_capability_registry(capability_definitions, factories)?;

    let sorted_indices = component_graph.get_build_order();

    let mut built_components = HashMap::new();

    for node_index in sorted_indices {
        if let Node::Component(definition) = &component_graph[node_index] {
            let temp_component_registry = ComponentRegistry {
                components: Arc::new(built_components.clone()),
            };

            let component_spec = process_component(
                node_index,
                component_graph,
                &temp_component_registry,
                &capability_registry,
            )
            .await?;

            built_components.insert(definition.name.clone(), component_spec);
        }
    }

    Ok((
        ComponentRegistry {
            components: Arc::new(built_components),
        },
        capability_registry,
    ))
}

fn create_capability_registry(
    capability_definitions: Vec<CapabilityDefinition>,
    factories: HashMap<&'static str, HostCapabilityFactory>,
) -> Result<CapabilityRegistry> {
    let mut capabilities = HashMap::new();

    for def in capability_definitions {
        let (interfaces, capability_instance) = if let Some(capability_name) =
            def.uri.strip_prefix("host:")
        {
            let factory = factories.get(capability_name).ok_or_else(|| {
                anyhow::anyhow!(
                    "Host capability '{}' (URI: '{}') not registered. Use Runtime::builder().with_capability::<T>(\"{}\")",
                    capability_name,
                    def.uri,
                    capability_name
                )
            })?;

            // Deserialize config into capability instance
            let config_value = serde_json::to_value(&def.config)?;
            let cap = factory(config_value).map_err(|e| {
                anyhow::anyhow!(
                    "Failed to create host capability '{}' from TOML block '{}': {}",
                    capability_name,
                    def.name,
                    e
                )
            })?;

            (cap.interfaces(), Some(cap))
        } else {
            // wasmtime capability
            if !def.config.is_empty() {
                tracing::warn!(
                    "Config provided for capability '{}' but only host capabilities support config",
                    def.name
                );
            }
            (get_interfaces_for_capability(&def.uri), None)
        };

        let capability = Capability {
            uri: def.uri.clone(),
            scope: def.scope.clone(),
            interfaces,
            instance: capability_instance,
        };
        capabilities.insert(def.name, capability);
    }

    Ok(CapabilityRegistry::new(capabilities))
}

fn get_interfaces_for_capability(uri: &str) -> Vec<String> {
    match uri {
        "wasmtime:http" => vec![
            "wasi:http/outgoing-handler@0.2.6".to_string(),
            "wasi:http/types@0.2.6".to_string(),
        ],
        "wasmtime:io" => vec![
            "wasi:io/error@0.2.6".to_string(),
            "wasi:io/poll@0.2.6".to_string(),
            "wasi:io/streams@0.2.6".to_string(),
        ],
        "wasmtime:random" => vec![
            "wasi:random/random@0.2.6".to_string(),
            "wasi:random/insecure-seed@0.2.6".to_string(),
        ],
        "wasmtime:inherit-network" => vec![
            "wasi:sockets/tcp@0.2.6".to_string(),
            "wasi:sockets/udp@0.2.6".to_string(),
            "wasi:sockets/network@0.2.6".to_string(),
            "wasi:sockets/instance-network@0.2.6".to_string(),
        ],
        "wasmtime:allow-ip-name-lookup" => vec!["wasi:sockets/ip-name-lookup@0.2.6".to_string()],
        "wasmtime:inherit-stdio" => vec![
            "wasi:cli/stdin@0.2.6".to_string(),
            "wasi:cli/stdout@0.2.6".to_string(),
            "wasi:cli/stderr@0.2.6".to_string(),
        ],
        "wasmtime:wasip2" => vec![
            "wasi:cli/environment@0.2.6".to_string(),
            "wasi:cli/exit@0.2.6".to_string(),
            "wasi:cli/stderr@0.2.6".to_string(),
            "wasi:cli/stdin@0.2.6".to_string(),
            "wasi:cli/stdout@0.2.6".to_string(),
            "wasi:cli/terminal-input@0.2.6".to_string(),
            "wasi:cli/terminal-output@0.2.6".to_string(),
            "wasi:cli/terminal-stdin@0.2.6".to_string(),
            "wasi:cli/terminal-stdout@0.2.6".to_string(),
            "wasi:cli/terminal-stderr@0.2.6".to_string(),
            "wasi:clocks/monotonic-clock@0.2.6".to_string(),
            "wasi:clocks/wall-clock@0.2.6".to_string(),
            "wasi:filesystem/preopens@0.2.6".to_string(),
            "wasi:filesystem/types@0.2.6".to_string(),
            "wasi:io/error@0.2.6".to_string(),
            "wasi:io/poll@0.2.6".to_string(),
            "wasi:io/streams@0.2.6".to_string(),
            "wasi:random/random@0.2.6".to_string(),
            "wasi:random/insecure-seed@0.2.6".to_string(),
            "wasi:sockets/tcp@0.2.6".to_string(),
            "wasi:sockets/udp@0.2.6".to_string(),
            "wasi:sockets/network@0.2.6".to_string(),
            "wasi:sockets/instance-network@0.2.6".to_string(),
            "wasi:sockets/ip-name-lookup@0.2.6".to_string(),
            "wasi:sockets/tcp-create-socket@0.2.6".to_string(),
            "wasi:sockets/udp-create-socket@0.2.6".to_string(),
        ],
        _ => {
            tracing::warn!("Unknown capability URI: {uri}");
            vec![]
        }
    }
}

fn is_import_satisfied(import: &str, capability_interfaces: &HashSet<String>) -> bool {
    // First try exact match for performance
    if capability_interfaces.contains(import) {
        return true;
    }

    if let Some((interface_name, requested_version)) = import.rsplit_once('@')
        && let Some(requested_semver) = parse_semver(requested_version)
    {
        for available in capability_interfaces {
            if let Some((available_name, available_version)) = available.rsplit_once('@')
                && interface_name == available_name
                && let Some(available_semver) = parse_semver(available_version)
            {
                // same major, same minor, patch >= requested
                if available_semver.0 == requested_semver.0
                    && available_semver.1 == requested_semver.1
                    && available_semver.2 >= requested_semver.2
                {
                    return true;
                }
            }
        }
    }
    false
}

fn parse_semver(version: &str) -> Option<(u32, u32, u32)> {
    let parts: Vec<&str> = version.split('.').collect();
    if parts.len() == 3
        && let (Ok(major), Ok(minor), Ok(patch)) = (
            parts[0].parse::<u32>(),
            parts[1].parse::<u32>(),
            parts[2].parse::<u32>(),
        )
    {
        return Some((major, minor, patch));
    }
    None
}

async fn process_component(
    node_index: petgraph::graph::NodeIndex,
    component_graph: &ComponentGraph,
    component_registry: &ComponentRegistry,
    capability_registry: &CapabilityRegistry,
) -> Result<ComponentSpec> {
    let definition = if let Node::Component(def) = &component_graph[node_index] {
        def
    } else {
        return Err(anyhow::anyhow!(
            "Internal error: process_component called on a non-component node"
        ));
    };

    let mut bytes = read_bytes(&definition.uri).await?;

    let (metadata, mut imports, exports, functions) =
        Parser::parse(&bytes).map_err(|e| anyhow::anyhow!("Failed to parse component: {e}"))?;

    let imports_config = imports
        .iter()
        .any(|import| import.starts_with("wasi:config/store"));

    if imports_config {
        bytes = Composer::compose_with_config(&bytes, &definition.config).map_err(|e| {
            anyhow::anyhow!(
                "Failed to compose component '{}' with config: {}",
                definition.name,
                e
            )
        })?;

        let config_keys: Vec<_> = definition.config.keys().collect();
        tracing::info!(
            "Composed component '{}' with config: {config_keys:?}",
            definition.name
        );

        imports.retain(|import| !import.starts_with("wasi:config/store"));
    } else if !definition.config.is_empty() {
        tracing::warn!(
            "Config provided for component '{}' but component doesn't import wasi:config/store",
            definition.name
        );
    }

    let mut all_capabilities = HashSet::new();

    let dependencies = component_graph.get_dependencies(node_index);
    for dependency_node_index in dependencies {
        let dependency_node = &component_graph[dependency_node_index];
        match dependency_node {
            Node::Component(dependency_def) => {
                let component_spec = component_registry.get_required_import(
                    dependency_def,
                    definition,
                    &metadata,
                )?;
                bytes =
                    Composer::compose_components(&bytes, &component_spec.bytes).map_err(|e| {
                        anyhow::anyhow!(
                            "Failed composing '{}' with dependency '{}': {e}",
                            definition.name,
                            dependency_def.name
                        )
                    })?;
                tracing::info!(
                    "Composed component '{}' with dependency '{}'",
                    definition.name,
                    dependency_def.name
                );

                for export in &component_spec.exports {
                    imports.retain(|import| import != export);
                }
                all_capabilities.extend(component_spec.capabilities.iter().cloned());
            }
            Node::Capability(capability_def) => {
                capability_registry.verify_importable(capability_def, definition)?;
                all_capabilities.insert(capability_def.name.clone());
            }
        }
    }

    let capability_interfaces: std::collections::HashSet<String> = all_capabilities
        .iter()
        .filter_map(|name| capability_registry.get_capability(name))
        .flat_map(|cap| cap.interfaces.iter().cloned())
        .collect();

    // Check for imports not satisfied by capabilities
    let unsatisfied: Vec<_> = imports
        .iter()
        .filter(|import| !is_import_satisfied(import, &capability_interfaces))
        .cloned()
        .collect();

    if !unsatisfied.is_empty() {
        return Err(anyhow::anyhow!(
            "Component '{}' has unsatisfied imports: {:?}",
            definition.name,
            unsatisfied
        ));
    }

    Ok(ComponentSpec {
        name: definition.name.clone(),
        namespace: metadata.namespace,
        package: metadata.package,
        bytes: Arc::from(bytes),
        imports,
        exports,
        capabilities: all_capabilities.into_iter().collect(),
        functions,
    })
}

async fn read_bytes(uri: &str) -> Result<Vec<u8>> {
    if let Some(oci_ref) = uri.strip_prefix("oci://") {
        let client = wasm_pkg_client::oci::client::Client::new(Default::default());
        let image_ref = oci_ref.parse()?;
        let auth = oci_client::secrets::RegistryAuth::Anonymous;
        let media_types = vec!["application/wasm", "application/vnd.wasm.component"];

        let image_data = client.pull(&image_ref, &auth, media_types).await?;

        // Get the component bytes from the first layer
        if let Some(layer) = image_data.layers.first() {
            Ok(layer.data.clone())
        } else {
            Err(anyhow::anyhow!("No layers found in OCI image: {oci_ref}"))
        }
    } else {
        // Handle both file:// and plain paths
        let path = if let Some(path_str) = uri.strip_prefix("file://") {
            PathBuf::from(path_str)
        } else {
            PathBuf::from(uri)
        };
        Ok(std::fs::read(path)?)
    }
}
