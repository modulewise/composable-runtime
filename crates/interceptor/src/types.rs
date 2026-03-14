use anyhow::Result;
use wit_parser::{InterfaceId, Resolve, Type, TypeDefKind, WorldId, abi::WasmType};

/// The parsed target world with intercepted/bypassed classification applied.
pub struct TargetWorld {
    resolve: Resolve,
    pub world_id: WorldId,
    pub exports: Vec<WorldExport>,
    pub type_imports: Vec<TypeImport>,
}

impl TargetWorld {
    /// Create a new TargetWorld.
    pub fn new(
        resolve: Resolve,
        world_id: WorldId,
        exports: Vec<WorldExport>,
        type_imports: Vec<TypeImport>,
    ) -> Self {
        Self {
            resolve,
            world_id,
            exports,
            type_imports,
        }
    }

    /// Access the Resolve for type encoding operations.
    ///
    /// Used by encoder.rs functions that must recursively walk the type
    /// graph to translate wit_parser types into wasm_encoder types.
    pub fn resolve(&self) -> &Resolve {
        &self.resolve
    }
}

/// An interface imported to provide types via `use` statements.
///
/// This captures only the type-providing aspect of the import, not the full
/// interface contract. The interface_id is retained for type encoding operations.
#[derive(Debug, Clone)]
pub struct TypeImport {
    pub interface_id: InterfaceId,
    pub full_name: String,
    pub type_names: Vec<String>,
}

/// A world export. Either an interface or a direct function.
#[derive(Debug, Clone)]
pub enum WorldExport {
    Interface(InterfaceExport),
    Function(FunctionExport),
}

/// An exported interface containing a mix of intercepted and bypassed functions.
#[derive(Debug, Clone)]
pub struct InterfaceExport {
    pub interface_id: InterfaceId,
    pub full_name: String,
    pub functions: Vec<FunctionExport>,
    /// Types directly defined by this interface.
    pub owned_types: Vec<String>,
    /// Types imported via `use` from other interfaces.
    pub used_types: Vec<String>,
}

/// A function export that is either intercepted (advice) or bypassed (alias).
#[derive(Debug, Clone)]
pub enum FunctionExport {
    /// Matched by --match: goes through the advice interceptor.
    Intercepted(wit_parser::Function),
    /// Not matched: aliased directly from the target import.
    Bypassed(wit_parser::Function),
}

impl FunctionExport {
    pub fn func(&self) -> &wit_parser::Function {
        match self {
            FunctionExport::Intercepted(f) => f,
            FunctionExport::Bypassed(f) => f,
        }
    }

    pub fn name(&self) -> &str {
        &self.func().name
    }
}

// ============================================================
// WAT generation helpers
// ============================================================

/// Map a WasmType to its core wasm type string for WAT generation.
pub fn wasm_type_str(wt: &WasmType) -> &'static str {
    match wt {
        WasmType::I32 | WasmType::Pointer | WasmType::Length => "i32",
        WasmType::I64 | WasmType::PointerOrI64 => "i64",
        WasmType::F32 => "f32",
        WasmType::F64 => "f64",
    }
}

/// Flatten a wit_parser::Type into core wasm type strings.
pub fn flat_types(resolve: &Resolve, ty: Type) -> Result<Vec<&'static str>> {
    let mut buf = [WasmType::I32; 64];
    let mut flat = wit_parser::abi::FlatTypes::new(&mut buf);
    let ok = resolve.push_flat(&ty, &mut flat);
    if !ok {
        anyhow::bail!(
            "type '{:?}' has too many flat fields to represent directly; use indirection",
            ty
        );
    }
    Ok(flat.to_vec().iter().map(wasm_type_str).collect())
}

/// Value variant discriminant for a wit_parser::Type.
/// 0=str, 1=num-s64, 2=num-u64, 3=num-f32, 4=num-f64, 5=boolean, 6=complex
pub fn value_discriminant(resolve: &Resolve, ty: Type) -> u8 {
    match ty {
        Type::String => 0,
        Type::S8 | Type::S16 | Type::S32 | Type::S64 => 1,
        Type::U8 | Type::U16 | Type::U32 | Type::U64 | Type::Char => 2,
        Type::F32 => 3,
        Type::F64 => 4,
        Type::Bool => 5,
        Type::Id(id) => {
            let type_def = &resolve.types[id];
            match &type_def.kind {
                TypeDefKind::Type(inner) => value_discriminant(resolve, *inner),
                _ => 6, // complex
            }
        }
        Type::ErrorContext => 6, // complex
    }
}

