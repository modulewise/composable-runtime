//! Builds an interceptor component from extracted target information and generated core modules.
//!
//! Public entry point: `build`
//!
//! Internally, `InterceptorBuilder` wraps `wasm_encoder::ComponentBuilder`
//! and executes four phases in sequence:
//! 1. `import_targets`: import type-providing interfaces, target interfaces, direct functions
//! 2. `import_advice`: import advice types and instance
//! 3. `embed_core_modules`: embed the 3 generated core modules (main, shim, fixup)
//! 4. `wire_and_export`: wire modules, canon lower/lift, build exports

use std::collections::HashMap;

use anyhow::Result;
use wasm_encoder::*;
use wit_parser::{Resolve, TypeDefKind, TypeOwner};

use crate::encoder::{self, TypeEncoder, encode_functype};

const ADVICE_WIT: &str = include_str!("../wit/package.wit");
use crate::generator::InterceptedFunction;
use crate::types::{self, FunctionExport, InterfaceExport, TargetWorld, WorldExport};

/// Build an interceptor component from extracted target info and generated core modules.
///
/// Creates an `InterceptorBuilder`, calls its phase methods in sequence, and
/// returns the finished component bytes.
pub fn build(
    target: &TargetWorld,
    intercepted: &[InterceptedFunction],
    main_bytes: Vec<u8>,
    shim_bytes: Vec<u8>,
    fixup_bytes: Vec<u8>,
) -> Result<Vec<u8>> {
    let mut b = InterceptorBuilder::default();

    b.import_targets(target)?;
    b.import_advice()?;
    b.embed_core_modules(&main_bytes, &shim_bytes, &fixup_bytes);
    b.wire_and_export(target, intercepted)?;

    Ok(b.finish())
}

// An export instance to be emitted after `ComponentBuilder::finish()`.
//
// Used to build component-level instances from individually aliased types
// and lifted/aliased functions, preserving type provenance.
struct PendingExportInstance {
    name: String,
    items: Vec<(String, ComponentExportKind, u32)>,
}

// Builds an interceptor component from target info and generated core modules.
//
// Wraps `wasm_encoder::ComponentBuilder` and carries state between phases.
// Only used through the `build()` function. Fields are populated progressively
// by phase methods called in order.
#[derive(Default)]
struct InterceptorBuilder {
    inner: ComponentBuilder,
    pending_exports: Vec<PendingExportInstance>,

    // State from import_targets
    iface_instances: Vec<(usize, u32)>,
    direct_func_imports: HashMap<String, u32>,
    type_instances: HashMap<String, u32>,
    foreign_type_indices: HashMap<(wit_parser::InterfaceId, String), u32>,

    // State from import_advice
    advice_instance: u32,

    // State from embed_core_modules
    main_module: u32,
    shim_module: u32,
    fixup_module: u32,
}

impl InterceptorBuilder {
    // ============================================================
    // Phase 1: Import targets
    // ============================================================

    // Import type-providing interfaces, target interfaces, and direct functions.
    fn import_targets(&mut self, target: &TargetWorld) -> Result<()> {
        let resolve = target.resolve();

        // Import type-providing interfaces and pre-alias their types.
        for ti in &target.type_imports {
            let instance_idx = self.import_type_interface(
                resolve,
                ti.interface_id,
                &ti.full_name,
                &ti.type_names,
            )?;
            for type_name in &ti.type_names {
                let comp_type_idx =
                    self.inner
                        .alias_export(instance_idx, type_name, ComponentExportKind::Type);
                self.foreign_type_indices
                    .insert((ti.interface_id, type_name.clone()), comp_type_idx);
                self.type_instances.insert(type_name.clone(), instance_idx);
            }
        }

        // Import target interfaces and direct functions.
        for (export_idx, we) in target.exports.iter().enumerate() {
            match we {
                WorldExport::Interface(ie) => {
                    let (_target_type, target_instance) =
                        self.emit_target_type_and_import(resolve, ie)?;
                    self.iface_instances.push((export_idx, target_instance));
                }
                WorldExport::Function(fe) => {
                    let func_idx = self.import_direct_function(resolve, fe.func())?;
                    self.direct_func_imports
                        .insert(fe.name().to_string(), func_idx);
                }
            }
        }

        Ok(())
    }

