use crate::loader::{ComponentDefinition, RuntimeFeatureDefinition};
use anyhow::Result;
use petgraph::graph::{DiGraph, NodeIndex};
use petgraph::visit::EdgeRef;
use std::collections::HashMap;
use std::ops::{Index, IndexMut};

pub struct ComponentGraph {
    graph: DiGraph<Node, Edge>,
    node_map: HashMap<String, NodeIndex>,
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

impl ComponentGraph {
    /// Create a graph where each component and runtime feature is a node
    /// and each dependency or interceptor relationship is an edge.
    pub fn build(
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
                .filter(|def| is_interceptor_enabled(def, consumer_def))
                .collect();

            if !interceptors.is_empty() {
                // Check if the consumer of this edge is one of the interceptors.
                // If so, this is the interceptor's own dependency, so we shouldn't redirect it.
                let consumer_name = match &graph[target_node_index] {
                    Node::Component(def) => &def.name,
                    Node::RuntimeFeature(def) => &def.name,
                };
                if interceptors.iter().any(|i| &i.name == consumer_name) {
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
                "Circular dependency detected involving '{}'",
                node_name
            ));
        }

        Ok(Self { graph, node_map })
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

    pub fn get_dependencies(&self, index: NodeIndex) -> petgraph::graph::Neighbors<Edge> {
        self.graph
            .neighbors_directed(index, petgraph::Direction::Incoming)
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
