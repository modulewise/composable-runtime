use anyhow::Result;
use std::collections::HashMap;
use std::collections::hash_map::Entry;
use wasmtime::{
    Cache, Config, Engine, Store,
    component::{Component as WasmComponent, Linker},
};
use wasmtime_wasi::cli::{WasiCli, WasiCliView};
use wasmtime_wasi::clocks::{WasiClocks, WasiClocksView};
use wasmtime_wasi::filesystem::{WasiFilesystem, WasiFilesystemView};
use wasmtime_wasi::random::{WasiRandom, WasiRandomView};
use wasmtime_wasi::sockets::{WasiSockets, WasiSocketsView};
use wasmtime_wasi::{DirPerms, FilePerms, ResourceTable, WasiCtxBuilder, WasiCtxView, WasiView};
use wasmtime_wasi_http::WasiHttpCtx;
use wasmtime_wasi_http::{p2 as http_p2, p3 as http_p3};
use wasmtime_wasi_io::IoView;

use crate::composition::registry::{
    CapabilityRegistry, ComponentRegistry, WasiVersion, split_wasi_kind,
};
use crate::context::PROPAGATION_CONTEXT;
use crate::runtime::component::{ComponentInstance, Val};
use crate::types::{
    Component, ComponentInvoker, ComponentMetadata, ComponentState, Function, HttpHooks,
    PROPAGATED_HEADERS,
};

// Component host: wasmtime engine + registries, provides instantiation + invocation.
#[derive(Clone)]
pub(crate) struct ComponentHost {
    invoker: Invoker,
    components: HashMap<String, Component>,
    pub(crate) component_registry: ComponentRegistry,
    pub(crate) capability_registry: CapabilityRegistry,
}

impl ComponentHost {
    pub(crate) fn new(
        component_registry: ComponentRegistry,
        capability_registry: CapabilityRegistry,
    ) -> Result<Self> {
        let invoker = Invoker::new()?;
        let components = component_registry
            .get_components()
            .map(|spec| {
                let component = Component {
                    metadata: ComponentMetadata {
                        name: spec.name.clone(),
                        namespace: spec.namespace.clone(),
                        package: spec.package.clone(),
                        labels: spec.labels.clone(),
                        dependents: Some(spec.dependents.clone()),
                        exports: spec.exports.clone(),
                    },
                    functions: spec.functions.clone(),
                };
                (spec.name.clone(), component)
            })
            .collect();
        Ok(Self {
            invoker,
            components,
            component_registry,
            capability_registry,
        })
    }