    // Emit a target interface instance type definition + import.
    fn emit_target_type_and_import(
        &mut self,
        resolve: &Resolve,
        ie: &InterfaceExport,
    ) -> Result<(u32, u32)> {
        let inst = encoder::encode_instance_type(
            resolve,
            ie.interface_id,
            &ie.owned_types,
            &self.foreign_type_indices,
        )?;
        let type_idx = self.inner.type_instance(None, &inst);
        let instance_idx = self
            .inner
            .import(&ie.full_name, ComponentTypeRef::Instance(type_idx));
        Ok((type_idx, instance_idx))
    }

    // Import a type-providing interface for type resolution.
    fn import_type_interface(
        &mut self,
        resolve: &Resolve,
        interface_id: wit_parser::InterfaceId,
        full_name: &str,
        type_names: &[String],
    ) -> Result<u32> {
        let inst = encoder::encode_instance_type(
            resolve,
            interface_id,
            type_names,
            &self.foreign_type_indices,
        )?;
        let type_idx = self.inner.type_instance(None, &inst);
        let instance_idx = self
            .inner
            .import(full_name, ComponentTypeRef::Instance(type_idx));
        Ok(instance_idx)
    }

    // Import a direct (world-level) function at the component level.
    fn import_direct_function(
        &mut self,
        resolve: &Resolve,
        wit_func: &wit_parser::Function,
    ) -> Result<u32> {
        let mut type_map = HashMap::new();
        let mut enc = OuterTypeEncoder {
            builder: &mut self.inner,
            type_instances: &self.type_instances,
            home_interface: None,
        };
        let functype_idx = encode_functype(resolve, wit_func, &mut enc, &mut type_map)?;
        let func_idx = self
            .inner
            .import(&wit_func.name, ComponentTypeRef::Func(functype_idx));
        Ok(func_idx)
    }

    // ============================================================
    // Phase 2: Import advice
    // ============================================================

    // Import the types and advice instances from the embedded WIT definition.
    fn import_advice(&mut self) -> Result<()> {
        let mut resolve = Resolve::default();
        let pkg = resolve.push_str("package.wit", ADVICE_WIT)?;
        let package = &resolve.packages[pkg];
        let types_iface_id = package.interfaces["types"];
        let advice_iface_id = package.interfaces["advice"];

        // Encode and import the types interface
        let types_inst = encoder::encode_instance_type(
            &resolve,
            types_iface_id,
            &["value".into(), "arg".into()],
            &HashMap::new(),
        )?;
        let types_type = self.inner.type_instance(None, &types_inst);
        let types_instance = self.inner.import(
            "modulewise:interceptor/types@0.1.0",
            ComponentTypeRef::Instance(types_type),
        );

        // Alias types from the types instance for use by the advice interface
        let value_type =
            self.inner
                .alias_export(types_instance, "value", ComponentExportKind::Type);
        let arg_type = self
            .inner
            .alias_export(types_instance, "arg", ComponentExportKind::Type);

        let mut foreign_types = HashMap::new();
        foreign_types.insert((types_iface_id, "value".into()), value_type);
        foreign_types.insert((types_iface_id, "arg".into()), arg_type);

        // Determine owned types for the advice interface (exclude `use` aliases)
        let advice_iface = &resolve.interfaces[advice_iface_id];
        let owned_types: Vec<String> = advice_iface
            .types
            .iter()
            .filter(|(_, type_id)| {
                let td = &resolve.types[**type_id];
                matches!(td.owner, TypeOwner::Interface(id) if id == advice_iface_id)
                    && !matches!(td.kind, TypeDefKind::Type(_))
            })
            .map(|(name, _)| name.clone())
            .collect();

        // Encode and import the advice interface
        let advice_inst =
            encoder::encode_instance_type(&resolve, advice_iface_id, &owned_types, &foreign_types)?;
        let advice_type = self.inner.type_instance(None, &advice_inst);
        let advice_instance = self.inner.import(
            "modulewise:interceptor/advice@0.1.0",
            ComponentTypeRef::Instance(advice_type),
        );
        self.advice_instance = advice_instance;

        Ok(())
    }

