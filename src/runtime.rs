use anyhow::Result;
use serde::de::DeserializeOwned;
use std::collections::HashMap;
use std::collections::hash_map::Entry;
use wasmtime::{
    Cache, Config, Engine, Store,
    component::{Component as WasmComponent, Linker, Type, Val},
};
use wasmtime_wasi::random::{WasiRandom, WasiRandomView};
use wasmtime_wasi::{ResourceTable, WasiCtxBuilder, WasiCtxView, WasiView};
use wasmtime_wasi_http::{WasiHttpCtx, WasiHttpView};
use wasmtime_wasi_io::IoView;

use crate::graph::ComponentGraph;
use crate::registry::{
    ComponentRegistry, HostExtension, HostExtensionFactory, RuntimeFeatureRegistry,
    build_registries,
};
use crate::types::ComponentState;
use crate::wit::Function;

/// Wasm Component whose functions can be invoked
#[derive(Debug, Clone)]
pub struct Component {
    pub name: String,
    pub functions: HashMap<String, Function>,
}

/// Composable Runtime for invoking Wasm Components
#[derive(Clone)]
pub struct Runtime {
    invoker: Invoker,
    component_registry: ComponentRegistry,
    runtime_feature_registry: RuntimeFeatureRegistry,
}

impl Runtime {
    /// Create a RuntimeBuilder from a ComponentGraph
    pub fn builder(graph: &ComponentGraph) -> RuntimeBuilder<'_> {
        RuntimeBuilder::new(graph)
    }

    /// List all exposed components
    pub fn list_components(&self) -> Vec<Component> {
        self.component_registry
            .get_components()
            .map(|spec| Component {
                name: spec.name.clone(),
                functions: spec.functions.clone().unwrap_or_default(),
            })
            .collect()
    }

    /// Get a specific component by name
    pub fn get_component(&self, name: &str) -> Option<Component> {
        self.component_registry
            .get_component(name)
            .map(|spec| Component {
                name: spec.name.clone(),
                functions: spec.functions.clone().unwrap_or_default(),
            })
    }

    /// Invoke a component function
    pub async fn invoke(
        &self,
        component_name: &str,
        function_name: &str,
        args: Vec<serde_json::Value>,
    ) -> Result<serde_json::Value> {
        self.invoke_with_env(component_name, function_name, args, &[])
            .await
    }

    /// Invoke a component function with environment variables
    pub async fn invoke_with_env(
        &self,
        component_name: &str,
        function_name: &str,
        args: Vec<serde_json::Value>,
        env_vars: &[(&str, &str)],
    ) -> Result<serde_json::Value> {
        let spec = self
            .component_registry
            .get_component(component_name)
            .ok_or_else(|| anyhow::anyhow!("Component '{component_name}' not found"))?;

        let function = spec
            .functions
            .as_ref()
            .and_then(|funcs| funcs.get(function_name))
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "Function '{function_name}' not found in component '{component_name}'"
                )
            })?;

        self.invoker
            .invoke(
                &spec.bytes,
                &spec.runtime_features,
                &self.runtime_feature_registry,
                function.clone(),
                args,
                env_vars,
            )
            .await
    }

    /// Instantiate a component
    pub async fn instantiate(
        &self,
        component_name: &str,
    ) -> Result<(Store<ComponentState>, wasmtime::component::Instance)> {
        self.instantiate_with_env(component_name, &[]).await
    }

    /// Instantiate a component with environment variables
    pub async fn instantiate_with_env(
        &self,
        component_name: &str,
        env_vars: &[(&str, &str)],
    ) -> Result<(Store<ComponentState>, wasmtime::component::Instance)> {
        let spec = self
            .component_registry
            .get_component(component_name)
            .ok_or_else(|| anyhow::anyhow!("Component '{component_name}' not found"))?;

        self.invoker
            .instantiate_from_bytes(
                &spec.bytes,
                &spec.runtime_features,
                &self.runtime_feature_registry,
                env_vars,
            )
            .await
    }
}

/// Builder for configuring and creating a Runtime
pub struct RuntimeBuilder<'a> {
    graph: &'a ComponentGraph,
    factories: HashMap<&'static str, HostExtensionFactory>,
}

impl<'a> RuntimeBuilder<'a> {
    fn new(graph: &'a ComponentGraph) -> Self {
        Self {
            graph,
            factories: HashMap::new(),
        }
    }

