//! Encode `wit_parser` types into `wasm_encoder` component types.
//!
//! Provides a `TypeEncoder` trait (modeled on `wit-component`'s internal
//! `ValtypeEncoder`) that abstracts over different encoding contexts:
//! - `InstanceTypeEncoder` for interface instance type declarations (with foreign type aliasing)
//! - `OuterTypeEncoder` (in `builder.rs`) for outer component-level type aliasing
//!
//! The `encode_valtype` function recursively handles all `TypeDefKind` variants
//! using the Resolve as the single source of truth.

use std::collections::HashMap;

use anyhow::{Result, bail};
use wasm_encoder::*;
use wit_parser::*;

/// Trait for contexts that can receive component type definitions.
///
/// Different encoding contexts handle named types and resources differently:
/// - `InstanceTypeEncoder`: exports owned types, aliases foreign types via `Alias::Outer`
/// - `OuterTypeEncoder` (in `builder.rs`): aliases types from imported instances
///
/// Modeled after `wit-component`'s `ValtypeEncoder` trait.
pub trait TypeEncoder {
    /// Number of types defined so far (the index of the next type to be added).
    fn type_count(&self) -> u32;

    /// Begin defining a new type. Returns a `ComponentTypeEncoder` that must
    /// be used before calling any other method.
    fn ty(&mut self) -> ComponentTypeEncoder<'_>;

    /// Handle a named type export. Returns the index assigned to this type.
    ///
    /// For `InstanceTypeEncoder`: exports the type with `TypeBounds::Eq(type_idx)`.
    /// For `OuterTypeEncoder`: aliases the type from an imported instance.
    fn export_type(&mut self, name: &str, type_idx: u32) -> u32;

    /// Handle a resource type declaration. Returns the index assigned to the resource.
    ///
    /// For `InstanceTypeEncoder`: exports with `TypeBounds::SubResource`.
    /// For `OuterTypeEncoder`: aliases from an imported instance.
    fn declare_resource(&mut self, name: &str) -> u32;

    /// The interface this encoder is encoding for, if any.
    /// Returns `None` for component-level encoders.
    fn interface(&self) -> Option<InterfaceId> {
        None
    }

    /// Whether this encoder operates at the component level (vs instance level).
    ///
    /// Component-level encoders always alias named types from imported instances
    /// rather than encoding them structurally, because the structural definitions
    /// already exist at the instance level.
    fn is_component_level(&self) -> bool {
        false
    }

    /// Import a named type from a foreign interface via `Alias::Outer`.
    /// Only called on instance-type encoders when a referenced type belongs
    /// to a different interface than the one being encoded.
    fn import_type(&mut self, _name: &str, _owner: InterfaceId) -> u32 {
        panic!("import_type not supported on this encoder")
    }
}

/// Wrapper around `InstanceType` that knows which interface it encodes
/// and can alias types from foreign interfaces via `Alias::Outer`.
pub struct InstanceTypeEncoder<'a> {
    pub inst: InstanceType,
    interface_id: InterfaceId,
    /// Maps (InterfaceId, type_name) => component-level type index.
    /// Pre-aliased at the component level before encoding begins.
    foreign_types: &'a HashMap<(InterfaceId, String), u32>,
}

impl<'a> InstanceTypeEncoder<'a> {
    pub fn new(
        interface_id: InterfaceId,
        foreign_types: &'a HashMap<(InterfaceId, String), u32>,
    ) -> Self {
        Self {
            inst: InstanceType::new(),
            interface_id,
            foreign_types,
        }
    }
}

