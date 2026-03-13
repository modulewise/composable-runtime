use anyhow::Result;
use petgraph::graph::{DiGraph, NodeIndex};
use petgraph::visit::EdgeRef;
use std::collections::HashMap;
use std::ops::{Index, IndexMut};
use std::path::PathBuf;

use crate::config::loaders::{TomlLoader, WasmLoader};
use crate::config::processor::ConfigProcessor;
use crate::types::{CapabilityDefinition, ComponentDefinition};

/// Directed graph of component and capability definitions
/// with dependency and interceptor edges.
pub struct ComponentGraph {
    graph: DiGraph<Node, Edge>,
    node_map: HashMap<String, NodeIndex>,
}

impl ComponentGraph {
    /// Create a new GraphBuilder.
    pub fn builder() -> GraphBuilder {
        GraphBuilder::new()
    }

    /// Create a graph where each component and capability is a node
    /// and each dependency or interceptor relationship is an edge.
    pub(crate) fn build(
        component_definitions: &[ComponentDefinition],
        capability_definitions: &[CapabilityDefinition],
    ) -> Result<Self> {
        let mut graph = DiGraph::<Node, Edge>::new();
        let mut node_map = HashMap::<String, NodeIndex>::new();

        for definition in capability_definitions {
            let index = graph.add_node(Node::Capability(definition.clone()));
            node_map.insert(definition.name.clone(), index);
        }

        // Determine which components are used only as interceptor templates.
        // These don't get their own graph node — only synthetic clones do.
        let interceptor_names: std::collections::HashSet<&str> = component_definitions
            .iter()
            .flat_map(|d| d.interceptors.iter().map(|s| s.as_str()))
            .collect();
        let imported_names: std::collections::HashSet<&str> = component_definitions
            .iter()
            .flat_map(|d| d.imports.iter().map(|s| s.as_str()))
            .collect();

        for definition in component_definitions {
            let is_template_only = interceptor_names.contains(definition.name.as_str())
                && !imported_names.contains(definition.name.as_str());
            if is_template_only {
                continue;
            }
            let index = graph.add_node(Node::Component(definition.clone()));
            node_map.insert(definition.name.clone(), index);
        }

        // Build interceptor chains with name takeover.
        //
        // Interceptors wrap a component's exports, not its imports. The
        // component's own imports connect directly to its own node.
        //
        // List order is call order (outer-to-inner), reversed here to
        // build the chain outward from the component.
        //
        // Given: [component.client] interceptors = ["auth", "logger"]
        //   _client$0 (original) -> _client$1 (logger) -> client (auth, outermost)
        //
        // The outermost interceptor takes over the original name so that
        // importers and public APIs see the intercepted version transparently.
        // Internal nodes use _name$N naming (reserved, validated at config time).
        //
        // This pattern is reusable: future [interceptor.*] support will
        // apply the same rename-and-replace on top of the existing chain.
        let mut interceptor_clones = std::collections::HashSet::<NodeIndex>::new();

        for definition in component_definitions {
            if definition.interceptors.is_empty() {
                continue;
            }

            let original_name = &definition.name;
            let internal_name = format!("_{original_name}$0");

            // Rename the original component node to its internal name.
            let component_index = node_map.remove(original_name).unwrap();
            if let Node::Component(ref mut def) = graph[component_index] {
                def.name = internal_name.clone();
            }
            node_map.insert(internal_name, component_index);

            let mut current = component_index;
            let interceptor_count = definition.interceptors.len();

            for (position, interceptor_name) in definition.interceptors.iter().rev().enumerate() {
                let interceptor_def = component_definitions
                    .iter()
                    .find(|d| d.name == *interceptor_name)
                    .ok_or_else(|| {
                        anyhow::anyhow!(
                            "Component '{}' references interceptor '{}', which is not defined.",
                            definition.name,
                            interceptor_name,
                        )
                    })?;

                let is_outermost = position == interceptor_count - 1;
                let synthetic_name = if is_outermost {
                    original_name.clone()
                } else {
                    format!("_{original_name}${}", position + 1)
                };

                let mut cloned_def = interceptor_def.clone();
                cloned_def.name = synthetic_name.clone();

                let cloned_index = graph.add_node(Node::Component(cloned_def));
                node_map.insert(synthetic_name, cloned_index);
                interceptor_clones.insert(cloned_index);

                graph.update_edge(current, cloned_index, Edge::Interceptor(position as i32));
                current = cloned_index;
            }
        }

        // Add dependency edges for original component definitions.
        // For intercepted components, the node was renamed to _name$0.
        for definition in component_definitions {
            let lookup_name = if definition.interceptors.is_empty() {
                definition.name.clone()
            } else {
                format!("_{}$0", definition.name)
            };

            let Some(importer_index) = node_map.get(&lookup_name).copied() else {
                continue; // Template-only interceptor, not in the graph.
            };

            for exporter_name in &definition.imports {
                if let Some(exporter_index) = node_map.get(exporter_name).copied() {
                    graph.update_edge(exporter_index, importer_index, Edge::Dependency);
                } else {
                    tracing::warn!(
                        "Component '{}' imports '{}', which is not defined.",
                        definition.name,
                        exporter_name
                    );
                }
            }
        }

        // Add dependency edges for interceptor clones' own imports.
        for clone_index in &interceptor_clones {
            let Node::Component(def) = &graph[*clone_index] else {
                continue;
            };
            let clone_name = def.name.clone();
            let imports = def.imports.clone();
            for exporter_name in &imports {
                if let Some(exporter_index) = node_map.get(exporter_name).copied() {
                    graph.update_edge(exporter_index, *clone_index, Edge::Dependency);
                } else {
                    tracing::warn!(
                        "Interceptor '{}' imports '{}', which is not defined.",
                        clone_name,
                        exporter_name
                    );
                }
            }
        }

        // Validate the graph for cycles
        if let Err(cycle) = petgraph::algo::toposort(&graph, None) {
            let node_name = match &graph[cycle.node_id()] {
                Node::Component(def) => &def.name,
                Node::Capability(def) => &def.name,
            };
            return Err(anyhow::anyhow!(
                "Circular dependency detected involving '{node_name}'"
            ));
        }

        Ok(Self { graph, node_map })
    }

