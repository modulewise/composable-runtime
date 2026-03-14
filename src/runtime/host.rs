use anyhow::Result;
use std::collections::HashMap;
use std::collections::hash_map::Entry;
use wasmtime::{
    Cache, Config, Engine, Store,
    component::{Component as WasmComponent, Linker, Type, Val},
};
use wasmtime_wasi::cli::{WasiCli, WasiCliView};
use wasmtime_wasi::clocks::{WasiClocks, WasiClocksView};
use wasmtime_wasi::random::{WasiRandom, WasiRandomView};
use wasmtime_wasi::sockets::{WasiSockets, WasiSocketsView};
use wasmtime_wasi::{ResourceTable, WasiCtxBuilder, WasiCtxView, WasiView};
use wasmtime_wasi_http::bindings::http::types::ErrorCode;
use wasmtime_wasi_http::body::HyperOutgoingBody;
use wasmtime_wasi_http::types::{
    HostFutureIncomingResponse, OutgoingRequestConfig, default_send_request,
};
use wasmtime_wasi_http::{HttpResult, WasiHttpCtx, WasiHttpView};
use wasmtime_wasi_io::IoView;

use crate::composition::registry::{CapabilityRegistry, ComponentRegistry};
use crate::types::{Component, ComponentInvoker, ComponentState, Function};

// Component host: wasmtime engine + registries, provides instantiation + invocation.
#[derive(Clone)]
pub(crate) struct ComponentHost {
    invoker: Invoker,
    pub(crate) component_registry: ComponentRegistry,
    pub(crate) capability_registry: CapabilityRegistry,
}

impl ComponentHost {
    pub(crate) fn new(
        component_registry: ComponentRegistry,
        capability_registry: CapabilityRegistry,
    ) -> Result<Self> {
        let invoker = Invoker::new()?;
        Ok(Self {
            invoker,
            component_registry,
            capability_registry,
        })
    }

    pub(crate) async fn invoke(
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

        let function = spec.functions.get(function_name).ok_or_else(|| {
            anyhow::anyhow!("Function '{function_name}' not found in component '{component_name}'")
        })?;

        self.invoker
            .invoke(
                &spec.bytes,
                &spec.capabilities,
                &self.capability_registry,
                function.clone(),
                args,
                env_vars,
            )
            .await
    }

    pub(crate) async fn instantiate(
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
                &spec.capabilities,
                &self.capability_registry,
                env_vars,
            )
            .await
    }
}

impl ComponentInvoker for ComponentHost {
    fn get_component(&self, name: &str) -> Option<Component> {
        self.component_registry
            .get_component(name)
            .map(|spec| Component {
                name: spec.name.clone(),
                functions: spec.functions.clone(),
            })
    }

    fn invoke<'a>(
        &'a self,
        component_name: &'a str,
        function_name: &'a str,
        args: Vec<serde_json::Value>,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = anyhow::Result<serde_json::Value>> + Send + 'a>,
    > {
        Box::pin(self.invoke(component_name, function_name, args, &[]))
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
            .expect("Component requires 'http' capability, so HTTP context should be available")
    }

    fn table(&mut self) -> &mut ResourceTable {
        &mut self.resource_table
    }

    fn send_request(
        &mut self,
        request: hyper::Request<HyperOutgoingBody>,
        config: OutgoingRequestConfig,
    ) -> HttpResult<HostFutureIncomingResponse> {
        let is_grpc = request
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .is_some_and(|ct| ct.starts_with("application/grpc"));

        if is_grpc {
            if request.uri().scheme_str() == Some("https") {
                tracing::error!("gRPC over TLS (https) is not yet supported");
                return Err(ErrorCode::HttpProtocolError.into());
            }
            Ok(super::grpc::send_grpc_request(request, config))
        } else {
            Ok(default_send_request(request, config))
        }
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
        config.wasm_component_model_async(true);
        config.memory_init_cow(true);
        let engine = Engine::new(&config)?;
        Ok(Self { engine })
    }