    // ============================================================
    // Phase 3: Embed core modules
    // ============================================================

    // Embed the 3 generated core modules (main, shim, fixup).
    fn embed_core_modules(&mut self, main_bytes: &[u8], shim_bytes: &[u8], fixup_bytes: &[u8]) {
        self.main_module = self.inner.core_module_raw(None, main_bytes);
        self.shim_module = self.inner.core_module_raw(None, shim_bytes);
        self.fixup_module = self.inner.core_module_raw(None, fixup_bytes);
    }

    // ============================================================
    // Phase 4: Wire and export
    // ============================================================

    // Instantiate modules, canon lower/lift, and build exports.
    fn wire_and_export(
        &mut self,
        target: &TargetWorld,
        intercepted: &[InterceptedFunction],
    ) -> Result<()> {
        let resolve = target.resolve();

        // Instantiate shim (no imports)
        let shim_instance =
            self.inner
                .core_instantiate(None, self.shim_module, Vec::<(&str, ModuleArg)>::new());

        // Alias shim exports for advice methods (entries 0, 1, 2)
        let shim_constructor =
            self.inner
                .core_alias_export(None, shim_instance, "0", ExportKind::Func);
        let shim_before = self
            .inner
            .core_alias_export(None, shim_instance, "1", ExportKind::Func);
        let shim_after = self
            .inner
            .core_alias_export(None, shim_instance, "2", ExportKind::Func);

        // Alias invocation resource type for resource.drop
        let invocation_type = self.inner.alias_export(
            self.advice_instance,
            "invocation",
            ComponentExportKind::Type,
        );
        let resource_drop = self.inner.resource_drop(invocation_type);

        // Build "advice" import bag for core module
        let advice_bag = self.inner.core_instantiate_exports(
            None,
            [
                (
                    "[constructor]invocation",
                    ExportKind::Func,
                    shim_constructor,
                ),
                ("[method]invocation.before", ExportKind::Func, shim_before),
                ("[method]invocation.after", ExportKind::Func, shim_after),
                ("[resource-drop]invocation", ExportKind::Func, resource_drop),
            ],
        );

        // Alias shim exports for intercepted target functions (entries 3..3+N),
        // grouped by import_module into bags.
        let mut target_bags = Vec::new();
        let mut i = 0usize;
        while i < intercepted.len() {
            let module_name = &intercepted[i].import_module;
            let mut target_shim_funcs = Vec::new();
            while i < intercepted.len() && &intercepted[i].import_module == module_name {
                let func_name = intercepted[i]
                    .export_name
                    .rsplit_once('#')
                    .map(|(_, f)| f)
                    .unwrap_or(&intercepted[i].export_name)
                    .to_string();
                let shim_idx = self.inner.core_alias_export(
                    None,
                    shim_instance,
                    &(3 + i).to_string(),
                    ExportKind::Func,
                );
                target_shim_funcs.push((func_name, shim_idx));
                i += 1;
            }
            let target_exports: Vec<(&str, ExportKind, u32)> = target_shim_funcs
                .iter()
                .map(|(name, idx)| (name.as_str(), ExportKind::Func, *idx))
                .collect();
            let target_bag = self
                .inner
                .core_instantiate_exports(None, target_exports.iter().copied());
            target_bags.push((module_name.clone(), target_bag));
        }

        // Instantiate main module
        let mut main_import_args: Vec<(&str, ModuleArg)> =
            vec![("advice", ModuleArg::Instance(advice_bag))];
        let target_bag_refs: Vec<(&str, ModuleArg)> = target_bags
            .iter()
            .map(|(name, bag)| (name.as_str(), ModuleArg::Instance(*bag)))
            .collect();
        main_import_args.extend_from_slice(&target_bag_refs);
        let main_instance = self
            .inner
            .core_instantiate(None, self.main_module, main_import_args);

        // Alias memory from main module
        let memory =
            self.inner
                .core_alias_export(None, main_instance, "memory", ExportKind::Memory);

        // Alias shim table
        let table =
            self.inner
                .core_alias_export(None, shim_instance, "$imports", ExportKind::Table);

        // Canon lower advice methods
        let realloc =
            self.inner
                .core_alias_export(None, main_instance, "cabi_realloc", ExportKind::Func);

        let constructor_func = self.inner.alias_export(
            self.advice_instance,
            "[constructor]invocation",
            ComponentExportKind::Func,
        );
        let lowered_constructor = self.inner.lower_func(
            None,
            constructor_func,
            [
                CanonicalOption::Memory(memory),
                CanonicalOption::Realloc(realloc),
                CanonicalOption::UTF8,
            ],
        );

        let before_func = self.inner.alias_export(
            self.advice_instance,
            "[method]invocation.before",
            ComponentExportKind::Func,
        );
        let lowered_before = self.inner.lower_func(
            None,
            before_func,
            [
                CanonicalOption::Memory(memory),
                CanonicalOption::Realloc(realloc),
                CanonicalOption::UTF8,
            ],
        );

        let after_func = self.inner.alias_export(
            self.advice_instance,
            "[method]invocation.after",
            ComponentExportKind::Func,
        );
        let lowered_after = self.inner.lower_func(
            None,
            after_func,
            [
                CanonicalOption::Memory(memory),
                CanonicalOption::Realloc(realloc),
                CanonicalOption::UTF8,
            ],
        );

        // Canon lower intercepted target functions
        let mut lowered_target_funcs = Vec::new();
        for ifunc in intercepted {
            let comp_func = self.alias_target_func(&target.exports, ifunc);
            let mut opts = vec![CanonicalOption::Memory(memory), CanonicalOption::UTF8];
            if ifunc.needs_memory {
                opts.insert(1, CanonicalOption::Realloc(realloc));
            }
            let lowered = self.inner.lower_func(None, comp_func, opts);
            lowered_target_funcs.push(lowered);
        }

        // Fixup: patch shim table with real lowered functions
        let total_intercepted = intercepted.len();
        let mut fixup_exports: Vec<(&str, ExportKind, u32)> =
            vec![("$imports", ExportKind::Table, table)];

        fixup_exports.push(("0", ExportKind::Func, lowered_constructor));
        fixup_exports.push(("1", ExportKind::Func, lowered_before));
        fixup_exports.push(("2", ExportKind::Func, lowered_after));

        let entry_names: Vec<String> = (3..3 + total_intercepted).map(|i| i.to_string()).collect();
        for (i, lowered) in lowered_target_funcs.iter().enumerate() {
            fixup_exports.push((&entry_names[i], ExportKind::Func, *lowered));
        }

        let fixup_args_bag = self
            .inner
            .core_instantiate_exports(None, fixup_exports.iter().copied());
        let _fixup_instance = self.inner.core_instantiate(
            None,
            self.fixup_module,
            [("", ModuleArg::Instance(fixup_args_bag))],
        );

        // Canon lift + export
        let mut iface_instance_iter = self.iface_instances.clone().into_iter();
        for we in &target.exports {
            match we {
                WorldExport::Interface(ie) => {
                    let (_, target_instance) = iface_instance_iter.next().ok_or_else(|| {
                        anyhow::anyhow!("interface instance count mismatch during export phase")
                    })?;
                    self.canon_lift_and_export(
                        resolve,
                        ie,
                        main_instance,
                        memory,
                        realloc,
                        target_instance,
                    )?;
                }
                WorldExport::Function(fe) => {
                    let imported_func_idx = self.direct_func_imports[fe.name()];
                    self.canon_lift_and_export_direct(
                        resolve,
                        fe.name(),
                        fe.func(),
                        matches!(fe, FunctionExport::Intercepted(_)),
                        main_instance,
                        memory,
                        realloc,
                        imported_func_idx,
                    )?;
                }
            }
        }

        Ok(())
    }

