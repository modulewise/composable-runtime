wit_bindgen::generate!({
    path: "../wit",
    world: "guest",
    generate_all,
});

use wasi::otel::logs;

struct Logger;

impl exports::modulewise::otel_example::test::Guest for Logger {
    fn log(message: String) -> Result<(), String> {
        logs::on_emit(&logs::LogRecord {
            timestamp: None,
            observed_timestamp: None,
            severity_text: Some("INFO".to_string()),
            severity_number: Some(9),
            body: Some(message),
            attributes: Some(vec![logs::KeyValue {
                key: "component".to_string(),
                value: "guest".to_string(),
            }]),
            event_name: None,
            resource: Some(logs::Resource {
                attributes: vec![logs::KeyValue {
                    key: "service.name".to_string(),
                    value: "otel-example".to_string(),
                }],
                schema_url: None,
            }),
            instrumentation_scope: Some(logs::InstrumentationScope {
                name: "test-logger".to_string(),
                version: Some("0.1.0".to_string()),
                schema_url: None,
                attributes: vec![],
            }),
            trace_id: None,
            span_id: None,
            trace_flags: None,
        });

        Ok(())
    }
}

export!(Logger);
