//! Decodes `wasi:otel` values. A value is JSON text, as the `types` interface
//! specifies, and decodes to the typed value it encodes.

use std::collections::HashMap;

use opentelemetry::logs::AnyValue;
use opentelemetry::{Array, Key, StringValue, Value};
use serde_json::{Number, Value as Json};

/// An attribute value as a string, bool, integer, double, or an array of one
/// of those. `None` if the text does not match one of those as JSON.
pub(crate) fn attribute_value(json: &str) -> Option<Value> {
    match serde_json::from_str(json).ok()? {
        Json::String(s) => Some(Value::String(s.into())),
        Json::Bool(b) => Some(Value::Bool(b)),
        Json::Number(n) => match number(&n)? {
            Num::Int(i) => Some(Value::I64(i)),
            Num::Double(d) => Some(Value::F64(d)),
        },
        Json::Array(items) => array(items).map(Value::Array),
        Json::Null | Json::Object(_) => None,
    }
}

/// A log body or log attribute value, which may be a list of any values or a
/// map. `None` if the text is not valid JSON or contains a `null`.
pub(crate) fn log_value(json: &str) -> Option<AnyValue> {
    any_value(serde_json::from_str(json).ok()?)
}

enum Num {
    Int(i64),
    Double(f64),
}

// An integer that fits in an i64, or a double. An integer beyond i64's range
// has no OTel type.
fn number(n: &Number) -> Option<Num> {
    match n.as_i64() {
        Some(i) => Some(Num::Int(i)),
        None if n.is_f64() => n.as_f64().map(Num::Double),
        None => None,
    }
}

// An array whose items all have the same type. Integers and doubles together
// are all doubles. An empty array is an empty array of strings.
fn array(items: Vec<Json>) -> Option<Array> {
    match items.first() {
        None => Some(Array::String(Vec::new())),
        Some(Json::String(_)) => items
            .into_iter()
            .map(|item| match item {
                Json::String(s) => Some(StringValue::from(s)),
                _ => None,
            })
            .collect::<Option<_>>()
            .map(Array::String),
        Some(Json::Bool(_)) => items
            .into_iter()
            .map(|item| item.as_bool())
            .collect::<Option<_>>()
            .map(Array::Bool),
        Some(Json::Number(_)) => {
            let numbers = items
                .iter()
                .map(|item| item.as_number().and_then(number))
                .collect::<Option<Vec<_>>>()?;
            let ints = numbers
                .iter()
                .map(|n| match n {
                    Num::Int(i) => Some(*i),
                    Num::Double(_) => None,
                })
                .collect::<Option<Vec<_>>>();
            Some(match ints {
                Some(ints) => Array::I64(ints),
                None => Array::F64(
                    numbers
                        .iter()
                        .map(|n| match n {
                            Num::Int(i) => *i as f64,
                            Num::Double(d) => *d,
                        })
                        .collect(),
                ),
            })
        }
        Some(_) => None,
    }
}

fn any_value(json: Json) -> Option<AnyValue> {
    match json {
        Json::String(s) => Some(AnyValue::String(s.into())),
        Json::Bool(b) => Some(AnyValue::Boolean(b)),
        Json::Number(n) => match number(&n)? {
            Num::Int(i) => Some(AnyValue::Int(i)),
            Num::Double(d) => Some(AnyValue::Double(d)),
        },
        Json::Array(items) => items
            .into_iter()
            .map(any_value)
            .collect::<Option<Vec<_>>>()
            .map(|values| AnyValue::ListAny(Box::new(values))),
        Json::Object(entries) => entries
            .into_iter()
            .map(|(key, value)| Some((Key::from(key), any_value(value)?)))
            .collect::<Option<HashMap<_, _>>>()
            .map(|map| AnyValue::Map(Box::new(map))),
        Json::Null => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scalars_decode_to_their_types() {
        assert_eq!(
            attribute_value(r#""chat""#),
            Some(Value::String("chat".into()))
        );
        assert_eq!(attribute_value("true"), Some(Value::Bool(true)));
        assert_eq!(attribute_value("12"), Some(Value::I64(12)));
        assert_eq!(attribute_value("-3"), Some(Value::I64(-3)));
        assert_eq!(attribute_value("0.5"), Some(Value::F64(0.5)));
    }

    #[test]
    fn arrays_decode_when_their_items_share_a_type() {
        assert_eq!(
            attribute_value(r#"["stop", "length"]"#),
            Some(Value::Array(Array::String(vec![
                "stop".into(),
                "length".into()
            ])))
        );
        assert_eq!(
            attribute_value("[true, false]"),
            Some(Value::Array(Array::Bool(vec![true, false])))
        );
        assert_eq!(
            attribute_value("[1, 2]"),
            Some(Value::Array(Array::I64(vec![1, 2])))
        );
        assert_eq!(
            attribute_value("[1, 2.5]"),
            Some(Value::Array(Array::F64(vec![1.0, 2.5])))
        );
        assert_eq!(
            attribute_value("[]"),
            Some(Value::Array(Array::String(Vec::new())))
        );
    }

    #[test]
    fn other_attribute_values_are_invalid() {
        for json in [
            // Not JSON: a plain string must be quoted.
            "chat",
            "",
            "null",
            r#"{"a": 1}"#,
            r#"["a", 1]"#,
            "[[1]]",
            "[null]",
            // Beyond i64's range.
            "18446744073709551615",
        ] {
            assert_eq!(attribute_value(json), None, "{json}");
        }
    }

    #[test]
    fn log_values_may_be_lists_and_maps() {
        assert_eq!(
            log_value(r#"["a", 1]"#),
            Some(AnyValue::ListAny(Box::new(vec![
                AnyValue::String("a".into()),
                AnyValue::Int(1),
            ])))
        );
        assert_eq!(
            log_value(r#"{"n": 1.5}"#),
            Some(AnyValue::Map(Box::new(HashMap::from([(
                Key::from("n"),
                AnyValue::Double(1.5)
            )]))))
        );
    }

    #[test]
    fn a_log_value_with_a_null_is_invalid() {
        assert_eq!(log_value("null"), None);
        assert_eq!(log_value(r#"{"a": null}"#), None);
        assert_eq!(log_value("[1, null]"), None);
        assert_eq!(log_value("message"), None);
    }
}