    // Alias a target function from the appropriate target instance.
    //
    // Interface-bound functions (export_name = "iface#func") are aliased from
    // their target instance. Direct functions (export_name = func name) use
    // their imported component function directly.
    fn alias_target_func(&mut self, exports: &[WorldExport], ifunc: &InterceptedFunction) -> u32 {
        for &(export_idx, target_instance) in &self.iface_instances {
            if let WorldExport::Interface(ie) = &exports[export_idx]
                && let Some(func_name) = ifunc
                    .export_name
                    .strip_prefix(&format!("{}#", ie.full_name))
            {
                return self.inner.alias_export(
                    target_instance,
                    func_name,
                    ComponentExportKind::Func,
                );
            }
        }
        self.direct_func_imports[&ifunc.export_name]
    }

    // Canon lift intercepted functions and alias bypassed functions, then
    // collect the interface's types and functions into a pending export instance.
    fn canon_lift_and_export(
        &mut self,
        resolve: &Resolve,
        ie: &InterfaceExport,
        core_instance: u32,
        memory: u32,
        realloc: u32,
        target_instance: u32,
    ) -> Result<()> {
        // Build type_instances map: owned types alias from target_instance,
        // used (foreign) types alias from their type-providing instance.
        // Start with all known type-providing instances to cover transitive
        // dependencies (e.g., a used type's fields reference types from
        // yet another interface).
        let mut iface_type_instances: HashMap<String, u32> = self.type_instances.clone();
        for name in &ie.owned_types {
            iface_type_instances.insert(name.clone(), target_instance);
        }

        let mut component_funcs: Vec<(String, u32)> = Vec::new();
        let mut outer_type_map = HashMap::new();

        for fe in &ie.functions {
            let func_name = fe.name();

            match fe {
                FunctionExport::Intercepted(_) => {
                    let core_export_name = format!("{}#{}", ie.full_name, func_name);
                    let core_func = self.inner.core_alias_export(
                        None,
                        core_instance,
                        &core_export_name,
                        ExportKind::Func,
                    );

                    let post_return_name = format!("cabi_post_{}#{}", ie.full_name, func_name);
                    let post_return = self.inner.core_alias_export(
                        None,
                        core_instance,
                        &post_return_name,
                        ExportKind::Func,
                    );

                    let wit_func = fe.func();
                    let mut outer_enc = OuterTypeEncoder {
                        builder: &mut self.inner,
                        type_instances: &iface_type_instances,
                        home_interface: Some(ie.interface_id),
                    };
                    let functype =
                        encode_functype(resolve, wit_func, &mut outer_enc, &mut outer_type_map)?;

                    let needs_realloc = types::func_needs_memory(resolve, wit_func);
                    let mut opts = vec![CanonicalOption::Memory(memory), CanonicalOption::UTF8];
                    if needs_realloc {
                        opts.insert(1, CanonicalOption::Realloc(realloc));
                    }
                    opts.push(CanonicalOption::PostReturn(post_return));

                    let lifted = self.inner.lift_func(None, core_func, functype, opts);
                    component_funcs.push((func_name.to_string(), lifted));
                }
                FunctionExport::Bypassed(_) => {
                    let aliased = self.inner.alias_export(
                        target_instance,
                        func_name,
                        ComponentExportKind::Func,
                    );
                    component_funcs.push((func_name.to_string(), aliased));
                }
            }
        }

        if ie.used_types.is_empty() && ie.owned_types.is_empty() {
            // No named types - use flat instance pattern.
            let interface = &resolve.interfaces[ie.interface_id];
            let mut items: Vec<(String, ComponentExportKind, u32)> = Vec::new();

            for name in &ie.owned_types {
                let type_id = interface.types[name.as_str()];
                let type_idx = if let Some(&idx) = outer_type_map.get(&type_id) {
                    idx
                } else {
                    self.inner
                        .alias_export(target_instance, name, ComponentExportKind::Type)
                };
                items.push((name.clone(), ComponentExportKind::Type, type_idx));
            }

            for (func_name, func_idx) in &component_funcs {
                items.push((func_name.clone(), ComponentExportKind::Func, *func_idx));
            }

            self.pending_exports.push(PendingExportInstance {
                name: ie.full_name.clone(),
                items,
            });
        } else {
            // Used types present. Nested component avoids duplicate type registration.
            self.export_nested_interface(resolve, ie, &component_funcs, &iface_type_instances)?;
        }

        Ok(())
    }

