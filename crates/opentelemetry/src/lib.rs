//! OpenTelemetry host capability for composable-runtime.
//!
//! Provides `wasi:otel/logs` and `wasi:otel/tracing` as a host capability
//! backed by the OpenTelemetry Rust SDK with OTLP export.

use anyhow::Result;
use composable_runtime::{ComponentState, HostCapability, PROPAGATION_CONTEXT, Service};
use indexmap::IndexMap;
use opentelemetry::trace::{SpanContext, SpanId};
use opentelemetry_otlp::{LogExporter, SpanExporter, WithExportConfig};
use opentelemetry_sdk::{
    Resource,
    logs::{SdkLoggerProvider, log_processor_with_async_runtime::BatchLogProcessor},
    runtime::Tokio,
    trace::{SpanProcessor, span_processor_with_async_runtime::BatchSpanProcessor},
};
use serde::Deserialize;
use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex, OnceLock};
use wasmtime::component::{HasSelf, Linker};

mod bindings {
    wasmtime::component::bindgen!({
        path: "wit",
        world: "otel-capability",
        imports: { default: async | trappable },
    });
}

use bindings::wasi::otel;

// Initialized per OtelCapability instance in OtelService::start().
struct CapabilityProcessors {
    span_processor: Arc<BatchSpanProcessor<Tokio>>,
    // Used when log-record carries no resource. All no-resource records share this processor.
    default_log_provider: Arc<SdkLoggerProvider>,
    // Created on demand when a log-record carries a resource. Keyed by sorted attribute pairs.
    keyed_log_providers: Mutex<HashMap<Vec<(String, String)>, Arc<SdkLoggerProvider>>>,
}

// Per-component-invocation state placed into ComponentState extensions.
struct OtelInstanceState {
    // Ordered map of currently-open guest spans (span_id -> SpanContext), innermost last.
    guest_spans: IndexMap<SpanId, SpanContext>,
    processors: Arc<CapabilityProcessors>,
}

/// Configuration for the OTel capability, deserialized from `[capability.otel]` TOML.
#[derive(Deserialize)]
pub struct OtelCapability {
    #[serde(default = "default_endpoint")]
    pub endpoint: String,

    #[serde(default = "default_protocol")]
    pub protocol: String,

    #[serde(default)]
    pub resource: HashMap<String, String>,

    // Initialized by OtelService::start(); not from TOML.
    #[serde(skip)]
    processors: OnceLock<Arc<CapabilityProcessors>>,
}

fn default_endpoint() -> String {
    "http://localhost:4317".to_string()
}

fn default_protocol() -> String {
    "grpc".to_string()
}

impl Default for OtelCapability {
    fn default() -> Self {
        Self {
            endpoint: default_endpoint(),
            protocol: default_protocol(),
            resource: HashMap::new(),
            processors: OnceLock::new(),
        }
    }
}

impl OtelCapability {
    // Called by OtelService::start() to build exporters and processors from this instance's config.
    fn init(&self) -> Result<()> {
        let resource = build_resource(&self.resource);

        let span_exporter = match self.protocol.as_str() {
            "http/protobuf" => SpanExporter::builder()
                .with_http()
                .with_endpoint(&self.endpoint)
                .build()
                .map_err(|e| anyhow::anyhow!("Failed to build span exporter: {e}"))?,
            _ => SpanExporter::builder()
                .with_tonic()
                .with_endpoint(&self.endpoint)
                .build()
                .map_err(|e| anyhow::anyhow!("Failed to build span exporter: {e}"))?,
        };

        let mut span_processor = BatchSpanProcessor::builder(span_exporter, Tokio).build();
        span_processor.set_resource(&resource);

        let log_exporter = match self.protocol.as_str() {
            "http/protobuf" => LogExporter::builder()
                .with_http()
                .with_endpoint(&self.endpoint)
                .build()
                .map_err(|e| anyhow::anyhow!("Failed to build log exporter: {e}"))?,
            _ => LogExporter::builder()
                .with_tonic()
                .with_endpoint(&self.endpoint)
                .build()
                .map_err(|e| anyhow::anyhow!("Failed to build log exporter: {e}"))?,
        };

        let default_log_provider = SdkLoggerProvider::builder()
            .with_resource(resource)
            .with_log_processor(BatchLogProcessor::builder(log_exporter, Tokio).build())
            .build();

        let _ = self.processors.set(Arc::new(CapabilityProcessors {
            span_processor: Arc::new(span_processor),
            default_log_provider: Arc::new(default_log_provider),
            keyed_log_providers: Mutex::new(HashMap::new()),
        }));

        Ok(())
    }

