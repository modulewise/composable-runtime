//! WAT code generation: produces the 3 core modules (main, shim, fixup)
//! and derives ABI context for intercepted functions.

use std::fmt::Write;

use anyhow::Result;
use wit_parser::{Resolve, Type};

use crate::types::*;

// ============================================================
// Pre-computed context for intercepted functions
// ============================================================

/// Pre-computed ABI context for a single intercepted function.
///
/// Produced by `derive_intercepted_function` which computes flat types,
/// discriminants, retarea usage, and import module names from the enriched
/// `FunctionExport` data. All downstream WAT generation operates on a
/// flat `Vec<InterceptedFunction>`.
pub struct InterceptedFunction {
    /// Core module export name: "iface_full_name#func_name" for interface-bound,
    /// "func_name" for direct world-level functions.
    pub export_name: String,
    /// Core module import module name: "target-{iface_idx}" for interface-bound,
    /// "target-func" for direct world-level functions.
    pub import_module: String,
    /// Function name (from WIT).
    pub func_name: String,
    /// Parameter names and types (from WIT).
    pub params: Vec<(String, Type)>,
    /// Result type (from WIT).
    pub result: Option<Type>,
    /// Core param types for canon lower (GuestImport ABI).
    pub core_params_lower: Vec<&'static str>,
    /// Core result types for canon lower (GuestImport ABI).
    pub core_results_lower: Vec<&'static str>,
    /// Core param types for canon lift (GuestExport ABI).
    pub core_params_lift: Vec<&'static str>,
    /// Core result types for canon lift (GuestExport ABI).
    pub core_results_lift: Vec<&'static str>,
    /// Per-param core flat types (for marshaling individual args).
    pub param_flat_types: Vec<Vec<&'static str>>,
    /// Result flat types (empty if no result).
    pub result_flat_types: Vec<&'static str>,
    /// Whether the result uses a retarea (indirect return via pointer).
    pub uses_retarea: bool,
    /// Per-param value discriminants for the advice protocol.
    pub param_discriminants: Vec<u8>,
    /// Result value discriminant (if has result).
    pub result_discriminant: Option<u8>,
    /// Whether the function needs memory+realloc for canon lower.
    pub needs_memory: bool,
}

const MAX_FLAT_PARAMS: usize = 16;
const MAX_FLAT_RESULTS: usize = 1;

// Value variant discriminants for the advice protocol.
const DISC_STRING: u8 = 0;
const DISC_SIGNED: u8 = 1;
const DISC_UNSIGNED: u8 = 2;
const DISC_F32: u8 = 3;
const DISC_F64: u8 = 4;
const DISC_BOOL: u8 = 5;
const DISC_COMPLEX: u8 = 6;

// ============================================================
// Public entry point
// ============================================================

/// Generate the 3 core modules and the intercepted function list.
///
/// Walks `target.exports` once to build `Vec<InterceptedFunction>`, then
/// generates the main, shim, and fixup modules.
///
/// Returns (intercepted, main_bytes, shim_bytes, fixup_bytes).
pub fn generate_modules(
    target: &TargetWorld,
) -> Result<(Vec<InterceptedFunction>, Vec<u8>, Vec<u8>, Vec<u8>)> {
    let resolve = target.resolve();
    let intercepted = collect_intercepted(target, resolve)?;

    let main_bytes = generate_main_module(target, &intercepted)?;
    let shim_bytes = generate_shim_module(&intercepted)?;
    let fixup_bytes = generate_fixup_module(&intercepted)?;

    Ok((intercepted, main_bytes, shim_bytes, fixup_bytes))
}

// ============================================================
// Intercepted function derivation
// ============================================================

// Walk the target world's exports once, producing a flat list of intercepted
// functions with all context needed for WAT generation.
fn collect_intercepted(
    target: &TargetWorld,
    resolve: &Resolve,
) -> Result<Vec<InterceptedFunction>> {
    let mut result = Vec::new();
    let mut iface_idx = 0usize;

    for we in &target.exports {
        match we {
            WorldExport::Interface(ie) => {
                for fe in &ie.functions {
                    if let FunctionExport::Intercepted(ifn) = fe {
                        let export_name = format!("{}#{}", ie.full_name, ifn.name);
                        let import_module = format!("target-{iface_idx}");
                        result.push(derive_intercepted_function(
                            resolve,
                            export_name,
                            import_module,
                            ifn,
                        )?);
                    }
                }
                iface_idx += 1;
            }
            WorldExport::Function(fe) => {
                if let FunctionExport::Intercepted(ifn) = fe {
                    result.push(derive_intercepted_function(
                        resolve,
                        ifn.name.clone(),
                        "target-func".to_string(),
                        ifn,
                    )?);
                }
            }
        }
    }
    Ok(result)
}