    // Build a nested component for an interface export that has used types.
    //
    // The nested component creates a fresh type scope so that used types
    // can be re-exported without triggering duplicate type registration
    // in the decoder.
    //
    // Two-phase encoding (mirrors wit-component's NestedComponentTypeEncoder):
    //   Phase 1 (export_types=false): encode types structurally, named types
    //     become imports with Eq bounds. Encode and import functions.
    //   Phase 2 (export_types=true): re-encode all types as real exports,
    //     re-encode functions and export with type ascription.
    fn export_nested_interface(
        &mut self,
        resolve: &Resolve,
        ie: &InterfaceExport,
        component_funcs: &[(String, u32)],
        iface_type_instances: &HashMap<String, u32>,
    ) -> Result<()> {
        let mut nested = ComponentBuilder::default();
        let interface = &resolve.interfaces[ie.interface_id];

        // Collect (import_name, outer_kind, outer_idx) for instantiation args.
        let mut args: Vec<(String, ComponentExportKind, u32)> = Vec::new();

        // Phase 1: import types and functions.
        // encode_valtype with interface=None encodes everything structurally.
        // export_type (with export_types=false) converts named types into imports.
        let mut import_type_map = HashMap::new();
        let mut import_names: Vec<String> = Vec::new();

        // Encode used types — this creates structural definitions + Eq imports.
        for name in &ie.used_types {
            let type_id = interface.types[name.as_str()];
            let mut enc = NestedTypeEncoder {
                builder: &mut nested,
                export_types: false,
                import_names: &mut import_names,
            };
            encoder::encode_valtype(
                resolve,
                &wit_parser::Type::Id(type_id),
                &mut enc,
                &mut import_type_map,
            )?;
        }

        // Encode and import functions (using type indices from above).
        for (func_name, _) in component_funcs {
            let wit_func = ie
                .functions
                .iter()
                .find(|fe| fe.name() == func_name)
                .ok_or_else(|| anyhow::anyhow!("no FunctionExport for '{func_name}'"))?
                .func();
            let mut enc = NestedTypeEncoder {
                builder: &mut nested,
                export_types: false,
                import_names: &mut import_names,
            };
            let functype_idx = encode_functype(resolve, wit_func, &mut enc, &mut import_type_map)?;
            let import_name = format!("import-func-{func_name}");
            nested.import(&import_name, ComponentTypeRef::Func(functype_idx));
            import_names.push(import_name);
        }

        // Build instantiation args: type imports get outer type indices,
        // function imports get outer function indices.
        let mut func_idx_iter = component_funcs.iter();
        for import_name in &import_names {
            if let Some(type_name) = import_name.strip_prefix("import-type-") {
                let outer_idx = self.inner.alias_export(
                    iface_type_instances[type_name],
                    type_name,
                    ComponentExportKind::Type,
                );
                args.push((import_name.clone(), ComponentExportKind::Type, outer_idx));
            } else if import_name.starts_with("import-func-") {
                let (_, outer_func_idx) = func_idx_iter.next().ok_or_else(|| {
                    anyhow::anyhow!("function index count mismatch during instantiation")
                })?;
                args.push((
                    import_name.clone(),
                    ComponentExportKind::Func,
                    *outer_func_idx,
                ));
            }
        }

        // Phase 2: export types and functions.
        // Re-encode with export_types=true so named types become real exports.
        let mut export_type_map = HashMap::new();
        let mut export_names = Vec::new();

        // Export all types (used + owned).
        for name in ie.used_types.iter().chain(&ie.owned_types) {
            let type_id = interface.types[name.as_str()];
            let mut enc = NestedTypeEncoder {
                builder: &mut nested,
                export_types: true,
                import_names: &mut export_names,
            };
            encoder::encode_valtype(
                resolve,
                &wit_parser::Type::Id(type_id),
                &mut enc,
                &mut export_type_map,
            )?;
        }

        // Export functions with type ascription.
        for (i, (func_name, _)) in component_funcs.iter().enumerate() {
            let wit_func = ie
                .functions
                .iter()
                .find(|fe| fe.name() == func_name)
                .ok_or_else(|| anyhow::anyhow!("no FunctionExport for '{func_name}'"))?
                .func();
            let mut enc = NestedTypeEncoder {
                builder: &mut nested,
                export_types: true,
                import_names: &mut export_names,
            };
            let functype_idx = encode_functype(resolve, wit_func, &mut enc, &mut export_type_map)?;
            nested.export(
                func_name,
                ComponentExportKind::Func,
                i as u32,
                Some(ComponentTypeRef::Func(functype_idx)),
            );
        }

        let comp_idx = self.inner.component(None, nested);
        let inst_idx = self.inner.instantiate(None, comp_idx, args);
        self.inner
            .export(&ie.full_name, ComponentExportKind::Instance, inst_idx, None);
        Ok(())
    }