    fn shutdown(&self) {
        let Some(p) = self.processors.get() else {
            return;
        };
        let _ = p.span_processor.shutdown();
        let _ = p.default_log_provider.shutdown();
        for provider in p.keyed_log_providers.lock().unwrap().values() {
            let _ = provider.shutdown();
        }
    }
}

impl HostCapability for OtelCapability {
    fn interfaces(&self) -> Vec<String> {
        vec![
            "wasi:otel/logs@0.2.0-rc.2+patch".to_string(),
            "wasi:otel/tracing@0.2.0-rc.2+patch".to_string(),
        ]
    }

    fn link(&self, linker: &mut Linker<ComponentState>) -> wasmtime::Result<()> {
        otel::logs::add_to_linker::<_, HasSelf<_>>(linker, |state| state)?;
        otel::tracing::add_to_linker::<_, HasSelf<_>>(linker, |state| state)?;
        Ok(())
    }

    composable_runtime::create_state!(this, OtelInstanceState, {
        let processors = this
            .processors
            .get()
            .expect("OtelCapability requires OtelService::start() to be called before invoke()")
            .clone();
        OtelInstanceState {
            guest_spans: IndexMap::new(),
            processors,
        }
    });
}

impl otel::tracing::Host for ComponentState {
    async fn on_start(&mut self, context: otel::tracing::SpanContext) -> wasmtime::Result<()> {
        let span_context = convert_span_context(context);
        let span_id = span_context.span_id();
        self.get_extension_mut::<OtelInstanceState>()
            .expect("OtelInstanceState not initialized")
            .guest_spans
            .insert(span_id, span_context);
        Ok(())
    }

    async fn on_end(&mut self, span: otel::tracing::SpanData) -> wasmtime::Result<()> {
        let state = self
            .get_extension_mut::<OtelInstanceState>()
            .expect("OtelInstanceState not initialized");
        let span_context = convert_span_context(span.span_context.clone());
        state.guest_spans.shift_remove(&span_context.span_id());
        // Parent is remote if not in the current guest spans map.
        let parent_is_remote = opentelemetry::trace::SpanId::from_hex(&span.parent_span_id)
            .ok()
            .filter(|pid| *pid != opentelemetry::trace::SpanId::INVALID)
            .map(|pid| !state.guest_spans.contains_key(&pid))
            .unwrap_or(false);
        state
            .processors
            .span_processor
            .on_end(convert_span_data(span, parent_is_remote));
        Ok(())
    }

    async fn outer_span_context(&mut self) -> wasmtime::Result<otel::tracing::SpanContext> {
        let sc = PROPAGATION_CONTEXT
            .try_with(|ctx| {
                ctx.as_ref().and_then(|c| {
                    let traceparent = c.entries.get("traceparent")?;
                    let tracestate = c.entries.get("tracestate").map(|s| s.as_str());
                    parse_traceparent(traceparent, tracestate)
                })
            })
            .ok()
            .flatten();
        Ok(sc.unwrap_or_else(|| {
            otel_span_context_to_wasi(opentelemetry::trace::SpanContext::empty_context())
        }))
    }
}

impl otel::logs::Host for ComponentState {
    async fn on_emit(&mut self, data: otel::logs::LogRecord) -> wasmtime::Result<()> {
        use opentelemetry::logs::{Logger, LoggerProvider};

        let state = self
            .get_extension_mut::<OtelInstanceState>()
            .expect("OtelInstanceState not initialized");

        let provider: Arc<SdkLoggerProvider> = match &data.resource {
            None => Arc::clone(&state.processors.default_log_provider),
            Some(r) => {
                let key = resource_key(r);
                let mut map = state.processors.keyed_log_providers.lock().unwrap();
                if let Some(p) = map.get(&key) {
                    Arc::clone(p)
                } else {
                    let new_provider = Arc::new(
                        SdkLoggerProvider::builder()
                            .with_resource(wasi_resource_to_sdk(r))
                            .build(),
                    );
                    map.insert(key, Arc::clone(&new_provider));
                    new_provider
                }
            }
        };

        let scope = extract_instrumentation_scope(&data);
        let logger = provider.logger_with_scope(scope);
        let mut record = logger.create_log_record();
        populate_log_record(&mut record, &data);
        logger.emit(record);

        Ok(())
    }
}

