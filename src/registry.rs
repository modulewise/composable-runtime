use anyhow::Result;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use std::any::{Any, TypeId};
use std::collections::{HashMap, HashSet};
use std::marker::PhantomData;
use std::path::PathBuf;
use std::sync::Arc;
use wasmtime::component::Linker;

use crate::composer::Composer;
use crate::graph::{ComponentGraph, Node};
use crate::types::{ComponentDefinition, ComponentState, RuntimeFeatureDefinition};
use crate::wit::{ComponentMetadata, Parser};

/// Trait implemented by host extension instances.
///
/// An instance represents a configured feature (from one TOML block).
/// Multiple TOML blocks with the same `uri = "host:X"` create multiple instances.
pub trait HostExtension: Send + Sync {
    /// Fully qualified interfaces this extension provides (namespace:package/interface@version)
    fn interfaces(&self) -> Vec<String>;

    /// Add bindings to the linker. Called once per component instantiation.
    fn link(&self, linker: &mut Linker<ComponentState>) -> Result<()>;

    /// Create per-component-instance state. Called once per component instantiation.
    /// Returns None if extension needs no per-instance state.
    fn create_state_boxed(&self) -> Result<Option<(TypeId, Box<dyn Any + Send>)>> {
        Ok(None)
    }
}

/// Factory for creating HostExtension instances from TOML config.
///
/// Users don't implement this directly. The blanket impl handles it.
pub trait HostExtensionFactory: Send + Sync {
    fn create(&self, config: serde_json::Value) -> Result<Box<dyn HostExtension>>;
}

/// Blanket impl: any HostExtension + DeserializeOwned + Default gets a factory via PhantomData.
///
/// If the config is an empty object and deserialization fails, falls back to `Default::default()`.
/// This allows simple extensions with no config to use `#[derive(Default)]` without needing
/// `#[derive(Deserialize)]`.
impl<T> HostExtensionFactory for PhantomData<T>
where
    T: HostExtension + DeserializeOwned + Default + 'static,
{
    fn create(&self, config: serde_json::Value) -> Result<Box<dyn HostExtension>> {
        match serde_json::from_value::<T>(config.clone()) {
            Ok(instance) => Ok(Box::new(instance)),
            Err(e) => {
                // If config is empty object, fall back to Default
                if config == serde_json::json!({}) {
                    Ok(Box::new(T::default()))
                } else {
                    Err(e.into())
                }
            }
        }
    }
}

/// Macro for implementing `create_state_boxed()` with automatic TypeId inference.
///
/// The `$body` expression has access to `self` (the extension instance) and can use `?`
/// for fallible operations.
///
/// # Example
///
/// ```ignore
/// impl HostExtension for MyFeature {
///     // ...
///     create_state!(MyState, {
///         MyState {
///             shared_resource: self.get_resource(),
///             counter: 0,
///         }
///     });
/// }
/// ```
#[macro_export]
macro_rules! create_state {
    ($type:ty, $body:expr) => {
        fn create_state_boxed(
            &self,
        ) -> anyhow::Result<Option<(std::any::TypeId, Box<dyn std::any::Any + Send>)>> {
            let state: $type = $body;
            Ok(Some((std::any::TypeId::of::<$type>(), Box::new(state))))
        }
    };
}

#[derive(Serialize, Deserialize)]
pub struct RuntimeFeature {
    pub uri: String,
    pub enables: String,
    pub interfaces: Vec<String>,
    /// The host extension instance (for `host:` URIs)
    #[serde(skip)]
    pub extension: Option<Box<dyn HostExtension>>,
}

impl std::fmt::Debug for RuntimeFeature {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RuntimeFeature")
            .field("uri", &self.uri)
            .field("enables", &self.enables)
            .field("interfaces", &self.interfaces)
            .field(
                "extension",
                &self.extension.as_ref().map(|_| "<dyn HostExtension>"),
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
    pub runtime_features: Vec<String>,
    pub functions: Option<HashMap<String, crate::wit::Function>>,
}

#[derive(Debug, Clone)]
pub struct RuntimeFeatureRegistry {
    pub runtime_features: Arc<HashMap<String, RuntimeFeature>>,
}

#[derive(Debug, Clone)]
pub struct ComponentRegistry {
    pub components: Arc<HashMap<String, ComponentSpec>>,
    pub enabling_components: Arc<HashMap<String, EnablingComponent>>,
}

#[derive(Debug, Clone)]
pub struct EnablingComponent {
    pub component: ComponentSpec,
    pub exposed: bool,
    pub enables: String,
}

impl RuntimeFeatureRegistry {
    pub fn new(runtime_features: HashMap<String, RuntimeFeature>) -> Self {
        Self {
            runtime_features: Arc::new(runtime_features),
        }
    }

    pub fn get_runtime_feature(&self, name: &str) -> Option<&RuntimeFeature> {
        self.runtime_features.get(name)
    }

