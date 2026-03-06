use anyhow::Result;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::PathBuf;

use crate::types::{
    CapabilityDefinition, ComponentDefinition, ComponentDefinitionBase, DefinitionBase,
    default_scope,
};

pub(crate) fn parse_definition_files(
    definition_files: &[PathBuf], // .toml and .wasm files
) -> Result<(Vec<ComponentDefinition>, Vec<CapabilityDefinition>)> {
    let mut toml_files = Vec::new();
    let mut wasm_files = Vec::new();

    for path in definition_files {
        let path_str = path.to_string_lossy();

        // Handle OCI URIs as wasm components
        if path_str.starts_with("oci://") {
            wasm_files.push(path.clone());
        } else if let Some(extension) = path.extension().and_then(|s| s.to_str()) {
            match extension {
                "wasm" => wasm_files.push(path.clone()),
                "toml" => toml_files.push(path.clone()),
                _ => return Err(anyhow::anyhow!("Unsupported file type: {}", path.display())),
            }
        } else {
            return Err(anyhow::anyhow!(
                "File without extension: {}",
                path.display()
            ));
        }
    }
    build_definitions(&toml_files, &wasm_files)
}

fn build_definitions(
    toml_files: &[PathBuf],
    wasm_files: &[PathBuf],
) -> Result<(Vec<ComponentDefinition>, Vec<CapabilityDefinition>)> {
    let mut component_definitions = Vec::new();
    let mut capability_definitions = Vec::new();

    // Parse TOML files to extract both components and capabilities
    for file in toml_files {
        let (components, capabilities) = parse_toml_file(file)?;
        component_definitions.extend(components);
        capability_definitions.extend(capabilities);
    }

    // Add implicit component definitions from standalone .wasm files
    component_definitions.extend(create_implicit_component_definitions(wasm_files)?);

    for def in &capability_definitions {
        validate_capability_scope(&def.scope, &def.name)?;
    }
    for def in &component_definitions {
        validate_component_scope(&def.scope)?;
    }

    // Collision detection - ensure unique names across all definitions
    let mut all_names = HashSet::new();
    for def in &capability_definitions {
        if !all_names.insert(&def.name) {
            return Err(anyhow::anyhow!("Duplicate definition name: '{}'", def.name));
        }
    }
    for def in &component_definitions {
        if !all_names.insert(&def.name) {
            return Err(anyhow::anyhow!("Duplicate definition name: '{}'", def.name));
        }
    }

    // Validate component imports
    for def in &component_definitions {
        for expected_name in &def.imports {
            if !all_names.contains(expected_name) {
                return Err(anyhow::anyhow!(
                    "Component '{}' imports undefined definition '{}'",
                    def.name,
                    expected_name
                ));
            }
        }
    }

    Ok((component_definitions, capability_definitions))
}

// TODO: replace with label selector validation
fn validate_capability_scope(scope: &str, name: &str) -> Result<()> {
    match scope {
        "any" => Ok(()),
        "package" | "namespace" => Err(anyhow::anyhow!(
            "Capability '{name}' cannot use scope='{scope}' - only components support package/namespace scoping"
        )),
        _ => Err(anyhow::anyhow!("Invalid scope: '{scope}'. Must be: any")),
    }
}

// TODO: replace with label selector validation
fn validate_component_scope(scope: &str) -> Result<()> {
    match scope {
        "any" | "package" | "namespace" => Ok(()),
        _ => Err(anyhow::anyhow!(
            "Invalid scope: '{scope}'. Must be one of: any, package, namespace"
        )),
    }
}

fn parse_toml_file(
    path: &PathBuf,
) -> Result<(Vec<ComponentDefinition>, Vec<CapabilityDefinition>)> {
    let content = fs::read_to_string(path)?;
    let toml_doc: toml::Value = toml::from_str(&content)?;

    let mut components = Vec::new();
    let mut capabilities = Vec::new();

    if let toml::Value::Table(table) = toml_doc {
        for (category, category_value) in table {
            let toml::Value::Table(category_table) = category_value else {
                return Err(anyhow::anyhow!("Category '{category}' must be a table"));
            };

            match category.as_str() {
                "component" => {
                    for (name, value) in category_table {
                        let component = parse_component_definition(&name, value)?;
                        components.push(component);
                    }
                }
                "capability" => {
                    for (name, value) in category_table {
                        let capability = parse_capability_definition(&name, value)?;
                        capabilities.push(capability);
                    }
                }
                _ => {
                    return Err(anyhow::anyhow!(
                        "Unknown category '{category}'. Must be 'component' or 'capability'"
                    ));
                }
            }
        }
    } else {
        return Err(anyhow::anyhow!(
            "TOML file must contain a table at root level"
        ));
    }
    Ok((components, capabilities))
}

