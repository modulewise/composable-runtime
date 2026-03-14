use anyhow::Result;
use std::collections::HashMap;

use super::types::{CategoryClaim, ConfigHandler, PropertyMap};
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
    fn claimed_categories(&self) -> Vec<CategoryClaim> {
        vec![CategoryClaim::all("component")]
    }

    fn claimed_properties(&self) -> HashMap<&str, &[&str]> {
        HashMap::from([(
            "component",
            ["uri", "scope", "imports", "interceptors", "config"].as_slice(),
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
        let ctx = |e: PropertyError| e.with_context("component", name);
        let uri = take_required_string(&mut properties, "uri").map_err(ctx)?;
        let scope = take_optional_string(&mut properties, "scope")
            .map_err(ctx)?
            .unwrap_or_else(default_scope);
        let imports = take_string_array(&mut properties, "imports").map_err(ctx)?;
        let interceptors = take_string_array(&mut properties, "interceptors").map_err(ctx)?;
        let config = take_object(&mut properties, "config").map_err(ctx)?;

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
            interceptors,
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
    fn claimed_categories(&self) -> Vec<CategoryClaim> {
        vec![CategoryClaim::all("capability")]
    }

    fn claimed_properties(&self) -> HashMap<&str, &[&str]> {
        HashMap::from([("capability", ["type", "scope"].as_slice())])
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
        let ctx = |e: PropertyError| e.with_context("capability", name);
        let kind = take_required_string(&mut properties, "type").map_err(ctx)?;
        let scope = take_optional_string(&mut properties, "scope")
            .map_err(ctx)?
            .unwrap_or_else(default_scope);

        // Remaining properties are the capability's direct configuration
        let remaining: HashMap<String, serde_json::Value> = properties.into_iter().collect();

        self.definitions.push(CapabilityDefinition {
            name: name.to_string(),
            kind,
            scope,
            properties: remaining,
        });
        Ok(())
    }
}

// --- Property extractors ---

enum PropertyError {
    Missing {
        key: String,
    },
    TypeMismatch {
        key: String,
        expected: &'static str,
        got: serde_json::Value,
    },
}

impl PropertyError {
    fn with_context(self, category: &str, name: &str) -> anyhow::Error {
        match self {
            PropertyError::Missing { key } => {
                anyhow::anyhow!("{category} '{name}' missing required '{key}' field")
            }
            PropertyError::TypeMismatch { key, expected, got } => {
                anyhow::anyhow!("{category} '{name}': '{key}' must be {expected}, got {got}")
            }
        }
    }
}

fn take_required_string(properties: &mut PropertyMap, key: &str) -> Result<String, PropertyError> {
    match properties.remove(key) {
        Some(serde_json::Value::String(s)) => Ok(s),
        Some(got) => Err(PropertyError::TypeMismatch {
            key: key.into(),
            expected: "a string",
            got,
        }),
        None => Err(PropertyError::Missing { key: key.into() }),
    }
}

fn take_optional_string(
    properties: &mut PropertyMap,
    key: &str,
) -> Result<Option<String>, PropertyError> {
    match properties.remove(key) {
        Some(serde_json::Value::String(s)) => Ok(Some(s)),
        Some(got) => Err(PropertyError::TypeMismatch {
            key: key.into(),
            expected: "a string",
            got,
        }),
        None => Ok(None),
    }
}

fn take_string_array(
    properties: &mut PropertyMap,
    key: &str,
) -> Result<Vec<String>, PropertyError> {
    match properties.remove(key) {
        Some(serde_json::Value::Array(arr)) => {
            let mut result = Vec::with_capacity(arr.len());
            for item in arr {
                match item {
                    serde_json::Value::String(s) => result.push(s),
                    got => {
                        return Err(PropertyError::TypeMismatch {
                            key: key.into(),
                            expected: "an array of strings",
                            got,
                        });
                    }
                }
            }
            Ok(result)
        }
        Some(got) => Err(PropertyError::TypeMismatch {
            key: key.into(),
            expected: "an array",
            got,
        }),
        None => Ok(Vec::new()),
    }
}

fn take_object(
    properties: &mut PropertyMap,
    key: &str,
) -> Result<HashMap<String, serde_json::Value>, PropertyError> {
    match properties.remove(key) {
        Some(serde_json::Value::Object(map)) => Ok(map.into_iter().collect()),
        Some(got) => Err(PropertyError::TypeMismatch {
            key: key.into(),
            expected: "an object/table",
            got,
        }),
        None => Ok(HashMap::new()),
    }
}