/// Service that owns all `OtelCapability` instances and manages their lifecycle.
pub struct OtelService {
    instances: Arc<Mutex<Vec<Arc<OtelCapability>>>>,
}

impl Default for OtelService {
    fn default() -> Self {
        Self {
            instances: Arc::new(Mutex::new(Vec::new())),
        }
    }
}

impl Service for OtelService {
    fn capabilities(&self) -> Vec<(&'static str, composable_runtime::HostCapabilityFactory)> {
        let instances = Arc::clone(&self.instances);
        vec![composable_runtime::create_capability!("otel", |config| {
            let cap =
                Arc::new(serde_json::from_value::<OtelCapability>(config).unwrap_or_default());
            instances.lock().unwrap().push(Arc::clone(&cap));
            OtelCapabilityHandle(cap)
        })]
    }

    fn start(&self) -> Result<()> {
        for cap in self.instances.lock().unwrap().iter() {
            cap.init()?;
        }
        Ok(())
    }

    fn shutdown(&self) -> Pin<Box<dyn Future<Output = ()> + Send + '_>> {
        Box::pin(async move {
            let caps: Vec<_> = self.instances.lock().unwrap().clone();
            for cap in caps {
                // spawn_blocking since BatchSpanProcessor::shutdown() calls block_on internally.
                let _ = tokio::task::spawn_blocking(move || cap.shutdown()).await;
            }
        })
    }
}

// Wraps Arc<OtelCapability> so it can be returned as Box<dyn HostCapability>.
struct OtelCapabilityHandle(Arc<OtelCapability>);

impl HostCapability for OtelCapabilityHandle {
    fn interfaces(&self) -> Vec<String> {
        self.0.interfaces()
    }

    fn link(&self, linker: &mut Linker<ComponentState>) -> wasmtime::Result<()> {
        self.0.link(linker)
    }

    fn create_state_boxed(
        &self,
    ) -> Result<Option<(std::any::TypeId, Box<dyn std::any::Any + Send>)>> {
        self.0.create_state_boxed()
    }
}

// Parses a W3C `traceparent` header and optional `tracestate` into a wasi SpanContext.
// Returns None if the traceparent value is malformed.
fn parse_traceparent(
    traceparent: &str,
    tracestate: Option<&str>,
) -> Option<otel::tracing::SpanContext> {
    let parts: Vec<&str> = traceparent.splitn(4, '-').collect();
    if parts.len() != 4 {
        return None;
    }
    let trace_id = opentelemetry::trace::TraceId::from_hex(parts[1]).ok()?;
    let span_id = opentelemetry::trace::SpanId::from_hex(parts[2]).ok()?;
    let flags = u8::from_str_radix(parts[3], 16).ok()?;
    let trace_flags = if flags & 0x01 != 0 {
        otel::tracing::TraceFlags::SAMPLED
    } else {
        otel::tracing::TraceFlags::empty()
    };
    let trace_state = tracestate
        .unwrap_or("")
        .split(',')
        .filter_map(|s| {
            s.split_once('=')
                .map(|(k, v)| (k.to_string(), v.to_string()))
        })
        .collect();
    Some(otel::tracing::SpanContext {
        trace_id: format!("{:032x}", trace_id),
        span_id: format!("{:016x}", span_id),
        trace_flags,
        is_remote: true,
        trace_state,
    })
}

fn build_resource(attrs: &HashMap<String, String>) -> Resource {
    let kvs: Vec<opentelemetry::KeyValue> = attrs
        .iter()
        .map(|(k, v)| opentelemetry::KeyValue::new(k.clone(), v.clone()))
        .collect();
    Resource::builder().with_attributes(kvs).build()
}

fn wasi_resource_to_sdk(r: &otel::logs::Resource) -> Resource {
    let kvs: Vec<opentelemetry::KeyValue> = r
        .attributes
        .iter()
        .map(|kv| opentelemetry::KeyValue::new(kv.key.clone(), kv.value.clone()))
        .collect();
    Resource::builder().with_attributes(kvs).build()
}