    /// Register a host extension type for the given name.
    ///
    /// The name corresponds to the suffix in `uri = "host:name"` in TOML.
    ///
    /// If the TOML block has an empty config and deserialization fails,
    /// falls back to `Default::default()`.
    pub fn with_host_extension<T>(mut self, name: &'static str) -> Self
    where
        T: HostExtension + DeserializeOwned + Default + 'static,
    {
        self.factories.insert(
            name,
            Box::new(
                |config: serde_json::Value| -> Result<Box<dyn HostExtension>> {
                    match serde_json::from_value::<T>(config.clone()) {
                        Ok(instance) => Ok(Box::new(instance)),
                        Err(e) => {
                            if config == serde_json::json!({}) {
                                Ok(Box::new(T::default()))
                            } else {
                                Err(e.into())
                            }
                        }
                    }
                },
            ),
        );
        self
    }

    /// Build the Runtime
    pub async fn build(self) -> Result<Runtime> {
        let (component_registry, runtime_feature_registry) =
            build_registries(self.graph, self.factories).await?;
        let invoker = Invoker::new()?;
        Ok(Runtime {
            invoker,
            component_registry,
            runtime_feature_registry,
        })
    }
}

impl IoView for ComponentState {
    fn table(&mut self) -> &mut ResourceTable {
        &mut self.resource_table
    }
}

impl WasiView for ComponentState {
    fn ctx(&mut self) -> WasiCtxView<'_> {
        WasiCtxView {
            ctx: &mut self.wasi_ctx,
            table: &mut self.resource_table,
        }
    }
}

impl WasiHttpView for ComponentState {
    fn ctx(&mut self) -> &mut WasiHttpCtx {
        self.wasi_http_ctx
            .as_mut()
            .expect("Component requires 'http' feature, so HTTP context should be available")
    }

    fn table(&mut self) -> &mut ResourceTable {
        &mut self.resource_table
    }
}

#[derive(Clone)]
struct Invoker {
    engine: Engine,
}

impl Invoker {
    pub fn new() -> Result<Self> {
        let mut config = Config::new();
        config.cache(Some(Cache::from_file(None)?));
        config.parallel_compilation(true);
        config.async_support(true);
        config.wasm_component_model_async(true);
        config.memory_init_cow(true);
        let engine = Engine::new(&config)?;
        Ok(Self { engine })
    }

    fn create_linker(
        &self,
        runtime_features: &[String],
        runtime_feature_registry: &RuntimeFeatureRegistry,
    ) -> Result<Linker<ComponentState>> {
        let mut linker = Linker::new(&self.engine);

        // Multiple runtime features may provide the same interface
        linker.allow_shadowing(true);

        // Add WASI interfaces based on explicitly requested runtime features
        for feature_name in runtime_features {
            if let Some(runtime_feature) =
                runtime_feature_registry.get_runtime_feature(feature_name)
            {
                if let Some(wasmtime_feature) = runtime_feature.uri.strip_prefix("wasmtime:") {
                    match wasmtime_feature {
                        "wasip2" => {
                            // Comprehensive WASI Preview 2 support
                            wasmtime_wasi::p2::add_to_linker_async(&mut linker)?;
                        }
                        "http" => {
                            wasmtime_wasi_http::add_only_http_to_linker_async(&mut linker)?;
                        }
                        "io" => {
                            wasmtime_wasi_io::add_to_linker_async(&mut linker)?;
                        }
                        "random" => {
                            wasmtime_wasi::p2::bindings::random::random::add_to_linker::<
                                ComponentState,
                                WasiRandom,
                            >(&mut linker, |state| {
                                <ComponentState as WasiRandomView>::random(state)
                            })?;
                            wasmtime_wasi::p2::bindings::random::insecure_seed::add_to_linker::<
                                ComponentState,
                                WasiRandom,
                            >(&mut linker, |state| {
                                <ComponentState as WasiRandomView>::random(state)
                            })?;
                        }
                        "inherit-stdio" | "inherit-network" | "allow-ip-name-lookup" => {
                            // These runtime features are handled in WASI context, not linker
                            // No linker functions to add, only context configuration
                        }
                        _ => {
                            tracing::warn!(
                                "Unknown wasmtime feature for linker: {}",
                                runtime_feature.uri
                            );
                        }
                    }
                } else if runtime_feature.uri.starts_with("host:") {
                    if let Some(ext) = &runtime_feature.extension {
                        ext.link(&mut linker)?;
                    } else {
                        return Err(anyhow::anyhow!(
                            "Host feature '{}' requested but no extension registered",
                            feature_name
                        ));
                    }
                }
            }
            // Component runtime features are handled during composition, not at runtime
        }
        Ok(linker)
    }

