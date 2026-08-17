//! Conversion between JSON and wasmtime component values.

use anyhow::Result;
use wasmtime::component::{Type, Val};

use crate::types::Function;

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

        // String-to-number coercion (e.g. CLI arguments)
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

        // Variants: {"type": "case-name", "value": payload} or {"type": "case-name", ...fields}
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

            let payload = match &case.ty {
                Some(payload_type) => {
                    // Try "value" key first, then try reconstructing from remaining fields
                    let payload_json = if let Some(value) = obj.get("value") {
                        value.clone()
                    } else {
                        // Collect all fields except "variant" into an object
                        let mut payload_obj = serde_json::Map::new();
                        for (k, v) in obj {
                            if k != "variant" {
                                payload_obj.insert(k.clone(), v.clone());
                            }
                        }
                        serde_json::Value::Object(payload_obj)
                    };
                    Some(json_to_val(&payload_json, payload_type)?)
                }
                None => None,
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

        // Type mismatches
        _ => Err(anyhow::anyhow!(
            "Type mismatch: cannot convert JSON {json_value:?} to WIT type {val_type:?}"
        )),
    }
}

pub(crate) fn val_to_json(val: &Val) -> serde_json::Value {
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

        Val::Map(entries) => {
            // map<string, V> -> JSON object; map<non-string, V> -> array of
            // [key, val] pairs (preserving non-string keys).
            if entries.iter().all(|(k, _)| matches!(k, Val::String(_))) {
                let mut obj = serde_json::Map::new();
                for (k, v) in entries {
                    if let Val::String(key) = k {
                        obj.insert(key.clone(), val_to_json(v));
                    }
                }
                serde_json::Value::Object(obj)
            } else {
                let pairs: Vec<serde_json::Value> = entries
                    .iter()
                    .map(|(k, v)| serde_json::Value::Array(vec![val_to_json(k), val_to_json(v)]))
                    .collect();
                serde_json::Value::Array(pairs)
            }
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

// This handles the case where wasmtime decomposes tuples/records into separate Val objects
pub(crate) fn reconstruct_wit_return(
    results: &[Val],
    function: &Function,
) -> Result<serde_json::Value> {
    // Check if this is a record that needs field mapping to reconstruct as an object
    if let Some(return_schema) = function.result()
        && let Some(schema_obj) = return_schema.as_object()
        && schema_obj.get("type").and_then(|t| t.as_str()) == Some("object")
        && schema_obj.contains_key("properties")
    {
        return reconstruct_record(results, schema_obj);
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