// Derive an InterceptedFunction, pre-computing all ABI info from the function data.
fn derive_intercepted_function(
    resolve: &Resolve,
    export_name: String,
    import_module: String,
    ifn: &wit_parser::Function,
) -> Result<InterceptedFunction> {
    let param_flat_types: Vec<Vec<&'static str>> = ifn
        .params
        .iter()
        .map(|(_, ty)| flat_types(resolve, *ty))
        .collect::<Result<_>>()?;

    let result_flat_types: Vec<&'static str> = ifn
        .result
        .map(|ty| flat_types(resolve, ty))
        .transpose()?
        .unwrap_or_default();

    // Canon lower (GuestImport): all param flats, then retptr if results > MAX_FLAT_RESULTS
    let mut core_params_lower: Vec<&'static str> = param_flat_types
        .iter()
        .flat_map(|v| v.iter().copied())
        .collect();
    let uses_retarea = result_flat_types.len() > MAX_FLAT_RESULTS;
    let core_results_lower: Vec<&'static str> = if uses_retarea {
        core_params_lower.push("i32"); // retptr
        Vec::new()
    } else {
        result_flat_types.clone()
    };

    // Canon lift (GuestExport): indirect params if > MAX_FLAT_PARAMS, retptr for result
    let all_param_flats: Vec<&'static str> = param_flat_types
        .iter()
        .flat_map(|v| v.iter().copied())
        .collect();
    let core_params_lift = if all_param_flats.len() > MAX_FLAT_PARAMS {
        vec!["i32"] // indirect params pointer
    } else {
        all_param_flats
    };
    let core_results_lift = if uses_retarea {
        vec!["i32"] // retptr
    } else {
        result_flat_types.clone()
    };

    let param_discriminants: Vec<u8> = ifn
        .params
        .iter()
        .map(|(_, ty)| value_discriminant(resolve, *ty))
        .collect();

    let result_discriminant = ifn.result.map(|ty| value_discriminant(resolve, ty));

    let needs_memory = func_needs_memory(resolve, ifn);

    Ok(InterceptedFunction {
        export_name,
        import_module,
        func_name: ifn.name.clone(),
        params: ifn.params.clone(),
        result: ifn.result,
        core_params_lower,
        core_results_lower,
        core_params_lift,
        core_results_lift,
        param_flat_types,
        result_flat_types,
        uses_retarea,
        param_discriminants,
        result_discriminant,
        needs_memory,
    })
}

// ============================================================
// WAT module generators
// ============================================================

// Generate a minimal cabi_realloc implementation.
fn generate_realloc(wat: &mut String) {
    wat.push_str(r#"    (func (export "cabi_realloc") (param $old_ptr i32) (param $old_len i32) (param $align i32) (param $new_len i32) (result i32)
      (local $ptr i32)
      (local.set $ptr (global.get $heap))
      (local.set $ptr (i32.and
        (i32.add (local.get $ptr) (i32.sub (local.get $align) (i32.const 1)))
        (i32.sub (i32.const 0) (local.get $align))))
      (global.set $heap (i32.add (local.get $ptr) (local.get $new_len)))
      (local.get $ptr)
    )
"#);
}

// Generate the shim module.
//
// Entries 0-2: advice methods (constructor, before, after)
// Entries 3..3+N: intercepted target functions (across all interfaces)
fn generate_shim_module(intercepted: &[InterceptedFunction]) -> Result<Vec<u8>> {
    let n = intercepted.len();
    let total = 3 + n;
    let mut wat = String::new();
    wat.push_str(
        r#"(module
  (type $t_ctor (func (param i32 i32 i32 i32) (result i32)))
  (type $t_before (func (param i32 i32)))
  (type $t_after (func (param i32 i32 i32 i64 i32 i32)))
"#,
    );

    // Types for intercepted target functions
    for (idx, ifunc) in intercepted.iter().enumerate() {
        write!(wat, "  (type $t_target_{idx} (func")?;
        write_core_func_type(
            &mut wat,
            &ifunc.core_params_lower,
            &ifunc.core_results_lower,
        )?;
        writeln!(wat, "))")?;
    }

    write!(
        wat,
        r#"  (table (export "$imports") {total} {total} funcref)
  (func (export "0") (type $t_ctor)
    (local.get 0) (local.get 1) (local.get 2) (local.get 3)
    (i32.const 0) (call_indirect (type $t_ctor)))
  (func (export "1") (type $t_before)
    (local.get 0) (local.get 1)
    (i32.const 1) (call_indirect (type $t_before)))
  (func (export "2") (type $t_after)
    (local.get 0) (local.get 1) (local.get 2) (local.get 3) (local.get 4) (local.get 5)
    (i32.const 2) (call_indirect (type $t_after)))
"#
    )?;

    // Shim functions 3..3+N: intercepted target functions
    for (idx, ifunc) in intercepted.iter().enumerate() {
        let entry = 3 + idx;
        let n_params = ifunc.core_params_lower.len();
        writeln!(wat, "  (func (export \"{entry}\") (type $t_target_{idx})")?;
        if n_params > 0 {
            write!(wat, "    ")?;
            for p in 0..n_params {
                if p > 0 {
                    write!(wat, " ")?;
                }
                write!(wat, "(local.get {p})")?;
            }
            writeln!(wat)?;
        }
        writeln!(
            wat,
            "    (i32.const {entry}) (call_indirect (type $t_target_{idx})))"
        )?;
    }

    writeln!(wat, ")")?;
    Ok(wat::parse_str(&wat)?)
}