    /// Write the graph to a DOT file.
    pub fn write_dot_file<P: AsRef<std::path::Path>>(&self, path: P) -> Result<()> {
        let dot_content = self.dot();
        std::fs::write(path, dot_content)
            .map_err(|e| anyhow::anyhow!("Failed to write DOT file: {e}"))?;
        Ok(())
    }

    pub fn nodes(&self) -> impl Iterator<Item = &petgraph::graph::Node<Node>> {
        self.graph.raw_nodes().iter()
    }

    pub fn get_build_order(&self) -> Vec<NodeIndex> {
        petgraph::algo::toposort(&self.graph, None).unwrap()
    }

    pub fn get_node_index(&self, name: &str) -> Option<NodeIndex> {
        self.node_map.get(name).copied()
    }

    pub fn get_dependencies(&self, index: NodeIndex) -> impl Iterator<Item = (NodeIndex, &Edge)> {
        self.graph
            .edges_directed(index, petgraph::Direction::Incoming)
            .map(|edge_ref| (edge_ref.source(), edge_ref.weight()))
    }

    fn dot(&self) -> String {
        let mut output = String::from("digraph ComponentGraph {\n");
        output.push_str("  rankdir=BT;\n");
        output.push_str("  node [fontname=\"Arial\", fontsize=10];\n");
        output.push_str("  edge [fontname=\"Arial\", fontsize=9];\n");

        for node_index in self.graph.node_indices() {
            let node = &self.graph[node_index];
            let node_attrs = match node {
                Node::Component(def) => {
                    let is_internal = def.name.starts_with('_');
                    let color = if is_internal { "yellow" } else { "lightblue" };
                    format!(
                        "[label=\"{}\", shape=box, fillcolor={color}, style=\"rounded,filled\"]",
                        def.name
                    )
                }
                Node::Capability(def) => {
                    format!(
                        "[label=\"{}\", shape=ellipse, fillcolor=orange, style=\"rounded,filled\"]",
                        def.name
                    )
                }
            };
            output.push_str(&format!("  {} {};\n", node_index.index(), node_attrs));
        }

        for edge_ref in self.graph.edge_references() {
            let edge_attrs = match edge_ref.weight() {
                Edge::Dependency => "[color=blue, style=solid]".to_string(),
                Edge::Interceptor(position) => {
                    format!("[color=red, style=dashed, label=\"interceptor: {position}\"]")
                }
            };
            output.push_str(&format!(
                "  {} -> {} {};\n",
                edge_ref.source().index(),
                edge_ref.target().index(),
                edge_attrs
            ));
        }

        output.push_str("}\n");
        output
    }
}

