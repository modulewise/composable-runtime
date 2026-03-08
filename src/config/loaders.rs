use anyhow::Result;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use super::types::{DefinitionLoader, GenericDefinition, PropertyMap};

/// Loads definitions from TOML files.
/// Each `[category.name]` table becomes a GenericDefinition.
pub struct TomlLoader {
    paths: Vec<PathBuf>,
}

impl TomlLoader {
    pub fn new() -> Self {
        Self { paths: Vec::new() }
    }
}

impl DefinitionLoader for TomlLoader {
    fn claim(&mut self, path: &Path) -> bool {
        if path.extension().and_then(|s| s.to_str()) == Some("toml") {
            self.paths.push(path.to_path_buf());
            true
        } else {
            false
        }
    }

    fn load(&self) -> Result<Vec<GenericDefinition>> {
        let mut definitions = Vec::new();
        for path in &self.paths {
            definitions.extend(load_toml_file(path)?);
        }
        Ok(definitions)
    }
}

fn load_toml_file(path: &Path) -> Result<Vec<GenericDefinition>> {
    let content = std::fs::read_to_string(path)?;
    let toml_doc: toml::Value = toml::from_str(&content)?;

    let mut definitions = Vec::new();

    let toml::Value::Table(table) = toml_doc else {
        return Err(anyhow::anyhow!(
            "TOML file must contain a table at root level"
        ));
    };

    for (category, category_value) in table {
        let toml::Value::Table(category_table) = category_value else {
            return Err(anyhow::anyhow!("Category '{category}' must be a table"));
        };

        for (name, value) in category_table {
            let toml::Value::Table(def_table) = value else {
                return Err(anyhow::anyhow!(
                    "Definition '{name}' in category '{category}' must be a table"
                ));
            };

            let properties = convert_toml_table_to_property_map(&def_table)?;
            definitions.push(GenericDefinition {
                category: category.clone(),
                name,
                properties,
            });
        }
    }

    Ok(definitions)
}

/// Loads definitions from .wasm file paths and OCI URIs.
pub struct WasmLoader {
    paths: Vec<PathBuf>,
}

impl WasmLoader {
    pub fn new() -> Self {
        Self { paths: Vec::new() }
    }
}

impl DefinitionLoader for WasmLoader {
    fn claim(&mut self, path: &Path) -> bool {
        let path_str = path.to_string_lossy();
        if path_str.starts_with("oci://")
            || path.extension().and_then(|s| s.to_str()) == Some("wasm")
        {
            self.paths.push(path.to_path_buf());
            true
        } else {
            false
        }
    }

    fn load(&self) -> Result<Vec<GenericDefinition>> {
        let mut definitions = Vec::new();
        for path in &self.paths {
            let path_str = path.to_string_lossy();
            let name = extract_component_name(&path_str, path)?;
            let mut properties = PropertyMap::new();
            properties.insert(
                "uri".to_string(),
                serde_json::Value::String(path_str.to_string()),
            );
            definitions.push(GenericDefinition {
                category: "component".to_string(),
                name,
                properties,
            });
        }
        Ok(definitions)
    }
}

fn extract_component_name(path_str: &str, path: &Path) -> Result<String> {
    if path_str.starts_with("oci://") {
        let oci_ref = path_str.strip_prefix("oci://").unwrap();
        if let Some((pkg_part, _version)) = oci_ref.rsplit_once(':') {
            if let Some((_prefix, name)) = pkg_part.rsplit_once('/') {
                Ok(name.to_string())
            } else {
                Ok(pkg_part.to_string())
            }
        } else {
            Err(anyhow::anyhow!("Invalid OCI URI format: {path_str}"))
        }
    } else {
        path.file_stem()
            .and_then(|s| s.to_str())
            .map(|s| s.to_string())
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "Cannot extract component name from path: {}",
                    path.display()
                )
            })
    }
}

fn convert_toml_table_to_property_map(
    table: &toml::map::Map<String, toml::Value>,
) -> Result<PropertyMap> {
    let mut map = HashMap::new();
    for (key, value) in table {
        map.insert(key.clone(), convert_toml_value_to_json(value)?);
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
            let map = convert_toml_table_to_property_map(table)?;
            let obj: serde_json::Map<String, serde_json::Value> = map.into_iter().collect();
            Ok(serde_json::Value::Object(obj))
        }
        toml::Value::Datetime(dt) => Ok(serde_json::Value::String(dt.to_string())),
    }
}