    // Canon lift an intercepted direct function and export it, or
    // re-export a bypassed direct function's import.
    fn canon_lift_and_export_direct(
        &mut self,
        resolve: &Resolve,
        name: &str,
        wit_func: &wit_parser::Function,
        intercepted: bool,
        core_instance: u32,
        memory: u32,
        realloc: u32,
        imported_func_idx: u32,
    ) -> Result<()> {
        if intercepted {
            let core_func =
                self.inner
                    .core_alias_export(None, core_instance, name, ExportKind::Func);

            let post_return_name = format!("cabi_post_{name}");
            let post_return = self.inner.core_alias_export(
                None,
                core_instance,
                &post_return_name,
                ExportKind::Func,
            );

            let mut type_map = HashMap::new();
            let mut enc = OuterTypeEncoder {
                builder: &mut self.inner,
                type_instances: &self.type_instances,
                home_interface: None,
            };
            let functype = encode_functype(resolve, wit_func, &mut enc, &mut type_map)?;

            let needs_realloc = types::func_needs_memory(resolve, wit_func);
            let mut opts = vec![CanonicalOption::Memory(memory), CanonicalOption::UTF8];
            if needs_realloc {
                opts.insert(1, CanonicalOption::Realloc(realloc));
            }
            opts.push(CanonicalOption::PostReturn(post_return));

            let lifted = self.inner.lift_func(None, core_func, functype, opts);
            self.inner
                .export(name, ComponentExportKind::Func, lifted, None);
        } else {
            self.inner
                .export(name, ComponentExportKind::Func, imported_func_idx, None);
        }

        Ok(())
    }