// Generate the fixup module that patches the shim's function table.
fn generate_fixup_module(intercepted: &[InterceptedFunction]) -> Result<Vec<u8>> {
    let n = intercepted.len();
    let total = 3 + n;
    let mut wat = String::new();
    wat.push_str(
        r#"(module
  (type $t_ctor (func (param i32 i32 i32 i32) (result i32)))
  (type $t_before (func (param i32 i32)))
  (type $t_after (func (param i32 i32 i32 i64 i32 i32)))
"#,
    );

    for (idx, ifunc) in intercepted.iter().enumerate() {
        write!(wat, "  (type $t_target_{idx} (func")?;
        write_core_func_type(
            &mut wat,
            &ifunc.core_params_lower,
            &ifunc.core_results_lower,
        )?;
        writeln!(wat, "))")?;
    }

    wat.push_str(
        r#"  (import "" "0" (func (type $t_ctor)))
  (import "" "1" (func (type $t_before)))
  (import "" "2" (func (type $t_after)))
"#,
    );
    for i in 0..n {
        let entry = 3 + i;
        writeln!(
            wat,
            "  (import \"\" \"{entry}\" (func (type $t_target_{i})))"
        )?;
    }
    writeln!(
        wat,
        "  (import \"\" \"$imports\" (table {total} {total} funcref))"
    )?;

    // elem segment to patch the table
    let func_indices: Vec<String> = (0..total).map(|i| i.to_string()).collect();
    writeln!(
        wat,
        "  (elem (i32.const 0) func {})",
        func_indices.join(" ")
    )?;

    writeln!(wat, ")")?;
    Ok(wat::parse_str(&wat)?)
}

// Generate the main module that implements the interceptor logic.
//
// For each intercepted function, generates an interceptor that:
// 1. Marshals params into list<arg>
// 2. Creates an invocation resource
// 3. Calls before() and dispatches on the result
// 4. Calls the target function
// 5. Calls after() and dispatches on the result
// 6. Loops on repeat, returns on accept/skip
fn generate_main_module(
    target: &TargetWorld,
    intercepted: &[InterceptedFunction],
) -> Result<Vec<u8>> {
    let resolve = target.resolve();
    let mut wat = String::new();
    wat.push_str(
        r#"(module
  (import "advice" "[constructor]invocation" (func $inv_new (param i32 i32 i32 i32) (result i32)))
  (import "advice" "[method]invocation.before" (func $inv_before (param i32 i32)))
  (import "advice" "[method]invocation.after" (func $inv_after (param i32 i32 i32 i64 i32 i32)))
  (import "advice" "[resource-drop]invocation" (func $inv_drop (param i32)))
"#,
    );

    // Target function imports — each item knows its import_module
    for (idx, ifunc) in intercepted.iter().enumerate() {
        write!(
            wat,
            "  (import \"{}\" \"{}\" (func $target_{idx}",
            ifunc.import_module, ifunc.func_name
        )?;
        write_core_func_type(
            &mut wat,
            &ifunc.core_params_lower,
            &ifunc.core_results_lower,
        )?;
        writeln!(wat, "))")?;
    }

    // === String data segments ===
    // Collect all strings first so we know the total size before declaring the heap base.
    let mut all_strings = String::new();
    let mut str_offset = 0u32;

    // Function name entries: (offset, len) per intercepted function
    let mut fname_entries: Vec<(u32, u32)> = Vec::new();
    for ifunc in intercepted {
        let len = ifunc.func_name.len() as u32;
        fname_entries.push((str_offset, len));
        all_strings.push_str(&ifunc.func_name);
        str_offset += len;
    }

    // Param name + type name entries per intercepted function
    let mut param_string_entries: Vec<Vec<(u32, u32, u32, u32)>> = Vec::new();
    for ifunc in intercepted {
        let mut entries = Vec::new();
        for (param_name, param_ty) in &ifunc.params {
            let name_off = str_offset;
            let name_len = param_name.len() as u32;
            all_strings.push_str(param_name);
            str_offset += name_len;

            let tn = type_name_str(resolve, *param_ty);
            let type_off = str_offset;
            let type_len = tn.len() as u32;
            all_strings.push_str(&tn);
            str_offset += type_len;

            entries.push((name_off, name_len, type_off, type_len));
        }
        param_string_entries.push(entries);
    }

    // Heap starts immediately after the string data, aligned to 8 bytes.
    let heap_base = (str_offset + 7) & !7;

    // === Memory and allocator ===
    writeln!(wat, "  (memory (export \"memory\") 1)")?;
    writeln!(wat, "  (global $heap (mut i32) (i32.const {heap_base}))")?;
    wat.push_str(
        r#"  (func $alloc (param $size i32) (result i32)
    (local $ptr i32)
    (local.set $ptr (global.get $heap))
    (local.set $ptr (i32.and (i32.add (local.get $ptr) (i32.const 7)) (i32.const -8)))
    (global.set $heap (i32.add (local.get $ptr) (local.get $size)))
    (local.get $ptr)
  )
"#,
    );

    // cabi_realloc (called by canon lift/lower)
    generate_realloc(&mut wat);

    writeln!(
        wat,
        "  (data $strings (i32.const 0) \"{}\")",
        escape_wat_string(&all_strings)
    )?;

    // === Interceptor functions ===
    for (idx, ifunc) in intercepted.iter().enumerate() {
        let (fname_offset, fname_len) = fname_entries[idx];
        write_interceptor_func(
            &mut wat,
            ifunc,
            idx,
            fname_offset,
            fname_len,
            &param_string_entries[idx],
        )?;
    }

    // === Post-return stubs (cabi_post_*) ===
    // Per the canonical ABI, these run after canon lift has copied all return
    // data out of linear memory. We reset the bump allocator here to reclaim
    // all per-call allocations (work areas + cabi_realloc buffers).
    for ifunc in intercepted {
        let export_name = format!("cabi_post_{}", ifunc.export_name);
        if !ifunc.result_flat_types.is_empty() {
            if ifunc.uses_retarea {
                writeln!(
                    wat,
                    "  (func (export \"{export_name}\") (param i32) (global.set $heap (i32.const {heap_base})))"
                )?;
            } else {
                writeln!(
                    wat,
                    "  (func (export \"{export_name}\") (param {}) (global.set $heap (i32.const {heap_base})))",
                    ifunc.result_flat_types[0]
                )?;
            }
        } else {
            writeln!(
                wat,
                "  (func (export \"{export_name}\") (global.set $heap (i32.const {heap_base})))"
            )?;
        }
    }

    writeln!(wat, ")")?;
    Ok(wat::parse_str(&wat)?)
}