    async fn instantiate_from_bytes(
        &self,
        bytes: &[u8],
        runtime_features: &[String],
        runtime_feature_registry: &RuntimeFeatureRegistry,
        env_vars: &[(&str, &str)],
    ) -> Result<(Store<ComponentState>, wasmtime::component::Instance)> {
        let component_bytes = bytes.to_vec();
        let linker = self.create_linker(runtime_features, runtime_feature_registry)?;

        // Build WASI context based on runtime features
        let mut wasi_builder = WasiCtxBuilder::new();

        if !env_vars.is_empty() {
            wasi_builder.envs(env_vars);
        }

        for feature_name in runtime_features {
            if let Some(runtime_feature) =
                runtime_feature_registry.get_runtime_feature(feature_name)
                && let Some(wasmtime_feature) = runtime_feature.uri.strip_prefix("wasmtime:")
            {
                match wasmtime_feature {
                    "inherit-stdio" => {
                        wasi_builder.inherit_stdio();
                    }
                    "inherit-network" => {
                        wasi_builder.inherit_network();
                    }
                    "allow-ip-name-lookup" => {
                        wasi_builder.allow_ip_name_lookup(true);
                    }
                    _ => {}
                }
            }
        }

        // Check if HTTP context needed
        let needs_http = runtime_features.iter().any(|feature_name| {
            runtime_feature_registry
                .get_runtime_feature(feature_name)
                .and_then(|cap| cap.uri.strip_prefix("wasmtime:"))
                == Some("http")
        });

        // Collect extension states before creating ComponentState
        let mut extensions = HashMap::new();
        for feature_name in runtime_features {
            if let Some(runtime_feature) =
                runtime_feature_registry.get_runtime_feature(feature_name)
                && runtime_feature.uri.starts_with("host:")
                && let Some(ext) = &runtime_feature.extension
                && let Some((type_id, boxed_state)) = ext.create_state_boxed()?
            {
                match extensions.entry(type_id) {
                    Entry::Vacant(e) => {
                        e.insert(boxed_state);
                    }
                    Entry::Occupied(_) => {
                        anyhow::bail!(
                            "Duplicate extension state type for feature '{feature_name}'"
                        );
                    }
                }
            }
        }

        let state = ComponentState {
            wasi_ctx: wasi_builder.build(),
            wasi_http_ctx: if needs_http {
                Some(WasiHttpCtx::new())
            } else {
                None
            },
            resource_table: ResourceTable::new(),
            extensions,
        };

        let mut store = Store::new(&self.engine, state);
        let component = WasmComponent::from_binary(&self.engine, &component_bytes)?;
        let instance = linker.instantiate_async(&mut store, &component).await?;

        Ok((store, instance))
    }

    pub async fn invoke(
        &self,
        bytes: &[u8],
        runtime_features: &[String],
        runtime_feature_registry: &RuntimeFeatureRegistry,
        function: Function,
        args: Vec<serde_json::Value>,
        env_vars: &[(&str, &str)],
    ) -> Result<serde_json::Value> {
        let function_name = function.function_name();

        let (mut store, instance) = self
            .instantiate_from_bytes(bytes, runtime_features, runtime_feature_registry, env_vars)
            .await?;

        // Look up the function - either within an interface or as a direct export
        let func_export = if let Some(interface) = function.interface() {
            let interface_str = interface.as_str();
            let interface_export = instance
                .get_export(&mut store, None, interface_str)
                .ok_or_else(|| anyhow::anyhow!("Interface '{interface_str}' not found"))?;
            let parent_export_idx = Some(&interface_export.1);
            instance
                .get_export(&mut store, parent_export_idx, function_name)
                .ok_or_else(|| {
                    anyhow::anyhow!(
                        "Function '{function_name}' not found in interface '{interface_str}'"
                    )
                })?
        } else {
            instance
                .get_export(&mut store, None, function_name)
                .ok_or_else(|| {
                    anyhow::anyhow!("Function '{function_name}' not found in component exports")
                })?
        };
        let func = instance
            .get_func(&mut store, func_export.1)
            .ok_or_else(|| anyhow::anyhow!("Function handle invalid for '{function_name}'"))?;

        let mut arg_vals: Vec<Val> = vec![];
        let func_ty = func.ty(&store);
        let params: Vec<_> = func_ty.params().collect();
        if args.len() != params.len() {
            return Err(anyhow::anyhow!(
                "Wrong number of args: expected {}, got {}",
                params.len(),
                args.len()
            ));
        }
        for (index, json_arg) in args.iter().enumerate() {
            let param_type = &params[index].1;
            let val = json_to_val(json_arg, param_type)
                .map_err(|e| anyhow::anyhow!("Error converting parameter {index}: {e}"))?;
            arg_vals.push(val);
        }

        let num_results = func_ty.results().len();
        let mut results = vec![Val::Bool(false); num_results];

        func.call_async(&mut store, &arg_vals, &mut results).await?;

        // Handle results according to WIT function signature
        match results.len() {
            0 => Ok(serde_json::Value::Null),
            1 => {
                let value = &results[0];
                match value {
                    Val::Result(Err(Some(error_val))) => {
                        let error_json = val_to_json(error_val);
                        Err(anyhow::anyhow!("Component returned error: {error_json}"))
                    }
                    Val::Result(Err(None)) => Err(anyhow::anyhow!("Component returned error")),
                    _ => Ok(val_to_json(value)),
                }
            }
            _ => {
                // Multiple wasmtime results - reconstruct WIT tuple/record structure
                Self::reconstruct_wit_return(&results, &function)
            }
        }
    }

