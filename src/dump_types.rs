//! `dump-types` subcommand: serialize per-function type information
//! and struct layouts to a single JSON document.
//!
//! This is the half-step that lets external tooling (notably
//! `pretty-specs`) consume the same `TypeInfo` view that the SAW
//! constraint pipeline uses, without having to re-parse the underlying
//! clang AST / MIR JSON / LLVM IR.
//!
//! The wire format intentionally mirrors but does not re-export the
//! internal `constraints::types` enum hierarchy. Keeping a thin wire
//! struct insulates downstream consumers from refactors to the
//! constraint-derivation data model and lets us version the document
//! independently via `schema_version`.
//!
//! Input sources (any combination, at least one required):
//!   * `--ast <FILE>`       — clang AST JSON (`clang -Xclang -ast-dump=json`)
//!   * `--mir <FILE>`       — mir-json output (`linked-mir.json`)
//!   * `--llvm-ir <FILE>`   — LLVM textual IR (`.ll`)
//!
//! When multiple sources are supplied, function entries are merged by
//! `name`; later sources override earlier ones, so the recommended
//! ordering is `--ast --mir --llvm-ir` (broadest → most precise).
//! Struct layouts are unioned in the same way.

use std::collections::BTreeMap;
use std::path::Path;

use anyhow::{anyhow, Context, Result};
use serde::Serialize;

use crate::constraints::types::{
    Annotation, FunctionInfo, Mutability, Nullability, ParamInfo, TypeInfo,
};
use crate::{clang_ast, llvm_ir, mir_json};

/// Stable wire schema version emitted in the `schema_version` field of
/// the dump.  Bump whenever a field is renamed or its semantics change.
pub const TYPES_SCHEMA_VERSION: &str = "1";

/// Top-level document written to `--output`.
#[derive(Debug, Serialize)]
pub struct TypesDocument {
    pub schema_version: String,
    pub generator: String,
    /// Functions keyed by unmangled name (matches the `Function.name`
    /// field that `pretty-specs` uses for cross-referencing Cryptol
    /// items).
    pub functions: BTreeMap<String, WireFunction>,
    /// Structs keyed by source-level type name (no leading `struct.`
    /// or `class.` prefix).
    pub structs: BTreeMap<String, WireStruct>,
}

#[derive(Debug, Serialize)]
pub struct WireFunction {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mangled_name: Option<String>,
    pub params: Vec<WireParam>,
    pub return_type: WireType,
    pub can_throw: bool,
    pub is_virtual: bool,
    pub has_body: bool,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub annotations: Vec<String>,
    /// Source tag — which parser produced this entry. Helps downstream
    /// tools decide which fields are trustworthy when sources disagree.
    pub source: &'static str,
}

#[derive(Debug, Serialize)]
pub struct WireParam {
    pub name: String,
    pub r#type: WireType,
    pub mutability: &'static str,
    pub nullability: &'static str,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub annotations: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct WireStruct {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size_bytes: Option<usize>,
    pub fields: Vec<WireField>,
}

#[derive(Debug, Serialize)]
pub struct WireField {
    pub name: String,
    pub r#type: WireType,
}

/// Tagged wire encoding of [`TypeInfo`].  We deliberately use an
/// internally tagged enum so JSON consumers can dispatch on `kind`.
#[derive(Debug, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum WireType {
    SignedInt {
        bits: u32,
    },
    UnsignedInt {
        bits: u32,
    },
    Float {
        bits: u32,
    },
    Bool,
    ByteArray {
        len: usize,
    },
    Pointer {
        pointee: Box<WireType>,
    },
    Struct {
        name: String,
    },
    Enum {
        name: String,
        discriminant_bits: u32,
    },
    Option {
        inner: Box<WireType>,
    },
    Result {
        ok: Box<WireType>,
        err: Box<WireType>,
    },
    Opaque {
        name: String,
        size_bytes: usize,
    },
    Void,
}

