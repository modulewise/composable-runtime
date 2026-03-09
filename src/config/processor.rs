use anyhow::Result;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use super::handlers::{CapabilityConfigHandler, ComponentConfigHandler};
use super::types::{ConfigHandler, DefinitionLoader, GenericDefinition, PropertyMap};
use crate::types::{CapabilityDefinition, ComponentDefinition};

pub struct ConfigProcessor {
    loaders: Vec<Box<dyn DefinitionLoader>>,
    handlers: Vec<Box<dyn ConfigHandler>>,
}

impl ConfigProcessor {
    pub fn new() -> Self {
        Self {
            loaders: Vec::new(),
            handlers: Vec::new(),
        }
    }

    pub fn add_loader(&mut self, loader: Box<dyn DefinitionLoader>) {
        self.loaders.push(loader);
    }

    pub fn add_handler(&mut self, handler: Box<dyn ConfigHandler>) {
        self.handlers.push(handler);
    }

    /// Route paths to loaders via claim, then run the full config pipeline.
    pub fn process(
        mut self,
        paths: &[PathBuf],
    ) -> Result<(Vec<ComponentDefinition>, Vec<CapabilityDefinition>)> {
        // Route paths to loaders
        for path in paths {
            let mut claimed_by = Vec::new();
            for (idx, loader) in self.loaders.iter_mut().enumerate() {
                if loader.claim(path) {
                    claimed_by.push(idx);
                }
            }
            match claimed_by.len() {
                0 => {
                    return Err(anyhow::anyhow!(
                        "No loader can handle path: {}",
                        path.display()
                    ));
                }
                1 => {}
                _ => {
                    return Err(anyhow::anyhow!(
                        "Multiple loaders claimed path: {}",
                        path.display()
                    ));
                }
            }
        }

        // Collect definitions from all loaders
        let mut definitions = Vec::new();
        for loader in &self.loaders {
            definitions.extend(loader.load()?);
        }

        // Build unified handler collection: core handlers + registered handlers
        let mut component_definitions = Vec::new();
        let mut capability_definitions = Vec::new();
        {
            let mut all_handlers: Vec<Box<dyn ConfigHandler + '_>> = Vec::new();
            all_handlers.push(Box::new(ComponentConfigHandler::new(
                &mut component_definitions,
            )));
            all_handlers.push(Box::new(CapabilityConfigHandler::new(
                &mut capability_definitions,
            )));
            all_handlers.extend(self.handlers);

            dispatch(&mut definitions, &mut all_handlers)?;
        }

        // Resolve placeholders
        resolve_placeholders_in_components(&mut component_definitions)?;
        resolve_placeholders_in_capabilities(&mut capability_definitions)?;

        // Cross-definition validation
        validate_scopes(&component_definitions, &capability_definitions)?;
        validate_names(&component_definitions, &capability_definitions)?;
        validate_imports(&component_definitions, &capability_definitions)?;

        Ok((component_definitions, capability_definitions))
    }
}