impl TypeEncoder for InstanceTypeEncoder<'_> {
    fn type_count(&self) -> u32 {
        self.inst.type_count()
    }

    fn ty(&mut self) -> ComponentTypeEncoder<'_> {
        self.inst.ty()
    }

    fn export_type(&mut self, name: &str, type_idx: u32) -> u32 {
        self.inst
            .export(name, ComponentTypeRef::Type(TypeBounds::Eq(type_idx)));
        self.inst.type_count() - 1
    }

    fn declare_resource(&mut self, name: &str) -> u32 {
        self.inst
            .export(name, ComponentTypeRef::Type(TypeBounds::SubResource));
        self.inst.type_count() - 1
    }

    fn interface(&self) -> Option<InterfaceId> {
        Some(self.interface_id)
    }

    fn import_type(&mut self, name: &str, owner: InterfaceId) -> u32 {
        let component_idx = *self
            .foreign_types
            .get(&(owner, name.to_string()))
            .unwrap_or_else(|| {
                panic!(
                    "foreign type '{}' from interface {:?} not pre-aliased at component level",
                    name, owner
                )
            });

        // Alias from outer (component) scope and export so the type is named,
        // matching wit-component's encoding of `use`d types.
        self.inst.alias(Alias::Outer {
            count: 1,
            index: component_idx,
            kind: ComponentOuterAliasKind::Type,
        });
        let alias_idx = self.inst.type_count() - 1;
        self.inst
            .export(name, ComponentTypeRef::Type(TypeBounds::Eq(alias_idx)));
        self.inst.type_count() - 1
    }
}