impl From<&TypeInfo> for WireType {
    fn from(t: &TypeInfo) -> Self {
        match t {
            TypeInfo::SignedInt(b) => WireType::SignedInt { bits: *b },
            TypeInfo::UnsignedInt(b) => WireType::UnsignedInt { bits: *b },
            TypeInfo::Float(b) => WireType::Float { bits: *b },
            TypeInfo::Bool => WireType::Bool,
            TypeInfo::ByteArray(n) => WireType::ByteArray { len: *n },
            TypeInfo::Pointer(inner) => WireType::Pointer {
                pointee: Box::new(WireType::from(inner.as_ref())),
            },
            TypeInfo::Struct { name, .. } => WireType::Struct { name: name.clone() },
            TypeInfo::Enum {
                name,
                discriminant_bits,
                ..
            } => WireType::Enum {
                name: name.clone(),
                discriminant_bits: *discriminant_bits,
            },
            TypeInfo::Option(inner) => WireType::Option {
                inner: Box::new(WireType::from(inner.as_ref())),
            },
            TypeInfo::Result(ok, err) => WireType::Result {
                ok: Box::new(WireType::from(ok.as_ref())),
                err: Box::new(WireType::from(err.as_ref())),
            },
            TypeInfo::Opaque { name, size_bytes } => WireType::Opaque {
                name: name.clone(),
                size_bytes: *size_bytes,
            },
            TypeInfo::Void => WireType::Void,
        }
    }
}

fn mutability_str(m: &Mutability) -> &'static str {
    match m {
        Mutability::Readonly => "readonly",
        Mutability::Mutable => "mutable",
        Mutability::WriteOnly => "write_only",
    }
}

fn nullability_str(n: &Nullability) -> &'static str {
    match n {
        Nullability::NonNull => "nonnull",
        Nullability::Nullable => "nullable",
    }
}

fn annotation_str(a: &Annotation) -> String {
    match a {
        Annotation::InReads(n) => format!("in_reads({n})"),
        Annotation::OutWrites(n) => format!("out_writes({n})"),
        Annotation::InReadsParam(p) => format!("in_reads({p})"),
        Annotation::OutWritesParam(p) => format!("out_writes({p})"),
        Annotation::InZ(n) => format!("in_z({n})"),
        Annotation::Inout => "inout".to_string(),
        Annotation::PreValid => "pre_valid".to_string(),
        Annotation::PostInvalid => "post_invalid".to_string(),
        Annotation::MustInspectResult => "must_inspect_result".to_string(),
        Annotation::NoAlias => "noalias".to_string(),
        Annotation::NoCapture => "nocapture".to_string(),
        Annotation::Dereferenceable(n) => format!("dereferenceable({n})"),
        Annotation::NoThrow => "nothrow".to_string(),
        Annotation::Custom(s) => format!("custom({s})"),
    }
}

fn wire_param(p: &ParamInfo) -> WireParam {
    WireParam {
        name: p.name.clone(),
        r#type: WireType::from(&p.ty),
        mutability: mutability_str(&p.mutability),
        nullability: nullability_str(&p.nullable),
        annotations: p.annotations.iter().map(annotation_str).collect(),
    }
}

fn wire_function(f: &FunctionInfo, source: &'static str) -> WireFunction {
    WireFunction {
        name: f.name.clone(),
        mangled_name: f.mangled_name.clone(),
        params: f.params.iter().map(wire_param).collect(),
        return_type: WireType::from(&f.return_type),
        can_throw: f.can_throw,
        is_virtual: f.is_virtual,
        has_body: f.has_body,
        annotations: f.annotations.iter().map(annotation_str).collect(),
        source,
    }
}

