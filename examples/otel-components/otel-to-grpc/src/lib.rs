//! Converts wasi:otel log-records to OTLP protobuf and sends to gRPC endpoint

wit_bindgen::generate!({
    path: "../wit",
    world: "otel-to-grpc",
    generate_all,
});

struct OtelAdapter;

use exports::wasi::otel::logs::{Guest as LogsGuest, LogRecord};
use modulewise::grpc::endpoint;
use opentelemetry_proto::tonic::collector::logs::v1::ExportLogsServiceRequest;
use opentelemetry_proto::tonic::common::v1::{
    AnyValue, ArrayValue, KeyValue, KeyValueList, any_value,
};
use opentelemetry_proto::tonic::logs::v1::{LogRecord as OtlpLogRecord, ResourceLogs, ScopeLogs};
use opentelemetry_proto::tonic::resource::v1::Resource as OtlpResource;
use prost::Message;
use wasi::clocks::wall_clock;

impl LogsGuest for OtelAdapter {
    fn on_emit(record: LogRecord) {
        if let Err(e) = send_log(&record) {
            // errors are silently dropped
            let _ = e;
        }
    }
}

fn send_log(data: &LogRecord) -> Result<(), String> {
    let otlp_log = convert_log_record(data);

    let resource = data
        .resource
        .as_ref()
        .map(|r| {
            let (attributes, dropped_attributes_count) =
                key_values(r.attributes.iter().map(|kv| (&kv.key, &kv.value)));
            OtlpResource {
                attributes,
                dropped_attributes_count,
                entity_refs: Vec::new(),
            }
        })
        .unwrap_or_else(|| OtlpResource {
            attributes: vec![KeyValue {
                key: "service.name".to_string(),
                value: Some(AnyValue {
                    value: Some(any_value::Value::StringValue("unknown_service".to_string())),
                }),
                key_strindex: 0,
            }],
            dropped_attributes_count: 0,
            entity_refs: Vec::new(),
        });

    let scope = data.instrumentation_scope.as_ref().map(|s| {
        let (attributes, dropped_attributes_count) =
            key_values(s.attributes.iter().map(|kv| (&kv.key, &kv.value)));
        opentelemetry_proto::tonic::common::v1::InstrumentationScope {
            name: s.name.clone(),
            version: s.version.clone().unwrap_or_default(),
            attributes,
            dropped_attributes_count,
        }
    });

    let request = ExportLogsServiceRequest {
        resource_logs: vec![ResourceLogs {
            resource: Some(resource),
            scope_logs: vec![ScopeLogs {
                scope,
                log_records: vec![otlp_log],
                schema_url: String::new(),
            }],
            schema_url: String::new(),
        }],
    };

    let bytes = request.encode_to_vec();
    endpoint::send("logs", &bytes)?;

    Ok(())
}

fn convert_log_record(data: &LogRecord) -> OtlpLogRecord {
    let ts = data.timestamp.unwrap_or_else(wall_clock::now);
    let time_unix_nano = ts.seconds * 1_000_000_000 + ts.nanoseconds as u64;

    let observed_time_unix_nano = data
        .observed_timestamp
        .map(|dt| dt.seconds * 1_000_000_000 + dt.nanoseconds as u64)
        .unwrap_or(0);

    let severity_number = data.severity_number.unwrap_or(0) as i32;

    let body = data.body.as_deref().and_then(decode);

    let (attributes, dropped_attributes_count) = key_values(
        data.attributes
            .iter()
            .flatten()
            .map(|kv| (&kv.key, &kv.value)),
    );

    let trace_id = data
        .trace_id
        .as_ref()
        .and_then(|id| hex::decode(id).ok())
        .unwrap_or_default();

    let span_id = data
        .span_id
        .as_ref()
        .and_then(|id| hex::decode(id).ok())
        .unwrap_or_default();

    OtlpLogRecord {
        time_unix_nano,
        observed_time_unix_nano,
        severity_number,
        severity_text: data.severity_text.clone().unwrap_or_default(),
        body,
        attributes,
        dropped_attributes_count,
        flags: 0,
        trace_id,
        span_id,
        event_name: data.event_name.clone().unwrap_or_default(),
    }
}

// The key-values whose values decode and how many were dropped because their
// values do not decode.
fn key_values<'a>(kvs: impl IntoIterator<Item = (&'a String, &'a String)>) -> (Vec<KeyValue>, u32) {
    let mut dropped = 0;
    let decoded = kvs
        .into_iter()
        .filter_map(|(key, value)| match decode(value) {
            Some(value) => Some(KeyValue {
                key: key.clone(),
                value: Some(value),
                key_strindex: 0,
            }),
            None => {
                dropped += 1;
                None
            }
        })
        .collect();
    (decoded, dropped)
}

// A wasi:otel value is JSON text. `None` if it is not valid JSON or contains a
// `null`.
fn decode(json: &str) -> Option<AnyValue> {
    serde_json::from_str(json).ok().and_then(to_any_value)
}

fn to_any_value(json: serde_json::Value) -> Option<AnyValue> {
    use serde_json::Value as Json;
    let value = match json {
        Json::String(s) => any_value::Value::StringValue(s),
        Json::Bool(b) => any_value::Value::BoolValue(b),
        Json::Number(n) => match n.as_i64() {
            Some(i) => any_value::Value::IntValue(i),
            None if n.is_f64() => any_value::Value::DoubleValue(n.as_f64()?),
            None => return None,
        },
        Json::Array(items) => any_value::Value::ArrayValue(ArrayValue {
            values: items.into_iter().map(to_any_value).collect::<Option<_>>()?,
        }),
        Json::Object(entries) => any_value::Value::KvlistValue(KeyValueList {
            values: entries
                .into_iter()
                .map(|(key, value)| {
                    Some(KeyValue {
                        key,
                        value: Some(to_any_value(value)?),
                        key_strindex: 0,
                    })
                })
                .collect::<Option<_>>()?,
        }),
        Json::Null => return None,
    };
    Some(AnyValue { value: Some(value) })
}

export!(OtelAdapter);
