use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use crate::composer::Composer;
use crate::graph::{ComponentGraph, Node};
use crate::loader::{ComponentDefinition, RuntimeFeatureDefinition};
use crate::wit::{ComponentMetadata, Parser};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuntimeFeature {
    pub uri: String,
    pub enables: String,
    pub interfaces: Vec<String>, // WASI interfaces this runtime feature provides
}

#[derive(Debug, Clone)]
pub struct ComponentSpec {
    pub name: String,
    pub namespace: Option<String>,
    pub package: Option<String>,
    pub bytes: Vec<u8>,
    pub imports: Vec<String>,
    pub exports: Vec<String>,
    pub runtime_features: Vec<String>,
    pub functions: Option<HashMap<String, crate::wit::Function>>,
}

#[derive(Debug, Clone)]
pub struct RuntimeFeatureRegistry {
    pub runtime_features: HashMap<String, RuntimeFeature>,
}

#[derive(Debug, Clone)]
pub struct ComponentRegistry {
    pub components: HashMap<String, ComponentSpec>,
    pub enabling_components: HashMap<String, EnablingComponent>,
}

#[derive(Debug, Clone)]
pub struct EnablingComponent {
    pub component: ComponentSpec,
    pub exposed: bool,
    pub enables: String,
}

impl RuntimeFeatureRegistry {
    pub fn new(runtime_features: HashMap<String, RuntimeFeature>) -> Self {
        Self { runtime_features }
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
            components,
            enabling_components,
        }
    }

    pub fn empty() -> Self {
        Self::new(HashMap::new(), HashMap::new())
    }

    pub fn get_components(&self) -> impl Iterator<Item = &ComponentSpec> {
        self.components.values()
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
) -> Result<(RuntimeFeatureRegistry, ComponentRegistry)> {
    let mut runtime_feature_definitions = Vec::new();
    for node in component_graph.nodes() {
        if let Node::RuntimeFeature(def) = &node.weight {
            runtime_feature_definitions.push(def.clone());
        }
    }

    let runtime_feature_registry =
        create_runtime_feature_registry(runtime_feature_definitions).await?;

    let sorted_indices = component_graph.get_build_order();

    let mut exposed_components = HashMap::new();
    let mut built_components = HashMap::new();
    let mut enabling_components = HashMap::new();

    for node_index in sorted_indices {
        if let Node::Component(definition) = &component_graph[node_index] {
            let temp_component_registry = ComponentRegistry {
                components: built_components.clone(),
                enabling_components: enabling_components.clone(),
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
            components: exposed_components,
            enabling_components,
        },
    ))
}

async fn create_runtime_feature_registry(
    runtime_feature_definitions: Vec<RuntimeFeatureDefinition>,
) -> Result<RuntimeFeatureRegistry> {
    let mut runtime_features = HashMap::new();

    for def in runtime_feature_definitions {
        let interfaces = get_interfaces_for_runtime_feature(&def.uri);
        let runtime_feature = RuntimeFeature {
            uri: def.uri.clone(),
            enables: def.enables.clone(),
            interfaces,
        };
        runtime_features.insert(def.name.clone(), runtime_feature);
    }

    Ok(RuntimeFeatureRegistry::new(runtime_features))
}

fn get_interfaces_for_runtime_feature(uri: &str) -> Vec<String> {
    match uri {
        "wasmtime:http" => vec![
            "wasi:http/outgoing-handler@0.2.3".to_string(),
            "wasi:http/types@0.2.3".to_string(),
        ],
        "wasmtime:io" => vec![
            "wasi:io/error@0.2.3".to_string(),
            "wasi:io/poll@0.2.3".to_string(),
            "wasi:io/streams@0.2.3".to_string(),
        ],
        "wasmtime:inherit-network" => vec![
            "wasi:sockets/tcp@0.2.3".to_string(),
            "wasi:sockets/udp@0.2.3".to_string(),
            "wasi:sockets/network@0.2.3".to_string(),
            "wasi:sockets/instance-network@0.2.3".to_string(),
        ],
        "wasmtime:allow-ip-name-lookup" => vec!["wasi:sockets/ip-name-lookup@0.2.3".to_string()],
        "wasmtime:wasip2" => vec![
            "wasi:cli/environment@0.2.3".to_string(),
            "wasi:cli/exit@0.2.3".to_string(),
            "wasi:cli/stderr@0.2.3".to_string(),
            "wasi:cli/stdin@0.2.3".to_string(),
            "wasi:cli/stdout@0.2.3".to_string(),
            "wasi:clocks/monotonic-clock@0.2.3".to_string(),
            "wasi:clocks/wall-clock@0.2.3".to_string(),
            "wasi:filesystem/preopens@0.2.3".to_string(),
            "wasi:filesystem/types@0.2.3".to_string(),
            "wasi:io/error@0.2.3".to_string(),
            "wasi:io/poll@0.2.3".to_string(),
            "wasi:io/streams@0.2.3".to_string(),
            "wasi:random/random@0.2.3".to_string(),
            "wasi:sockets/tcp@0.2.3".to_string(),
            "wasi:sockets/udp@0.2.3".to_string(),
            "wasi:sockets/network@0.2.3".to_string(),
            "wasi:sockets/instance-network@0.2.3".to_string(),
            "wasi:sockets/ip-name-lookup@0.2.3".to_string(),
            "wasi:sockets/tcp-create-socket@0.2.3".to_string(),
            "wasi:sockets/udp-create-socket@0.2.3".to_string(),
        ],
        _ => {
            println!("Unknown runtime feature URI: {uri}");
            vec![]
        }
    }
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
        .map_err(|e| anyhow::anyhow!("Failed to parse component: {}", e))?;

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
        .filter(|import| !runtime_interfaces.contains(*import))
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
        bytes,
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
            Err(anyhow::anyhow!("No layers found in OCI image: {}", oci_ref))
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