fn resource_key(r: &otel::logs::Resource) -> Vec<(String, String)> {
    let mut pairs: Vec<(String, String)> = r
        .attributes
        .iter()
        .map(|kv| (kv.key.clone(), kv.value.clone()))
        .collect();
    pairs.sort_unstable_by(|a, b| a.0.cmp(&b.0));
    pairs
}

fn populate_log_record(
    record: &mut opentelemetry_sdk::logs::SdkLogRecord,
    data: &otel::logs::LogRecord,
) {
    use opentelemetry::logs::LogRecord as _;

    if let Some(body) = &data.body {
        record.set_body(opentelemetry::logs::AnyValue::String(body.clone().into()));
    }
    if let Some(ts) = data.timestamp {
        record.set_timestamp(convert_datetime(ts));
    }
    if let Some(ts) = data.observed_timestamp {
        record.set_observed_timestamp(convert_datetime(ts));
    }
    if let Some(n) = data.severity_number {
        record.set_severity_number(severity_from_u8(n));
    }
    if let Some(t) = &data.severity_text {
        record.set_severity_text(Box::leak(t.clone().into_boxed_str()));
    }
    if let Some(name) = &data.event_name {
        record.set_event_name(Box::leak(name.clone().into_boxed_str()));
    }
    if let (Some(trace_id), Some(span_id)) = (&data.trace_id, &data.span_id)
        && let (Ok(tid), Ok(sid)) = (
            opentelemetry::trace::TraceId::from_hex(trace_id),
            opentelemetry::trace::SpanId::from_hex(span_id),
        )
    {
        record.set_trace_context(
            tid,
            sid,
            data.trace_flags.map(|f| {
                if f.contains(otel::logs::TraceFlags::SAMPLED) {
                    opentelemetry::trace::TraceFlags::SAMPLED
                } else {
                    opentelemetry::trace::TraceFlags::default()
                }
            }),
        );
    }
}

fn extract_instrumentation_scope(
    data: &otel::logs::LogRecord,
) -> opentelemetry::InstrumentationScope {
    let Some(s) = &data.instrumentation_scope else {
        return opentelemetry::InstrumentationScope::default();
    };
    let mut builder =
        opentelemetry::InstrumentationScope::builder(std::borrow::Cow::Owned(s.name.clone()));
    if let Some(v) = &s.version {
        builder = builder.with_version(v.clone());
    }
    if let Some(url) = &s.schema_url {
        builder = builder.with_schema_url(std::borrow::Cow::Owned(url.clone()));
    }
    builder.build()
}

fn convert_span_context(sc: otel::tracing::SpanContext) -> opentelemetry::trace::SpanContext {
    let trace_id = opentelemetry::trace::TraceId::from_hex(&sc.trace_id)
        .unwrap_or(opentelemetry::trace::TraceId::INVALID);
    let span_id = opentelemetry::trace::SpanId::from_hex(&sc.span_id)
        .unwrap_or(opentelemetry::trace::SpanId::INVALID);
    let trace_flags = if sc.trace_flags.contains(otel::tracing::TraceFlags::SAMPLED) {
        opentelemetry::trace::TraceFlags::SAMPLED
    } else {
        opentelemetry::trace::TraceFlags::default()
    };
    let trace_state =
        opentelemetry::trace::TraceState::from_key_value(sc.trace_state).unwrap_or_default();
    opentelemetry::trace::SpanContext::new(
        trace_id,
        span_id,
        trace_flags,
        sc.is_remote,
        trace_state,
    )
}

fn otel_span_context_to_wasi(sc: opentelemetry::trace::SpanContext) -> otel::tracing::SpanContext {
    otel::tracing::SpanContext {
        trace_id: format!("{:032x}", sc.trace_id()),
        span_id: format!("{:016x}", sc.span_id()),
        trace_flags: if sc.trace_flags().is_sampled() {
            otel::tracing::TraceFlags::SAMPLED
        } else {
            otel::tracing::TraceFlags::empty()
        },
        is_remote: sc.is_remote(),
        trace_state: sc
            .trace_state()
            .header()
            .split(',')
            .filter_map(|s| {
                s.split_once('=')
                    .map(|(k, v)| (k.to_string(), v.to_string()))
            })
            .collect(),
    }
}

