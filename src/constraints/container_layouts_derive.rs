//! AST-driven container-layout derivation (`saw_spec_gen-26d`,
//! `saw_spec_gen-530`).
//!
//! The user-supplied TOML surface in [`super::container_layouts`] is
//! the legacy path: callers had to hand-write libstdc++ ABI field
//! paths like `_M_dataplus._M_p`. That's user-hostile because callers
//! consume `std::string`, they do not know its internals, and field
//! names vary by STL implementation / version / flags.
//!
//! This module walks the clang AST's struct/class table (built by
//! [`crate::parsers::clang_ast::type_ctx`]) and synthesizes
//! [`ContainerLayout`] entries for every record that has the shape
//!
//! ```text
//!     struct C {
//!         T*     data;        // some pointer field
//!         size_t size;        // sibling unsigned-int field, length-named
//!         size_t capacity;    // (optional) second unsigned-int field
//!     };
//! ```
//!
//! The recognizer also handles libstdc++ `std::basic_string`'s nested
//! shape where the data pointer is wrapped in a helper struct
//! (`_M_dataplus._M_p`): when the first field is itself a struct
//! containing a pointer, we splice the inner field path into the
//! catalog entry.
//!
//! This is what feeds the downstream `llvm_points_to` lowering
//! (`saw_spec_gen-qms`) and the functional STL overrides
//! (`saw_spec_gen-i47`) without any user TOML.

use super::container_layouts::{ContainerCatalog, ContainerLayout};
use crate::constraints::TypeInfo;
use std::collections::HashMap;

/// Length-name whitelist for the *field* recognizer. Wider than the
/// parameter-side list in [`crate::constraints::struct_shape_recognizer`]
/// because container types use ABI-flavored names: `_M_string_length`
/// (libstdc++ basic_string), `__size_` (libc++), `_Mysize` (MSVC STL),
/// `_len`, etc.
const FIELD_SIZE_NAMES: &[&str] = &[
    "size",
    "len",
    "length",
    "count",
    "nelem",
    "nelems",
    "_size",
    "_len",
    "_length",
    "_count",
    "__size_",
    "__len_",
    "_m_string_length",
    "_mysize",
];

/// Capacity-field name whitelist (looked up after `data` + `size`
/// have been pinned).
const FIELD_CAP_NAMES: &[&str] = &[
    "capacity",
    "cap",
    "_capacity",
    "__cap_",
    "_mycap",
    "_m_allocated_capacity",
];

/// Build a [`ContainerCatalog`] entirely from the AST's struct table.
///
/// `structs` is `TypeContext::structs`: every record the AST walker
/// observed, keyed by short name (no template arguments). For each
/// such record we attempt to derive a layout via
/// [`derive_layout_for_struct`]. Records that don't match the shape
/// are silently skipped.
pub fn derive_catalog_from_structs(
    structs: &HashMap<String, Vec<(String, TypeInfo)>>,
) -> ContainerCatalog {
    let mut catalog = ContainerCatalog::default();
    let mut entries: Vec<(String, ContainerLayout)> = Vec::new();
    for (name, fields) in structs {
        if let Some(layout) = derive_layout_for_struct(name, fields, structs) {
            entries.push((name.clone(), layout));
        }
    }
    // Sort for deterministic catalog dumps + reproducible test output.
    entries.sort_by(|a, b| a.0.cmp(&b.0));
    for (_, layout) in entries {
        catalog.insert(layout);
    }
    catalog
}

/// Attempt to derive a single [`ContainerLayout`] for one record.
///
/// Walks the record's fields in declaration order looking for the
/// canonical `{ data, size [, capacity] }` shape, with one level of
/// nested-struct unwrapping to accommodate libstdc++ `basic_string`'s
/// `_Alloc_hider` indirection. Returns `None` when the shape doesn't
/// match.
pub fn derive_layout_for_struct(
    record_name: &str,
    fields: &[(String, TypeInfo)],
    structs: &HashMap<String, Vec<(String, TypeInfo)>>,
) -> Option<ContainerLayout> {
    let mut data_path: Option<(String, u32)> = None;
    let mut size_path: Option<String> = None;
    let mut cap_path: Option<String> = None;

    for (fname, fty) in fields {
        if data_path.is_none() {
            if let Some((path, bits)) = pointer_field_path(fname, fty, structs) {
                data_path = Some((path, bits));
                continue;
            }
        }
        if size_path.is_none() && is_size_named_uint(fname, fty) {
            size_path = Some(fname.clone());
            continue;
        }
        if size_path.is_some() && cap_path.is_none() && is_cap_named_uint(fname, fty) {
            cap_path = Some(fname.clone());
        }
    }

    let (data, bits) = data_path?;
    let size = size_path?;
    let _ = cap_path; // currently unused — published for future emitter work.
    Some(ContainerLayout {
        name: record_name.to_string(),
        data_ptr: data,
        size,
        elem_bits: bits,
    })
}