// ============================================================
// WAT helpers
// ============================================================

// Write a core function type from pre-computed flat types: (param ...) (result ...)
fn write_core_func_type(wat: &mut String, params: &[&str], results: &[&str]) -> Result<()> {
    if !params.is_empty() {
        write!(wat, " (param")?;
        for p in params {
            write!(wat, " {p}")?;
        }
        write!(wat, ")")?;
    }
    if !results.is_empty() {
        write!(wat, " (result")?;
        for r in results {
            write!(wat, " {r}")?;
        }
        write!(wat, ")")?;
    }
    Ok(())
}

// Generate the interceptor function for a single function.
fn write_interceptor_func(
    wat: &mut String,
    ifunc: &InterceptedFunction,
    func_idx: usize,
    fname_offset: u32,
    fname_len: u32,
    param_strings: &[(u32, u32, u32, u32)],
) -> Result<()> {
    let n_params = ifunc.params.len();

    write!(wat, "  (func (export \"{}\")", ifunc.export_name)?;
    write_core_func_type(wat, &ifunc.core_params_lift, &ifunc.core_results_lift)?;
    writeln!(wat)?;

    // === Locals ===
    writeln!(wat, "    (local $args_area i32)")?;
    writeln!(wat, "    (local $handle i32)")?;
    writeln!(wat, "    (local $before_ret i32)")?;
    writeln!(wat, "    (local $after_ret i32)")?;
    writeln!(wat, "    (local $disc i32)")?;
    writeln!(wat, "    (local $watermark i32)")?;

    if ifunc.uses_retarea {
        writeln!(wat, "    (local $target_ret i32)")?;
        writeln!(wat, "    (local $out i32)")?;
    } else if !ifunc.result_flat_types.is_empty() {
        let core_ty = ifunc.result_flat_types[0];
        writeln!(wat, "    (local $result_val {core_ty})")?;
        writeln!(wat, "    (local $out {core_ty})")?;
    }

    // Temp locals for unwrapping proceed args
    for (pi, param_flats) in ifunc.param_flat_types.iter().enumerate() {
        if param_flats.len() == 1 {
            writeln!(wat, "    (local $arg{pi}_val {})", param_flats[0])?;
        } else {
            for (j, ct) in param_flats.iter().enumerate() {
                writeln!(wat, "    (local $arg{pi}_{j} {ct})")?;
            }
        }
    }

    // For retarea results, individual result component locals
    if ifunc.uses_retarea {
        for (j, ct) in ifunc.result_flat_types.iter().enumerate() {
            writeln!(wat, "    (local $result_{j} {ct})")?;
        }
    }

    // === Allocate work areas ===
    if n_params > 0 {
        let args_size = n_params * 32;
        writeln!(
            wat,
            "    (local.set $args_area (call $alloc (i32.const {args_size})))"
        )?;
    }
    writeln!(
        wat,
        "    (local.set $before_ret (call $alloc (i32.const 32)))"
    )?;
    writeln!(
        wat,
        "    (local.set $after_ret (call $alloc (i32.const 32)))"
    )?;

    if ifunc.uses_retarea {
        let mut ret_size = 0u32;
        for ct in &ifunc.result_flat_types {
            let size = match *ct {
                "i64" | "f64" => 8u32,
                _ => 4u32,
            };
            ret_size = (ret_size + size - 1) & !(size - 1);
            ret_size += size;
        }
        writeln!(
            wat,
            "    (local.set $target_ret (call $alloc (i32.const {ret_size})))"
        )?;
    }
    writeln!(wat, "    (local.set $watermark (global.get $heap))")?;

    // === Marshal initial params into args_area ===
    // Each arg record is 32 bytes:
    //   +0: name ptr (i32), +4: name len (i32)
    //   +8: type-name ptr (i32), +12: type-name len (i32)
    //   +16: value disc (u8, padded to 8)
    //   +24: value payload (8 bytes)
    let mut core_param_idx = 0u32;
    for (pi, &(name_off, name_len, type_off, type_len)) in param_strings.iter().enumerate() {
        let offset = pi * 32;
        let disc = ifunc.param_discriminants[pi];

        // name: string (ptr + len)
        writeln!(
            wat,
            "    (i32.store (i32.add (local.get $args_area) (i32.const {offset})) (i32.const {name_off}))"
        )?;
        writeln!(
            wat,
            "    (i32.store offset=4 (i32.add (local.get $args_area) (i32.const {offset})) (i32.const {name_len}))"
        )?;
        // type-name: string (ptr + len)
        writeln!(
            wat,
            "    (i32.store offset=8 (i32.add (local.get $args_area) (i32.const {offset})) (i32.const {type_off}))"
        )?;
        writeln!(
            wat,
            "    (i32.store offset=12 (i32.add (local.get $args_area) (i32.const {offset})) (i32.const {type_len}))"
        )?;
        // value disc
        writeln!(
            wat,
            "    (i32.store8 offset=16 (i32.add (local.get $args_area) (i32.const {offset})) (i32.const {disc}))"
        )?;

        // value payload at +24
        write_marshal_param(
            wat,
            disc,
            &ifunc.param_flat_types[pi],
            offset,
            &mut core_param_idx,
        )?;
    }

    // === Main interceptor loop ===
    writeln!(wat, "    (block $done")?;
    writeln!(wat, "      (loop $retry")?;
    writeln!(wat, "        (global.set $heap (local.get $watermark))")?;

    // Step 1: Create invocation
    writeln!(
        wat,
        "        (local.set $handle (call $inv_new (i32.const {fname_offset}) (i32.const {fname_len}) (local.get $args_area) (i32.const {n_params})))"
    )?;

    // Step 2: Call before
    writeln!(
        wat,
        "        (call $inv_before (local.get $handle) (local.get $before_ret))"
    )?;
    writeln!(
        wat,
        "        (local.set $disc (i32.load8_u (local.get $before_ret)))"
    )?;

    // disc 2 = error => trap
    writeln!(
        wat,
        "        (if (i32.eq (local.get $disc) (i32.const 2)) (then (call $inv_drop (local.get $handle)) (unreachable)))"
    )?;

    // disc 1 = skip(option<value>) => unwrap return value and exit
    if ifunc.result.is_some() {
        write_skip_return(wat, ifunc)?;
    } else {
        writeln!(
            wat,
            "        (if (i32.eq (local.get $disc) (i32.const 1)) (then"
        )?;
        writeln!(wat, "          (call $inv_drop (local.get $handle))")?;
        writeln!(wat, "          (br $done)))")?;
    }

    // disc 0 = proceed(list<arg>) => unwrap args and call target
    write_proceed_and_call_target(wat, ifunc, func_idx)?;

    // Step 4: Call after with result
    write_call_after(wat, ifunc)?;

    // Dispatch on after-action
    writeln!(
        wat,
        "        (local.set $disc (i32.load8_u (local.get $after_ret)))"
    )?;

    // disc 2 = error => trap
    writeln!(
        wat,
        "        (if (i32.eq (local.get $disc) (i32.const 2)) (then"
    )?;
    writeln!(
        wat,
        "          (call $inv_drop (local.get $handle)) (unreachable)))"
    )?;

    // disc 1 = repeat(list<arg>) => update args, loop
    writeln!(
        wat,
        "        (if (i32.eq (local.get $disc) (i32.const 1)) (then"
    )?;
    writeln!(
        wat,
        "          (local.set $args_area (i32.load offset=8 (local.get $after_ret)))"
    )?;
    writeln!(
        wat,
        "          (call $inv_drop (local.get $handle)) (br $retry)))"
    )?;

    // disc 0 = accept(option<value>) => unwrap return value and exit
    if ifunc.result.is_some() {
        write_accept_return(wat, ifunc)?;
    }
    writeln!(
        wat,
        "        (call $inv_drop (local.get $handle)) (br $done)"
    )?;

    writeln!(wat, "      ) ;; end loop")?;
    writeln!(wat, "    ) ;; end block")?;

    // Return
    if !ifunc.core_results_lift.is_empty() {
        writeln!(wat, "    (local.get $out)")?;
    }
    writeln!(wat, "  )")?;

    Ok(())
}