/// WIT type name string for the `type-name` field of the `arg` record.
pub fn type_name_str(resolve: &Resolve, ty: Type) -> String {
    match ty {
        Type::Bool => "bool".to_string(),
        Type::U8 => "u8".to_string(),
        Type::U16 => "u16".to_string(),
        Type::U32 => "u32".to_string(),
        Type::U64 => "u64".to_string(),
        Type::S8 => "s8".to_string(),
        Type::S16 => "s16".to_string(),
        Type::S32 => "s32".to_string(),
        Type::S64 => "s64".to_string(),
        Type::F32 => "f32".to_string(),
        Type::F64 => "f64".to_string(),
        Type::Char => "char".to_string(),
        Type::String => "string".to_string(),
        Type::Id(id) => {
            let type_def = &resolve.types[id];
            if let Some(name) = &type_def.name {
                name.clone()
            } else {
                match &type_def.kind {
                    TypeDefKind::Type(inner) => type_name_str(resolve, *inner),
                    TypeDefKind::Record(_) => "record".to_string(),
                    TypeDefKind::List(_) => "list".to_string(),
                    TypeDefKind::Tuple(_) => "tuple".to_string(),
                    TypeDefKind::Variant(_) => "variant".to_string(),
                    TypeDefKind::Enum(_) => "enum".to_string(),
                    TypeDefKind::Flags(_) => "flags".to_string(),
                    TypeDefKind::Option(_) => "option".to_string(),
                    TypeDefKind::Result(_) => "result".to_string(),
                    TypeDefKind::Handle(_) => "handle".to_string(),
                    TypeDefKind::Resource => "resource".to_string(),
                    TypeDefKind::Future(_) => "future".to_string(),
                    TypeDefKind::Stream(_) => "stream".to_string(),
                    TypeDefKind::Map(_, _) => "map".to_string(),
                    TypeDefKind::FixedLengthList(_, _) => "fixed-length-list".to_string(),
                    TypeDefKind::Unknown => "unknown".to_string(),
                }
            }
        }
        _ => "unknown".to_string(),
    }
}

/// Whether a wit_parser::Type requires memory+realloc (strings, lists, records, etc.).
pub fn type_needs_memory(resolve: &Resolve, ty: &Type) -> bool {
    match ty {
        Type::String => true,
        Type::Id(id) => {
            let type_def = &resolve.types[*id];
            match &type_def.kind {
                TypeDefKind::Record(r) => {
                    r.fields.iter().any(|f| type_needs_memory(resolve, &f.ty))
                }
                TypeDefKind::List(_) => true,
                TypeDefKind::Tuple(t) => t.types.iter().any(|ty| type_needs_memory(resolve, ty)),
                TypeDefKind::Variant(v) => v.cases.iter().any(|c| {
                    c.ty.as_ref()
                        .is_some_and(|ty| type_needs_memory(resolve, ty))
                }),
                TypeDefKind::Option(t) => type_needs_memory(resolve, t),
                TypeDefKind::Result(r) => {
                    r.ok.as_ref().is_some_and(|t| type_needs_memory(resolve, t))
                        || r.err
                            .as_ref()
                            .is_some_and(|t| type_needs_memory(resolve, t))
                }
                TypeDefKind::Enum(_) => false,
                TypeDefKind::Flags(_) => false,
                TypeDefKind::Type(inner) => type_needs_memory(resolve, inner),
                TypeDefKind::Handle(_) => false,
                TypeDefKind::Resource => false,
                TypeDefKind::Future(_) => false,
                TypeDefKind::Stream(_) => false,
                TypeDefKind::Map(k, v) => {
                    type_needs_memory(resolve, k) || type_needs_memory(resolve, v)
                }
                TypeDefKind::FixedLengthList(t, _) => type_needs_memory(resolve, t),
                TypeDefKind::Unknown => false,
            }
        }
        _ => false,
    }
}

