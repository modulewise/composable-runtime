//! Converts wasi:otel log-records to OTLP protobuf and sends to gRPC endpoint

#![no_main]

wit_bindgen::generate!({
    path: "../wit",
    world: "otel-to-grpc",
    generate_all,
});

struct OtelAdapter;

use exports::wasi::otel::logs::{Guest as LogsGuest, LogRecord};
use modulewise::grpc::endpoint;
use opentelemetry_proto::tonic::collector::logs::v1::ExportLogsServiceRequest;
use opentelemetry_proto::tonic::common::v1::{AnyValue, KeyValue, any_value};
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
        .map(|r| OtlpResource {
            attributes: r
                .attributes
                .iter()
                .map(|kv| KeyValue {
                    key: kv.key.clone(),
                    value: Some(AnyValue {
                        value: Some(any_value::Value::StringValue(kv.value.clone())),
                    }),
                })
                .collect(),
            dropped_attributes_count: 0,
        })
        .unwrap_or_else(|| OtlpResource {
            attributes: vec![KeyValue {
                key: "service.name".to_string(),
                value: Some(AnyValue {
                    value: Some(any_value::Value::StringValue("unknown_service".to_string())),
                }),
            }],
            dropped_attributes_count: 0,
        });

    let scope = data.instrumentation_scope.as_ref().map(|s| {
        opentelemetry_proto::tonic::common::v1::InstrumentationScope {
            name: s.name.clone(),
            version: s.version.clone().unwrap_or_default(),
            attributes: s
                .attributes
                .iter()
                .map(|kv| KeyValue {
                    key: kv.key.clone(),
                    value: Some(AnyValue {
                        value: Some(any_value::Value::StringValue(kv.value.clone())),
                    }),
                })
                .collect(),
            dropped_attributes_count: 0,
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

    let body = data.body.as_ref().map(|b| AnyValue {
        value: Some(any_value::Value::StringValue(b.clone())),
    });

    let attributes = data
        .attributes
        .as_ref()
        .map(|attrs| {
            attrs
                .iter()
                .map(|kv| KeyValue {
                    key: kv.key.clone(),
                    value: Some(AnyValue {
                        value: Some(any_value::Value::StringValue(kv.value.clone())),
                    }),
                })
                .collect()
        })
        .unwrap_or_default();

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
        dropped_attributes_count: 0,
        flags: 0,
        trace_id,
        span_id,
        event_name: data.event_name.clone().unwrap_or_default(),
    }
}

export!(OtelAdapter);