    // This handles the case where wasmtime decomposes tuples/records into separate Val objects
    fn reconstruct_wit_return(results: &[Val], function: &Function) -> Result<serde_json::Value> {
        // Check if this is a record that needs field mapping to reconstruct as an object
        if let Some(return_schema) = function.result()
            && let Some(schema_obj) = return_schema.as_object()
            && schema_obj.get("type").and_then(|t| t.as_str()) == Some("object")
            && schema_obj.contains_key("properties")
        {
            return Self::reconstruct_record(results, schema_obj);
        }

        // All other cases (tuples, unknown schemas, malformed schemas) -> array
        let json_results: Vec<serde_json::Value> = results.iter().map(val_to_json).collect();
        Ok(serde_json::Value::Array(json_results))
    }

    // Reconstruct a WIT record from multiple wasmtime results
    fn reconstruct_record(
        results: &[Val],
        schema_obj: &serde_json::Map<String, serde_json::Value>,
    ) -> Result<serde_json::Value> {
        let properties = schema_obj
            .get("properties")
            .and_then(|p| p.as_object())
            .ok_or_else(|| anyhow::anyhow!("Record schema missing properties"))?;

        let mut record = serde_json::Map::new();
        let field_names: Vec<&String> = properties.keys().collect();

        if results.len() != field_names.len() {
            return Err(anyhow::anyhow!(
                "Mismatch between wasmtime results ({}) and record fields ({})",
                results.len(),
                field_names.len()
            ));
        }

        for (i, field_name) in field_names.iter().enumerate() {
            record.insert(field_name.to_string(), val_to_json(&results[i]));
        }

        Ok(serde_json::Value::Object(record))
    }
}