/// Walk a [`TypeInfo`] tree and record every `Struct { fields }` node
/// we encounter into `structs`.  Idempotent — later visits with the
/// same name overwrite earlier ones (with-fields wins over without).
fn collect_struct_layouts(t: &TypeInfo, structs: &mut BTreeMap<String, WireStruct>) {
    match t {
        TypeInfo::Struct {
            name,
            size_bytes,
            fields,
        } => {
            let entry = WireStruct {
                name: name.clone(),
                size_bytes: *size_bytes,
                fields: fields
                    .iter()
                    .map(|(fname, fty)| WireField {
                        name: fname.clone(),
                        r#type: WireType::from(fty),
                    })
                    .collect(),
            };
            // Prefer entries with fields over field-less repeats.
            let should_insert = match structs.get(name) {
                Some(existing) => existing.fields.is_empty() && !entry.fields.is_empty(),
                None => true,
            };
            if should_insert {
                structs.insert(name.clone(), entry);
            }
            for (_, fty) in fields {
                collect_struct_layouts(fty, structs);
            }
        }
        TypeInfo::Pointer(inner) | TypeInfo::Option(inner) => {
            collect_struct_layouts(inner, structs);
        }
        TypeInfo::Result(a, b) => {
            collect_struct_layouts(a, structs);
            collect_struct_layouts(b, structs);
        }
        _ => {}
    }
}

fn ingest_functions(fns: &[FunctionInfo], source: &'static str, doc: &mut TypesDocument) {
    for f in fns {
        // Collect struct layouts from every param + return type.
        for p in &f.params {
            collect_struct_layouts(&p.ty, &mut doc.structs);
        }
        collect_struct_layouts(&f.return_type, &mut doc.structs);

        // Later sources override earlier ones.
        doc.functions
            .insert(f.name.clone(), wire_function(f, source));
    }
}

