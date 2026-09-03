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

pub use crate::selector::{Condition, Operator, Selector};

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

    /// Whether this handler accepts properties not declared in
    /// `claimed_properties` as pass-through configuration. Defaults to
    /// `false`: the framework rejects any property not claimed by some
    /// handler. Handlers that intentionally consume arbitrary keys (e.g.
    /// capability handlers that forward unknown keys to capability-specific
    /// config) override this to return `true` for the relevant categories.
    fn accepts_unclaimed_properties(&self, _category: &str) -> bool {
        false
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

#[cfg(test)]
mod tests {
    use super::*;

    fn props(pairs: &[(&str, &str)]) -> HashMap<String, Option<String>> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), Some(v.to_string())))
            .collect()
    }

    #[test]
    fn parse_equals() {
        let s = Selector::parse("name=foo").unwrap();
        assert_eq!(s.conditions.len(), 1);
        assert_eq!(s.conditions[0].key, "name");
        assert_eq!(
            s.conditions[0].operator,
            Operator::Equals("foo".to_string())
        );
    }

    #[test]
    fn parse_not_equals() {
        let s = Selector::parse("name!=foo").unwrap();
        assert_eq!(
            s.conditions[0].operator,
            Operator::NotEquals("foo".to_string())
        );
    }

    #[test]
    fn parse_exists_and_not_exists() {
        let s = Selector::parse("dependents,!labels.internal").unwrap();
        assert_eq!(s.conditions.len(), 2);
        assert_eq!(s.conditions[0].operator, Operator::Exists);
        assert_eq!(s.conditions[0].key, "dependents");
        assert_eq!(s.conditions[1].operator, Operator::DoesNotExist);
        assert_eq!(s.conditions[1].key, "labels.internal");
    }

    #[test]
    fn parse_in() {
        let s = Selector::parse("labels.domain in (payments,inventory)").unwrap();
        assert_eq!(
            s.conditions[0].operator,
            Operator::In(vec!["payments".to_string(), "inventory".to_string()])
        );
    }

    #[test]
    fn parse_notin() {
        let s = Selector::parse("labels.env notin (dev,staging)").unwrap();
        assert_eq!(
            s.conditions[0].operator,
            Operator::NotIn(vec!["dev".to_string(), "staging".to_string()])
        );
    }

    #[test]
    fn parse_contains() {
        let s = Selector::parse("exports contains get-value").unwrap();
        assert_eq!(
            s.conditions[0].operator,
            Operator::Contains("get-value".to_string())
        );
    }

    #[test]
    fn parse_notcontains() {
        let s = Selector::parse("dependents notcontains logger").unwrap();
        assert_eq!(
            s.conditions[0].operator,
            Operator::NotContains("logger".to_string())
        );
    }

    #[test]
    fn parse_multiple_conditions() {
        let s = Selector::parse("name=foo,labels.domain=payments,!dependents").unwrap();
        assert_eq!(s.conditions.len(), 3);
    }

    #[test]
    fn parse_in_with_commas_preserved() {
        let s = Selector::parse("labels.env in (prod,staging),name=api").unwrap();
        assert_eq!(s.conditions.len(), 2);
        assert_eq!(
            s.conditions[0].operator,
            Operator::In(vec!["prod".to_string(), "staging".to_string()])
        );
        assert_eq!(
            s.conditions[1].operator,
            Operator::Equals("api".to_string())
        );
    }

    #[test]
    fn parse_empty_selector_fails() {
        assert!(Selector::parse("").is_err());
    }

    #[test]
    fn parse_in_missing_parens_fails() {
        assert!(Selector::parse("key in a,b").is_err());
    }

    #[test]
    fn parse_in_empty_list_fails() {
        assert!(Selector::parse("key in ()").is_err());
    }

    #[test]
    fn parse_notin_empty_list_fails() {
        assert!(Selector::parse("key notin ()").is_err());
    }

    #[test]
    fn match_equals() {
        let s = Selector::parse("name=foo").unwrap();
        assert!(s.matches(&props(&[("name", "foo")])));
        assert!(!s.matches(&props(&[("name", "bar")])));
    }

    #[test]
    fn match_exists_and_not_exists() {
        let s = Selector::parse("!dependents").unwrap();
        assert!(s.matches(&props(&[("name", "foo")])));
        assert!(!s.matches(&props(&[("name", "foo"), ("dependents", "[api]")])));
    }

    #[test]
    fn match_contains_list_element() {
        let s = Selector::parse("exports contains get-value").unwrap();
        assert!(s.matches(&props(&[("exports", "[get-value,run]")])));
        assert!(!s.matches(&props(&[("exports", "[run,calc]")])));
    }

    #[test]
    fn match_contains_list_no_substring_match() {
        let s = Selector::parse("dependents contains translator").unwrap();
        // Should NOT match: "logging-translator" is not the element "translator"
        assert!(!s.matches(&props(&[("dependents", "[logging-translator]")])));
        // Should match: "translator" is an exact element
        assert!(s.matches(&props(&[("dependents", "[translator,logger]")])));
    }

    #[test]
    fn match_contains_scalar_substring() {
        let s = Selector::parse("name contains foo").unwrap();
        assert!(s.matches(&props(&[("name", "foobar")])));
        assert!(s.matches(&props(&[("name", "bazfoo")])));
        assert!(!s.matches(&props(&[("name", "bar")])));
    }

    #[test]
    fn match_in_set() {
        let s = Selector::parse("labels.domain in (payments,inventory)").unwrap();
        assert!(s.matches(&props(&[("labels.domain", "payments")])));
        assert!(s.matches(&props(&[("labels.domain", "inventory")])));
        assert!(!s.matches(&props(&[("labels.domain", "shipping")])));
    }
}
