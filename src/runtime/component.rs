//! Runtime model for Component instances.
//!
//! [`ComponentInstance`] is an owned handle to one instantiated component. It
//! holds the wasmtime `Store` and `Instance` together since every operation on
//! an instance requires `&mut store`. Dropping the handle drops the store.

use anyhow::Result;
use wasmtime::Store;
use wasmtime::component::{
    ComponentExportIndex, Instance, Type as WasmtimeType, Val as WasmtimeVal,
};

use crate::runtime::conversion::{json_to_val, val_to_json};
use crate::types::{ComponentError, ComponentResource, ComponentState, Function, Val};

/// An owned handle to one instantiated component.
///
/// Obtained from `Runtime::instantiate`. Dropping it drops the underlying
/// store and any resources produced with this instance.
pub struct ComponentInstance {
    store: Store<ComponentState>,
    instance: Instance,
}

impl ComponentInstance {
    pub(crate) fn new(store: Store<ComponentState>, instance: Instance) -> Self {
        Self { store, instance }
    }

    /// Call an exported function on this instance.
    ///
    /// `function` identifies the export, including its interface when the
    /// function belongs to one. Arguments are JSON values or resource
    /// reference handles this instance produced.
    ///
    /// This covers every component model export shape. A resource method is
    /// represented as a function whose first parameter is the receiver, so can
    /// be called by passing the resource handle as that arg.
    pub async fn call(&mut self, function: &Function, args: Vec<Val>) -> Result<Option<Val>> {
        let export = self.resolve_function(function)?;
        let results = self
            .call_export(export, args, function.function_name())
            .await?;
        convert_results(
            results,
            function.returns_bytes(),
            function.returns_error_bytes(),
        )
    }

    // Resolve an exported function on an interface or directly at world-level.
    fn resolve_function(&mut self, function: &Function) -> Result<ComponentExportIndex> {
        let name = function.function_name();
        let export = match function.interface() {
            Some(_) => {
                let export_name = function.export_name();
                let interface_export = self
                    .instance
                    .get_export(&mut self.store, None, export_name)
                    .ok_or_else(|| anyhow::anyhow!("Export '{export_name}' not found"))?;
                self.instance
                    .get_export(&mut self.store, Some(&interface_export.1), name)
                    .ok_or_else(|| {
                        anyhow::anyhow!("Function '{name}' not found in export '{export_name}'")
                    })?
            }
            None => self
                .instance
                .get_export(&mut self.store, None, name)
                .ok_or_else(|| {
                    anyhow::anyhow!("Function '{name}' not found in component exports")
                })?,
        };
        Ok(export.1)
    }

    // Shared call path: convert args based on declared param types and run.
    async fn call_export(
        &mut self,
        export: ComponentExportIndex,
        args: Vec<Val>,
        name: &str,
    ) -> Result<Vec<WasmtimeVal>> {
        let func = self
            .instance
            .get_func(&mut self.store, export)
            .ok_or_else(|| anyhow::anyhow!("Function handle invalid for '{name}'"))?;

        let func_ty = func.ty(&self.store);
        let params: Vec<_> = func_ty.params().collect();
        if args.len() != params.len() {
            anyhow::bail!(
                "Wrong number of args for '{name}': expected {}, got {}",
                params.len(),
                args.len()
            );
        }

        let mut arg_vals: Vec<WasmtimeVal> = Vec::with_capacity(args.len());
        for (index, arg) in args.into_iter().enumerate() {
            let val = match arg {
                Val::Resource(resource) => WasmtimeVal::Resource(resource.resource),
                Val::Bytes(bytes) => {
                    let element_type = match &params[index].1 {
                        WasmtimeType::List(list) => list.ty(),
                        other => {
                            anyhow::bail!("parameter {index} is {other:?}, but bytes were provided")
                        }
                    };
                    if !matches!(element_type, WasmtimeType::U8) {
                        anyhow::bail!(
                            "parameter {index} is a list of {element_type:?}, but bytes were provided"
                        );
                    }
                    WasmtimeVal::List(bytes.into_iter().map(WasmtimeVal::U8).collect())
                }
                Val::Json(json) => json_to_val(&json, &params[index].1)
                    .map_err(|e| anyhow::anyhow!("Error converting parameter {index}: {e}"))?,
            };
            arg_vals.push(val);
        }

        let mut results = vec![WasmtimeVal::Bool(false); func_ty.results().len()];

        // `run_concurrent` keeps the async executor active so an async-typed
        // export can block on async imports (e.g. wasi:http) and be driven to
        // completion; `call_async` would trap if the call idles awaiting I/O.
        let call_result = self
            .store
            .run_concurrent(async |accessor| {
                func.call_concurrent(accessor, &arg_vals, &mut results)
                    .await
            })
            .await?;

        // A guest calling `wasi:cli/exit` surfaces as an `I32Exit` error.
        if let Err(e) = call_result {
            return match e.downcast_ref::<wasmtime_wasi::I32Exit>() {
                Some(wasmtime_wasi::I32Exit(0)) => Ok(Vec::new()),
                Some(wasmtime_wasi::I32Exit(code)) => {
                    Err(anyhow::anyhow!("component exited with code {code}"))
                }
                None => Err(e.into()),
            };
        }

        Ok(results)
    }
}