// Marshal a single param's value payload into the args_area at +24.
fn write_marshal_param(
    wat: &mut String,
    disc: u8,
    param_flats: &[&str],
    offset: usize,
    core_param_idx: &mut u32,
) -> Result<()> {
    if disc != DISC_COMPLEX {
        match disc {
            DISC_STRING => {
                // string: ptr at +24, len at +28
                writeln!(
                    wat,
                    "    (i32.store offset=24 (i32.add (local.get $args_area) (i32.const {offset})) (local.get {core_param_idx}))"
                )?;
                writeln!(
                    wat,
                    "    (i32.store offset=28 (i32.add (local.get $args_area) (i32.const {offset})) (local.get {}))",
                    *core_param_idx + 1
                )?;
                *core_param_idx += 2;
            }
            DISC_SIGNED => {
                // signed int => i64 (sign-extend i32 types)
                if param_flats[0] == "i32" {
                    writeln!(
                        wat,
                        "    (i64.store offset=24 (i32.add (local.get $args_area) (i32.const {offset})) (i64.extend_i32_s (local.get {core_param_idx})))"
                    )?;
                } else {
                    writeln!(
                        wat,
                        "    (i64.store offset=24 (i32.add (local.get $args_area) (i32.const {offset})) (local.get {core_param_idx}))"
                    )?;
                }
                *core_param_idx += 1;
            }
            DISC_UNSIGNED => {
                // unsigned int / char => i64 (zero-extend i32 types)
                if param_flats[0] == "i32" {
                    writeln!(
                        wat,
                        "    (i64.store offset=24 (i32.add (local.get $args_area) (i32.const {offset})) (i64.extend_i32_u (local.get {core_param_idx})))"
                    )?;
                } else {
                    writeln!(
                        wat,
                        "    (i64.store offset=24 (i32.add (local.get $args_area) (i32.const {offset})) (local.get {core_param_idx}))"
                    )?;
                }
                *core_param_idx += 1;
            }
            DISC_F32 => {
                writeln!(
                    wat,
                    "    (f32.store offset=24 (i32.add (local.get $args_area) (i32.const {offset})) (local.get {core_param_idx}))"
                )?;
                *core_param_idx += 1;
            }
            DISC_F64 => {
                writeln!(
                    wat,
                    "    (f64.store offset=24 (i32.add (local.get $args_area) (i32.const {offset})) (local.get {core_param_idx}))"
                )?;
                *core_param_idx += 1;
            }
            DISC_BOOL => {
                // boolean => i64
                writeln!(
                    wat,
                    "    (i64.store offset=24 (i32.add (local.get $args_area) (i32.const {offset})) (i64.extend_i32_u (local.get {core_param_idx})))"
                )?;
                *core_param_idx += 1;
            }
            _ => unreachable!("primitive should have disc 0-5"),
        }
    } else {
        // Complex type: payload = empty string (opaque — advice sees name + type-name only)
        writeln!(
            wat,
            "    (i32.store offset=24 (i32.add (local.get $args_area) (i32.const {offset})) (i32.const 0))"
        )?;
        writeln!(
            wat,
            "    (i32.store offset=28 (i32.add (local.get $args_area) (i32.const {offset})) (i32.const 0))"
        )?;
        *core_param_idx += param_flats.len() as u32;
    }
    Ok(())
}

