use anyhow::Result;
use petgraph::graph::{DiGraph, NodeIndex};
use petgraph::visit::EdgeRef;
use std::collections::HashMap;
use std::ops::{Index, IndexMut};
use std::path::PathBuf;

use crate::loader;
use crate::types::{ComponentDefinition, RuntimeFeatureDefinition};

pub struct ComponentGraph {
    graph: DiGraph<Node, Edge>,
    node_map: HashMap<String, NodeIndex>,
}

impl ComponentGraph {
    /// Create a new GraphBuilder
    pub fn builder() -> GraphBuilder {
        GraphBuilder::new()
    }

    /// Create a graph where each component and runtime feature is a node
    /// and each dependency or interceptor relationship is an edge.
    pub(crate) fn build(
        component_definitions: &[ComponentDefinition],
        runtime_feature_definitions: &[RuntimeFeatureDefinition],
    ) -> Result<Self> {
        let mut graph = DiGraph::<Node, Edge>::new();
        let mut node_map = HashMap::<String, NodeIndex>::new();

        for definition in runtime_feature_definitions {
            let index = graph.add_node(Node::RuntimeFeature(definition.clone()));
            node_map.insert(definition.name.clone(), index);
        }

        for definition in component_definitions {
            let index = graph.add_node(Node::Component(definition.clone()));
            node_map.insert(definition.name.clone(), index);
        }

        for definition in component_definitions {
            let source_index = *node_map.get(&definition.name).unwrap();
            let mut expects = definition.expects.clone();
            // `intercepts` implies `expects` because the interceptor component
            // must be composed with the component it intercepts.
            for target_name in &definition.intercepts {
                if !expects.contains(target_name) {
                    expects.push(target_name.clone());
                }
            }

            for target_name in &expects {
                if let Some(target_index) = node_map.get(target_name) {
                    graph.update_edge(*target_index, source_index, Edge::Dependency);
                } else {
                    println!(
                        "Warning: Component '{}' expects '{}', which is not defined.",
                        definition.name, target_name
                    );
                }
            }
        }

        // Process interceptors to redirect edges
        let mut edges_to_add = Vec::new();
        let mut edges_to_remove = Vec::new();

        for edge_ref in graph.edge_references() {
            let source_node_index = edge_ref.source();
            let target_node_index = edge_ref.target();

            let provider_name = match &graph[source_node_index] {
                Node::Component(def) => &def.name,
                Node::RuntimeFeature(def) => &def.name,
            };

            // RuntimeFeatures can be providers, but not consumers.
            let Node::Component(consumer_def) = &graph[target_node_index] else {
                unreachable!()
            };

            // Iterate all components defined to intercept this provider,
            // but filter out any that do not enable this specific consumer.
            let mut interceptors: Vec<_> = component_definitions
                .iter()
                .filter(|def| def.intercepts.contains(provider_name))
                .filter(|def| {
                    let enabled = is_interceptor_enabled(def, consumer_def);
                    if !enabled && def.name != consumer_def.name {
                        println!(
                            "Interceptor '{}' skipped for consumer '{}' (enables='{}', consumer exposed={})",
                            def.name, consumer_def.name, def.enables, consumer_def.exposed
                        );
                    }
                    enabled
                })
                .collect();

            if !interceptors.is_empty() {
                // Don't redirect dependencies for consumers that are interceptors.
                // Interceptor routing is configured when processing non-interceptor consumers.
                if interceptors.iter().any(|i| i.name == consumer_def.name) {
                    continue;
                }

                interceptors.sort_by_key(|a| a.precedence);

                // Remove direct dependency edges for interceptors other than the first one.
                for interceptor in &interceptors[1..] {
                    let interceptor_index = *node_map.get(&interceptor.name).unwrap();
                    if let Some(edge_id) = graph.find_edge(source_node_index, interceptor_index) {
                        edges_to_remove.push(edge_id);
                    }
                }

                // Original edge will be replaced by interceptor routing.
                edges_to_remove.push(edge_ref.id());

                let mut current_provider_index = source_node_index;
                for interceptor in &interceptors {
                    let interceptor_index = *node_map.get(&interceptor.name).unwrap();
                    edges_to_add.push((
                        current_provider_index,
                        interceptor_index,
                        Edge::Interceptor(interceptor.precedence),
                    ));
                    current_provider_index = interceptor_index;
                }
                edges_to_add.push((current_provider_index, target_node_index, Edge::Dependency));
            }
        }

        graph.retain_edges(|_, edge| !edges_to_remove.contains(&edge));

        for (source, target, data) in edges_to_add {
            graph.update_edge(source, target, data);
        }

        // Validate the graph for cycles
        if let Err(cycle) = petgraph::algo::toposort(&graph, None) {
            let node_name = match &graph[cycle.node_id()] {
                Node::Component(def) => &def.name,
                Node::RuntimeFeature(def) => &def.name,
            };
            return Err(anyhow::anyhow!(
                "Circular dependency detected involving '{node_name}'"
            ));
        }

        Ok(Self { graph, node_map })
    }

