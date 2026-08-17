//! Runtime model for Component instances.
//!
//! [`ComponentInstance`] is an owned handle to one instantiated component. It
//! holds the wasmtime `Store` and `Instance` together since every operation on
//! an instance requires `&mut store`. Dropping the handle drops the store.

use anyhow::Result;
use wasmtime::Store;
use wasmtime::component::{ComponentExportIndex, Instance, ResourceAny, Val as WasmtimeVal};

use crate::runtime::conversion::{json_to_val, val_to_json};
use crate::types::{ComponentState, Function};

/// A value crossing the call boundary, either as JSON (passed by value) or as
/// a Component-owned Resource (passed by reference).
pub enum Val {
    /// Data passed by value.
    Json(serde_json::Value),
    /// Reference to a resource owned by a component instance.
    Resource(ComponentResource),
}

impl Val {
    /// The JSON value, if this is data.
    pub fn as_json(&self) -> Option<&serde_json::Value> {
        match self {
            Self::Json(value) => Some(value),
            Self::Resource(_) => None,
        }
    }

    /// The reference handle, if this is a resource.
    pub fn as_resource(&self) -> Option<&ComponentResource> {
        match self {
            Self::Resource(resource) => Some(resource),
            Self::Json(_) => None,
        }
    }
}

impl From<serde_json::Value> for Val {
    fn from(value: serde_json::Value) -> Self {
        Self::Json(value)
    }
}

impl From<ComponentResource> for Val {
    fn from(resource: ComponentResource) -> Self {
        Self::Resource(resource)
    }
}

/// A reference handle to a resource owned by a [`ComponentInstance`].
///
/// Can only be used with its owning instance, as the "receiver" (first arg) of
/// a [`ComponentInstance::call`] or as an arg passed to another call. It is no
/// longer valid once the owning instance drops.
#[derive(Clone, Copy)]
pub struct ComponentResource {
    resource: ResourceAny,
}

/// An owned handle to one instantiated component.
///
/// Obtained from `Runtime::instantiate`. Dropping it drops the underlying
/// store, reclaiming the instance and any resources it produced.
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
        convert_results(results)
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
// resource remains a handle; everything else converts to JSON.
fn convert_results(results: Vec<WasmtimeVal>) -> Result<Option<Val>> {
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
        WasmtimeVal::Result(Ok(Some(ok_val))) => Ok(Some(convert_result(ok_val)?)),
        WasmtimeVal::Result(Ok(None)) => Ok(None),
        WasmtimeVal::Result(Err(Some(error_val))) => {
            let error_json =
                val_to_json(error_val).map_or_else(|e| format!("<{e}>"), |json| json.to_string());
            anyhow::bail!("Component returned error: {error_json}")
        }
        WasmtimeVal::Result(Err(None)) => anyhow::bail!("Component returned error"),
        value => Ok(Some(convert_result(value)?)),
    }
}

fn convert_result(val: &WasmtimeVal) -> Result<Val> {
    Ok(match val {
        WasmtimeVal::Resource(resource) => Val::Resource(ComponentResource {
            resource: *resource,
        }),
        other => Val::Json(val_to_json(other)?),
    })
}
