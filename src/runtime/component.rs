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
use crate::types::{ComponentResource, ComponentState, Function, Val};

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
        convert_results(results, function.returns_bytes())
    }

    // Resolve an exported function on an interface or directly at world-level.
    fn resolve_function(&mut self, function: &Function) -> Result<ComponentExportIndex> {
        let name = function.function_name();
        let export = match function.interface() {
            Some(interface) => {
                let interface_name = interface.as_str();
                let interface_export = self
                    .instance
                    .get_export(&mut self.store, None, interface_name)
                    .ok_or_else(|| anyhow::anyhow!("Interface '{interface_name}' not found"))?;
                self.instance
                    .get_export(&mut self.store, Some(&interface_export.1), name)
                    .ok_or_else(|| {
                        anyhow::anyhow!(
                            "Function '{name}' not found in interface '{interface_name}'"
                        )
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

// Convert a call's wasmtime results into [`Val`]s.
//
// A WIT `result<T, E>` maps onto Rust's `Result`: an `err` becomes an `Err`
// here, so callers handle failure before ever checking for an `ok` value. A
// resource remains a handle, a `list<u8>` becomes bytes, and everything else
// converts to JSON.
//
// `as_bytes` is determined from the function's declared result type rather
// than the returned values, so an empty `list<u8>` can be recognized as bytes.
fn convert_results(results: Vec<WasmtimeVal>, as_bytes: bool) -> Result<Option<Val>> {
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
            let error_json =
                val_to_json(error_val).map_or_else(|e| format!("<{e}>"), |json| json.to_string());
            anyhow::bail!("Component returned error: {error_json}")
        }
        WasmtimeVal::Result(Err(None)) => anyhow::bail!("Component returned error"),
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