fn convert_span_data(
    span: otel::tracing::SpanData,
    parent_is_remote: bool,
) -> opentelemetry_sdk::trace::SpanData {
    use opentelemetry_sdk::trace::{SpanEvents, SpanLinks};

    let mut events = SpanEvents::default();
    events.events = span.events.into_iter().map(convert_event).collect();
    events.dropped_count = span.dropped_events;

    let mut links = SpanLinks::default();
    links.links = span.links.into_iter().map(convert_link).collect();
    links.dropped_count = span.dropped_links;

    opentelemetry_sdk::trace::SpanData {
        span_context: convert_span_context(span.span_context),
        parent_span_id: opentelemetry::trace::SpanId::from_hex(&span.parent_span_id)
            .unwrap_or(opentelemetry::trace::SpanId::INVALID),
        parent_span_is_remote: parent_is_remote,
        span_kind: convert_span_kind(span.span_kind),
        name: span.name.into(),
        start_time: convert_datetime(span.start_time),
        end_time: convert_datetime(span.end_time),
        attributes: span.attributes.into_iter().map(convert_key_value).collect(),
        dropped_attributes_count: span.dropped_attributes,
        events,
        links,
        status: convert_status(span.status),
        instrumentation_scope: convert_instrumentation_scope_tracing(span.instrumentation_scope),
    }
}

fn convert_event(e: otel::tracing::Event) -> opentelemetry::trace::Event {
    opentelemetry::trace::Event::new(
        e.name,
        convert_datetime(e.time),
        e.attributes.into_iter().map(convert_key_value).collect(),
        0,
    )
}

fn convert_link(l: otel::tracing::Link) -> opentelemetry::trace::Link {
    opentelemetry::trace::Link::new(
        convert_span_context(l.span_context),
        l.attributes.into_iter().map(convert_key_value).collect(),
        0,
    )
}

fn convert_span_kind(kind: otel::tracing::SpanKind) -> opentelemetry::trace::SpanKind {
    match kind {
        otel::tracing::SpanKind::Client => opentelemetry::trace::SpanKind::Client,
        otel::tracing::SpanKind::Server => opentelemetry::trace::SpanKind::Server,
        otel::tracing::SpanKind::Producer => opentelemetry::trace::SpanKind::Producer,
        otel::tracing::SpanKind::Consumer => opentelemetry::trace::SpanKind::Consumer,
        otel::tracing::SpanKind::Internal => opentelemetry::trace::SpanKind::Internal,
    }
}

fn convert_status(status: otel::tracing::Status) -> opentelemetry::trace::Status {
    match status {
        otel::tracing::Status::Unset => opentelemetry::trace::Status::Unset,
        otel::tracing::Status::Ok => opentelemetry::trace::Status::Ok,
        otel::tracing::Status::Error(s) => opentelemetry::trace::Status::Error {
            description: s.into(),
        },
    }
}

fn convert_key_value(kv: otel::tracing::KeyValue) -> opentelemetry::KeyValue {
    opentelemetry::KeyValue::new(kv.key, kv.value)
}

fn convert_datetime(dt: bindings::wasi::clocks::wall_clock::Datetime) -> std::time::SystemTime {
    std::time::UNIX_EPOCH + std::time::Duration::new(dt.seconds, dt.nanoseconds)
}

fn convert_instrumentation_scope_tracing(
    scope: otel::tracing::InstrumentationScope,
) -> opentelemetry::InstrumentationScope {
    let mut builder =
        opentelemetry::InstrumentationScope::builder(std::borrow::Cow::Owned(scope.name));
    if let Some(v) = scope.version {
        builder = builder.with_version(v);
    }
    if let Some(url) = scope.schema_url {
        builder = builder.with_schema_url(std::borrow::Cow::Owned(url));
    }
    builder.build()
}

fn severity_from_u8(n: u8) -> opentelemetry::logs::Severity {
    use opentelemetry::logs::Severity::*;
    match n {
        1 => Trace,
        2 => Trace2,
        3 => Trace3,
        4 => Trace4,
        5 => Debug,
        6 => Debug2,
        7 => Debug3,
        8 => Debug4,
        9 => Info,
        10 => Info2,
        11 => Info3,
        12 => Info4,
        13 => Warn,
        14 => Warn2,
        15 => Warn3,
        16 => Warn4,
        17 => Error,
        18 => Error2,
        19 => Error3,
        20 => Error4,
        21 => Fatal,
        22 => Fatal2,
        23 => Fatal3,
        24 => Fatal4,
        _ => Info,
    }
}