fn json_to_val(json_value: &serde_json::Value, val_type: &Type) -> Result<Val> {
    match (json_value, val_type) {
        // Direct JSON type mappings
        (serde_json::Value::Bool(b), wasmtime::component::Type::Bool) => Ok(Val::Bool(*b)),
        (serde_json::Value::String(s), wasmtime::component::Type::String) => {
            Ok(Val::String(s.clone()))
        }
        (serde_json::Value::String(s), wasmtime::component::Type::Char) => {
            let chars: Vec<char> = s.chars().collect();
            if chars.len() == 1 {
                Ok(Val::Char(chars[0]))
            } else {
                Err(anyhow::anyhow!("Expected single character, got: {s}"))
            }
        }

        // Number types - JSON number maps to all WIT numeric types
        (serde_json::Value::Number(n), wasmtime::component::Type::U8) => {
            let val = n
                .as_u64()
                .ok_or_else(|| anyhow::anyhow!("Invalid number for u8: {n}"))?
                as u8;
            Ok(Val::U8(val))
        }
        (serde_json::Value::Number(n), wasmtime::component::Type::U16) => {
            let val = n
                .as_u64()
                .ok_or_else(|| anyhow::anyhow!("Invalid number for u16: {n}"))?
                as u16;
            Ok(Val::U16(val))
        }
        (serde_json::Value::Number(n), wasmtime::component::Type::U32) => {
            let val = n
                .as_u64()
                .ok_or_else(|| anyhow::anyhow!("Invalid number for u32: {n}"))?
                as u32;
            Ok(Val::U32(val))
        }
        (serde_json::Value::Number(n), wasmtime::component::Type::U64) => {
            let val = n
                .as_u64()
                .ok_or_else(|| anyhow::anyhow!("Invalid number for u64: {n}"))?;
            Ok(Val::U64(val))
        }
        (serde_json::Value::Number(n), wasmtime::component::Type::S8) => {
            let val = n
                .as_i64()
                .ok_or_else(|| anyhow::anyhow!("Invalid number for s8: {n}"))?
                as i8;
            Ok(Val::S8(val))
        }
        (serde_json::Value::Number(n), wasmtime::component::Type::S16) => {
            let val = n
                .as_i64()
                .ok_or_else(|| anyhow::anyhow!("Invalid number for s16: {n}"))?
                as i16;
            Ok(Val::S16(val))
        }
        (serde_json::Value::Number(n), wasmtime::component::Type::S32) => {
            let val = n
                .as_i64()
                .ok_or_else(|| anyhow::anyhow!("Invalid number for s32: {n}"))?
                as i32;
            Ok(Val::S32(val))
        }
        (serde_json::Value::Number(n), wasmtime::component::Type::S64) => {
            let val = n
                .as_i64()
                .ok_or_else(|| anyhow::anyhow!("Invalid number for s64: {n}"))?;
            Ok(Val::S64(val))
        }
        (serde_json::Value::Number(n), wasmtime::component::Type::Float32) => {
            let val = n
                .as_f64()
                .ok_or_else(|| anyhow::anyhow!("Invalid number for f32: {n}"))?
                as f32;
            Ok(Val::Float32(val))
        }
        (serde_json::Value::Number(n), wasmtime::component::Type::Float64) => {
            let val = n
                .as_f64()
                .ok_or_else(|| anyhow::anyhow!("Invalid number for f64: {n}"))?;
            Ok(Val::Float64(val))
        }

        // Arrays map to lists
        (serde_json::Value::Array(arr), wasmtime::component::Type::List(list_type)) => {
            let element_type = list_type.ty();
            let mut items = Vec::new();
            for (index, item) in arr.iter().enumerate() {
                items.push(json_to_val(item, &element_type).map_err(|e| {
                    anyhow::anyhow!("Error converting list item at index {index}: {e}")
                })?);
            }
            Ok(Val::List(items))
        }

        // Arrays map to tuples
        (serde_json::Value::Array(arr), wasmtime::component::Type::Tuple(tuple_type)) => {
            let tuple_types: Vec<_> = tuple_type.types().collect();
            if arr.len() != tuple_types.len() {
                return Err(anyhow::anyhow!(
                    "Tuple length mismatch: expected {}, got {}",
                    tuple_types.len(),
                    arr.len()
                ));
            }
            let mut items = Vec::new();
            for (index, (item, item_type)) in arr.iter().zip(tuple_types.iter()).enumerate() {
                items.push(json_to_val(item, item_type).map_err(|e| {
                    anyhow::anyhow!("Error converting tuple item at index {index}: {e}")
                })?);
            }
            Ok(Val::Tuple(items))
        }

        // Objects map to records
        (serde_json::Value::Object(obj), wasmtime::component::Type::Record(record_type)) => {
            let mut fields = Vec::new();
            for field in record_type.fields() {
                let field_name = field.name.to_string();
                let field_type = &field.ty;

                if let Some(json_value) = obj.get(&field_name) {
                    let field_val = json_to_val(json_value, field_type)?;
                    fields.push((field_name, field_val));
                } else {
                    // Check if field is optional
                    match field_type {
                        wasmtime::component::Type::Option(_) => {
                            fields.push((field_name, Val::Option(None)));
                        }
                        _ => {
                            return Err(anyhow::anyhow!(
                                "Missing required field '{field_name}' in record"
                            ));
                        }
                    }
                }
            }

            // Check for extra fields that aren't in the WIT record
            for (key, _) in obj {
                if !record_type.fields().any(|field| field.name == key) {
                    return Err(anyhow::anyhow!("Unexpected field '{key}' in record"));
                }
            }

            Ok(Val::Record(fields))
        }

        // Handle null for options
        (serde_json::Value::Null, wasmtime::component::Type::Option(_)) => Ok(Val::Option(None)),

        // Handle non-null values for options
        (json_val, wasmtime::component::Type::Option(option_type)) => {
            let inner_type = option_type.ty();
            let inner_val = json_to_val(json_val, &inner_type)?;
            Ok(Val::Option(Some(Box::new(inner_val))))
        }

        // Type mismatches
        _ => Err(anyhow::anyhow!(
            "Type mismatch: cannot convert JSON {json_value:?} to WIT type {val_type:?}"
        )),
    }
}

