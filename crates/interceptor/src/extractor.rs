use std::collections::HashSet;
use std::path::Path;

use anyhow::{Result, bail};
use wit_parser::{InterfaceId, Resolve, Type, TypeDefKind, TypeId, TypeOwner, WorldItem};

use crate::matcher::Pattern;
use crate::types::{FunctionExport, InterfaceExport, TargetWorld, TypeImport, WorldExport};

/// Extract exported interfaces and functions from a component, classifying as intercepted/bypassed.
pub fn extract_from_wasm(component_bytes: &[u8], patterns: &[Pattern]) -> Result<TargetWorld> {
    let decoded = wit_parser::decoding::decode(component_bytes)?;
    let resolve = decoded.resolve().clone();

    if resolve.worlds.len() != 1 {
        bail!(
            "Expected exactly one world in component, found {}",
            resolve.worlds.len()
        );
    }
    let (world_id, _) = resolve
        .worlds
        .iter()
        .next()
        .ok_or_else(|| anyhow::anyhow!("resolve contains no worlds"))?;

    let exports = extract_world_exports(&resolve, world_id, patterns)?;
    let type_imports = extract_type_imports(&resolve, world_id, &exports)?;
    Ok(TargetWorld::new(resolve, world_id, exports, type_imports))
}

/// Extract all exported interfaces from a WIT path and world name.
pub fn extract_from_wit(
    wit_path: &Path,
    world_name: &str,
    patterns: &[Pattern],
) -> Result<TargetWorld> {
    let mut resolve = Resolve::default();
    let (pkg, _) = resolve.push_path(wit_path)?;
    let world_id = resolve.select_world(&[pkg], Some(world_name))?;

    let exports = extract_world_exports(&resolve, world_id, patterns)?;
    let type_imports = extract_type_imports(&resolve, world_id, &exports)?;
    Ok(TargetWorld::new(resolve, world_id, exports, type_imports))
}

// Extract interfaces from a world's exports, classifying functions as intercepted or bypassed.
//
// With no patterns: all functions in all interfaces are intercepted.
// With patterns: a function is intercepted if ANY pattern matches, otherwise bypassed.
fn extract_world_exports(
    resolve: &Resolve,
    world_id: wit_parser::WorldId,
    patterns: &[Pattern],
) -> Result<Vec<WorldExport>> {
    let world = &resolve.worlds[world_id];
    let mut exports = Vec::new();

    for (_, item) in &world.exports {
        match item {
            WorldItem::Interface { id, .. } => {
                let full_name = format_interface_name(resolve, *id)?;
                let interface = &resolve.interfaces[*id];

                let mut owned_types = Vec::new();
                let mut used_types = Vec::new();
                for (name, type_id) in &interface.types {
                    if is_used_type(resolve, *type_id, *id) {
                        used_types.push(name.clone());
                    } else {
                        owned_types.push(name.clone());
                    }
                }

                let mut functions = Vec::new();

                for (_, func) in &interface.functions {
                    let func_matched = patterns.is_empty()
                        || patterns
                            .iter()
                            .any(|p| p.matches(Some(&full_name), &func.name));

                    if func_matched {
                        functions.push(FunctionExport::Intercepted(func.clone()));
                    } else {
                        functions.push(FunctionExport::Bypassed(func.clone()));
                    }
                }

                if !functions.is_empty() {
                    exports.push(WorldExport::Interface(InterfaceExport {
                        interface_id: *id,
                        full_name,
                        functions,
                        owned_types,
                        used_types,
                    }));
                }
            }
            WorldItem::Function(func) => {
                let func_matched =
                    patterns.is_empty() || patterns.iter().any(|p| p.matches(None, &func.name));

                if func_matched {
                    exports.push(WorldExport::Function(FunctionExport::Intercepted(
                        func.clone(),
                    )));
                } else {
                    exports.push(WorldExport::Function(FunctionExport::Bypassed(
                        func.clone(),
                    )));
                }
            }
            WorldItem::Type(_) => {}
        }
    }

    Ok(exports)
}