/// Resolve a field that's either a direct pointer or a nested struct
/// whose first field is a pointer (libstdc++ `_Alloc_hider` shape).
/// Returns `(field_path, elem_bits)` or `None`.
fn pointer_field_path(
    fname: &str,
    fty: &TypeInfo,
    structs: &HashMap<String, Vec<(String, TypeInfo)>>,
) -> Option<(String, u32)> {
    match fty {
        TypeInfo::Pointer(inner) => Some((fname.to_string(), pointee_bits(inner))),
        TypeInfo::Struct { name, fields, .. } => {
            // Inline the nested record's fields when the AST captured
            // them, else look the name up in the struct table.
            let inner_fields: &[(String, TypeInfo)] = if !fields.is_empty() {
                fields
            } else if let Some(found) = structs.get(name.as_str()) {
                found.as_slice()
            } else {
                return None;
            };
            // The first pointer field inside the nested struct wins —
            // basic_string's `_Alloc_hider` carries `_M_p` first.
            for (inner_name, inner_ty) in inner_fields {
                if let TypeInfo::Pointer(pointee) = inner_ty {
                    let path = format!("{fname}.{inner_name}");
                    return Some((path, pointee_bits(pointee)));
                }
            }
            None
        }
        _ => None,
    }
}

/// Pointee element width in bits used as `elem_bits` in the synthesized
/// layout. Falls back to 8 for opaque / structured pointees (the
/// emitter currently treats element width as advisory; the downstream
/// wiring under `saw_spec_gen-qms` will refine this once it consults
/// the LLVM struct table).
fn pointee_bits(ty: &TypeInfo) -> u32 {
    match ty {
        TypeInfo::SignedInt(b) | TypeInfo::UnsignedInt(b) => *b,
        TypeInfo::Bool => 1,
        TypeInfo::Float(b) => *b,
        _ => 8,
    }
}

fn is_size_named_uint(fname: &str, fty: &TypeInfo) -> bool {
    if !matches!(fty, TypeInfo::UnsignedInt(_) | TypeInfo::SignedInt(_)) {
        return false;
    }
    let lower = fname.to_lowercase();
    FIELD_SIZE_NAMES.iter().any(|n| lower == *n)
}