/// Whether a function needs memory+realloc (any param or result contains strings/lists/etc.).
pub fn func_needs_memory(resolve: &Resolve, func: &wit_parser::Function) -> bool {
    func.params
        .iter()
        .any(|p| type_needs_memory(resolve, &p.ty))
        || func
            .result
            .as_ref()
            .is_some_and(|ty| type_needs_memory(resolve, ty))
}

#[cfg(test)]
mod tests {
    use super::*;
    use wit_parser::{Resolve, Type, TypeDef, TypeDefKind, TypeOwner};

    // === wasm_type_str ===

    #[test]
    fn wasm_type_str_i32() {
        assert_eq!(wasm_type_str(&WasmType::I32), "i32");
    }

    #[test]
    fn wasm_type_str_i64() {
        assert_eq!(wasm_type_str(&WasmType::I64), "i64");
    }

    #[test]
    fn wasm_type_str_f32() {
        assert_eq!(wasm_type_str(&WasmType::F32), "f32");
    }

    #[test]
    fn wasm_type_str_f64() {
        assert_eq!(wasm_type_str(&WasmType::F64), "f64");
    }

    #[test]
    fn wasm_type_str_pointer() {
        assert_eq!(wasm_type_str(&WasmType::Pointer), "i32");
    }

    #[test]
    fn wasm_type_str_length() {
        assert_eq!(wasm_type_str(&WasmType::Length), "i32");
    }

    #[test]
    fn wasm_type_str_pointer_or_i64() {
        assert_eq!(wasm_type_str(&WasmType::PointerOrI64), "i64");
    }

    // === value_discriminant ===

    #[test]
    fn value_discriminant_string() {
        let resolve = Resolve::default();
        assert_eq!(value_discriminant(&resolve, Type::String), 0);
    }

    #[test]
    fn value_discriminant_signed_ints() {
        let resolve = Resolve::default();
        for ty in [Type::S8, Type::S16, Type::S32, Type::S64] {
            assert_eq!(value_discriminant(&resolve, ty), 1, "{ty:?}");
        }
    }

    #[test]
    fn value_discriminant_unsigned_ints_and_char() {
        let resolve = Resolve::default();
        for ty in [Type::U8, Type::U16, Type::U32, Type::U64, Type::Char] {
            assert_eq!(value_discriminant(&resolve, ty), 2, "{ty:?}");
        }
    }

    #[test]
    fn value_discriminant_f32() {
        let resolve = Resolve::default();
        assert_eq!(value_discriminant(&resolve, Type::F32), 3);
    }

    #[test]
    fn value_discriminant_f64() {
        let resolve = Resolve::default();
        assert_eq!(value_discriminant(&resolve, Type::F64), 4);
    }

    #[test]
    fn value_discriminant_bool() {
        let resolve = Resolve::default();
        assert_eq!(value_discriminant(&resolve, Type::Bool), 5);
    }

    #[test]
    fn value_discriminant_record_is_complex() {
        let mut resolve = Resolve::default();
        let id = resolve.types.alloc(TypeDef {
            name: Some("point".into()),
            kind: TypeDefKind::Record(wit_parser::Record { fields: Vec::new() }),
            owner: TypeOwner::None,
            docs: Default::default(),
            stability: Default::default(),
            span: Default::default(),
        });
        assert_eq!(value_discriminant(&resolve, Type::Id(id)), 6);
    }

    #[test]
    fn value_discriminant_alias_follows_through() {
        let mut resolve = Resolve::default();
        let id = resolve.types.alloc(TypeDef {
            name: Some("my-s32".into()),
            kind: TypeDefKind::Type(Type::S32),
            owner: TypeOwner::None,
            docs: Default::default(),
            stability: Default::default(),
            span: Default::default(),
        });
        assert_eq!(value_discriminant(&resolve, Type::Id(id)), 1);
    }

    #[test]
    fn value_discriminant_alias_f32() {
        let mut resolve = Resolve::default();
        let id = resolve.types.alloc(TypeDef {
            name: Some("my-f32".into()),
            kind: TypeDefKind::Type(Type::F32),
            owner: TypeOwner::None,
            docs: Default::default(),
            stability: Default::default(),
            span: Default::default(),
        });
        assert_eq!(value_discriminant(&resolve, Type::Id(id)), 3);
    }