    pub fn get_enabled_runtime_feature(
        &self,
        requesting_component: &ComponentDefinition,
        feature_name: &str,
    ) -> Option<&RuntimeFeature> {
        if let Some(runtime_feature) = self.runtime_features.get(feature_name) {
            match runtime_feature.enables.as_str() {
                "none" => None,
                "any" => Some(runtime_feature),
                "exposed" => {
                    if requesting_component.exposed {
                        Some(runtime_feature)
                    } else {
                        None
                    }
                }
                "unexposed" => {
                    if !requesting_component.exposed {
                        Some(runtime_feature)
                    } else {
                        None
                    }
                }
                "package" => None,
                "namespace" => None,
                _ => None, // Unknown enables scope
            }
        } else {
            None
        }
    }
}

impl ComponentRegistry {
    fn new(
        components: HashMap<String, ComponentSpec>,
        enabling_components: HashMap<String, EnablingComponent>,
    ) -> Self {
        Self {
            components: Arc::new(components),
            enabling_components: Arc::new(enabling_components),
        }
    }

    pub fn empty() -> Self {
        Self::new(HashMap::new(), HashMap::new())
    }

    pub fn get_components(&self) -> impl Iterator<Item = &ComponentSpec> {
        self.components.values()
    }

    pub fn get_component(&self, name: &str) -> Option<&ComponentSpec> {
        self.components.get(name)
    }