/// Run the `dump-types` subcommand.
///
/// At least one of `ast`, `mir`, `llvm_ir` must be `Some`.  `filter`
/// is the same case-sensitive substring filter accepted by
/// `gen-verify` and applied to each parser independently.
pub fn run(
    ast: Option<&Path>,
    mir: Option<&Path>,
    llvm_ir_path: Option<&Path>,
    output: &Path,
    filter: Option<&str>,
) -> Result<()> {
    if ast.is_none() && mir.is_none() && llvm_ir_path.is_none() {
        return Err(anyhow!(
            "dump-types requires at least one of --ast, --mir, or --llvm-ir",
        ));
    }

    let mut doc = TypesDocument {
        schema_version: TYPES_SCHEMA_VERSION.to_string(),
        generator: "saw-spec-gen dump-types".to_string(),
        functions: BTreeMap::new(),
        structs: BTreeMap::new(),
    };

    // Ordering: ast → mir → llvm_ir.  Later passes refine earlier
    // ones (the LLVM IR knows real byte sizes; the AST knows source
    // names).
    if let Some(path) = ast {
        eprintln!("dump-types: parsing clang AST {}", path.display());
        let node = clang_ast::parse_ast(path)
            .with_context(|| format!("failed to parse clang AST: {}", path.display()))?;
        let fns = clang_ast::extract_functions(&node, filter)
            .with_context(|| format!("failed to extract functions from {}", path.display()))?;
        ingest_functions(&fns, "clang_ast", &mut doc);
    }

    if let Some(path) = mir {
        eprintln!("dump-types: parsing MIR JSON {}", path.display());
        let mir_doc = mir_json::parse_mir(path)
            .with_context(|| format!("failed to parse MIR JSON: {}", path.display()))?;
        let fns = mir_json::extract_functions(&mir_doc, filter)
            .with_context(|| format!("failed to extract functions from {}", path.display()))?;
        ingest_functions(&fns, "mir_json", &mut doc);
    }

    if let Some(path) = llvm_ir_path {
        eprintln!("dump-types: parsing LLVM IR {}", path.display());
        let ir = llvm_ir::parse_llvm_ir(path)
            .with_context(|| format!("failed to parse LLVM IR: {}", path.display()))?;
        let fns = llvm_ir::extract_functions(&ir, filter)
            .with_context(|| format!("failed to extract functions from {}", path.display()))?;
        ingest_functions(&fns, "llvm_ir", &mut doc);
    }

    let json = serde_json::to_string_pretty(&doc).context("failed to serialize types document")?;
    std::fs::write(output, json)
        .with_context(|| format!("failed to write {}", output.display()))?;
    eprintln!(
        "Wrote {} function(s), {} struct(s) to {}",
        doc.functions.len(),
        doc.structs.len(),
        output.display(),
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::constraints::types::{FunctionInfo, Mutability, Nullability, ParamInfo, TypeInfo};

    fn dummy_fn() -> FunctionInfo {
        FunctionInfo {
            name: "add_one".to_string(),
            mangled_name: Some("_Z7add_onei".to_string()),
            params: vec![ParamInfo {
                name: "x".to_string(),
                ty: TypeInfo::SignedInt(32),
                mutability: Mutability::Readonly,
                nullable: Nullability::NonNull,
                annotations: vec![Annotation::NoCapture],
            }],
            return_type: TypeInfo::SignedInt(32),
            can_throw: false,
            is_virtual: false,
            has_body: true,
            is_system: false,
            annotations: vec![],
            referenced_globals: vec![],
            called_functions: vec![],
        }
    }

    #[test]
    fn serializes_primitive_function() {
        let mut doc = TypesDocument {
            schema_version: TYPES_SCHEMA_VERSION.to_string(),
            generator: "test".to_string(),
            functions: BTreeMap::new(),
            structs: BTreeMap::new(),
        };
        ingest_functions(&[dummy_fn()], "clang_ast", &mut doc);
        let json = serde_json::to_string(&doc).unwrap();
        assert!(json.contains("\"add_one\""));
        assert!(json.contains("\"signed_int\""));
        assert!(json.contains("\"bits\":32"));
        assert!(json.contains("\"source\":\"clang_ast\""));
        assert!(json.contains("\"nocapture\""));
    }

    #[test]
    fn collects_struct_layouts_recursively() {
        let inner = TypeInfo::Struct {
            name: "Inner".to_string(),
            size_bytes: Some(4),
            fields: vec![("a".to_string(), TypeInfo::SignedInt(32))],
        };
        let outer = TypeInfo::Struct {
            name: "Outer".to_string(),
            size_bytes: Some(8),
            fields: vec![
                ("hdr".to_string(), TypeInfo::UnsignedInt(32)),
                ("body".to_string(), inner),
            ],
        };

        let mut structs = BTreeMap::new();
        collect_struct_layouts(&outer, &mut structs);
        assert!(structs.contains_key("Inner"));
        assert!(structs.contains_key("Outer"));
        assert_eq!(structs["Outer"].fields.len(), 2);
        assert_eq!(structs["Inner"].fields.len(), 1);
    }

    #[test]
    fn rejects_when_no_sources_supplied() {
        let tmp = std::env::temp_dir().join("saw-spec-gen-dump-types-empty.json");
        let res = run(None, None, None, &tmp, None);
        assert!(res.is_err());
        assert!(res
            .unwrap_err()
            .to_string()
            .contains("at least one of --ast"),);
    }

    #[test]
    fn fields_only_struct_overrides_empty_repeat() {
        // First an empty layout, then a populated one — the populated
        // one should win even though we visit empty first.
        let empty = TypeInfo::Struct {
            name: "S".to_string(),
            size_bytes: None,
            fields: vec![],
        };
        let populated = TypeInfo::Struct {
            name: "S".to_string(),
            size_bytes: Some(4),
            fields: vec![("x".to_string(), TypeInfo::SignedInt(32))],
        };
        let mut structs = BTreeMap::new();
        collect_struct_layouts(&empty, &mut structs);
        collect_struct_layouts(&populated, &mut structs);
        assert_eq!(structs["S"].fields.len(), 1);
        assert_eq!(structs["S"].size_bytes, Some(4));

        // Reverse order should also leave populated in place.
        let mut structs2 = BTreeMap::new();
        collect_struct_layouts(&populated, &mut structs2);
        collect_struct_layouts(&empty, &mut structs2);
        assert_eq!(structs2["S"].fields.len(), 1);
    }
}