    // === type_name_str ===

    #[test]
    fn type_name_str_primitives() {
        let resolve = Resolve::default();
        assert_eq!(type_name_str(&resolve, Type::Bool), "bool");
        assert_eq!(type_name_str(&resolve, Type::U8), "u8");
        assert_eq!(type_name_str(&resolve, Type::U16), "u16");
        assert_eq!(type_name_str(&resolve, Type::U32), "u32");
        assert_eq!(type_name_str(&resolve, Type::U64), "u64");
        assert_eq!(type_name_str(&resolve, Type::S8), "s8");
        assert_eq!(type_name_str(&resolve, Type::S16), "s16");
        assert_eq!(type_name_str(&resolve, Type::S32), "s32");
        assert_eq!(type_name_str(&resolve, Type::S64), "s64");
        assert_eq!(type_name_str(&resolve, Type::F32), "f32");
        assert_eq!(type_name_str(&resolve, Type::F64), "f64");
        assert_eq!(type_name_str(&resolve, Type::Char), "char");
        assert_eq!(type_name_str(&resolve, Type::String), "string");
    }

    #[test]
    fn type_name_str_named_record() {
        let mut resolve = Resolve::default();
        let id = resolve.types.alloc(TypeDef {
            name: Some("point".into()),
            kind: TypeDefKind::Record(wit_parser::Record { fields: Vec::new() }),
            owner: TypeOwner::None,
            docs: Default::default(),
            stability: Default::default(),
            span: Default::default(),
        });
        assert_eq!(type_name_str(&resolve, Type::Id(id)), "point");
    }

    #[test]
    fn type_name_str_anonymous_record() {
        let mut resolve = Resolve::default();
        let id = resolve.types.alloc(TypeDef {
            name: None,
            kind: TypeDefKind::Record(wit_parser::Record { fields: Vec::new() }),
            owner: TypeOwner::None,
            docs: Default::default(),
            stability: Default::default(),
            span: Default::default(),
        });
        assert_eq!(type_name_str(&resolve, Type::Id(id)), "record");
    }

    #[test]
    fn type_name_str_anonymous_list() {
        let mut resolve = Resolve::default();
        let id = resolve.types.alloc(TypeDef {
            name: None,
            kind: TypeDefKind::List(Type::U8),
            owner: TypeOwner::None,
            docs: Default::default(),
            stability: Default::default(),
            span: Default::default(),
        });
        assert_eq!(type_name_str(&resolve, Type::Id(id)), "list");
    }

    // === flat_types ===

    #[test]
    fn flat_types_s32() {
        let resolve = Resolve::default();
        assert_eq!(flat_types(&resolve, Type::S32).unwrap(), vec!["i32"]);
    }

    #[test]
    fn flat_types_u64() {
        let resolve = Resolve::default();
        assert_eq!(flat_types(&resolve, Type::U64).unwrap(), vec!["i64"]);
    }

    #[test]
    fn flat_types_f32() {
        let resolve = Resolve::default();
        assert_eq!(flat_types(&resolve, Type::F32).unwrap(), vec!["f32"]);
    }

    #[test]
    fn flat_types_f64() {
        let resolve = Resolve::default();
        assert_eq!(flat_types(&resolve, Type::F64).unwrap(), vec!["f64"]);
    }

    #[test]
    fn flat_types_bool() {
        let resolve = Resolve::default();
        assert_eq!(flat_types(&resolve, Type::Bool).unwrap(), vec!["i32"]);
    }

    #[test]
    fn flat_types_string() {
        let resolve = Resolve::default();
        assert_eq!(
            flat_types(&resolve, Type::String).unwrap(),
            vec!["i32", "i32"]
        );
    }

    // === type_needs_memory ===

    #[test]
    fn type_needs_memory_primitives_false() {
        let resolve = Resolve::default();
        for ty in [
            Type::Bool,
            Type::U8,
            Type::U32,
            Type::S64,
            Type::F32,
            Type::F64,
            Type::Char,
        ] {
            assert!(
                !type_needs_memory(&resolve, &ty),
                "{ty:?} should not need memory"
            );
        }
    }