impl std::fmt::Debug for ComponentGraph {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        struct FlatNode<'a>(&'a Node);
        impl<'a> std::fmt::Debug for FlatNode<'a> {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                match self.0 {
                    Node::Component(def) => std::fmt::Debug::fmt(def, f),
                    Node::Capability(def) => std::fmt::Debug::fmt(def, f),
                }
            }
        }

        let mut debug_struct = f.debug_struct("ComponentGraph");

        let nodes: Vec<_> = self
            .graph
            .raw_nodes()
            .iter()
            .map(|n| FlatNode(&n.weight))
            .collect();
        debug_struct.field("nodes", &nodes);

        let edges: Vec<String> = self
            .graph
            .edge_references()
            .map(|edge| {
                let source_node = &self.graph[edge.source()];
                let target_node = &self.graph[edge.target()];
                let source_name = match source_node {
                    Node::Component(def) => &def.name,
                    Node::Capability(def) => &def.name,
                };
                let target_name = match target_node {
                    Node::Component(def) => &def.name,
                    Node::Capability(def) => &def.name,
                };
                format!("{} -> {} ({:?})", source_name, target_name, edge.weight())
            })
            .collect();
        debug_struct.field("edges", &edges);
        debug_struct.finish()
    }
}

impl Index<NodeIndex> for ComponentGraph {
    type Output = Node;

    fn index(&self, index: NodeIndex) -> &Self::Output {
        &self.graph[index]
    }
}

impl IndexMut<NodeIndex> for ComponentGraph {
    fn index_mut(&mut self, index: NodeIndex) -> &mut Self::Output {
        &mut self.graph[index]
    }
}

#[derive(Debug, Clone)]
pub enum Node {
    Component(ComponentDefinition),
    Capability(CapabilityDefinition),
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub enum Edge {
    Dependency,
    Interceptor(i32), // Position in chain (0 = innermost)
}

/// Builder for constructing a ComponentGraph.
pub struct GraphBuilder {
    paths: Vec<PathBuf>,
    loaders: Vec<Box<dyn crate::config::types::DefinitionLoader>>,
    handlers: Vec<Box<dyn crate::config::types::ConfigHandler>>,
    use_default_loaders: bool,
}

impl GraphBuilder {
    fn new() -> Self {
        Self {
            paths: Vec::new(),
            loaders: Vec::new(),
            handlers: Vec::new(),
            use_default_loaders: true,
        }
    }

    /// Add a definition source path (.toml, .wasm, oci://, etc.).
    pub fn from_path(mut self, path: impl Into<PathBuf>) -> Self {
        self.paths.push(path.into());
        self
    }

    /// Add multiple definition source paths.
    pub fn from_paths(mut self, paths: &[PathBuf]) -> Self {
        self.paths.extend_from_slice(paths);
        self
    }

    /// Add a definition loader.
    pub fn add_loader(mut self, loader: Box<dyn crate::config::types::DefinitionLoader>) -> Self {
        self.loaders.push(loader);
        self
    }

    /// Add a config handler.
    pub fn add_handler(mut self, handler: Box<dyn crate::config::types::ConfigHandler>) -> Self {
        self.handlers.push(handler);
        self
    }

    /// Do not enable the default definition loaders (.toml and .wasm).
    pub fn no_default_loaders(mut self) -> Self {
        self.use_default_loaders = false;
        self
    }

    /// Build the ComponentGraph from all loaded definitions.
    pub fn build(self) -> Result<ComponentGraph> {
        let mut processor = ConfigProcessor::new();

        if self.use_default_loaders {
            processor.add_loader(Box::new(TomlLoader::new()));
            processor.add_loader(Box::new(WasmLoader::new()));
        }
        for loader in self.loaders {
            processor.add_loader(loader);
        }
        for handler in self.handlers {
            processor.add_handler(handler);
        }

        let (component_definitions, capability_definitions) = processor.process(&self.paths)?;
        ComponentGraph::build(&component_definitions, &capability_definitions)
    }
}