// Convert wasmtime `results` into a `Val`, or `None` if there are none. A
// resource remains a handle, a `list<u8>` becomes bytes, and any other value
// converts to JSON. The bytes flags represent the function's declared result
// type, since an empty list in `results` cannot show that it is a `list<u8>`.
//
// For a WIT `result<T, E>`, an `ok` value converts as described. An `err`
// returns as a [`ComponentError`] holding its value, converted the same way.
fn convert_results(
    results: Vec<WasmtimeVal>,
    as_bytes: bool,
    error_as_bytes: bool,
) -> Result<Option<Val>> {
    if results.len() > 1 {
        anyhow::bail!(
            "got {} results; a WIT function declares at most one",
            results.len()
        );
    }
    let Some(result) = results.first() else {
        return Ok(None);
    };
    match result {
        WasmtimeVal::Result(Ok(Some(ok_val))) => Ok(Some(convert_value(ok_val, as_bytes)?)),
        WasmtimeVal::Result(Ok(None)) => Ok(None),
        WasmtimeVal::Result(Err(Some(error_val))) => {
            let value = convert_value(error_val, error_as_bytes).map_err(|e| {
                anyhow::anyhow!("Component returned an error that cannot be converted: {e}")
            })?;
            Err(ComponentError { value: Some(value) }.into())
        }
        WasmtimeVal::Result(Err(None)) => Err(ComponentError { value: None }.into()),
        value => Ok(Some(convert_value(value, as_bytes)?)),
    }
}

fn convert_value(val: &WasmtimeVal, as_bytes: bool) -> Result<Val> {
    Ok(match val {
        WasmtimeVal::Resource(resource) => Val::Resource(ComponentResource {
            resource: *resource,
        }),
        WasmtimeVal::List(items) if as_bytes => {
            let mut bytes = Vec::with_capacity(items.len());
            for item in items {
                let WasmtimeVal::U8(byte) = item else {
                    unreachable!("a WIT list is homogeneous, and this one is declared list<u8>")
                };
                bytes.push(*byte);
            }
            Val::Bytes(bytes)
        }
        other => Val::Json(val_to_json(other)?),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn component_error(result: Result<Option<Val>>) -> ComponentError {
        result
            .err()
            .expect("the call should fail")
            .downcast::<ComponentError>()
            .expect("the error should be a ComponentError")
    }

    #[test]
    fn an_err_becomes_a_component_error_holding_its_value() {
        let results = vec![WasmtimeVal::Result(Err(Some(Box::new(
            WasmtimeVal::Record(vec![
                ("code".into(), WasmtimeVal::S32(-32602)),
                ("message".into(), WasmtimeVal::String("Unknown tool".into())),
            ]),
        ))))];
        let error = component_error(convert_results(results, false, false));
        assert_eq!(
            error.value.as_ref().and_then(Val::as_json),
            Some(&json!({"code": -32602, "message": "Unknown tool"}))
        );
    }

    #[test]
    fn an_err_declared_as_bytes_holds_bytes() {
        let results = vec![WasmtimeVal::Result(Err(Some(Box::new(WasmtimeVal::List(
            vec![WasmtimeVal::U8(1), WasmtimeVal::U8(2)],
        )))))];
        let error = component_error(convert_results(results, false, true));
        assert_eq!(
            error.value.as_ref().and_then(Val::as_bytes),
            Some(&[1, 2][..])
        );
    }

    #[test]
    fn an_err_with_no_error_type_holds_no_value() {
        let results = vec![WasmtimeVal::Result(Err(None))];
        let error = component_error(convert_results(results, false, false));
        assert!(error.value.is_none());
    }

    #[test]
    fn a_component_error_message_displays_its_value() {
        let error = ComponentError {
            value: Some(Val::Json(json!("denied"))),
        };
        assert_eq!(error.to_string(), r#"Component returned error: "denied""#);

        let error = ComponentError {
            value: Some(Val::Bytes(vec![1, 2])),
        };
        assert_eq!(error.to_string(), "Component returned error: <2 bytes>");
    }

    #[test]
    fn an_ok_value_converts_to_json() {
        let results = vec![WasmtimeVal::Result(Ok(Some(Box::new(
            WasmtimeVal::String("done".into()),
        ))))];
        let value = convert_results(results, false, false).unwrap().unwrap();
        assert_eq!(value.as_json(), Some(&json!("done")));
    }
}