fn parse_component_definition(name: &str, value: toml::Value) -> Result<ComponentDefinition> {
    let toml::Value::Table(mut def_table) = value else {
        return Err(anyhow::anyhow!(
            "Component definition '{name}' must be a table"
        ));
    };

    if !def_table.contains_key("uri") {
        return Err(anyhow::anyhow!(
            "Component '{name}' missing required 'uri' field"
        ));
    }

    let config = if let Some(toml::Value::Table(config_table)) = def_table.remove("config") {
        convert_toml_table_to_json_map(&config_table)?
    } else {
        HashMap::new()
    };

    let mut component_base: ComponentDefinitionBase = toml::Value::Table(def_table)
        .try_into()
        .map_err(|e| anyhow::anyhow!("Failed to parse component '{name}': {e}"))?;

    component_base.config = config;
    Ok(ComponentDefinition {
        name: name.to_string(),
        base: component_base,
    })
}

fn parse_capability_definition(name: &str, value: toml::Value) -> Result<CapabilityDefinition> {
    let toml::Value::Table(mut def_table) = value else {
        return Err(anyhow::anyhow!(
            "Capability definition '{name}' must be a table"
        ));
    };

    if !def_table.contains_key("uri") {
        return Err(anyhow::anyhow!(
            "Capability '{name}' missing required 'uri' field"
        ));
    }

    let config = if let Some(toml::Value::Table(config_table)) = def_table.remove("config") {
        Some(convert_toml_table_to_json_map(&config_table)?)
    } else {
        None
    };

    let definition_base: DefinitionBase = toml::Value::Table(def_table)
        .try_into()
        .map_err(|e| anyhow::anyhow!("Failed to parse capability '{name}': {e}"))?;

    Ok(CapabilityDefinition {
        name: name.to_string(),
        base: definition_base,
        config: config.unwrap_or_default(),
    })
}

fn create_implicit_component_definitions(
    wasm_files: &[PathBuf],
) -> Result<Vec<ComponentDefinition>> {
    let mut definitions = Vec::new();
    for path in wasm_files {
        let path_str = path.to_string_lossy();
        let name = if path_str.starts_with("oci://") {
            // Extract component name from OCI URI: oci://ghcr.io/modulewise/hello:0.1.0 -> hello
            let oci_ref = path_str.strip_prefix("oci://").unwrap();
            if let Some((pkg_part, _version)) = oci_ref.rsplit_once(':') {
                if let Some(name_part) = pkg_part.rsplit_once('/') {
                    name_part.1.to_string()
                } else {
                    pkg_part.to_string()
                }
            } else {
                return Err(anyhow::anyhow!("Invalid OCI URI format: {path_str}"));
            }
        } else {
            path.file_stem()
                .and_then(|s| s.to_str())
                .ok_or_else(|| {
                    anyhow::anyhow!(
                        "Cannot extract component name from path: {}",
                        path.display()
                    )
                })?
                .to_string()
        };

        let definition = ComponentDefinition {
            name,
            base: ComponentDefinitionBase {
                base: DefinitionBase {
                    uri: path.to_string_lossy().to_string(),
                    scope: default_scope(),
                },
                imports: Vec::new(),
                intercepts: Vec::new(),
                precedence: 0,
                config: HashMap::new(),
            },
        };
        definitions.push(definition);
    }
    Ok(definitions)
}

fn convert_toml_table_to_json_map(
    table: &toml::map::Map<String, toml::Value>,
) -> Result<HashMap<String, serde_json::Value>> {
    let mut map = HashMap::new();
    for (key, value) in table {
        let json_value = convert_toml_value_to_json(value)?;
        map.insert(key.clone(), json_value);
    }
    Ok(map)
}

fn convert_toml_value_to_json(value: &toml::Value) -> Result<serde_json::Value> {
    match value {
        toml::Value::String(s) => Ok(serde_json::Value::String(s.clone())),
        toml::Value::Integer(i) => Ok(serde_json::Value::Number((*i).into())),
        toml::Value::Float(f) => Ok(serde_json::Number::from_f64(*f)
            .map(serde_json::Value::Number)
            .unwrap_or(serde_json::Value::Null)),
        toml::Value::Boolean(b) => Ok(serde_json::Value::Bool(*b)),
        toml::Value::Array(arr) => {
            let json_arr: Result<Vec<_>, _> = arr.iter().map(convert_toml_value_to_json).collect();
            Ok(serde_json::Value::Array(json_arr?))
        }
        toml::Value::Table(table) => {
            let json_map = convert_toml_table_to_json_map(table)?;
            let json_obj: serde_json::Map<String, serde_json::Value> =
                json_map.into_iter().collect();
            Ok(serde_json::Value::Object(json_obj))
        }
        toml::Value::Datetime(dt) => Ok(serde_json::Value::String(dt.to_string())),
    }
}