    pub(crate) async fn invoke(
        &self,
        component_name: &str,
        function_name: &str,
        args: Vec<serde_json::Value>,
        env_vars: &[(String, String)],
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
        env_vars: &[(String, String)],
    ) -> Result<ComponentInstance> {
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
    fn get_component(&self, name: &str) -> Option<&Component> {
        self.components.get(name)
    }

    fn list_components(
        &self,
        selector: Option<&crate::config::types::Selector>,
    ) -> Vec<&Component> {
        match selector {
            Some(selector) => self
                .components
                .values()
                .filter(|c| selector.matches(&c.metadata.to_selectable()))
                .collect(),
            None => self.components.values().collect(),
        }
    }

    fn invoke<'a>(
        &'a self,
        component_name: &'a str,
        function_name: &'a str,
        args: Vec<serde_json::Value>,
        env: Option<HashMap<String, String>>,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = anyhow::Result<serde_json::Value>> + Send + 'a>,
    > {
        Box::pin(async move {
            let env_pairs: Vec<(String, String)> =
                env.map(|m| m.into_iter().collect()).unwrap_or_default();
            self.invoke(component_name, function_name, args, &env_pairs)
                .await
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

impl HttpHooks {
    // Inject propagation-context headers (e.g. tracecontext) onto an outgoing
    // request. Shared by the p2 and p3 hooks.
    fn propagate_headers<B>(request: &mut http::Request<B>) {
        let ctx_entries: Option<HashMap<String, String>> = PROPAGATION_CONTEXT
            .try_with(|ctx| ctx.as_ref().map(|c| c.entries.clone()))
            .ok()
            .flatten();
        if let Some(entries) = ctx_entries {
            for key in PROPAGATED_HEADERS {
                if let Some(val) = entries.get(*key)
                    && let Ok(hv) = val.parse()
                {
                    request.headers_mut().insert(*key, hv);
                }
            }
        }
    }

    // Whether a request is gRPC (content-type application/grpc).
    fn is_grpc<B>(request: &http::Request<B>) -> bool {
        request
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .is_some_and(|ct| ct.starts_with("application/grpc"))
    }
}

impl http_p2::WasiHttpView for ComponentState {
    fn http(&mut self) -> http_p2::WasiHttpCtxView<'_> {
        http_p2::WasiHttpCtxView {
            hooks: &mut self.http_hooks,
            table: &mut self.resource_table,
            ctx: self.wasi_http_ctx.as_mut().expect(
                "Component requires 'http' capability, so HTTP context should be available",
            ),
        }
    }
}

impl http_p3::WasiHttpView for ComponentState {
    fn http(&mut self) -> http_p3::WasiHttpCtxView<'_> {
        http_p3::WasiHttpCtxView {
            hooks: &mut self.http_hooks,
            table: &mut self.resource_table,
            ctx: self.wasi_http_ctx.as_mut().expect(
                "Component requires 'http' capability, so HTTP context should be available",
            ),
        }
    }
}

impl http_p2::WasiHttpHooks for HttpHooks {
    fn send_request(
        &mut self,
        mut request: hyper::Request<http_p2::body::HyperOutgoingBody>,
        config: http_p2::types::OutgoingRequestConfig,
    ) -> http_p2::HttpResult<http_p2::types::HostFutureIncomingResponse> {
        Self::propagate_headers(&mut request);

        if self.h2c_for_grpc && Self::is_grpc(&request) {
            if request.uri().scheme_str() == Some("https") {
                tracing::error!("h2c-for-grpc does not support TLS (https)");
                return Err(http_p2::bindings::http::types::ErrorCode::HttpProtocolError.into());
            }
            Ok(super::grpc::send_grpc_request_p2(request, config))
        } else {
            Ok(http_p2::default_send_request(request, config))
        }
    }
}

type P3ErrorCode = http_p3::bindings::http::types::ErrorCode;
type P3Body = http_body_util::combinators::UnsyncBoxBody<bytes::Bytes, P3ErrorCode>;
type P3IoFuture = Box<dyn std::future::Future<Output = Result<(), P3ErrorCode>> + Send>;

impl http_p3::WasiHttpHooks for HttpHooks {
    fn send_request(
        &mut self,
        mut request: http::Request<P3Body>,
        options: Option<http_p3::RequestOptions>,
        fut: P3IoFuture,
    ) -> Box<
        dyn std::future::Future<
                Output = Result<
                    (http::Response<P3Body>, P3IoFuture),
                    wasmtime_wasi::TrappableError<P3ErrorCode>,
                >,
            > + Send,
    > {
        Self::propagate_headers(&mut request);

        if self.h2c_for_grpc && Self::is_grpc(&request) {
            if request.uri().scheme_str() == Some("https") {
                tracing::error!("h2c-for-grpc does not support TLS (https)");
                let err = P3ErrorCode::HttpProtocolError;
                return Box::new(async move { Err(err.into()) });
            }
            super::grpc::send_grpc_request_p3(request, options)
        } else {
            // `fut` is the guest-side request-error channel,
            // unused by `default_send_request`.
            let _ = fut;
            Box::new(async move {
                use http_body_util::BodyExt;
                let (res, io) = http_p3::default_send_request(request, options).await?;
                Ok((res.map(BodyExt::boxed_unsync), Box::new(io) as P3IoFuture))
            })
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
        // Synchronous stream.read/future.read builtins.
        config.wasm_component_model_more_async_builtins(true);
        // Stackful async lift, so an async call can block on those reads.
        config.wasm_component_model_async_stackful(true);
        config.wasm_component_model_map(true);
        config.memory_init_cow(true);
        config.wasm_gc(true);
        config.wasm_exceptions(true);
        config.wasm_function_references(true);
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
                if capability.kind.starts_with("wasi:") {
                    use wasmtime_wasi::p2::bindings::{cli, clocks, filesystem, random, sockets};

                    // `wasi:p2` is a bundle alias (hardcoded version), so it
                    // is matched exactly rather than through split_wasi_kind.
                    if capability.kind == "wasi:p2" {
                        wasmtime_wasi::p2::add_to_linker_async(&mut linker)?;
                        continue;
                    }

                    match split_wasi_kind(&capability.kind) {
                        ("wasi:cli", WasiVersion::P3) => {
                            wasmtime_wasi::p3::cli::add_to_linker(&mut linker)?;
                        }
                        ("wasi:cli", WasiVersion::P2) => {
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
                        ("wasi:clocks", WasiVersion::P3) => {
                            wasmtime_wasi::p3::clocks::add_to_linker(&mut linker)?;
                        }
                        ("wasi:clocks", WasiVersion::P2) => {
                            clocks::wall_clock::add_to_linker::<ComponentState, WasiClocks>(
                                &mut linker,
                                ComponentState::clocks,
                            )?;
                            clocks::monotonic_clock::add_to_linker::<ComponentState, WasiClocks>(
                                &mut linker,
                                ComponentState::clocks,
                            )?;
                        }
                        ("wasi:filesystem", WasiVersion::P3) => {
                            wasmtime_wasi::p3::filesystem::add_to_linker(&mut linker)?;
                        }
                        ("wasi:filesystem", WasiVersion::P2) => {
                            filesystem::types::add_to_linker::<ComponentState, WasiFilesystem>(
                                &mut linker,
                                ComponentState::filesystem,
                            )?;
                            filesystem::preopens::add_to_linker::<ComponentState, WasiFilesystem>(
                                &mut linker,
                                ComponentState::filesystem,
                            )?;
                            wasmtime_wasi_io::add_to_linker_async(&mut linker)?;
                        }
                        ("wasi:http", WasiVersion::P3) => {
                            wasmtime_wasi_http::p3::add_to_linker(&mut linker)?;
                        }
                        ("wasi:http", WasiVersion::P2) => {
                            wasmtime_wasi_http::p2::add_only_http_to_linker_async(&mut linker)?;
                            wasmtime_wasi_io::add_to_linker_async(&mut linker)?;
                        }
                        ("wasi:io", WasiVersion::P2) => {
                            wasmtime_wasi_io::add_to_linker_async(&mut linker)?;
                        }
                        ("wasi:random", WasiVersion::P3) => {
                            wasmtime_wasi::p3::random::add_to_linker(&mut linker)?;
                        }
                        ("wasi:random", WasiVersion::P2) => {
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
                        ("wasi:sockets", WasiVersion::P3) => {
                            wasmtime_wasi::p3::sockets::add_to_linker(&mut linker)?;
                        }
                        ("wasi:sockets", WasiVersion::P2) => {
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
        env_vars: &[(String, String)],
    ) -> Result<ComponentInstance> {
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
                // Match the base kind for consistent behavior with or without a version suffix.
                match split_wasi_kind(&capability.kind).0 {
                    "wasi:p2" => {
                        wasi_builder.inherit_stdio();
                        wasi_builder.inherit_network();
                        wasi_builder.allow_ip_name_lookup(true);
                        add_preopens(&mut wasi_builder, props, capability_name)?;
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
                    "wasi:filesystem" => {
                        add_preopens(&mut wasi_builder, props, capability_name)?;
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

        // Find the wasi:http capability (if any). Its properties configure the
        // HTTP context and hooks. Matches any version; WasiHttpCtx is shared.
        let http_capability = capabilities.iter().find_map(|capability_name| {
            capability_registry
                .get_capability(capability_name)
                .filter(|cap| split_wasi_kind(&cap.kind).0 == "wasi:http")
        });
        let needs_http = http_capability.is_some();
        let http_hooks = http_capability
            .map(|cap| HttpHooks::from_properties(&cap.properties))
            .unwrap_or_default();

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
            http_hooks,
            extensions,
        };

        let mut store = Store::new(&self.engine, state);
        let component = WasmComponent::from_binary(&self.engine, &component_bytes)?;
        let instance = linker.instantiate_async(&mut store, &component).await?;

        Ok(ComponentInstance::new(store, instance))
    }

    /// Single-use invocation: instantiate, call, drop.
    pub async fn invoke(
        &self,
        bytes: &[u8],
        capabilities: &[String],
        capability_registry: &CapabilityRegistry,
        function: Function,
        args: Vec<serde_json::Value>,
        env_vars: &[(String, String)],
    ) -> Result<serde_json::Value> {
        let mut instance = self
            .instantiate_from_bytes(bytes, capabilities, capability_registry, env_vars)
            .await?;

        let args = args.into_iter().map(Val::Json).collect();
        let results = instance.call(&function, args).await?;

        match results {
            None => Ok(serde_json::Value::Null),
            Some(Val::Json(value)) => Ok(value),
            Some(Val::Resource(_)) => Err(anyhow::anyhow!(
                "function '{}' returned a resource, which has no JSON value representation; \
                 instantiate the component and use `ComponentInstance::call` instead",
                function.function_name()
            )),
        }
    }
}

// Apply a filesystem capability's `preopens` to the WASI context.
//
// Each entry maps a host directory into the guest with explicit permissions:
//
//     [[capability.fs.preopens]]
//     host = "./data"
//     guest = "/data"
//     perms = "read-only"    # or "read-write"
//
// The `perms` configuration is required. It covers file and directory
// permissions for the preopen ("read-write" grants directory mutation).
fn add_preopens(
    builder: &mut WasiCtxBuilder,
    props: &HashMap<String, serde_json::Value>,
    capability_name: &str,
) -> Result<()> {
    let Some(value) = props.get("preopens") else {
        return Ok(());
    };
    let entries = value.as_array().ok_or_else(|| {
        anyhow::anyhow!("Capability '{capability_name}': 'preopens' must be an array")
    })?;

    for (index, entry) in entries.iter().enumerate() {
        let ctx = |detail: String| {
            anyhow::anyhow!("Capability '{capability_name}' preopen {index}: {detail}")
        };
        let field = |name: &str| -> Result<String> {
            entry
                .get(name)
                .and_then(|v| v.as_str())
                .map(str::to_string)
                .ok_or_else(|| ctx(format!("missing required string '{name}'")))
        };

        let host = field("host")?;
        let guest = field("guest")?;
        let (dir_perms, file_perms) = match field("perms")?.as_str() {
            "read-only" => (DirPerms::READ, FilePerms::READ),
            "read-write" => (
                DirPerms::READ | DirPerms::MUTATE,
                FilePerms::READ | FilePerms::WRITE,
            ),
            other => {
                return Err(ctx(format!(
                    "'perms' must be \"read-only\" or \"read-write\", got \"{other}\""
                )));
            }
        };

        builder
            .preopened_dir(&host, &guest, dir_perms, file_perms)
            .map_err(|e| ctx(format!("cannot open host path '{host}': {e}")))?;
    }
    Ok(())
}
