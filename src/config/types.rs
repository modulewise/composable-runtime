use anyhow::Result;
use std::collections::HashMap;
use std::path::Path;

/// Source-agnostic property map with JSON values.
pub type PropertyMap = HashMap<String, serde_json::Value>;

/// A definition entry from any source (TOML file, .wasm path, programmatic API).
#[derive(Debug, Clone)]
pub struct GenericDefinition {
    pub category: String,
    pub name: String,
    pub properties: PropertyMap,
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
    /// Categories this handler owns.
    fn claimed_categories(&self) -> &[&str];

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
}