    fn create_linker(
        &self,
        capabilities: &[String],
        capability_registry: &CapabilityRegistry,
    ) -> Result<Linker<ComponentState>> {
        let mut linker = Linker::new(&self.engine);

        // Multiple capabilities may provide the same interface
        linker.allow_shadowing(true);

        // Add WASI interfaces based on explicitly requested capabilities
        for capability_name in capabilities {
            if let Some(capability) = capability_registry.get_capability(capability_name) {
                if let Some(wasi_capability) = capability.kind.strip_prefix("wasi:") {
                    use wasmtime_wasi::p2::bindings::{cli, clocks, random, sockets};

                    match wasi_capability {
                        "p2" => {
                            wasmtime_wasi::p2::add_to_linker_async(&mut linker)?;
                        }
                        "cli" => {
                            cli::stdin::add_to_linker::<ComponentState, WasiCli>(
                                &mut linker,
                                ComponentState::cli,
                            )?;
                            cli::stdout::add_to_linker::<ComponentState, WasiCli>(
                                &mut linker,
                                ComponentState::cli,
                            )?;
                            cli::stderr::add_to_linker::<ComponentState, WasiCli>(
                                &mut linker,
                                ComponentState::cli,
                            )?;
                            cli::environment::add_to_linker::<ComponentState, WasiCli>(
                                &mut linker,
                                ComponentState::cli,
                            )?;
                        }
                        "clocks" => {
                            clocks::wall_clock::add_to_linker::<ComponentState, WasiClocks>(
                                &mut linker,
                                ComponentState::clocks,
                            )?;
                            clocks::monotonic_clock::add_to_linker::<ComponentState, WasiClocks>(
                                &mut linker,
                                ComponentState::clocks,
                            )?;
                        }
                        "http" => {
                            wasmtime_wasi_http::add_only_http_to_linker_async(&mut linker)?;
                            // io is a transitive dep
                            wasmtime_wasi_io::add_to_linker_async(&mut linker)?;
                        }
                        "io" => {
                            wasmtime_wasi_io::add_to_linker_async(&mut linker)?;
                        }
                        "random" => {
                            random::random::add_to_linker::<ComponentState, WasiRandom>(
                                &mut linker,
                                |state| <ComponentState as WasiRandomView>::random(state),
                            )?;
                            random::insecure::add_to_linker::<ComponentState, WasiRandom>(
                                &mut linker,
                                |state| <ComponentState as WasiRandomView>::random(state),
                            )?;
                            random::insecure_seed::add_to_linker::<ComponentState, WasiRandom>(
                                &mut linker,
                                |state| <ComponentState as WasiRandomView>::random(state),
                            )?;
                        }
                        "sockets" => {
                            sockets::tcp::add_to_linker::<ComponentState, WasiSockets>(
                                &mut linker,
                                ComponentState::sockets,
                            )?;
                            sockets::udp::add_to_linker::<ComponentState, WasiSockets>(
                                &mut linker,
                                ComponentState::sockets,
                            )?;
                            sockets::network::add_to_linker::<ComponentState, WasiSockets>(
                                &mut linker,
                                &Default::default(),
                                ComponentState::sockets,
                            )?;
                            sockets::instance_network::add_to_linker::<ComponentState, WasiSockets>(
                                &mut linker,
                                ComponentState::sockets,
                            )?;
                            sockets::ip_name_lookup::add_to_linker::<ComponentState, WasiSockets>(
                                &mut linker,
                                ComponentState::sockets,
                            )?;
                            sockets::tcp_create_socket::add_to_linker::<ComponentState, WasiSockets>(
                                &mut linker,
                                ComponentState::sockets,
                            )?;
                            sockets::udp_create_socket::add_to_linker::<ComponentState, WasiSockets>(
                                &mut linker,
                                ComponentState::sockets,
                            )?;
                        }
                        _ => {
                            anyhow::bail!("Unknown capability type: '{}'", capability.kind);
                        }
                    }
                } else {
                    // Custom capability
                    if let Some(cap) = &capability.instance {
                        cap.link(&mut linker)?;
                    } else {
                        return Err(anyhow::anyhow!(
                            "Capability '{}' requested but no capability instance registered",
                            capability_name
                        ));
                    }
                }
            }
        }
        Ok(linker)
    }

    async fn instantiate_from_bytes(
        &self,
        bytes: &[u8],
        capabilities: &[String],
        capability_registry: &CapabilityRegistry,
        env_vars: &[(&str, &str)],
    ) -> Result<(Store<ComponentState>, wasmtime::component::Instance)> {
        let component_bytes = bytes.to_vec();
        let linker = self.create_linker(capabilities, capability_registry)?;

        // Build WASI context based on capabilities
        let mut wasi_builder = WasiCtxBuilder::new();

        if !env_vars.is_empty() {
            wasi_builder.envs(env_vars);
        }

        for capability_name in capabilities {
            if let Some(capability) = capability_registry.get_capability(capability_name) {
                let props = &capability.properties;
                match capability.kind.as_str() {
                    "wasi:p2" => {
                        wasi_builder.inherit_stdio();
                        wasi_builder.inherit_network();
                        wasi_builder.allow_ip_name_lookup(true);
                    }
                    "wasi:cli" => {
                        if props.get("inherit-stdio").and_then(|v| v.as_bool()) == Some(true) {
                            wasi_builder.inherit_stdio();
                        } else {
                            if props.get("inherit-stdin").and_then(|v| v.as_bool()) == Some(true) {
                                wasi_builder.inherit_stdin();
                            }
                            if props.get("inherit-stdout").and_then(|v| v.as_bool()) == Some(true) {
                                wasi_builder.inherit_stdout();
                            }
                            if props.get("inherit-stderr").and_then(|v| v.as_bool()) == Some(true) {
                                wasi_builder.inherit_stderr();
                            }
                        }
                    }
                    "wasi:sockets" => {
                        if props.get("inherit-network").and_then(|v| v.as_bool()) == Some(true) {
                            wasi_builder.inherit_network();
                        }
                        if props.get("allow-ip-name-lookup").and_then(|v| v.as_bool()) == Some(true)
                        {
                            wasi_builder.allow_ip_name_lookup(true);
                        }
                    }
                    _ => {}
                }
            }
        }

        // Check if HTTP context needed
        let needs_http = capabilities.iter().any(|capability_name| {
            capability_registry
                .get_capability(capability_name)
                .and_then(|cap| cap.kind.strip_prefix("wasi:"))
                == Some("http")
        });

        // Collect capability states before creating ComponentState
        let mut extensions = HashMap::new();
        for capability_name in capabilities {
            if let Some(capability) = capability_registry.get_capability(capability_name)
                && !capability.kind.starts_with("wasi:")
                && let Some(cap) = &capability.instance
                && let Some((type_id, boxed_state)) = cap.create_state_boxed()?
            {
                match extensions.entry(type_id) {
                    Entry::Vacant(e) => {
                        e.insert(boxed_state);
                    }
                    Entry::Occupied(_) => {
                        anyhow::bail!("Duplicate state type for capability '{capability_name}'");
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
        capabilities: &[String],
        capability_registry: &CapabilityRegistry,
        function: Function,
        args: Vec<serde_json::Value>,
        env_vars: &[(&str, &str)],
    ) -> Result<serde_json::Value> {
        let function_name = function.function_name();

        let (mut store, instance) = self
            .instantiate_from_bytes(bytes, capabilities, capability_registry, env_vars)
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