// Generate code for skip(option<value>) => unwrap as return value.
//
// Layout in before_ret: disc=1 at +0, option<value> payload starts at +8:
// - option disc at +8
// - value disc at +16
// - value payload at +24
fn write_skip_return(wat: &mut String, ifunc: &InterceptedFunction) -> Result<()> {
    writeln!(
        wat,
        "        (if (i32.eq (local.get $disc) (i32.const 1)) (then"
    )?;

    let disc = ifunc
        .result_discriminant
        .ok_or_else(|| anyhow::anyhow!("skip path requires a return type discriminant"))?;
    if disc != DISC_COMPLEX {
        // Guard: trap if advice returned a different value variant than expected.
        writeln!(
            wat,
            "          (if (i32.ne (i32.load8_u offset=16 (local.get $before_ret)) (i32.const {disc})) (then (call $inv_drop (local.get $handle)) (unreachable)))"
        )?;
    }
    let result_flat = ifunc.result_flat_types.first().copied().unwrap_or("i32");
    write_unwrap_value_to_out(
        wat,
        disc,
        result_flat,
        ifunc.uses_retarea,
        "$before_ret",
        24,
    )?;

    writeln!(wat, "          (call $inv_drop (local.get $handle))")?;
    writeln!(wat, "          (br $done)))")?;
    Ok(())
}

// Generate code for accept(option<value>) => unwrap as return value.
//
// For primitive returns: reads the advice's replacement value from the after-action payload.
// For complex returns: uses the target's actual return value (advice can't provide complex values).
//
// Layout in after_ret: disc=0 at +0, option<value> payload starts at +8:
// - option disc at +8
// - value disc at +16
// - value payload at +24
fn write_accept_return(wat: &mut String, ifunc: &InterceptedFunction) -> Result<()> {
    let disc = ifunc
        .result_discriminant
        .ok_or_else(|| anyhow::anyhow!("accept path requires a return type discriminant"))?;
    // Guard: trap if advice returned a different value variant than expected.
    writeln!(
        wat,
        "          (if (i32.ne (i32.load8_u offset=16 (local.get $after_ret)) (i32.const {disc})) (then (unreachable)))"
    )?;
    if disc != DISC_COMPLEX {
        let result_flat = ifunc.result_flat_types.first().copied().unwrap_or("i32");
        write_unwrap_value_to_out(wat, disc, result_flat, ifunc.uses_retarea, "$after_ret", 24)?;
    } else {
        // Complex return: use the target's actual return value
        if ifunc.uses_retarea {
            writeln!(wat, "          (local.set $out (local.get $target_ret))")?;
        } else if !ifunc.result_flat_types.is_empty() {
            writeln!(wat, "          (local.set $out (local.get $result_val))")?;
        }
    }
    Ok(())
}

// Unwrap a value payload from memory into $out.
//
// For primitive types: loads the value from the payload and stores in $out.
// For complex types: traps (skip is not supported, accept does not call this).
fn write_unwrap_value_to_out(
    wat: &mut String,
    disc: u8,
    result_flat: &str,
    uses_retarea: bool,
    base_local: &str,
    payload_offset: u32,
) -> Result<()> {
    if disc != DISC_COMPLEX {
        if uses_retarea {
            // String: ptr at payload_offset, len at payload_offset+4
            let ptr_off = payload_offset;
            let len_off = payload_offset + 4;
            writeln!(
                wat,
                "          (local.set $out (call $alloc (i32.const 8)))"
            )?;
            writeln!(
                wat,
                "          (i32.store (local.get $out) (i32.load offset={ptr_off} (local.get {base_local})))"
            )?;
            writeln!(
                wat,
                "          (i32.store offset=4 (local.get $out) (i32.load offset={len_off} (local.get {base_local})))"
            )?;
        } else {
            match disc {
                DISC_STRING => unreachable!("string should use retarea path"),
                DISC_SIGNED | DISC_UNSIGNED | DISC_BOOL => {
                    // int/bool: load i64, wrap to i32 if needed
                    if result_flat == "i32" {
                        writeln!(
                            wat,
                            "          (local.set $out (i32.wrap_i64 (i64.load offset={payload_offset} (local.get {base_local}))))"
                        )?;
                    } else {
                        writeln!(
                            wat,
                            "          (local.set $out (i64.load offset={payload_offset} (local.get {base_local})))"
                        )?;
                    }
                }
                DISC_F32 => {
                    writeln!(
                        wat,
                        "          (local.set $out (f32.load offset={payload_offset} (local.get {base_local})))"
                    )?;
                }
                DISC_F64 => {
                    writeln!(
                        wat,
                        "          (local.set $out (f64.load offset={payload_offset} (local.get {base_local})))"
                    )?;
                }
                _ => unreachable!("primitive should have disc 0-5"),
            }
        }
    } else {
        // Complex return type: skip traps (advice can't write a complex value)
        writeln!(
            wat,
            "          (unreachable) ;; skip with complex return type"
        )?;
    }
    Ok(())
}

