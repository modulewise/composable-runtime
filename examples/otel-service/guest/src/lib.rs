wit_bindgen::generate!({
    path: "../wit",
    world: "guest",
    generate_all,
});

use wasi::otel::{logs, tracing};

struct Component;

impl Guest for Component {
    fn run() -> Result<(), String> {
        // Get the host's current span context (propagated from incoming request headers).
        let outer = tracing::outer_span_context();

        // Start a guest span as a child of the host's span.
        let now = wasi::clocks::wall_clock::now();
        let span_id = wasi::random::random::get_random_bytes(8)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>();
        let span_context = tracing::SpanContext {
            trace_id: outer.trace_id.clone(),
            span_id,
            trace_flags: tracing::TraceFlags::SAMPLED,
            is_remote: false,
            trace_state: vec![],
        };
        tracing::on_start(&span_context);

        // Emit a log record with no resource (routes through the default batch processor).
        logs::on_emit(&logs::LogRecord {
            timestamp: Some(now),
            observed_timestamp: None,
            severity_text: Some("INFO".to_string()),
            severity_number: Some(9),
            body: Some("otel-service example ran".to_string()),
            attributes: Some(vec![logs::KeyValue {
                key: "component".to_string(),
                value: "otel-service-guest".to_string(),
            }]),
            event_name: None,
            resource: None,
            instrumentation_scope: Some(logs::InstrumentationScope {
                name: "otel-service-example".to_string(),
                version: Some("0.1.0".to_string()),
                schema_url: None,
                attributes: vec![],
            }),
            trace_id: Some(outer.trace_id.clone()),
            span_id: Some(outer.span_id.clone()),
            trace_flags: None,
        });

        // End the guest span.
        let end_time = wasi::clocks::wall_clock::now();
        tracing::on_end(&tracing::SpanData {
            span_context: span_context.clone(),
            parent_span_id: outer.span_id.clone(),
            span_kind: tracing::SpanKind::Internal,
            name: "otel-service-example".to_string(),
            start_time: now,
            end_time,
            attributes: vec![tracing::KeyValue {
                key: "example".to_string(),
                value: "otel-service".to_string(),
            }],
            events: vec![],
            links: vec![],
            status: tracing::Status::Ok,
            instrumentation_scope: wasi::otel::types::InstrumentationScope {
                name: "otel-service-example".to_string(),
                version: Some("0.1.0".to_string()),
                schema_url: None,
                attributes: vec![],
            },
            dropped_attributes: 0,
            dropped_events: 0,
            dropped_links: 0,
        });

        Ok(())
    }
}

export!(Component);