/// Encode a `wit_parser::Type` into a `wasm_encoder::ComponentValType`,
/// defining any necessary complex types in the provided encoder.
///
/// `type_map` deduplicates: if a `TypeId` was already defined, returns
/// `ComponentValType::Type(existing_index)` instead of redefining.
pub fn encode_valtype(
    resolve: &Resolve,
    ty: &Type,
    enc: &mut impl TypeEncoder,
    type_map: &mut HashMap<TypeId, u32>,
) -> Result<ComponentValType> {
    Ok(match *ty {
        Type::Bool => ComponentValType::Primitive(PrimitiveValType::Bool),
        Type::U8 => ComponentValType::Primitive(PrimitiveValType::U8),
        Type::U16 => ComponentValType::Primitive(PrimitiveValType::U16),
        Type::U32 => ComponentValType::Primitive(PrimitiveValType::U32),
        Type::U64 => ComponentValType::Primitive(PrimitiveValType::U64),
        Type::S8 => ComponentValType::Primitive(PrimitiveValType::S8),
        Type::S16 => ComponentValType::Primitive(PrimitiveValType::S16),
        Type::S32 => ComponentValType::Primitive(PrimitiveValType::S32),
        Type::S64 => ComponentValType::Primitive(PrimitiveValType::S64),
        Type::F32 => ComponentValType::Primitive(PrimitiveValType::F32),
        Type::F64 => ComponentValType::Primitive(PrimitiveValType::F64),
        Type::Char => ComponentValType::Primitive(PrimitiveValType::Char),
        Type::String => ComponentValType::Primitive(PrimitiveValType::String),
        Type::ErrorContext => ComponentValType::Primitive(PrimitiveValType::ErrorContext),

        Type::Id(id) => {
            if let Some(&index) = type_map.get(&id) {
                return Ok(ComponentValType::Type(index));
            }

            let type_def = &resolve.types[id];

            // Short-circuit: alias named types instead of encoding structurally when:
            // - Component-level encoders: always alias (types are already defined at instance level)
            // - Instance-level encoders: alias only foreign types (owned by a different interface)
            if let Some(name) = &type_def.name
                && let TypeOwner::Interface(owner_iface) = type_def.owner
            {
                let is_foreign = enc.interface().is_some_and(|enc_iface| owner_iface != enc_iface);
                if enc.is_component_level() || is_foreign {
                    let index = enc.import_type(name, owner_iface);
                    type_map.insert(id, index);
                    return Ok(ComponentValType::Type(index));
                }
            }

            let encoded = match &type_def.kind {
                TypeDefKind::Record(r) => {
                    let fields = r
                        .fields
                        .iter()
                        .map(|f| {
                            Ok((
                                f.name.as_str(),
                                encode_valtype(resolve, &f.ty, enc, type_map)?,
                            ))
                        })
                        .collect::<Result<Vec<_>>>()?;
                    let index = enc.type_count();
                    enc.ty().defined_type().record(fields);
                    ComponentValType::Type(index)
                }
                TypeDefKind::Tuple(t) => {
                    let tys = t
                        .types
                        .iter()
                        .map(|ty| encode_valtype(resolve, ty, enc, type_map))
                        .collect::<Result<Vec<_>>>()?;
                    let index = enc.type_count();
                    enc.ty().defined_type().tuple(tys);
                    ComponentValType::Type(index)
                }
                TypeDefKind::Flags(f) => {
                    let index = enc.type_count();
                    enc.ty()
                        .defined_type()
                        .flags(f.flags.iter().map(|f| f.name.as_str()));
                    ComponentValType::Type(index)
                }
                TypeDefKind::Variant(v) => {
                    let cases = v
                        .cases
                        .iter()
                        .map(|c| {
                            let ty =
                                c.ty.as_ref()
                                    .map(|t| encode_valtype(resolve, t, enc, type_map))
                                    .transpose()?;
                            Ok((c.name.as_str(), ty, None))
                        })
                        .collect::<Result<Vec<_>>>()?;
                    let index = enc.type_count();
                    enc.ty().defined_type().variant(cases);
                    ComponentValType::Type(index)
                }
                TypeDefKind::Enum(e) => {
                    let index = enc.type_count();
                    enc.ty()
                        .defined_type()
                        .enum_type(e.cases.iter().map(|c| c.name.as_str()));
                    ComponentValType::Type(index)
                }
                TypeDefKind::Option(t) => {
                    let inner = encode_valtype(resolve, t, enc, type_map)?;
                    let index = enc.type_count();
                    enc.ty().defined_type().option(inner);
                    ComponentValType::Type(index)
                }
                TypeDefKind::Result(r) => {
                    let ok =
                        r.ok.as_ref()
                            .map(|t| encode_valtype(resolve, t, enc, type_map))
                            .transpose()?;
                    let err = r
                        .err
                        .as_ref()
                        .map(|t| encode_valtype(resolve, t, enc, type_map))
                        .transpose()?;
                    let index = enc.type_count();
                    enc.ty().defined_type().result(ok, err);
                    ComponentValType::Type(index)
                }
                TypeDefKind::List(t) => {
                    let inner = encode_valtype(resolve, t, enc, type_map)?;
                    let index = enc.type_count();
                    enc.ty().defined_type().list(inner);
                    ComponentValType::Type(index)
                }
                TypeDefKind::Type(inner) => {
                    let encoded = encode_valtype(resolve, inner, enc, type_map)?;
                    if let ComponentValType::Type(idx) = encoded {
                        type_map.insert(id, idx);
                    }
                    return Ok(encoded);
                }
                TypeDefKind::Handle(wit_parser::Handle::Own(resource_id)) => {
                    let resource_ty =
                        encode_valtype(resolve, &Type::Id(*resource_id), enc, type_map)?;
                    let resource_idx = match resource_ty {
                        ComponentValType::Type(idx) => idx,
                        _ => bail!("resource must be an indexed type"),
                    };
                    let index = enc.type_count();
                    enc.ty().defined_type().own(resource_idx);
                    ComponentValType::Type(index)
                }
                TypeDefKind::Handle(wit_parser::Handle::Borrow(resource_id)) => {
                    let resource_ty =
                        encode_valtype(resolve, &Type::Id(*resource_id), enc, type_map)?;
                    let resource_idx = match resource_ty {
                        ComponentValType::Type(idx) => idx,
                        _ => bail!("resource must be an indexed type"),
                    };
                    let index = enc.type_count();
                    enc.ty().defined_type().borrow(resource_idx);
                    ComponentValType::Type(index)
                }
                TypeDefKind::Resource => {
                    let name = type_def
                        .name
                        .as_ref()
                        .ok_or_else(|| anyhow::anyhow!("resource must be named"))?;
                    let index = enc.declare_resource(name);
                    type_map.insert(id, index);
                    return Ok(ComponentValType::Type(index));
                }
                TypeDefKind::Future(payload) => {
                    let inner = payload
                        .as_ref()
                        .map(|t| encode_valtype(resolve, t, enc, type_map))
                        .transpose()?;
                    let index = enc.type_count();
                    enc.ty().defined_type().future(inner);
                    ComponentValType::Type(index)
                }
                TypeDefKind::Stream(payload) => {
                    let inner = payload
                        .as_ref()
                        .map(|t| encode_valtype(resolve, t, enc, type_map))
                        .transpose()?;
                    let index = enc.type_count();
                    enc.ty().defined_type().stream(inner);
                    ComponentValType::Type(index)
                }
                TypeDefKind::Map(key_ty, value_ty) => {
                    let key = encode_valtype(resolve, key_ty, enc, type_map)?;
                    let value = encode_valtype(resolve, value_ty, enc, type_map)?;
                    let index = enc.type_count();
                    enc.ty().defined_type().map(key, value);
                    ComponentValType::Type(index)
                }
                TypeDefKind::FixedSizeList(t, size) => {
                    let inner = encode_valtype(resolve, t, enc, type_map)?;
                    let index = enc.type_count();
                    enc.ty().defined_type().fixed_size_list(inner, *size);
                    ComponentValType::Type(index)
                }
                TypeDefKind::Unknown => bail!("unknown type"),
            };

            // Named types get exported from the encoding context
            if let Some(name) = &type_def.name {
                let index = match encoded {
                    ComponentValType::Type(idx) => idx,
                    ComponentValType::Primitive(p) => {
                        let idx = enc.type_count();
                        enc.ty().defined_type().primitive(p);
                        idx
                    }
                };
                let export_index = enc.export_type(name, index);
                type_map.insert(id, export_index);
                ComponentValType::Type(export_index)
            } else {
                if let ComponentValType::Type(index) = encoded {
                    type_map.insert(id, index);
                }
                encoded
            }
        }
    })
}

