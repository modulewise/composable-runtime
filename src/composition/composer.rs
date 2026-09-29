use anyhow::{Result, bail};
use std::collections::HashMap;
use std::sync::Arc;
use wac_graph::{CompositionGraph, EncodeOptions};
use wac_types::Package;

use crate::types::{Export, Import, Interface, InterfaceName};

/// The interface every generated config component exports.
const CONFIG_STORE: &str = "wasi:config/store@0.2.0-rc.1";

/// A component during the composition phase. Composing two produces another.
#[derive(Debug, Clone)]
pub struct Composable {
    /// Matches named imports. None for a generated component with no TOML.
    pub name: Option<String>,
    pub bytes: Arc<[u8]>,
    /// The imports not yet satisfied.
    pub imports: Vec<Import>,
    pub exports: Vec<Export>,
}

pub struct Composer;

impl Composer {
    /// Compose a component with a wasi:config/store generated from the provided config
    pub fn compose_with_config(
        component: &Composable,
        config: &HashMap<String, serde_json::Value>,
    ) -> Result<Composable> {
        Self::compose_components(component, &config_component(None, config)?)
    }

    /// Compose a socket component with a plug component. The result has the
    /// socket's name and exports, and any remaining unsatisfied imports.
    pub fn compose_components(socket: &Composable, plug: &Composable) -> Result<Composable> {
        let mut graph = CompositionGraph::new();

        let socket_pkg =
            Package::from_bytes("socket", None, socket.bytes.to_vec(), graph.types_mut())?;
        let plug_pkg = Package::from_bytes("plug", None, plug.bytes.to_vec(), graph.types_mut())?;

        let socket_id = graph.register_package(socket_pkg)?;
        let plug_id = graph.register_package(plug_pkg)?;

        let socket_instance = graph.instantiate(socket_id);
        let mut plug_instance = None;
        let mut imports = Vec::new();
        for import in &socket.imports {
            let Some(export) = select_export(import, plug.name.as_deref(), &plug.exports) else {
                imports.push(import.clone());
                continue;
            };
            let instance = *plug_instance.get_or_insert_with(|| graph.instantiate(plug_id));
            let argument = graph.alias_instance_export(instance, &export.name)?;
            graph.set_instantiation_argument(socket_instance, &import.name, argument)?;
        }
        if plug_instance.is_none() {
            bail!("no import is satisfied by the plug's exports");
        }

        let socket_exports: Vec<String> = graph.types()[graph[socket_id].ty()]
            .exports
            .keys()
            .cloned()
            .collect();
        for name in socket_exports {
            let export = graph.alias_instance_export(socket_instance, &name)?;
            graph.export(export, &name)?;
        }

        let encode_options = EncodeOptions {
            define_components: true,
            ..Default::default()
        };

        let bytes = graph
            .encode(encode_options)
            .map_err(|e| anyhow::anyhow!("Failed to encode composition: {e}"))?;

        Ok(Composable {
            name: socket.name.clone(),
            bytes: Arc::from(bytes),
            imports,
            exports: socket.exports.clone(),
        })
    }
}

// A generated wasi:config/store component with the given name and values.
// Empty config will create an empty wasi:config/store component.
fn config_component(
    name: Option<String>,
    config: &HashMap<String, serde_json::Value>,
) -> Result<Composable> {
    let export = Export {
        name: CONFIG_STORE.to_string(),
        interface: Some(Interface {
            name: Some(InterfaceName::parse(CONFIG_STORE)?),
        }),
    };
    Ok(Composable {
        name,
        bytes: Arc::from(create_config_component(config)?),
        imports: Vec::new(),
        exports: vec![export],
    })
}

/// The export, of the component named `provider`, that satisfies `import`. A
/// named import is satisfied by the named export of the same name or, if there
/// is none, by the component of the same name through its unnamed export of the
/// interface. Interface versions match on the same semver track. Inline
/// interfaces and functions match by name.
pub(crate) fn select_export<'a>(
    import: &Import,
    provider: Option<&str>,
    exports: &'a [Export],
) -> Option<&'a Export> {
    let Some(imported) = import.interface_name() else {
        // A function or an inline interface.
        return exports.iter().find(|export| {
            export.name == import.name
                && export.interface_name().is_none()
                && export.interface.is_some() == import.interface.is_some()
        });
    };
    let implements = |export: &&Export| {
        export.interface_name().is_some_and(|exported| {
            wac_graph::types::are_semver_compatible(imported.as_str(), exported.as_str())
        })
    };
    if !import.is_named() {
        return exports
            .iter()
            .filter(implements)
            .find(|export| !export.is_named());
    }
    exports
        .iter()
        .filter(implements)
        .find(|export| export.is_named() && export.name == import.name)
        .or_else(|| {
            if provider != Some(import.name.as_str()) {
                return None;
            }
            exports
                .iter()
                .filter(implements)
                .find(|export| !export.is_named())
        })
}

// Generate a wasi:config/store component from key/value configuration
fn create_config_component(config: &HashMap<String, serde_json::Value>) -> Result<Vec<u8>> {
    let mut config_properties = Vec::new();
    for (key, value) in config {
        flatten_config(key, value, &mut config_properties)?;
    }
    static_config::create_component(config_properties)
        .map_err(|e| anyhow::anyhow!("Failed to create config component: {e}"))
}

/// Recursively flatten nested JSON objects into dot-delimited keys.
fn flatten_config(
    prefix: &str,
    value: &serde_json::Value,
    out: &mut Vec<(String, String)>,
) -> Result<()> {
    match value {
        serde_json::Value::Object(map) => {
            for (k, v) in map {
                let key = format!("{prefix}.{k}");
                flatten_config(&key, v, out)?;
            }
            Ok(())
        }
        other => {
            out.push((prefix.to_string(), convert_json_value_to_string(other)?));
            Ok(())
        }
    }
}

fn convert_json_value_to_string(value: &serde_json::Value) -> Result<String> {
    match value {
        serde_json::Value::String(s) => Ok(s.clone()),
        serde_json::Value::Number(n) => Ok(n.to_string()),
        serde_json::Value::Bool(b) => Ok(b.to_string()),
        serde_json::Value::Array(arr) => {
            // Convert array to comma-separated string
            let string_items: Result<Vec<String>, _> =
                arr.iter().map(convert_json_value_to_string).collect();
            Ok(string_items?.join(","))
        }
        serde_json::Value::Object(_) => Err(anyhow::anyhow!(
            "Nested objects should be handled by flatten_config"
        )),
        serde_json::Value::Null => Err(anyhow::anyhow!("Null values not supported in config")),
    }
}
