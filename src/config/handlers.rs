use anyhow::Result;
use std::collections::HashMap;

use super::types::{ConfigHandler, PropertyMap};
use crate::types::{CapabilityDefinition, ComponentDefinition, default_scope};

/// Handles `[component.*]` definitions.
pub struct ComponentConfigHandler<'a> {
    definitions: &'a mut Vec<ComponentDefinition>,
}

impl<'a> ComponentConfigHandler<'a> {
    pub fn new(definitions: &'a mut Vec<ComponentDefinition>) -> Self {
        Self { definitions }
    }
}

impl ConfigHandler for ComponentConfigHandler<'_> {
    fn claimed_categories(&self) -> &[&str] {
        &["component"]
    }

    fn claimed_properties(&self) -> HashMap<&str, &[&str]> {
        HashMap::from([(
            "component",
            [
                "uri",
                "scope",
                "imports",
                "intercepts",
                "precedence",
                "config",
            ]
            .as_slice(),
        )])
    }

    fn handle_category(
        &mut self,
        category: &str,
        name: &str,
        mut properties: PropertyMap,
    ) -> Result<()> {
        if category != "component" {
            return Err(anyhow::anyhow!(
                "ComponentConfigHandler received unexpected category '{category}'"
            ));
        }
        let uri = take_required_string(&mut properties, "uri", "component", name)?;
        let scope = take_optional_string(&mut properties, "scope").unwrap_or_else(default_scope);
        let imports = take_string_array(&mut properties, "imports");
        let intercepts = take_string_array(&mut properties, "intercepts");
        let precedence = take_optional_i32(&mut properties, "precedence").unwrap_or(0);
        let config = take_object(&mut properties, "config");

        if !properties.is_empty() {
            let unknown: Vec<_> = properties.keys().collect();
            return Err(anyhow::anyhow!(
                "Component '{name}' has unknown properties: {unknown:?}"
            ));
        }

        self.definitions.push(ComponentDefinition {
            name: name.to_string(),
            uri,
            scope,
            imports,
            intercepts,
            precedence,
            config,
        });
        Ok(())
    }
}

/// Handles `[capability.*]` definitions.
pub struct CapabilityConfigHandler<'a> {
    definitions: &'a mut Vec<CapabilityDefinition>,
}

impl<'a> CapabilityConfigHandler<'a> {
    pub fn new(definitions: &'a mut Vec<CapabilityDefinition>) -> Self {
        Self { definitions }
    }
}

impl ConfigHandler for CapabilityConfigHandler<'_> {
    fn claimed_categories(&self) -> &[&str] {
        &["capability"]
    }

    fn claimed_properties(&self) -> HashMap<&str, &[&str]> {
        HashMap::from([("capability", ["uri", "scope", "config"].as_slice())])
    }

    fn handle_category(
        &mut self,
        category: &str,
        name: &str,
        mut properties: PropertyMap,
    ) -> Result<()> {
        if category != "capability" {
            return Err(anyhow::anyhow!(
                "CapabilityConfigHandler received unexpected category '{category}'"
            ));
        }
        let uri = take_required_string(&mut properties, "uri", "capability", name)?;
        let scope = take_optional_string(&mut properties, "scope").unwrap_or_else(default_scope);
        let config = take_object(&mut properties, "config");

        if !properties.is_empty() {
            let unknown: Vec<_> = properties.keys().collect();
            return Err(anyhow::anyhow!(
                "Capability '{name}' has unknown properties: {unknown:?}"
            ));
        }

        self.definitions.push(CapabilityDefinition {
            name: name.to_string(),
            uri,
            scope,
            config,
        });
        Ok(())
    }
}

// --- Property extractors ---

fn take_required_string(
    properties: &mut PropertyMap,
    key: &str,
    category: &str,
    name: &str,
) -> Result<String> {
    match properties.remove(key) {
        Some(serde_json::Value::String(s)) => Ok(s),
        Some(other) => Err(anyhow::anyhow!(
            "{category} '{name}': '{key}' must be a string, got {other}"
        )),
        None => Err(anyhow::anyhow!(
            "{category} '{name}' missing required '{key}' field"
        )),
    }
}

fn take_optional_string(properties: &mut PropertyMap, key: &str) -> Option<String> {
    match properties.remove(key) {
        Some(serde_json::Value::String(s)) => Some(s),
        _ => None,
    }
}

fn take_string_array(properties: &mut PropertyMap, key: &str) -> Vec<String> {
    match properties.remove(key) {
        Some(serde_json::Value::Array(arr)) => arr
            .into_iter()
            .filter_map(|v| match v {
                serde_json::Value::String(s) => Some(s),
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    }
}

fn take_optional_i32(properties: &mut PropertyMap, key: &str) -> Option<i32> {
    match properties.remove(key) {
        Some(serde_json::Value::Number(n)) => n.as_i64().map(|i| i as i32),
        _ => None,
    }
}

fn take_object(properties: &mut PropertyMap, key: &str) -> HashMap<String, serde_json::Value> {
    match properties.remove(key) {
        Some(serde_json::Value::Object(map)) => map.into_iter().collect(),
        _ => HashMap::new(),
    }
}