fn val_to_json(val: &Val) -> serde_json::Value {
    match val {
        // Direct mappings
        Val::Bool(b) => serde_json::Value::Bool(*b),
        Val::String(s) => serde_json::Value::String(s.clone()),
        Val::Char(c) => serde_json::Value::String(c.to_string()),

        // All numbers become JSON numbers
        Val::U8(n) => serde_json::Value::Number((*n as u64).into()),
        Val::U16(n) => serde_json::Value::Number((*n as u64).into()),
        Val::U32(n) => serde_json::Value::Number((*n as u64).into()),
        Val::U64(n) => serde_json::Value::Number((*n).into()),
        Val::S8(n) => serde_json::Value::Number((*n as i64).into()),
        Val::S16(n) => serde_json::Value::Number((*n as i64).into()),
        Val::S32(n) => serde_json::Value::Number((*n as i64).into()),
        Val::S64(n) => serde_json::Value::Number((*n).into()),
        Val::Float32(n) => serde_json::Number::from_f64(*n as f64)
            .map(serde_json::Value::Number)
            .unwrap_or(serde_json::Value::Null),
        Val::Float64(n) => serde_json::Number::from_f64(*n)
            .map(serde_json::Value::Number)
            .unwrap_or(serde_json::Value::Null),

        // Collections
        Val::List(items) => {
            let json_items: Vec<serde_json::Value> = items.iter().map(val_to_json).collect();
            serde_json::Value::Array(json_items)
        }

        Val::Record(fields) => {
            let mut obj = serde_json::Map::new();
            for (name, val) in fields {
                obj.insert(name.clone(), val_to_json(val));
            }
            serde_json::Value::Object(obj)
        }

        // Options
        Val::Option(opt) => match opt {
            Some(val) => val_to_json(val),
            None => serde_json::Value::Null,
        },

        Val::Tuple(vals) => {
            let json_items: Vec<serde_json::Value> = vals.iter().map(val_to_json).collect();
            serde_json::Value::Array(json_items)
        }

        Val::Variant(name, val) => {
            let mut obj = serde_json::Map::new();
            obj.insert("type".to_string(), serde_json::Value::String(name.clone()));
            if let Some(v) = val {
                match val_to_json(v) {
                    serde_json::Value::Object(payload_obj) => {
                        for (k, v) in payload_obj {
                            obj.insert(k, v);
                        }
                    }
                    other => {
                        // If payload is not an object (primitive, array, etc.),
                        // fall back to "value" key to maintain valid JSON
                        obj.insert("value".to_string(), other);
                    }
                }
            }
            serde_json::Value::Object(obj)
        }

        Val::Enum(variant) => serde_json::Value::String(variant.clone()),

        Val::Flags(items) => {
            let json_items: Vec<serde_json::Value> = items
                .iter()
                .map(|s| serde_json::Value::String(s.clone()))
                .collect();
            serde_json::Value::Array(json_items)
        }

        Val::Result(result) => {
            let mut obj = serde_json::Map::new();
            match result {
                Ok(Some(v)) => {
                    obj.insert("ok".to_string(), val_to_json(v));
                }
                Ok(None) => {
                    obj.insert("ok".to_string(), serde_json::Value::Null);
                }
                Err(Some(v)) => {
                    obj.insert("error".to_string(), val_to_json(v));
                }
                Err(None) => {
                    obj.insert("error".to_string(), serde_json::Value::Null);
                }
            }
            serde_json::Value::Object(obj)
        }

        Val::Resource(resource_any) => {
            unreachable!(
                "Resource types should be caught by validation: {:?}",
                resource_any
            )
        }

        Val::Future(future_any) => {
            unreachable!(
                "Future types should be caught by validation: {:?}",
                future_any
            )
        }

        Val::Stream(stream_any) => {
            unreachable!(
                "Stream types should be caught by validation: {:?}",
                stream_any
            )
        }

        Val::ErrorContext(error_context_any) => {
            unreachable!(
                "ErrorContext types should be caught by validation: {:?}",
                error_context_any
            )
        }
    }
}