// Extract only the type-providing interface imports actually referenced by exported functions.
//
// Walks all exported function signatures (params + results) recursively through the
// type graph. For each named type owned by a foreign interface, records the
// (InterfaceId, type_name) pair. Then builds TypeImport entries containing only
// those interfaces and type names that are actually needed.
fn extract_type_imports(
    resolve: &Resolve,
    world_id: wit_parser::WorldId,
    exports: &[WorldExport],
) -> Result<Vec<TypeImport>> {
    let referenced = collect_referenced_foreign_types(resolve, exports);

    if referenced.is_empty() {
        return Ok(Vec::new());
    }

    // Walk world imports in their stable wit_parser order, filtering to only
    // interfaces that provide types we actually reference. Within each interface,
    // preserve the type ordering from the interface definition.
    let world = &resolve.worlds[world_id];
    let mut type_imports = Vec::new();

    for (_key, item) in &world.imports {
        if let WorldItem::Interface { id, .. } = item {
            let interface = &resolve.interfaces[*id];
            let type_names: Vec<String> = interface
                .types
                .keys()
                .filter(|name| referenced.contains(&(*id, name.to_string())))
                .cloned()
                .collect();

            if type_names.is_empty() {
                continue;
            }

            let full_name = format_interface_name(resolve, *id)?;
            type_imports.push(TypeImport {
                interface_id: *id,
                full_name,
                type_names,
            });
        }
    }

    Ok(type_imports)
}

// Collect all (InterfaceId, type_name) pairs referenced by exported function signatures.
//
// For interface exports, a type is "foreign" if it's owned by a different interface
// than the one being exported. For direct function exports, any interface-owned type
// is foreign (the function itself hangs off the world, not an interface).
fn collect_referenced_foreign_types(
    resolve: &Resolve,
    exports: &[WorldExport],
) -> HashSet<(InterfaceId, String)> {
    let mut referenced = HashSet::new();
    let mut visited = HashSet::new();

    for we in exports {
        match we {
            WorldExport::Interface(ie) => {
                for fe in &ie.functions {
                    let func = fe.func();
                    for (_, ty) in &func.params {
                        walk_type(
                            resolve,
                            ty,
                            Some(ie.interface_id),
                            &mut referenced,
                            &mut visited,
                        );
                    }
                    if let Some(ty) = &func.result {
                        walk_type(
                            resolve,
                            ty,
                            Some(ie.interface_id),
                            &mut referenced,
                            &mut visited,
                        );
                    }
                }
            }
            WorldExport::Function(fe) => {
                let func = fe.func();
                for (_, ty) in &func.params {
                    walk_type(resolve, ty, None, &mut referenced, &mut visited);
                }
                if let Some(ty) = &func.result {
                    walk_type(resolve, ty, None, &mut referenced, &mut visited);
                }
            }
        }
    }

    referenced
}

// Check if a type in an interface was imported via `use` from another interface.
//
// `use other.{foo}` creates a `TypeDefKind::Type(inner)` alias owned by the
// importing interface. This follows the alias chain to the concrete type and
// returns true if it's owned by a different interface.
fn is_used_type(resolve: &Resolve, type_id: TypeId, home: InterfaceId) -> bool {
    let td = &resolve.types[type_id];
    match &td.kind {
        TypeDefKind::Type(Type::Id(inner)) => is_used_type(resolve, *inner, home),
        _ => matches!(td.owner, TypeOwner::Interface(owner) if owner != home),
    }
}