// Generate code for proceed: unwrap args from before-action, call target function.
fn write_proceed_and_call_target(
    wat: &mut String,
    ifunc: &InterceptedFunction,
    func_idx: usize,
) -> Result<()> {
    // proceed payload: list<arg> ptr at before_ret+8, count at before_ret+12
    // Each arg record is 32 bytes

    // Unwrap each arg's value from the proceed list
    for (pi, disc) in ifunc.param_discriminants.iter().enumerate() {
        let arg_base = format!(
            "(i32.add (i32.load offset=8 (local.get $before_ret)) (i32.const {}))",
            pi * 32
        );

        // Guard: trap if advice returned a different value variant than expected.
        writeln!(
            wat,
            "        (if (i32.ne (i32.load8_u offset=16 {arg_base}) (i32.const {disc})) (then (unreachable)))"
        )?;
        if *disc != DISC_COMPLEX {
            let param_flats = &ifunc.param_flat_types[pi];
            match *disc {
                DISC_STRING => {
                    // string: ptr at +24, len at +28
                    writeln!(
                        wat,
                        "        (local.set $arg{pi}_0 (i32.load offset=24 {arg_base}))"
                    )?;
                    writeln!(
                        wat,
                        "        (local.set $arg{pi}_1 (i32.load offset=28 {arg_base}))"
                    )?;
                }
                DISC_SIGNED | DISC_UNSIGNED | DISC_BOOL => {
                    // int/bool: load i64, wrap if i32
                    if param_flats[0] == "i32" {
                        writeln!(
                            wat,
                            "        (local.set $arg{pi}_val (i32.wrap_i64 (i64.load offset=24 {arg_base})))"
                        )?;
                    } else {
                        writeln!(
                            wat,
                            "        (local.set $arg{pi}_val (i64.load offset=24 {arg_base}))"
                        )?;
                    }
                }
                DISC_F32 => {
                    writeln!(
                        wat,
                        "        (local.set $arg{pi}_val (f32.load offset=24 {arg_base}))"
                    )?;
                }
                DISC_F64 => {
                    writeln!(
                        wat,
                        "        (local.set $arg{pi}_val (f64.load offset=24 {arg_base}))"
                    )?;
                }
                _ => unreachable!("primitive should have disc 0-5"),
            }
        }
        // Complex type in proceed: opaque, original core params used in target call below
    }

    // Build the target call args.
    // For primitive params: use the unwrapped arg locals (populated from proceed list above).
    // For complex params: use the original core function parameters (locals 0..N)
    // since complex values in the proceed list are opaque and ignored (placeholders).
    let mut call_args = String::new();
    let mut core_param_idx = 0u32;
    for (pi, disc) in ifunc.param_discriminants.iter().enumerate() {
        let param_flats = &ifunc.param_flat_types[pi];
        if *disc != DISC_COMPLEX {
            if param_flats.len() == 1 {
                write!(call_args, " (local.get $arg{pi}_val)")?;
            } else {
                for j in 0..param_flats.len() {
                    write!(call_args, " (local.get $arg{pi}_{j})")?;
                }
            }
            core_param_idx += param_flats.len() as u32;
        } else {
            // Complex param: pass original core function parameters unchanged
            for _ in 0..param_flats.len() {
                write!(call_args, " (local.get {core_param_idx})")?;
                core_param_idx += 1;
            }
        }
    }

    if ifunc.uses_retarea {
        writeln!(
            wat,
            "        (call $target_{func_idx}{call_args} (local.get $target_ret))"
        )?;

        let mut byte_offset = 0u32;
        for (j, ct) in ifunc.result_flat_types.iter().enumerate() {
            let (size, load) = match *ct {
                "i64" => (8u32, "i64.load"),
                "f64" => (8, "f64.load"),
                "f32" => (4, "f32.load"),
                _ => (4, "i32.load"),
            };
            byte_offset = (byte_offset + size - 1) & !(size - 1);
            writeln!(
                wat,
                "        (local.set $result_{j} ({load} offset={byte_offset} (local.get $target_ret)))"
            )?;
            byte_offset += size;
        }
    } else if ifunc.result.is_some() {
        writeln!(
            wat,
            "        (local.set $result_val (call $target_{func_idx}{call_args}))"
        )?;
    } else {
        writeln!(wat, "        (call $target_{func_idx}{call_args})")?;
    }

    Ok(())
}