fn dispatch(
    definitions: &mut Vec<GenericDefinition>,
    handlers: &mut [Box<dyn ConfigHandler + '_>],
) -> Result<()> {
    // Build category => handler index map
    let mut category_owners: HashMap<String, usize> = HashMap::new();
    for (idx, handler) in handlers.iter().enumerate() {
        for cat in handler.claimed_categories() {
            if let Some(&existing_idx) = category_owners.get(*cat)
                && existing_idx != idx
            {
                return Err(anyhow::anyhow!(
                    "Category '{cat}' claimed by multiple handlers"
                ));
            }
            category_owners.insert(cat.to_string(), idx);
        }
    }

    // Build claimed properties map: (category, property) => handler index
    let mut property_claims: HashMap<(String, String), usize> = HashMap::new();
    for (idx, handler) in handlers.iter().enumerate() {
        for (category, properties) in handler.claimed_properties() {
            for prop in properties {
                let key = (category.to_string(), prop.to_string());
                if let Some(&existing_idx) = property_claims.get(&key)
                    && existing_idx != idx
                {
                    return Err(anyhow::anyhow!(
                        "Property '{prop}' on category '{category}' claimed by multiple handlers"
                    ));
                }
                property_claims.insert(key, idx);
            }
        }
    }

    for def in definitions.drain(..) {
        let &owner_idx = category_owners.get(&def.category).ok_or_else(|| {
            anyhow::anyhow!(
                "Unknown category '{}'. Known categories: {:?}",
                def.category,
                category_owners.keys().collect::<Vec<_>>()
            )
        })?;

        let (core_properties, claimed_by_handler) =
            split_properties(def.properties, &def.category, owner_idx, &property_claims);

        handlers[owner_idx].handle_category(&def.category, &def.name, core_properties)?;

        for (handler_idx, properties) in claimed_by_handler {
            handlers[handler_idx].handle_properties(&def.category, &def.name, properties)?;
        }
    }

    Ok(())
}

fn split_properties(
    mut properties: PropertyMap,
    category: &str,
    owner_idx: usize,
    property_claims: &HashMap<(String, String), usize>,
) -> (PropertyMap, HashMap<usize, PropertyMap>) {
    let mut claimed: HashMap<usize, PropertyMap> = HashMap::new();

    let keys: Vec<String> = properties.keys().cloned().collect();
    for key in keys {
        let lookup = (category.to_string(), key.clone());
        if let Some(&handler_idx) = property_claims.get(&lookup)
            && handler_idx != owner_idx
            && let Some(value) = properties.remove(&key)
        {
            claimed.entry(handler_idx).or_default().insert(key, value);
        }
    }

    (properties, claimed)
}

// --- Placeholder resolvers ---

fn resolve_placeholders_in_components(definitions: &mut [ComponentDefinition]) -> Result<()> {
    for def in definitions {
        resolve_placeholders_in_map(&mut def.config)?;
    }
    Ok(())
}

fn resolve_placeholders_in_capabilities(definitions: &mut [CapabilityDefinition]) -> Result<()> {
    for def in definitions {
        resolve_placeholders_in_map(&mut def.config)?;
    }
    Ok(())
}

fn resolve_placeholders_in_map(map: &mut HashMap<String, serde_json::Value>) -> Result<()> {
    for value in map.values_mut() {
        resolve_placeholders_in_value(value)?;
    }
    Ok(())
}

fn resolve_placeholders_in_value(value: &mut serde_json::Value) -> Result<()> {
    match value {
        serde_json::Value::String(s) => {
            if let Some(resolved) = resolve_placeholder(s)? {
                *s = resolved;
            }
        }
        serde_json::Value::Array(arr) => {
            for item in arr {
                resolve_placeholders_in_value(item)?;
            }
        }
        serde_json::Value::Object(map) => {
            for v in map.values_mut() {
                resolve_placeholders_in_value(v)?;
            }
        }
        _ => {}
    }
    Ok(())
}

fn resolve_placeholder(s: &str) -> Result<Option<String>> {
    if !s.starts_with("${") || !s.ends_with('}') {
        return Ok(None);
    }

    let inner = &s[2..s.len() - 1];
    if let Some(env_expr) = inner.strip_prefix("process:env:") {
        let (env_key, default) = match env_expr.split_once('|') {
            Some((key, default)) => (key, Some(default)),
            None => (env_expr, None),
        };
        match (std::env::var(env_key), default) {
            (Ok(value), _) => Ok(Some(value)),
            (Err(_), Some(default)) => Ok(Some(default.to_string())),
            (Err(_), None) => Err(anyhow::anyhow!(
                "Environment variable '{env_key}' not set (referenced in config placeholder '{s}')"
            )),
        }
    } else {
        Err(anyhow::anyhow!("Unknown placeholder pattern: '{s}'"))
    }
}

// --- Cross-definition validation ---

// TODO: replace with label selector validation
fn validate_scopes(
    components: &[ComponentDefinition],
    capabilities: &[CapabilityDefinition],
) -> Result<()> {
    for def in capabilities {
        match def.scope.as_str() {
            "any" => {}
            "package" | "namespace" => {
                return Err(anyhow::anyhow!(
                    "Capability '{}' cannot use scope='{}' - only components support package/namespace scoping",
                    def.name,
                    def.scope
                ));
            }
            _ => {
                return Err(anyhow::anyhow!(
                    "Invalid scope: '{}'. Must be: any",
                    def.scope
                ));
            }
        }
    }
    for def in components {
        match def.scope.as_str() {
            "any" | "package" | "namespace" => {}
            _ => {
                return Err(anyhow::anyhow!(
                    "Invalid scope: '{}'. Must be one of: any, package, namespace",
                    def.scope
                ));
            }
        }
    }
    Ok(())
}

fn validate_names(
    components: &[ComponentDefinition],
    capabilities: &[CapabilityDefinition],
) -> Result<()> {
    let mut all_names = HashSet::new();
    for def in capabilities {
        validate_name_chars(&def.name)?;
        if !all_names.insert(&def.name) {
            return Err(anyhow::anyhow!("Duplicate definition name: '{}'", def.name));
        }
    }
    for def in components {
        validate_name_chars(&def.name)?;
        if !all_names.insert(&def.name) {
            return Err(anyhow::anyhow!("Duplicate definition name: '{}'", def.name));
        }
    }
    Ok(())
}

fn validate_name_chars(name: &str) -> Result<()> {
    if name.starts_with('_') || name.contains('$') {
        return Err(anyhow::anyhow!(
            "Definition name '{name}' is invalid: names cannot start with '_' or contain '$' (reserved for internal use)"
        ));
    }
    Ok(())
}

fn validate_imports(
    components: &[ComponentDefinition],
    capabilities: &[CapabilityDefinition],
) -> Result<()> {
    let all_names: HashSet<&str> = components
        .iter()
        .map(|d| d.name.as_str())
        .chain(capabilities.iter().map(|d| d.name.as_str()))
        .collect();

    for def in components {
        for import_name in &def.imports {
            if !all_names.contains(import_name.as_str()) {
                return Err(anyhow::anyhow!(
                    "Component '{}' imports undefined definition '{}'",
                    def.name,
                    import_name
                ));
            }
        }
    }
    Ok(())
}