// Recursively walk a type, collecting foreign (InterfaceId, type_name) references.
//
// `home_interface` is the interface the current export belongs to (None for direct
// function exports). A type is "foreign" when it has a name, is owned by an interface,
// and that interface differs from `home_interface`.
fn walk_type(
    resolve: &Resolve,
    ty: &Type,
    home_interface: Option<InterfaceId>,
    referenced: &mut HashSet<(InterfaceId, String)>,
    visited: &mut HashSet<TypeId>,
) {
    let Type::Id(id) = ty else {
        return; // Primitive type, no foreign references.
    };

    if !visited.insert(*id) {
        return;
    }

    let type_def = &resolve.types[*id];

    // Check if this is a named type from a foreign interface.
    // Skip `use` aliases (TypeDefKind::Type pointing to another interface's type)
    // since the recursion will reach the original owner and insert the correct entry.
    let is_use_alias = matches!(
        (&type_def.kind, type_def.owner),
        (TypeDefKind::Type(Type::Id(inner)), TypeOwner::Interface(owner))
            if matches!(resolve.types[*inner].owner, TypeOwner::Interface(inner_owner) if inner_owner != owner)
    );

    if !is_use_alias
        && let Some(name) = &type_def.name
        && let TypeOwner::Interface(owner) = type_def.owner
    {
        let is_foreign = match home_interface {
            Some(home) => owner != home,
            None => true, // direct exports: all interface-owned types are foreign
        };
        if is_foreign {
            referenced.insert((owner, name.clone()));
        }
    }

    // Recurse into composite type structure.
    match &type_def.kind {
        TypeDefKind::Record(r) => {
            for f in &r.fields {
                walk_type(resolve, &f.ty, home_interface, referenced, visited);
            }
        }
        TypeDefKind::Tuple(t) => {
            for ty in &t.types {
                walk_type(resolve, ty, home_interface, referenced, visited);
            }
        }
        TypeDefKind::Variant(v) => {
            for c in &v.cases {
                if let Some(ty) = &c.ty {
                    walk_type(resolve, ty, home_interface, referenced, visited);
                }
            }
        }
        TypeDefKind::Option(t) => {
            walk_type(resolve, t, home_interface, referenced, visited);
        }
        TypeDefKind::Result(r) => {
            if let Some(t) = &r.ok {
                walk_type(resolve, t, home_interface, referenced, visited);
            }
            if let Some(t) = &r.err {
                walk_type(resolve, t, home_interface, referenced, visited);
            }
        }
        TypeDefKind::List(t) => {
            walk_type(resolve, t, home_interface, referenced, visited);
        }
        TypeDefKind::Type(inner) => {
            walk_type(resolve, inner, home_interface, referenced, visited);
        }
        TypeDefKind::Handle(wit_parser::Handle::Own(id))
        | TypeDefKind::Handle(wit_parser::Handle::Borrow(id)) => {
            walk_type(resolve, &Type::Id(*id), home_interface, referenced, visited);
        }
        TypeDefKind::Future(payload) | TypeDefKind::Stream(payload) => {
            if let Some(t) = payload {
                walk_type(resolve, t, home_interface, referenced, visited);
            }
        }
        TypeDefKind::Map(k, v) => {
            walk_type(resolve, k, home_interface, referenced, visited);
            walk_type(resolve, v, home_interface, referenced, visited);
        }
        TypeDefKind::FixedSizeList(t, _) => {
            walk_type(resolve, t, home_interface, referenced, visited);
        }
        TypeDefKind::Flags(_)
        | TypeDefKind::Enum(_)
        | TypeDefKind::Resource
        | TypeDefKind::Unknown => {}
    }
}

/// Format a fully-qualified interface name from the Resolve.
///
/// Produces names like "namespace:package/interface@version".
pub fn format_interface_name(resolve: &Resolve, id: wit_parser::InterfaceId) -> Result<String> {
    let interface = &resolve.interfaces[id];
    let iface_name = interface
        .name
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("Interface has no name"))?;

    let pkg_id = interface
        .package
        .ok_or_else(|| anyhow::anyhow!("Interface has no package"))?;
    let package = &resolve.packages[pkg_id];
    let pkg_name = &package.name;

    let version_suffix = pkg_name
        .version
        .as_ref()
        .map(|v| format!("@{v}"))
        .unwrap_or_default();

    Ok(format!(
        "{}:{}/{}{}",
        pkg_name.namespace, pkg_name.name, iface_name, version_suffix
    ))
}