/// Encode a `wit_parser::Function` into a function type within the given encoder.
/// Returns the type index of the function type.
pub fn encode_functype(
    resolve: &Resolve,
    func: &wit_parser::Function,
    enc: &mut impl TypeEncoder,
    type_map: &mut HashMap<TypeId, u32>,
) -> Result<u32> {
    let params: Vec<(&str, ComponentValType)> = func
        .params
        .iter()
        .map(|(name, ty)| Ok((name.as_str(), encode_valtype(resolve, ty, enc, type_map)?)))
        .collect::<Result<Vec<_>>>()?;

    let result = func
        .result
        .map(|ty| encode_valtype(resolve, &ty, enc, type_map))
        .transpose()?;

    let index = enc.type_count();
    enc.ty().function().params(params).result(result);
    Ok(index)
}

/// Encode a full interface's instance type from the Resolve.
///
/// `foreign_types` maps `(InterfaceId, type_name)` to component-level type
/// indices for types owned by other interfaces. These are pre-aliased at
/// the component level and referenced via `Alias::Outer` inside the instance type.
///
/// Returns the completed `InstanceType` with all types and functions declared.
pub fn encode_instance_type(
    resolve: &Resolve,
    interface_id: InterfaceId,
    owned_types: &[String],
    foreign_types: &HashMap<(InterfaceId, String), u32>,
) -> Result<InstanceType> {
    let interface = &resolve.interfaces[interface_id];
    let mut enc = InstanceTypeEncoder::new(interface_id, foreign_types);
    let mut type_map = HashMap::new();

    // Only encode types that this interface directly defines.
    // Types imported via `use` are handled by the foreign-type short-circuit
    // in encode_valtype when they're encountered in function signatures.
    for name in owned_types {
        let type_id = interface.types[name.as_str()];
        encode_valtype(resolve, &Type::Id(type_id), &mut enc, &mut type_map)?;
    }

    // Encode all functions
    for (name, func) in &interface.functions {
        let functype_idx = encode_functype(resolve, func, &mut enc, &mut type_map)?;
        enc.inst.export(name, ComponentTypeRef::Func(functype_idx));
    }

    Ok(enc.inst)
}
