use anyhow::Result;
use std::collections::HashMap;
use std::path::Path;

use crate::types::{CapabilityDefinition, ComponentDefinition};

/// Source-agnostic property map with JSON values.
pub type PropertyMap = HashMap<String, serde_json::Value>;

/// A definition entry from any source (TOML file, .wasm path, programmatic API).
#[derive(Debug, Clone)]
pub struct GenericDefinition {
    pub category: String,
    pub name: String,
    pub properties: PropertyMap,
}

// --- Selector types ---

/// Comparison operator for a selector condition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Operator {
    Equals(String),
    NotEquals(String),
    In(Vec<String>),
    NotIn(Vec<String>),
    Exists,
    DoesNotExist,
}

/// A single condition within a selector.
#[derive(Debug, Clone)]
pub struct Condition {
    pub key: String,
    pub operator: Operator,
}

/// Matches against a flattened string-to-string map.
/// All conditions must match (AND semantics).
#[derive(Debug, Clone)]
pub struct Selector {
    pub conditions: Vec<Condition>,
}

impl Selector {
    pub fn matches(&self, properties: &HashMap<String, Option<String>>) -> bool {
        self.conditions.iter().all(|c| c.matches(properties))
    }
}

impl Condition {
    fn matches(&self, properties: &HashMap<String, Option<String>>) -> bool {
        match &self.operator {
            Operator::Equals(expected) => properties
                .get(&self.key)
                .is_some_and(|v| v.as_ref().is_some_and(|v| v == expected)),
            Operator::NotEquals(expected) => properties
                .get(&self.key)
                .is_some_and(|v| v.as_ref().is_some_and(|v| v != expected)),
            Operator::In(values) => properties
                .get(&self.key)
                .is_some_and(|v| v.as_ref().is_some_and(|v| values.iter().any(|e| v == e))),
            Operator::NotIn(values) => properties
                .get(&self.key)
                .is_some_and(|v| v.as_ref().is_some_and(|v| values.iter().all(|e| v != e))),
            Operator::Exists => properties.contains_key(&self.key),
            Operator::DoesNotExist => !properties.contains_key(&self.key),
        }
    }
}

/// A category claim with an optional selector for discriminator-based dispatch.
#[derive(Debug, Clone)]
pub struct CategoryClaim {
    pub category: &'static str,
    pub selector: Option<Selector>,
}

impl CategoryClaim {
    /// Claim all definitions in a category (no selector filtering).
    pub fn all(category: &'static str) -> Self {
        Self {
            category,
            selector: None,
        }
    }

    /// Claim definitions in a category that match a selector.
    pub fn with_selector(category: &'static str, selector: Selector) -> Self {
        Self {
            category,
            selector: Some(selector),
        }
    }
}

/// Reads configuration from a source and produces generic definitions.
pub trait DefinitionLoader {
    /// Claim a path and store it if this loader should handle it.
    /// Default returns false for self-contained loaders.
    fn claim(&mut self, _path: &Path) -> bool {
        false
    }

    /// Load definitions from all claimed paths and/or internal sources.
    fn load(&self) -> Result<Vec<GenericDefinition>>;
}

/// Handles configuration for one or more categories.
pub trait ConfigHandler {
    /// Categories this handler owns, with optional selector filtering.
    /// A claim with no selector owns all definitions in that category.
    /// Multiple handlers may claim the same category only if all use selectors.
    fn claimed_categories(&self) -> Vec<CategoryClaim>;

    /// Properties this handler uses, keyed by category.
    /// Category owners should include their own properties (under their own category)
    /// to prevent other handlers from claiming them. Properties claimed on other
    /// handlers' categories will be split off and routed via `handle_properties`.
    fn claimed_properties(&self) -> HashMap<&str, &[&str]> {
        HashMap::new()
    }

    /// Handle a definition in an owned category.
    /// Properties claimed by other handlers will be excluded.
    fn handle_category(
        &mut self,
        category: &str,
        name: &str,
        properties: PropertyMap,
    ) -> Result<()>;

    /// Handle claimed properties from a category this handler does not own.
    fn handle_properties(
        &mut self,
        _category: &str,
        _name: &str,
        _properties: PropertyMap,
    ) -> Result<()> {
        Ok(())
    }

    /// Return component definitions generated during config handling.
    fn generated_component_definitions(&mut self) -> Vec<ComponentDefinition> {
        vec![]
    }

    /// Return capability definitions generated during config handling.
    fn generated_capability_definitions(&mut self) -> Vec<CapabilityDefinition> {
        vec![]
    }
}