    #[test]
    fn type_needs_memory_string_true() {
        let resolve = Resolve::default();
        assert!(type_needs_memory(&resolve, &Type::String));
    }

    #[test]
    fn type_needs_memory_list_true() {
        let mut resolve = Resolve::default();
        let id = resolve.types.alloc(TypeDef {
            name: None,
            kind: TypeDefKind::List(Type::U8),
            owner: TypeOwner::None,
            docs: Default::default(),
            stability: Default::default(),
            span: Default::default(),
        });
        assert!(type_needs_memory(&resolve, &Type::Id(id)));
    }

    #[test]
    fn type_needs_memory_record_with_string_field() {
        let mut resolve = Resolve::default();
        let id = resolve.types.alloc(TypeDef {
            name: Some("greeting".into()),
            kind: TypeDefKind::Record(wit_parser::Record {
                fields: vec![wit_parser::Field {
                    name: "message".into(),
                    ty: Type::String,
                    docs: Default::default(),
                    span: Default::default(),
                }],
            }),
            owner: TypeOwner::None,
            docs: Default::default(),
            stability: Default::default(),
            span: Default::default(),
        });
        assert!(type_needs_memory(&resolve, &Type::Id(id)));
    }

    #[test]
    fn type_needs_memory_record_with_only_primitives() {
        let mut resolve = Resolve::default();
        let id = resolve.types.alloc(TypeDef {
            name: Some("point".into()),
            kind: TypeDefKind::Record(wit_parser::Record {
                fields: vec![
                    wit_parser::Field {
                        name: "x".into(),
                        ty: Type::F64,
                        docs: Default::default(),
                        span: Default::default(),
                    },
                    wit_parser::Field {
                        name: "y".into(),
                        ty: Type::F64,
                        docs: Default::default(),
                        span: Default::default(),
                    },
                ],
            }),
            owner: TypeOwner::None,
            docs: Default::default(),
            stability: Default::default(),
            span: Default::default(),
        });
        assert!(!type_needs_memory(&resolve, &Type::Id(id)));
    }

    #[test]
    fn type_needs_memory_option_of_string() {
        let mut resolve = Resolve::default();
        let id = resolve.types.alloc(TypeDef {
            name: None,
            kind: TypeDefKind::Option(Type::String),
            owner: TypeOwner::None,
            docs: Default::default(),
            stability: Default::default(),
            span: Default::default(),
        });
        assert!(type_needs_memory(&resolve, &Type::Id(id)));
    }

    #[test]
    fn type_needs_memory_option_of_u32() {
        let mut resolve = Resolve::default();
        let id = resolve.types.alloc(TypeDef {
            name: None,
            kind: TypeDefKind::Option(Type::U32),
            owner: TypeOwner::None,
            docs: Default::default(),
            stability: Default::default(),
            span: Default::default(),
        });
        assert!(!type_needs_memory(&resolve, &Type::Id(id)));
    }

    #[test]
    fn type_needs_memory_enum_false() {
        let mut resolve = Resolve::default();
        let id = resolve.types.alloc(TypeDef {
            name: Some("color".into()),
            kind: TypeDefKind::Enum(wit_parser::Enum {
                cases: vec![
                    wit_parser::EnumCase {
                        name: "red".into(),
                        docs: Default::default(),
                        span: Default::default(),
                    },
                    wit_parser::EnumCase {
                        name: "green".into(),
                        docs: Default::default(),
                        span: Default::default(),
                    },
                ],
            }),
            owner: TypeOwner::None,
            docs: Default::default(),
            stability: Default::default(),
            span: Default::default(),
        });
        assert!(!type_needs_memory(&resolve, &Type::Id(id)));
    }

    #[test]
    fn type_needs_memory_alias_follows_through() {
        let mut resolve = Resolve::default();
        let id = resolve.types.alloc(TypeDef {
            name: Some("my-string".into()),
            kind: TypeDefKind::Type(Type::String),
            owner: TypeOwner::None,
            docs: Default::default(),
            stability: Default::default(),
            span: Default::default(),
        });
        assert!(type_needs_memory(&resolve, &Type::Id(id)));
    }
}