// Generate code for calling inv_after with the target's return value.
//
// Core sig: (self, option_disc, value_disc, payload_i64, payload_i32, retptr)
// For void: option_disc=0 (none), rest are zero/ignored.
// For non-void: option_disc=1 (some), value_disc + payload carry the value.
fn write_call_after(wat: &mut String, ifunc: &InterceptedFunction) -> Result<()> {
    match ifunc.result_discriminant {
        None => {
            writeln!(
                wat,
                "        (call $inv_after (local.get $handle) (i32.const 0) (i32.const 0) (i64.const 0) (i32.const 0) (local.get $after_ret))"
            )?;
        }
        Some(disc) => {
            let result_flat = ifunc.result_flat_types.first().copied().unwrap_or("i32");
            if disc != DISC_COMPLEX {
                match disc {
                    DISC_STRING => {
                        // string: ptr in $result_0, len in $result_1
                        writeln!(
                            wat,
                            "        (call $inv_after (local.get $handle) (i32.const 1) (i32.const {disc}) (i64.extend_i32_u (local.get $result_0)) (local.get $result_1) (local.get $after_ret))"
                        )?;
                    }
                    DISC_SIGNED => {
                        // signed int
                        if result_flat == "i32" {
                            writeln!(
                                wat,
                                "        (call $inv_after (local.get $handle) (i32.const 1) (i32.const {disc}) (i64.extend_i32_s (local.get $result_val)) (i32.const 0) (local.get $after_ret))"
                            )?;
                        } else {
                            writeln!(
                                wat,
                                "        (call $inv_after (local.get $handle) (i32.const 1) (i32.const {disc}) (local.get $result_val) (i32.const 0) (local.get $after_ret))"
                            )?;
                        }
                    }
                    DISC_UNSIGNED | DISC_BOOL => {
                        // unsigned int / bool / char
                        if result_flat == "i32" {
                            writeln!(
                                wat,
                                "        (call $inv_after (local.get $handle) (i32.const 1) (i32.const {disc}) (i64.extend_i32_u (local.get $result_val)) (i32.const 0) (local.get $after_ret))"
                            )?;
                        } else {
                            writeln!(
                                wat,
                                "        (call $inv_after (local.get $handle) (i32.const 1) (i32.const {disc}) (local.get $result_val) (i32.const 0) (local.get $after_ret))"
                            )?;
                        }
                    }
                    DISC_F32 => {
                        writeln!(
                            wat,
                            "        (call $inv_after (local.get $handle) (i32.const 1) (i32.const {disc}) (i64.extend_i32_u (i32.reinterpret_f32 (local.get $result_val))) (i32.const 0) (local.get $after_ret))"
                        )?;
                    }
                    DISC_F64 => {
                        writeln!(
                            wat,
                            "        (call $inv_after (local.get $handle) (i32.const 1) (i32.const {disc}) (i64.reinterpret_f64 (local.get $result_val)) (i32.const 0) (local.get $after_ret))"
                        )?;
                    }
                    _ => unreachable!("primitive should have disc 0-5"),
                }
            } else {
                // Complex return: pass as complex(empty string), informational only
                writeln!(
                    wat,
                    "        (call $inv_after (local.get $handle) (i32.const 1) (i32.const {disc}) (i64.const 0) (i32.const 0) (local.get $after_ret))"
                )?;
            }
        }
    }
    Ok(())
}

// Escape a string for WAT data segments.
fn escape_wat_string(s: &str) -> String {
    let mut out = String::new();
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c.is_ascii_graphic() || c == ' ' => out.push(c),
            c => {
                for b in c.to_string().as_bytes() {
                    write!(out, "\\{b:02x}").unwrap();
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    // === escape_wat_string ===

    #[test]
    fn escape_ascii_passthrough() {
        assert_eq!(escape_wat_string("hello"), "hello");
    }

    #[test]
    fn escape_spaces() {
        assert_eq!(escape_wat_string("hello world"), "hello world");
    }

    #[test]
    fn escape_quotes() {
        assert_eq!(escape_wat_string(r#"say "hi""#), r#"say \"hi\""#);
    }

    #[test]
    fn escape_backslash() {
        assert_eq!(escape_wat_string(r"a\b"), r"a\\b");
    }

    #[test]
    fn escape_newline() {
        assert_eq!(escape_wat_string("a\nb"), r"a\nb");
    }

    #[test]
    fn escape_tab() {
        assert_eq!(escape_wat_string("a\tb"), r"a\tb");
    }

    #[test]
    fn escape_carriage_return() {
        assert_eq!(escape_wat_string("a\rb"), r"a\rb");
    }

    #[test]
    fn escape_non_ascii() {
        assert_eq!(escape_wat_string("café"), r"caf\c3\a9");
    }

    #[test]
    fn escape_empty() {
        assert_eq!(escape_wat_string(""), "");
    }

    // === write_core_func_type ===

    #[test]
    fn func_type_empty() {
        let mut s = String::new();
        write_core_func_type(&mut s, &[], &[]).unwrap();
        assert_eq!(s, "");
    }

    #[test]
    fn func_type_params_only() {
        let mut s = String::new();
        write_core_func_type(&mut s, &["i32", "f64"], &[]).unwrap();
        assert_eq!(s, " (param i32 f64)");
    }

    #[test]
    fn func_type_results_only() {
        let mut s = String::new();
        write_core_func_type(&mut s, &[], &["i32"]).unwrap();
        assert_eq!(s, " (result i32)");
    }

    #[test]
    fn func_type_both() {
        let mut s = String::new();
        write_core_func_type(&mut s, &["i32", "i32"], &["f32"]).unwrap();
        assert_eq!(s, " (param i32 i32) (result f32)");
    }
}