    /// Write the graph to a DOT file
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

    pub fn get_dependencies(&self, index: NodeIndex) -> petgraph::graph::Neighbors<'_, Edge> {
        self.graph
            .neighbors_directed(index, petgraph::Direction::Incoming)
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
                    let shape = if def.exposed { "doubleoctagon" } else { "box" };
                    let color = if def.exposed {
                        "lightgreen"
                    } else if def.intercepts.is_empty() {
                        "lightblue"
                    } else {
                        "yellow"
                    };
                    let label = if def.intercepts.is_empty() {
                        def.name.to_string()
                    } else {
                        format!("{}\\n(intercepts: {})", def.name, def.intercepts.join(", "))
                    };
                    format!(
                        "[label=\"{label}\", shape={shape}, fillcolor={color}, style=\"rounded,filled\"]"
                    )
                }
                Node::RuntimeFeature(def) => {
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
                Edge::Interceptor(precedence) => {
                    format!("[color=red, style=dashed, label=\"precedence: {precedence}\"]")
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
                    Node::RuntimeFeature(def) => std::fmt::Debug::fmt(def, f),
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
                    Node::RuntimeFeature(def) => &def.name,
                };
                let target_name = match target_node {
                    Node::Component(def) => &def.name,
                    Node::RuntimeFeature(def) => &def.name,
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

fn is_interceptor_enabled(
    interceptor: &ComponentDefinition,
    consumer: &ComponentDefinition,
) -> bool {
    match interceptor.enables.as_str() {
        "none" => false,
        "any" => true,
        "exposed" => consumer.exposed,
        "unexposed" => !consumer.exposed,
        // TODO: The graph builder does not have access to component metadata, so we
        // cannot evaluate these scopes here. The final check is performed in the
        // registry builder, which does have the metadata. Accepting here means it could
        // fail later, rather than be skipped.
        "package" => true,
        "namespace" => true,
        _ => false, // Unknown enables scope, default to false.
    }
}

#[derive(Debug, Clone)]
pub enum Node {
    Component(ComponentDefinition),
    RuntimeFeature(RuntimeFeatureDefinition),
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub enum Edge {
    Dependency,
    Interceptor(i32), // Precedence
}

/// Builder for constructing a ComponentGraph
pub struct GraphBuilder {
    paths: Vec<PathBuf>,
}

impl GraphBuilder {
    fn new() -> Self {
        Self { paths: Vec::new() }
    }

    /// Load definitions from a file (.toml or .wasm)
    pub fn load_file(mut self, path: impl Into<PathBuf>) -> Self {
        self.paths.push(path.into());
        self
    }

    /// Build the ComponentGraph from all loaded definitions
    pub fn build(self) -> Result<ComponentGraph> {
        let (component_definitions, runtime_feature_definitions) =
            loader::parse_definition_files(&self.paths)?;
        ComponentGraph::build(&component_definitions, &runtime_feature_definitions)
    }
}
