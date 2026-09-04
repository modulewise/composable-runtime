//! Conversion between JSON and wasmtime component values.

use anyhow::Result;
use wasmtime::component::{Type, Val};

pub(crate) fn json_to_val(json_value: &serde_json::Value, val_type: &Type) -> Result<Val> {
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

        // Numeric strings convert to numbers.
        (serde_json::Value::String(s), wasmtime::component::Type::U8) => Ok(Val::U8(
            s.parse::<u8>()
                .map_err(|_| anyhow::anyhow!("Invalid u8: {s}"))?,
        )),
        (serde_json::Value::String(s), wasmtime::component::Type::U16) => Ok(Val::U16(
            s.parse::<u16>()
                .map_err(|_| anyhow::anyhow!("Invalid u16: {s}"))?,
        )),
        (serde_json::Value::String(s), wasmtime::component::Type::U32) => Ok(Val::U32(
            s.parse::<u32>()
                .map_err(|_| anyhow::anyhow!("Invalid u32: {s}"))?,
        )),
        (serde_json::Value::String(s), wasmtime::component::Type::U64) => Ok(Val::U64(
            s.parse::<u64>()
                .map_err(|_| anyhow::anyhow!("Invalid u64: {s}"))?,
        )),
        (serde_json::Value::String(s), wasmtime::component::Type::S8) => Ok(Val::S8(
            s.parse::<i8>()
                .map_err(|_| anyhow::anyhow!("Invalid s8: {s}"))?,
        )),
        (serde_json::Value::String(s), wasmtime::component::Type::S16) => Ok(Val::S16(
            s.parse::<i16>()
                .map_err(|_| anyhow::anyhow!("Invalid s16: {s}"))?,
        )),
        (serde_json::Value::String(s), wasmtime::component::Type::S32) => Ok(Val::S32(
            s.parse::<i32>()
                .map_err(|_| anyhow::anyhow!("Invalid s32: {s}"))?,
        )),
        (serde_json::Value::String(s), wasmtime::component::Type::S64) => Ok(Val::S64(
            s.parse::<i64>()
                .map_err(|_| anyhow::anyhow!("Invalid s64: {s}"))?,
        )),
        (serde_json::Value::String(s), wasmtime::component::Type::Float32) => Ok(Val::Float32(
            s.parse::<f32>()
                .map_err(|_| anyhow::anyhow!("Invalid f32: {s}"))?,
        )),
        (serde_json::Value::String(s), wasmtime::component::Type::Float64) => Ok(Val::Float64(
            s.parse::<f64>()
                .map_err(|_| anyhow::anyhow!("Invalid f64: {s}"))?,
        )),

        // Number types - JSON number maps to all WIT numeric types
        (serde_json::Value::Number(n), wasmtime::component::Type::U8) => {
            let val = n
                .as_u64()
                .and_then(|v| u8::try_from(v).ok())
                .ok_or_else(|| anyhow::anyhow!("Invalid number for u8: {n}"))?;
            Ok(Val::U8(val))
        }
        (serde_json::Value::Number(n), wasmtime::component::Type::U16) => {
            let val = n
                .as_u64()
                .and_then(|v| u16::try_from(v).ok())
                .ok_or_else(|| anyhow::anyhow!("Invalid number for u16: {n}"))?;
            Ok(Val::U16(val))
        }
        (serde_json::Value::Number(n), wasmtime::component::Type::U32) => {
            let val = n
                .as_u64()
                .and_then(|v| u32::try_from(v).ok())
                .ok_or_else(|| anyhow::anyhow!("Invalid number for u32: {n}"))?;
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
                .and_then(|v| i8::try_from(v).ok())
                .ok_or_else(|| anyhow::anyhow!("Invalid number for s8: {n}"))?;
            Ok(Val::S8(val))
        }
        (serde_json::Value::Number(n), wasmtime::component::Type::S16) => {
            let val = n
                .as_i64()
                .and_then(|v| i16::try_from(v).ok())
                .ok_or_else(|| anyhow::anyhow!("Invalid number for s16: {n}"))?;
            Ok(Val::S16(val))
        }
        (serde_json::Value::Number(n), wasmtime::component::Type::S32) => {
            let val = n
                .as_i64()
                .and_then(|v| i32::try_from(v).ok())
                .ok_or_else(|| anyhow::anyhow!("Invalid number for s32: {n}"))?;
            Ok(Val::S32(val))
        }
        (serde_json::Value::Number(n), wasmtime::component::Type::S64) => {
            let val = n
                .as_i64()
                .ok_or_else(|| anyhow::anyhow!("Invalid number for s64: {n}"))?;
            Ok(Val::S64(val))
        }
        // Narrowing to f32 loses precision rather than failing.
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

        // Objects map to map<string, V>
        (serde_json::Value::Object(obj), wasmtime::component::Type::Map(map_type)) => {
            let key_type = map_type.key();
            let value_type = map_type.value();
            if !matches!(key_type, wasmtime::component::Type::String) {
                return Err(anyhow::anyhow!(
                    "JSON object can only map to a WIT map with string keys"
                ));
            }
            let mut entries = Vec::new();
            for (key, value) in obj {
                let val = json_to_val(value, &value_type).map_err(|e| {
                    anyhow::anyhow!("Error converting map value for key '{key}': {e}")
                })?;
                entries.push((Val::String(key.clone()), val));
            }
            Ok(Val::Map(entries))
        }

        // Arrays of [key, value] pairs map to map<non-string, V>
        (serde_json::Value::Array(arr), wasmtime::component::Type::Map(map_type)) => {
            let key_type = map_type.key();
            let value_type = map_type.value();
            let mut entries = Vec::new();
            for (index, item) in arr.iter().enumerate() {
                let pair = item.as_array().filter(|p| p.len() == 2).ok_or_else(|| {
                    anyhow::anyhow!("Map entry at index {index} must be a [key, value] pair")
                })?;
                let key = json_to_val(&pair[0], &key_type).map_err(|e| {
                    anyhow::anyhow!("Error converting map key at index {index}: {e}")
                })?;
                let val = json_to_val(&pair[1], &value_type).map_err(|e| {
                    anyhow::anyhow!("Error converting map value at index {index}: {e}")
                })?;
                entries.push((key, val));
            }
            Ok(Val::Map(entries))
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

        // Variants: {"type": "case-name"}, plus "value" when the case has a
        // payload. The payload fields do not collide with the case tag.
        (serde_json::Value::Object(obj), wasmtime::component::Type::Variant(variant_type)) => {
            let case_name = obj.get("type").and_then(|v| v.as_str()).ok_or_else(|| {
                anyhow::anyhow!("Variant object must have a \"type\" field with the case name")
            })?;

            let case = variant_type
                .cases()
                .find(|c| c.name == case_name)
                .ok_or_else(|| {
                    let valid: Vec<_> = variant_type.cases().map(|c| c.name.to_string()).collect();
                    anyhow::anyhow!("Unknown variant case '{case_name}'. Valid cases: {valid:?}")
                })?;

            for key in obj.keys() {
                if key != "type" && key != "value" {
                    return Err(anyhow::anyhow!(
                        "Unexpected field '{key}' in variant. A variant object \
                         has only \"type\" and, when the case has a payload, \
                         \"value\""
                    ));
                }
            }

            let payload = match &case.ty {
                Some(payload_type) => {
                    let value = obj.get("value").ok_or_else(|| {
                        anyhow::anyhow!(
                            "Variant case '{case_name}' has a payload, which must be \
                             under a \"value\" key"
                        )
                    })?;
                    Some(json_to_val(value, payload_type)?)
                }
                None => {
                    if obj.contains_key("value") {
                        return Err(anyhow::anyhow!(
                            "Variant case '{case_name}' has no payload, so it must \
                             not have a \"value\" key"
                        ));
                    }
                    None
                }
            };

            Ok(Val::Variant(case_name.to_string(), payload.map(Box::new)))
        }

        // Enums: plain string matching a case name
        (serde_json::Value::String(s), wasmtime::component::Type::Enum(enum_type)) => {
            if enum_type.names().any(|name| name == s.as_str()) {
                Ok(Val::Enum(s.clone()))
            } else {
                let valid: Vec<_> = enum_type.names().map(|n| n.to_string()).collect();
                Err(anyhow::anyhow!(
                    "Unknown enum value '{s}'. Valid values: {valid:?}"
                ))
            }
        }

        // Results: {"ok": value} or {"error": value}
        (serde_json::Value::Object(obj), wasmtime::component::Type::Result(result_type)) => {
            if let Some(ok_val) = obj.get("ok") {
                let val = match result_type.ok() {
                    Some(ok_type) => Some(Box::new(json_to_val(ok_val, &ok_type)?)),
                    None => None,
                };
                Ok(Val::Result(Ok(val)))
            } else if let Some(err_val) = obj.get("error") {
                let val = match result_type.err() {
                    Some(err_type) => Some(Box::new(json_to_val(err_val, &err_type)?)),
                    None => None,
                };
                Ok(Val::Result(Err(val)))
            } else {
                Err(anyhow::anyhow!(
                    "Result object must have either an \"ok\" or \"error\" field"
                ))
            }
        }

        // Flags: array of flag name strings
        (serde_json::Value::Array(arr), wasmtime::component::Type::Flags(flags_type)) => {
            let valid_names: Vec<_> = flags_type.names().map(|n| n.to_string()).collect();
            let mut flag_names = Vec::new();
            for item in arr {
                let name = item
                    .as_str()
                    .ok_or_else(|| anyhow::anyhow!("Flag values must be strings, got: {item:?}"))?;
                if !valid_names.iter().any(|n| n == name) {
                    return Err(anyhow::anyhow!(
                        "Unknown flag '{name}'. Valid flags: {valid_names:?}"
                    ));
                }
                flag_names.push(name.to_string());
            }
            Ok(Val::Flags(flag_names))
        }

        // Types that cannot convert. Distinguished from type mismatch below.
        (_, Type::Own(_) | Type::Borrow(_)) => {
            Err(anyhow::anyhow!("cannot convert JSON to a resource"))
        }
        (_, Type::Future(_)) => Err(anyhow::anyhow!("cannot convert JSON to a future")),
        (_, Type::Stream(_)) => Err(anyhow::anyhow!("cannot convert JSON to a stream")),
        (_, Type::ErrorContext) => Err(anyhow::anyhow!("cannot convert JSON to an error-context")),

        // Type mismatches
        _ => Err(anyhow::anyhow!(
            "Type mismatch: cannot convert JSON {json_value:?} to WIT type {val_type:?}"
        )),
    }
}