    // Finish building and return the component bytes.
    fn finish(self) -> Vec<u8> {
        let base_instance_idx = self.inner.instance_count();
        let mut bytes = self.inner.finish();

        // Emit all instance sections first
        for pending in &self.pending_exports {
            let mut instances = ComponentInstanceSection::new();
            let items: Vec<(&str, ComponentExportKind, u32)> = pending
                .items
                .iter()
                .map(|(n, k, i)| (n.as_str(), *k, *i))
                .collect();
            instances.export_items(items);
            instances.append_to_component(&mut bytes);
        }

        // Then emit all export sections (after all instances are defined)
        for (i, pending) in self.pending_exports.iter().enumerate() {
            let mut exports = ComponentExportSection::new();
            exports.export(
                &pending.name,
                ComponentExportKind::Instance,
                base_instance_idx + i as u32,
                None,
            );
            exports.append_to_component(&mut bytes);
        }

        bytes
    }
}

// Component-level type encoder that aliases named types from imported instances.
// Used for both interface-bound and direct function exports.
struct OuterTypeEncoder<'a> {
    builder: &'a mut ComponentBuilder,
    type_instances: &'a HashMap<String, u32>,
    // Set to the interface's ID for interface exports, None for direct exports.
    // When set, foreign types are aliased directly rather than re-encoded.
    home_interface: Option<wit_parser::InterfaceId>,
}