    pub fn get_enabled_component_dependency(
        &self,
        requesting_component: &ComponentDefinition,
        requesting_metadata: &ComponentMetadata,
        dependency_name: &str,
    ) -> Option<&ComponentSpec> {
        if let Some(enabling_component) = self.enabling_components.get(dependency_name) {
            match enabling_component.enables.as_str() {
                "none" => None,
                "any" => Some(&enabling_component.component),
                "exposed" => {
                    if requesting_component.exposed {
                        Some(&enabling_component.component)
                    } else {
                        None
                    }
                }
                "unexposed" => {
                    if !requesting_component.exposed {
                        Some(&enabling_component.component)
                    } else {
                        None
                    }
                }
                "package" => {
                    match (
                        requesting_metadata.package.as_deref(),
                        enabling_component.component.package.as_deref(),
                    ) {
                        (Some(req_pkg), Some(enable_pkg)) if req_pkg == enable_pkg => {
                            Some(&enabling_component.component)
                        }
                        _ => None,
                    }
                }
                "namespace" => {
                    match (
                        requesting_metadata.namespace.as_deref(),
                        enabling_component.component.namespace.as_deref(),
                    ) {
                        (Some(req_ns), Some(enable_ns)) if req_ns == enable_ns => {
                            Some(&enabling_component.component)
                        }
                        _ => None,
                    }
                }
                _ => None,
            }
        } else {
            None
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
    factories: HashMap<&'static str, Box<dyn HostExtensionFactory>>,
) -> Result<(RuntimeFeatureRegistry, ComponentRegistry)> {
    let mut runtime_feature_definitions = Vec::new();
    for node in component_graph.nodes() {
        if let Node::RuntimeFeature(def) = &node.weight {
            runtime_feature_definitions.push(def.clone());
        }
    }

    let runtime_feature_registry =
        create_runtime_feature_registry(runtime_feature_definitions, factories)?;

    let sorted_indices = component_graph.get_build_order();

    let mut exposed_components = HashMap::new();
    let mut built_components = HashMap::new();
    let mut enabling_components = HashMap::new();

    for node_index in sorted_indices {
        if let Node::Component(definition) = &component_graph[node_index] {
            let temp_component_registry = ComponentRegistry {
                components: Arc::new(built_components.clone()),
                enabling_components: Arc::new(enabling_components.clone()),
            };

            match process_component(
                node_index,
                component_graph,
                &temp_component_registry,
                &runtime_feature_registry,
            )
            .await
            {
                Ok(component_spec) => {
                    built_components.insert(definition.name.clone(), component_spec.clone());
                    if definition.exposed {
                        exposed_components.insert(definition.name.clone(), component_spec.clone());
                    }
                    if definition.enables != "none" {
                        let enabling = EnablingComponent {
                            component: component_spec,
                            exposed: definition.exposed,
                            enables: definition.enables.clone(),
                        };
                        enabling_components.insert(definition.name.clone(), enabling);
                    }
                }
                Err(e) => {
                    if definition.exposed {
                        eprintln!(
                            "Warning: Skipping exposed component '{}': {}",
                            definition.name, e
                        );
                    } else {
                        return Err(e);
                    }
                }
            }
        }
    }

    Ok((
        runtime_feature_registry,
        ComponentRegistry {
            components: Arc::new(exposed_components),
            enabling_components: Arc::new(enabling_components),
        },
    ))
}

fn create_runtime_feature_registry(
    runtime_feature_definitions: Vec<RuntimeFeatureDefinition>,
    factories: HashMap<&'static str, Box<dyn HostExtensionFactory>>,
) -> Result<RuntimeFeatureRegistry> {
    let mut runtime_features = HashMap::new();

    for def in runtime_feature_definitions {
        let (interfaces, extension) = if let Some(feature_name) = def.uri.strip_prefix("host:") {
            let factory = factories.get(feature_name).ok_or_else(|| {
                anyhow::anyhow!(
                    "Host extension '{}' (URI: '{}') not registered. Use Runtime::builder().with_host_extension::<T>(\"{}\")",
                    feature_name,
                    def.uri,
                    feature_name
                )
            })?;

            // Deserialize config into extension instance
            let config_value = serde_json::to_value(&def.config)?;
            let ext = factory.create(config_value).map_err(|e| {
                anyhow::anyhow!(
                    "Failed to create host extension '{}' from TOML block '{}': {}",
                    feature_name,
                    def.name,
                    e
                )
            })?;

            (ext.interfaces(), Some(ext))
        } else {
            // wasmtime feature
            if !def.config.is_empty() {
                println!(
                    "Warning: Config provided for runtime feature '{}' but only host extensions support config",
                    def.name
                );
            }
            (get_interfaces_for_runtime_feature(&def.uri), None)
        };

        let runtime_feature = RuntimeFeature {
            uri: def.uri.clone(),
            enables: def.enables.clone(),
            interfaces,
            extension,
        };
        runtime_features.insert(def.name, runtime_feature);
    }

    Ok(RuntimeFeatureRegistry::new(runtime_features))
}

fn get_interfaces_for_runtime_feature(uri: &str) -> Vec<String> {
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
            println!("Unknown runtime feature URI: {uri}");
            vec![]
        }
    }
}

fn is_import_satisfied(import: &str, runtime_interfaces: &HashSet<String>) -> bool {
    // First try exact match for performance
    if runtime_interfaces.contains(import) {
        return true;
    }

    if let Some((interface_name, requested_version)) = import.rsplit_once('@')
        && let Some(requested_semver) = parse_semver(requested_version)
    {
        for available in runtime_interfaces {
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
    runtime_feature_registry: &RuntimeFeatureRegistry,
) -> Result<ComponentSpec> {
    let definition = if let Node::Component(def) = &component_graph[node_index] {
        def
    } else {
        return Err(anyhow::anyhow!(
            "Internal error: process_component called on a non-component node"
        ));
    };

    let mut bytes = read_bytes(&definition.uri).await?;

    let (metadata, mut imports, exports, functions) = Parser::parse(&bytes, definition.exposed)
        .map_err(|e| anyhow::anyhow!("Failed to parse component: {e}"))?;

    let imports_config = imports
        .iter()
        .any(|import| import.starts_with("wasi:config/store"));

    if imports_config {
        let config_to_use = match &definition.config {
            Some(c) => c,
            None => &HashMap::new(),
        };
        bytes = Composer::compose_with_config(&bytes, config_to_use).map_err(|e| {
            anyhow::anyhow!(
                "Failed to compose component '{}' with config: {}",
                definition.name,
                e
            )
        })?;

        let config_keys: Vec<_> = config_to_use.keys().collect();
        println!(
            "Composed component '{}' with config: {config_keys:?}",
            definition.name
        );

        imports.retain(|import| !import.starts_with("wasi:config/store"));
    } else if definition.config.is_some() {
        println!(
            "Warning: Config provided for component '{}' but component doesn't import wasi:config/store",
            definition.name
        );
    }

    let mut all_runtime_features = HashSet::new();

    let dependencies = component_graph.get_dependencies(node_index);
    for dependency_node_index in dependencies {
        let dependency_node = &component_graph[dependency_node_index];
        match dependency_node {
            Node::Component(dependency_def) => {
                if let Some(component_spec) = component_registry.get_enabled_component_dependency(
                    definition,
                    &metadata,
                    &dependency_def.name,
                ) {
                    bytes = Composer::compose_components(&bytes, &component_spec.bytes)?;
                    println!(
                        "Composed component '{}' with dependency '{}'",
                        definition.name, dependency_def.name
                    );

                    for export in &component_spec.exports {
                        imports.retain(|import| import != export);
                    }
                    all_runtime_features.extend(component_spec.runtime_features.iter().cloned());
                } else {
                    return Err(anyhow::anyhow!(
                        "Component '{}' requested dependency '{}', but access is not enabled",
                        definition.name,
                        dependency_def.name
                    ));
                }
            }
            Node::RuntimeFeature(feature_def) => {
                if runtime_feature_registry
                    .get_enabled_runtime_feature(definition, &feature_def.name)
                    .is_some()
                {
                    all_runtime_features.insert(feature_def.name.clone());
                } else {
                    return Err(anyhow::anyhow!(
                        "Component '{}' requested runtime feature '{}', but access is not enabled",
                        definition.name,
                        feature_def.name
                    ));
                }
            }
        }
    }

    let runtime_interfaces: std::collections::HashSet<String> = all_runtime_features
        .iter()
        .filter_map(|name| runtime_feature_registry.get_runtime_feature(name))
        .flat_map(|rf| rf.interfaces.iter().cloned())
        .collect();

    // Check for imports not satisfied by runtime features
    let unsatisfied: Vec<_> = imports
        .iter()
        .filter(|import| !is_import_satisfied(import, &runtime_interfaces))
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
        runtime_features: all_runtime_features.into_iter().collect(),
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