pub(crate) fn val_to_json(val: &Val) -> Result<serde_json::Value> {
    let json = match val {
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
            let mut json_items = Vec::with_capacity(items.len());
            for item in items {
                json_items.push(val_to_json(item)?);
            }
            serde_json::Value::Array(json_items)
        }

        Val::Map(entries) => {
            // map<string, V> -> JSON object; map<non-string, V> -> array of
            // [key, val] pairs (preserving non-string keys).
            if entries.iter().all(|(k, _)| matches!(k, Val::String(_))) {
                let mut obj = serde_json::Map::new();
                for (k, v) in entries {
                    if let Val::String(key) = k {
                        obj.insert(key.clone(), val_to_json(v)?);
                    }
                }
                serde_json::Value::Object(obj)
            } else {
                let mut pairs = Vec::with_capacity(entries.len());
                for (k, v) in entries {
                    pairs.push(serde_json::Value::Array(vec![
                        val_to_json(k)?,
                        val_to_json(v)?,
                    ]));
                }
                serde_json::Value::Array(pairs)
            }
        }

        Val::Record(fields) => {
            let mut obj = serde_json::Map::new();
            for (name, val) in fields {
                obj.insert(name.clone(), val_to_json(val)?);
            }
            serde_json::Value::Object(obj)
        }

        // Options
        Val::Option(opt) => match opt {
            Some(val) => val_to_json(val)?,
            None => serde_json::Value::Null,
        },

        Val::Tuple(vals) => {
            let mut json_items = Vec::with_capacity(vals.len());
            for val in vals {
                json_items.push(val_to_json(val)?);
            }
            serde_json::Value::Array(json_items)
        }

        Val::Variant(name, val) => {
            let mut obj = serde_json::Map::new();
            obj.insert("type".to_string(), serde_json::Value::String(name.clone()));
            if let Some(v) = val {
                obj.insert("value".to_string(), val_to_json(v)?);
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
                    obj.insert("ok".to_string(), val_to_json(v)?);
                }
                Ok(None) => {
                    obj.insert("ok".to_string(), serde_json::Value::Null);
                }
                Err(Some(v)) => {
                    obj.insert("error".to_string(), val_to_json(v)?);
                }
                Err(None) => {
                    obj.insert("error".to_string(), serde_json::Value::Null);
                }
            }
            serde_json::Value::Object(obj)
        }

        Val::Resource(_) => {
            anyhow::bail!("cannot convert a resource to JSON")
        }
        Val::Future(_) => {
            anyhow::bail!("cannot convert a future to JSON")
        }
        Val::Stream(_) => {
            anyhow::bail!("cannot convert a stream to JSON")
        }
        Val::ErrorContext(_) => {
            anyhow::bail!("cannot convert an error-context to JSON")
        }
    };
    Ok(json)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // A variant has case name as "type" and payload, if any, as "value".

    #[test]
    fn a_variant_without_a_payload_has_type_only() {
        let val = Val::Variant("pending".into(), None);
        assert_eq!(val_to_json(&val).unwrap(), json!({"type": "pending"}));
    }

    #[test]
    fn a_primitive_payload_on_a_variant_is_represented_as_value() {
        let val = Val::Variant("count".into(), Some(Box::new(Val::U32(7))));
        assert_eq!(
            val_to_json(&val).unwrap(),
            json!({"type": "count", "value": 7})
        );
    }

    #[test]
    fn a_record_payload_on_a_variant_nests_under_value() {
        let val = Val::Variant(
            "created".into(),
            Some(Box::new(Val::Record(vec![
                ("id".into(), Val::U32(1)),
                ("name".into(), Val::String("widget".into())),
            ]))),
        );
        assert_eq!(
            val_to_json(&val).unwrap(),
            json!({"type": "created", "value": {"id": 1, "name": "widget"}})
        );
    }

    #[test]
    fn a_variant_payload_may_contain_a_type_field() {
        // A field named "type" does not collide with the case tag.
        let val = Val::Variant(
            "created".into(),
            Some(Box::new(Val::Record(vec![
                ("type".into(), Val::String("widget".into())),
                ("id".into(), Val::U32(1)),
            ]))),
        );
        assert_eq!(
            val_to_json(&val).unwrap(),
            json!({"type": "created", "value": {"type": "widget", "id": 1}})
        );
    }

    // Numbers outside a type's range must be rejected, not silently truncated.

    #[test]
    fn a_number_above_the_u8_range_is_rejected() {
        assert!(json_to_val(&json!(300), &Type::U8).is_err());
    }

    #[test]
    fn a_number_above_the_u16_range_is_rejected() {
        assert!(json_to_val(&json!(70_000), &Type::U16).is_err());
    }

    #[test]
    fn a_number_above_the_u32_range_is_rejected() {
        assert!(json_to_val(&json!(5_000_000_000_u64), &Type::U32).is_err());
    }

    #[test]
    fn a_number_above_the_s8_range_is_rejected() {
        assert!(json_to_val(&json!(200), &Type::S8).is_err());
    }

    #[test]
    fn a_number_below_the_s8_range_is_rejected() {
        assert!(json_to_val(&json!(-200), &Type::S8).is_err());
    }

    #[test]
    fn a_number_above_the_s16_range_is_rejected() {
        assert!(json_to_val(&json!(40_000), &Type::S16).is_err());
    }

    #[test]
    fn a_number_above_the_s32_range_is_rejected() {
        assert!(json_to_val(&json!(3_000_000_000_u64), &Type::S32).is_err());
    }

    #[test]
    fn a_negative_number_for_an_unsigned_type_is_rejected() {
        assert!(json_to_val(&json!(-1), &Type::U8).is_err());
    }

    #[test]
    fn numbers_within_range_convert() {
        assert_eq!(json_to_val(&json!(255), &Type::U8).unwrap(), Val::U8(255));
        assert_eq!(json_to_val(&json!(-128), &Type::S8).unwrap(), Val::S8(-128));
        assert_eq!(json_to_val(&json!(127), &Type::S8).unwrap(), Val::S8(127));
    }

    #[test]
    fn a_string_input_agrees_with_a_number_input_on_range() {
        // The two paths agree on what is representable within range.
        assert!(json_to_val(&json!("300"), &Type::U8).is_err());
        assert!(json_to_val(&json!("255"), &Type::U8).is_ok());
    }
}