fn is_cap_named_uint(fname: &str, fty: &TypeInfo) -> bool {
    if !matches!(fty, TypeInfo::UnsignedInt(_) | TypeInfo::SignedInt(_)) {
        return false;
    }
    let lower = fname.to_lowercase();
    FIELD_CAP_NAMES.iter().any(|n| lower == *n)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ptr(elem: TypeInfo) -> TypeInfo {
        TypeInfo::Pointer(Box::new(elem))
    }
    fn st(name: &str, fields: Vec<(&str, TypeInfo)>) -> TypeInfo {
        TypeInfo::Struct {
            name: name.into(),
            size_bytes: None,
            fields: fields
                .into_iter()
                .map(|(n, t)| (n.to_string(), t))
                .collect(),
        }
    }
    fn fields(items: Vec<(&str, TypeInfo)>) -> Vec<(String, TypeInfo)> {
        items.into_iter().map(|(n, t)| (n.into(), t)).collect()
    }

    #[test]
    fn recognizes_flat_data_size_shape() {
        let view = fields(vec![
            ("data", ptr(TypeInfo::UnsignedInt(32))),
            ("size", TypeInfo::UnsignedInt(64)),
        ]);
        let l = derive_layout_for_struct("MyView", &view, &HashMap::new()).unwrap();
        assert_eq!(l.name, "MyView");
        assert_eq!(l.data_ptr, "data");
        assert_eq!(l.size, "size");
        assert_eq!(l.elem_bits, 32);
    }

    #[test]
    fn recognizes_data_size_capacity_shape() {
        let blob = fields(vec![
            ("data", ptr(TypeInfo::UnsignedInt(8))),
            ("size", TypeInfo::UnsignedInt(64)),
            ("capacity", TypeInfo::UnsignedInt(64)),
        ]);
        let l = derive_layout_for_struct("Blob", &blob, &HashMap::new()).unwrap();
        // The current schema doesn't surface `capacity`; the recognizer
        // still accepts the shape. Capacity plumbing is tracked under
        // saw_spec_gen-qms.
        assert_eq!(l.data_ptr, "data");
        assert_eq!(l.size, "size");
    }

    #[test]
    fn handles_libstdcpp_basic_string_nesting() {
        // _M_dataplus is a struct with a single pointer field _M_p.
        // _M_string_length sits next to it as a plain size_t.
        let alloc_hider = st(
            "_Alloc_hider",
            vec![("_M_p", ptr(TypeInfo::UnsignedInt(8)))],
        );
        let bs = fields(vec![
            ("_M_dataplus", alloc_hider),
            ("_M_string_length", TypeInfo::UnsignedInt(64)),
            (
                "_M_local_buf",
                TypeInfo::ByteArray(16), // SSO local buffer
            ),
        ]);
        let l = derive_layout_for_struct("basic_string", &bs, &HashMap::new()).unwrap();
        assert_eq!(l.data_ptr, "_M_dataplus._M_p");
        assert_eq!(l.size, "_M_string_length");
        assert_eq!(l.elem_bits, 8);
    }

    #[test]
    fn nested_struct_resolved_via_table_when_fields_empty() {
        // The AST occasionally records nested-struct fields with an
        // empty `fields` slot, expecting downstream code to look the
        // type up by name. Make sure derivation still resolves.
        let mut structs: HashMap<String, Vec<(String, TypeInfo)>> = HashMap::new();
        structs.insert(
            "_Alloc_hider".into(),
            fields(vec![("_M_p", ptr(TypeInfo::UnsignedInt(8)))]),
        );
        let opaque_nested = TypeInfo::Struct {
            name: "_Alloc_hider".into(),
            size_bytes: None,
            fields: Vec::new(),
        };
        let bs = fields(vec![
            ("_M_dataplus", opaque_nested),
            ("_M_string_length", TypeInfo::UnsignedInt(64)),
        ]);
        let l = derive_layout_for_struct("basic_string", &bs, &structs).unwrap();
        assert_eq!(l.data_ptr, "_M_dataplus._M_p");
        assert_eq!(l.size, "_M_string_length");
    }

    #[test]
    fn skips_struct_without_size_field() {
        let v = fields(vec![
            ("_M_start", ptr(TypeInfo::UnsignedInt(32))),
            ("_M_finish", ptr(TypeInfo::UnsignedInt(32))),
            ("_M_end_of_storage", ptr(TypeInfo::UnsignedInt(32))),
        ]);
        // No size-named integer field → no layout.
        assert!(derive_layout_for_struct("vector", &v, &HashMap::new()).is_none());
    }

    #[test]
    fn skips_struct_without_pointer_field() {
        let s = fields(vec![
            ("count", TypeInfo::UnsignedInt(64)),
            ("size", TypeInfo::UnsignedInt(64)),
        ]);
        assert!(derive_layout_for_struct("PairOfCounts", &s, &HashMap::new()).is_none());
    }

    #[test]
    fn skips_when_size_field_is_not_an_integer() {
        let s = fields(vec![
            ("data", ptr(TypeInfo::UnsignedInt(8))),
            ("size", TypeInfo::Float(64)), // not an int
        ]);
        assert!(derive_layout_for_struct("Weird", &s, &HashMap::new()).is_none());
    }

    #[test]
    fn ignores_size_named_field_unless_uint() {
        // length-named *pointer* field is the data; the next size_t wins.
        let s = fields(vec![
            ("count_ptr", ptr(TypeInfo::UnsignedInt(8))),
            ("size", TypeInfo::UnsignedInt(64)),
        ]);
        let l = derive_layout_for_struct("X", &s, &HashMap::new()).unwrap();
        assert_eq!(l.data_ptr, "count_ptr");
        assert_eq!(l.size, "size");
    }

    #[test]
    fn derive_catalog_is_deterministic_and_picks_only_matching_records() {
        let mut structs: HashMap<String, Vec<(String, TypeInfo)>> = HashMap::new();
        structs.insert(
            "View".into(),
            fields(vec![
                ("data", ptr(TypeInfo::UnsignedInt(32))),
                ("size", TypeInfo::UnsignedInt(64)),
            ]),
        );
        structs.insert(
            "NotAContainer".into(),
            fields(vec![("x", TypeInfo::UnsignedInt(32))]),
        );
        let cat = derive_catalog_from_structs(&structs);
        assert_eq!(cat.len(), 1);
        assert!(cat.lookup("View").is_some());
        assert!(cat.lookup("NotAContainer").is_none());
    }
}