impl TypeEncoder for OuterTypeEncoder<'_> {
    fn type_count(&self) -> u32 {
        self.builder.type_count()
    }

    fn ty(&mut self) -> ComponentTypeEncoder<'_> {
        self.builder.ty(None).1
    }

    fn export_type(&mut self, name: &str, _type_idx: u32) -> u32 {
        self.alias_type_from_instance(name)
    }

    fn declare_resource(&mut self, name: &str) -> u32 {
        self.alias_type_from_instance(name)
    }

    fn is_component_level(&self) -> bool {
        true
    }

    fn interface(&self) -> Option<wit_parser::InterfaceId> {
        self.home_interface
    }

    fn import_type(&mut self, name: &str, _owner: wit_parser::InterfaceId) -> u32 {
        self.alias_type_from_instance(name)
    }
}

impl OuterTypeEncoder<'_> {
    fn alias_type_from_instance(&mut self, name: &str) -> u32 {
        let instance = self.type_instances.get(name).unwrap_or_else(|| {
            panic!(
                "function references type '{}' but no imported instance provides it",
                name
            )
        });
        self.builder
            .alias_export(*instance, name, ComponentExportKind::Type)
    }
}

// Type encoder for nested export components.
//
// Two modes controlled by `export_types`:
// - false (import phase): named types become imports with Eq bounds
// - true (export phase): named types become exports
//
// interface() returns None so all types are encoded structurally.
struct NestedTypeEncoder<'a> {
    builder: &'a mut ComponentBuilder,
    export_types: bool,
    import_names: &'a mut Vec<String>,
}

impl TypeEncoder for NestedTypeEncoder<'_> {
    fn type_count(&self) -> u32 {
        self.builder.type_count()
    }

    fn ty(&mut self) -> ComponentTypeEncoder<'_> {
        self.builder.ty(None).1
    }

    fn export_type(&mut self, name: &str, type_idx: u32) -> u32 {
        if self.export_types {
            self.builder
                .export(name, ComponentExportKind::Type, type_idx, None)
        } else {
            let import_name = format!("import-type-{name}");
            let idx = self.builder.import(
                &import_name,
                ComponentTypeRef::Type(TypeBounds::Eq(type_idx)),
            );
            self.import_names.push(import_name);
            idx
        }
    }

    fn declare_resource(&mut self, name: &str) -> u32 {
        if self.export_types {
            self.builder.export(
                name,
                ComponentExportKind::Type,
                self.builder.type_count(),
                None,
            )
        } else {
            let import_name = format!("import-type-{name}");
            let idx = self.builder.import(
                &import_name,
                ComponentTypeRef::Type(TypeBounds::SubResource),
            );
            self.import_names.push(import_name);
            idx
        }
    }

    fn interface(&self) -> Option<wit_parser::InterfaceId> {
        None
    }
}
