use anyhow::Result;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::PathBuf;

use crate::graph::{
    ComponentDefinition, ComponentDefinitionBase, ComponentGraph, DefinitionBase,
    RuntimeFeatureDefinition, default_enables,
};

/// Load component definitions and runtime feature definitions from configuration files
/// and build a component graph
pub fn load_definitions(
    definition_files: &[PathBuf], // .toml and .wasm files
) -> Result<ComponentGraph> {
    let (runtime_feature_definitions, component_definitions) =
        parse_definition_files(definition_files)?;
    ComponentGraph::build(&component_definitions, &runtime_feature_definitions)
}

fn parse_definition_files(
    definition_files: &[PathBuf], // .toml and .wasm files
) -> Result<(Vec<RuntimeFeatureDefinition>, Vec<ComponentDefinition>)> {
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
) -> Result<(Vec<RuntimeFeatureDefinition>, Vec<ComponentDefinition>)> {
    let mut runtime_feature_definitions = Vec::new();
    let mut component_definitions = Vec::new();

    // Parse TOML files to extract both runtime features and components
    for file in toml_files {
        let (runtime_features, components) = parse_toml_file(file)?;
        runtime_feature_definitions.extend(runtime_features);
        component_definitions.extend(components);
    }

    // Add implicit component definitions from standalone .wasm files
    component_definitions.extend(create_implicit_component_definitions(wasm_files)?);

    for def in &runtime_feature_definitions {
        validate_runtime_feature_enables_scope(&def.enables, &def.name)?;
    }
    for def in &component_definitions {
        validate_component_enables_scope(&def.enables)?;
    }

    // Collision detection - ensure unique names across all definitions
    let mut all_names = HashSet::new();
    for def in &runtime_feature_definitions {
        if !all_names.insert(&def.name) {
            return Err(anyhow::anyhow!("Duplicate definition name: '{}'", def.name));
        }
    }
    for def in &component_definitions {
        if !all_names.insert(&def.name) {
            return Err(anyhow::anyhow!("Duplicate definition name: '{}'", def.name));
        }
    }

    // Validate component expectations - different error handling based on exposed flag
    for def in &component_definitions {
        for expected_name in &def.expects {
            if !all_names.contains(expected_name) {
                if def.exposed {
                    continue;
                } else {
                    return Err(anyhow::anyhow!(
                        "Component '{}' expects undefined definition '{}' - server cannot start",
                        def.name,
                        expected_name
                    ));
                }
            }
        }
    }

    Ok((runtime_feature_definitions, component_definitions))
}

fn validate_runtime_feature_enables_scope(enables: &str, name: &str) -> Result<()> {
    match enables {
        "none" | "unexposed" | "exposed" | "any" => Ok(()),
        "package" | "namespace" => Err(anyhow::anyhow!(
            "RuntimeFeature '{name}' cannot use enables='{enables}' - only components support package/namespace scoping"
        )),
        _ => Err(anyhow::anyhow!(
            "Invalid enables scope: '{enables}'. Must be one of: none, unexposed, exposed, any"
        )),
    }
}

fn validate_component_enables_scope(enables: &str) -> Result<()> {
    match enables {
        "none" | "package" | "namespace" | "unexposed" | "exposed" | "any" => Ok(()),
        _ => Err(anyhow::anyhow!(
            "Invalid enables scope: '{enables}'. Must be one of: none, package, namespace, unexposed, exposed, any"
        )),
    }
}

fn parse_toml_file(
    path: &PathBuf,
) -> Result<(Vec<RuntimeFeatureDefinition>, Vec<ComponentDefinition>)> {
    let content = fs::read_to_string(path)?;
    let toml_doc: toml::Value = toml::from_str(&content)?;

    let mut runtime_features = Vec::new();
    let mut components = Vec::new();

    if let toml::Value::Table(table) = toml_doc {
        for (name, value) in table {
            if let toml::Value::Table(def_table) = value {
                // Check if this is a runtime feature (wasmtime:* or host:*) or component
                if let Some(uri) = def_table.get("uri").and_then(|v| v.as_str()) {
                    if uri.starts_with("wasmtime:") || uri.starts_with("host:") {
                        let definition_base: DefinitionBase =
                            toml::Value::Table(def_table).try_into().map_err(|e| {
                                anyhow::anyhow!("Failed to parse runtime feature '{name}': {e}")
                            })?;
                        runtime_features.push(RuntimeFeatureDefinition {
                            name: name.clone(),
                            base: definition_base,
                        });
                    } else {
                        let mut definition_value = def_table.clone();
                        let config = if let Some(toml::Value::Table(config_table)) =
                            definition_value.remove("config")
                        {
                            Some(convert_toml_table_to_json_map(&config_table)?)
                        } else {
                            None
                        };

                        let mut component_base: ComponentDefinitionBase =
                            toml::Value::Table(definition_value)
                                .try_into()
                                .map_err(|e| {
                                    anyhow::anyhow!("Failed to parse component '{name}': {e}")
                                })?;

                        component_base.config = config;
                        components.push(ComponentDefinition {
                            name: name.clone(),
                            base: component_base,
                        });
                    }
                } else {
                    return Err(anyhow::anyhow!(
                        "Definition '{name}' missing required 'uri' field"
                    ));
                }
            } else {
                return Err(anyhow::anyhow!("Definition '{name}' must be a table"));
            }
        }
    } else {
        return Err(anyhow::anyhow!(
            "TOML file must contain a table at root level"
        ));
    }
    Ok((runtime_features, components))
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

        // Implicit components from .wasm files are treated as exposed components
        let definition = ComponentDefinition {
            name,
            base: ComponentDefinitionBase {
                base: DefinitionBase {
                    uri: path.to_string_lossy().to_string(),
                    enables: default_enables(),
                },
                expects: Vec::new(),
                intercepts: Vec::new(),
                precedence: 0,
                exposed: true,
                config: None,
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
