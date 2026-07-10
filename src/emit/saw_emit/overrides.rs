//! Override-index emission (`interface_overrides.saw`) and per-class
//! `alloc_<class>_this` helpers plus the `FieldKind`-aware container
//! `this` allocator used by the verification script.

use super::names::{sanitize_name, stub_function_name};
use crate::clang_ast::{ClassConstructor, InterfaceMethod};
use crate::constraints::TypeInfo;
use std::collections::{BTreeMap, HashSet};

/// Description of a single `this`-class field for [`emit_container_this`].
pub enum FieldKind {
    /// Pointer to a known interface — allocate via `alloc_<iface>_this`.
    Interface(String, String), // (field_name, interface_class_name)
    /// Anything else — allocate a fresh 8-byte slot so reads at this
    /// offset return a deterministic value rather than tripping
    /// "outside of the allocation".
    Other(String), // field_name
}

/// Generate the index file with vtable-aware override setup.
///
/// Produces:
/// - `include` statements for all per-method havoc spec files.
/// - `llvm_unsafe_assume_spec` for each stub function.
/// - Per-class `alloc_<class>_this` helper.
/// - One constructor override per [`ClassConstructor`].
pub fn generate_override_index_with_vtable(
    methods: &[InterfaceMethod],
    by_class: &BTreeMap<String, Vec<&InterfaceMethod>>,
    constructors: &[ClassConstructor],
    classes_with_vdtor: &HashSet<String>,
) -> String {
    let mut out = String::new();
    emit_header(&mut out);
    emit_includes(&mut out, methods);
    let all_overrides = emit_override_bindings(&mut out, methods);
    emit_per_class_helpers(&mut out, by_class, classes_with_vdtor);
    emit_override_list_comment(&mut out, &all_overrides);
    let mut all_overrides = all_overrides;
    if !constructors.is_empty() {
        emit_ctor_overrides(
            &mut out,
            constructors,
            by_class,
            classes_with_vdtor,
            &mut all_overrides,
        );
    }
    out
}

fn emit_header(out: &mut String) {
    out.push_str("// Auto-generated interface override setup with vtable resolution\n");
    out.push_str("//\n");
    out.push_str("// For DIRECT calls (function is 'declare' in bitcode):\n");
    out.push_str("//   Just use llvm_unsafe_assume_spec with the mangled name directly.\n");
    out.push_str("//   No extra files needed — the havoc specs bind to the declared symbol.\n");
    out.push_str("//\n");
    out.push_str("// For VTABLE INDIRECT calls (virtual dispatch through this->vptr):\n");
    out.push_str("//   m_stubs <- llvm_load_module \"vtable_stubs.ll\";\n");
    out.push_str("//   m       <- llvm_combine_modules m_main m_stubs;\n");
    out.push_str("//   The vtable global provides the function pointer resolution chain:\n");
    out.push_str("//     this->vptr -> vtable[slot] -> stub function -> havoc spec\n");
    out.push_str("//\n");
    out.push_str("// Usage:\n");
    out.push_str("//   m_main  <- llvm_load_module \"your_code.bc\";\n");
    out.push_str("//   m_stubs <- llvm_load_module \"vtable_stubs.ll\";\n");
    out.push_str("//   m       <- llvm_combine_modules m_main m_stubs;\n");
    out.push_str("//   include \"interface_overrides.saw\";\n");
    out.push_str("//\n");
    out.push_str("// For each interface class, a helper is generated:\n");
    out.push_str("//   alloc_<class>_this : LLVMSetup Term\n");
    out.push_str("// which allocates a properly-formed `this` pointer with vptr wired\n");
    out.push_str("// to the stub vtable. Use it in your verification specs.\n\n");
}

fn emit_includes(out: &mut String, methods: &[InterfaceMethod]) {
    for method in methods {
        if method.is_override {
            continue;
        }
        let filename = format!(
            "{}_{}_havoc_spec.saw",
            sanitize_name(&method.class_name),
            sanitize_name(&method.method.name),
        );
        out.push_str(&format!("include \"{filename}\";\n"));
    }
    out.push('\n');
}

fn emit_override_bindings(out: &mut String, methods: &[InterfaceMethod]) -> Vec<String> {
    let mut all_overrides = Vec::new();
    out.push_str("// --- Override bindings ---\n");
    out.push_str("// For vtable dispatch: binds to stub function names (from vtable_stubs.ll)\n");
    out.push_str(
        "// For direct calls: binds to the mangled C++ name (already in your bitcode)\n\n",
    );
    for method in methods {
        if method.is_override {
            continue;
        }
        let stub_name = stub_function_name(method);
        let safe_name = sanitize_name(&stub_name);
        let mangled = method.method.mangled_name.as_deref();
        out.push_str(&format!(
            "// {}::{} — havoc: any behavior consistent with type constraints\n",
            method.class_name, method.method.name,
        ));
        out.push_str(&format!(
            "ov_{safe_name} <- llvm_unsafe_assume_spec m \"{stub_name}\" {safe_name}_havoc;\n",
        ));
        all_overrides.push(format!("ov_{safe_name}"));
        if let Some(mangled_name) = mangled {
            let safe_mangled = sanitize_name(mangled_name);
            if method.is_pure {
                out.push_str("// Pure virtual — no real symbol in bitcode.\n");
            } else {
                out.push_str("// Devirtualized direct calls hit the real symbol — no override.\n");
            }
            out.push_str("// To force havoc instead, uncomment:\n");
            out.push_str(&format!(
                "// ov_{safe_mangled} <- llvm_unsafe_assume_spec m \"{mangled_name}\" {safe_name}_havoc;\n",
            ));
        }
        out.push('\n');
    }
    all_overrides
}

fn emit_per_class_helpers(
    out: &mut String,
    by_class: &BTreeMap<String, Vec<&InterfaceMethod>>,
    classes_with_vdtor: &HashSet<String>,
) {
    for (class_name, class_methods) in by_class {
        let safe_class = sanitize_name(class_name).to_lowercase();

        out.push_str("// ===========================================================\n");
        out.push_str(&format!("// {class_name}: vtable setup helper\n"));
        out.push_str("// ===========================================================\n");
        out.push_str("//\n");
        out.push_str("// Allocates a `this` pointer (8 bytes for vptr) with the vtable\n");
        out.push_str(&format!(
            "// pointer wired to {safe_class}_vtable. SAW resolves:\n"
        ));
        out.push_str(&format!(
            "//   this -> vptr -> {safe_class}_vtable[slot] -> stub -> havoc spec\n"
        ));
        out.push_str("//\n");
        out.push_str("// Use in your spec like:\n");
        out.push_str(&format!("//   this_ptr <- alloc_{safe_class}_this;\n"));

        let class_ov_names: Vec<String> = class_methods
            .iter()
            .map(|m| format!("ov_{}", sanitize_name(&stub_function_name(m))))
            .collect();
        if !class_ov_names.is_empty() {
            out.push_str(&format!(
                "//   llvm_verify m \"target\" [{}] false spec z3;\n",
                class_ov_names.join(", "),
            ));
        }
        out.push('\n');

        let dtor_slots = if classes_with_vdtor.contains(class_name.as_str()) {
            1
        } else {
            0
        };
        let slot_count = class_methods.len() + dtor_slots;

        out.push_str(&format!(
            "let alloc_{safe_class}_this : LLVMSetup SetupValue = do {{\n"
        ));
        out.push_str(&format!(
            "    // Allocate the vtable: {slot_count} function pointers\n"
        ));
        out.push_str(&format!(
            "    let vtable_ty = llvm_array {slot_count} (llvm_int 64);\n"
        ));
        out.push_str("    vtable_ptr <- llvm_alloc_readonly_aligned 8 vtable_ty;\n");
        out.push_str("\n    // Point vtable slots to the global stub vtable\n");
        out.push_str("    llvm_points_to_at_type vtable_ptr vtable_ty\n");
        out.push_str(&format!(
            "        (llvm_global_initializer \"{safe_class}_vtable\");\n"
        ));
        out.push_str("\n    // Allocate `this`: first 8 bytes = vptr -> vtable\n");
        out.push_str("    this_ptr <- llvm_alloc (llvm_int 64);\n");
        out.push_str("    llvm_points_to this_ptr vtable_ptr;\n");
        out.push_str("\n    return this_ptr;\n");
        out.push_str("};\n\n");
    }
}

fn emit_override_list_comment(out: &mut String, all_overrides: &[String]) {
    out.push_str("// All overrides for use in llvm_verify:\n");
    out.push_str(&format!(
        "// let all_interface_overrides = [{}];\n",
        all_overrides.join(", "),
    ));
}

fn emit_ctor_overrides(
    out: &mut String,
    constructors: &[ClassConstructor],
    by_class: &BTreeMap<String, Vec<&InterfaceMethod>>,
    classes_with_vdtor: &HashSet<String>,
    all_overrides: &mut Vec<String>,
) {
    out.push_str("\n// ===========================================================\n");
    out.push_str("// Constructor overrides for vtable wiring\n");
    out.push_str("// ===========================================================\n");
    out.push_str("//\n");
    out.push_str("// These override real constructors to wire vptr to stub vtables.\n");
    out.push_str("// Use when verifying functions that construct objects internally\n");
    out.push_str("// (e.g. `new OkLog()`) rather than receiving them as parameters.\n\n");

    for ctor in constructors {
        let safe_class = sanitize_name(&ctor.class_name).to_lowercase();
        if !by_class.contains_key(&ctor.class_name) {
            continue;
        }
        let slot_count = by_class
            .get(&ctor.class_name)
            .map(|ms| {
                let dtor_slots = if classes_with_vdtor.contains(&ctor.class_name) {
                    1
                } else {
                    0
                };
                ms.len() + dtor_slots
            })
            .unwrap_or(1);

        out.push_str(&format!(
            "// Constructor override for {}: wires vptr to {}_vtable\n",
            ctor.class_name, safe_class,
        ));
        out.push_str(&format!(
            "let {safe_class}_ctor_override : LLVMSetup () = do {{\n"
        ));

        let has_data_members = !ctor.layout_fields.is_empty();
        if has_data_members {
            // Use an anonymous packed struct type rather than
            // `llvm_alias "class.X"`. The latter relies on the named
            // type surviving in the post-`opt` bitcode, which it
            // doesn't once -O1 inlines / strips type metadata.
            let pad_bytes = ctor_trailing_padding(&ctor.layout_fields);
            out.push_str("\n    // this: full class layout (vptr + data members)\n");
            out.push_str("    this_ptr <- llvm_alloc (llvm_packed_struct_type\n");
            out.push_str("        [ llvm_pointer (llvm_int 64)  // vptr\n");
            for (_, fty, _) in &ctor.layout_fields {
                let width = ctor_field_width(fty);
                out.push_str(&format!("        , llvm_int {width}\n"));
            }
            if pad_bytes > 0 {
                out.push_str(&format!("        , llvm_array {pad_bytes} (llvm_int 8)\n",));
            }
            out.push_str("        ]);\n");
        } else {
            out.push_str("\n    // this: passed in from operator new (precondition)\n");
            out.push_str("    this_ptr <- llvm_alloc_aligned 8 (llvm_int 64);\n");
        }
        out.push_str("\n    llvm_execute_func [this_ptr];\n");
        out.push_str("\n    // Postcondition: allocate stub vtable and wire vptr\n");
        out.push_str(&format!(
            "    let vtable_ty = llvm_array {slot_count} (llvm_int 64);\n"
        ));
        out.push_str("    vtable_ptr <- llvm_alloc_readonly_aligned 8 vtable_ty;\n");
        out.push_str("    llvm_points_to_at_type vtable_ptr vtable_ty\n");
        out.push_str(&format!(
            "        (llvm_global_initializer \"{safe_class}_vtable\");\n",
        ));
        if has_data_members {
            let pad_bytes = ctor_trailing_padding(&ctor.layout_fields);
            out.push_str("    // Initialize vptr + all data members\n");
            out.push_str("    llvm_points_to this_ptr (llvm_packed_struct_value\n");
            out.push_str("        [ vtable_ptr\n");
            for (fname, fty, default_lit) in &ctor.layout_fields {
                let width = ctor_field_width(fty);
                let lit = if default_lit.is_empty() {
                    "0"
                } else {
                    default_lit.as_str()
                };
                out.push_str(&format!(
                    "        , llvm_term {{{{ {lit} : [{width}] }}}}  // {fname}\n",
                ));
            }
            if pad_bytes > 0 {
                // Padding slot is `[N x i8]` in the struct type, so the
                // value must be a Cryptol array literal of N bytes.
                let zeros: Vec<&str> = (0..pad_bytes).map(|_| "0").collect();
                out.push_str(&format!(
                    "        , llvm_term {{{{ [{}] : [{pad_bytes}][8] }}}}  // padding\n",
                    zeros.join(", "),
                ));
            }
            out.push_str("        ]);\n");
        } else {
            out.push_str("    llvm_points_to this_ptr vtable_ptr;\n");
        }
        // MSVC constructors return `ptr` (the `this` argument); Itanium
        // constructors return void. Selecting by mangling prefix:
        // `_Z…` → Itanium, anything else (MSVC `??…`) → MSVC.
        let returns_this = !ctor.mangled_name.starts_with("_Z");
        if returns_this {
            out.push_str("    llvm_return this_ptr;\n");
        }
        out.push_str("};\n");
        out.push_str(&format!(
            "ov_{safe_class}_ctor <- llvm_unsafe_assume_spec m \"{}\" {safe_class}_ctor_override;\n\n",
            ctor.mangled_name,
        ));
        all_overrides.push(format!("ov_{safe_class}_ctor"));
    }
}

/// A container class is a non-interface class whose own fields include
/// at least one pointer to an interface in `interface_classes`. Recognises
/// both raw pointers and the common smart-pointer wrappers
/// (`std::unique_ptr<IFoo>` / `std::shared_ptr<IFoo>`).
pub fn container_layout_for(
    ty: &TypeInfo,
    interface_classes: &HashSet<String>,
) -> Option<Vec<FieldKind>> {
    let TypeInfo::Pointer(inner) = ty else {
        return None;
    };
    let TypeInfo::Struct { name, fields, .. } = inner.as_ref() else {
        return None;
    };
    if fields.is_empty() || interface_classes.contains(name) {
        return None;
    }
    let kinds: Vec<FieldKind> = fields
        .iter()
        .map(
            |(fname, fty)| match field_interface_name(fty, interface_classes) {
                Some(iface) => FieldKind::Interface(fname.clone(), iface),
                None => FieldKind::Other(fname.clone()),
            },
        )
        .collect();
    if kinds.iter().any(|k| matches!(k, FieldKind::Interface(..))) {
        Some(kinds)
    } else {
        None
    }
}

/// Extract the interface class name from a field type. Recognises raw
/// pointers and the standard smart-pointer wrappers whose templated
/// type name embeds the pointee.
pub fn field_interface_name(ty: &TypeInfo, interface_classes: &HashSet<String>) -> Option<String> {
    if let TypeInfo::Pointer(inner) = ty {
        let pointee = match inner.as_ref() {
            TypeInfo::Struct { name, .. } | TypeInfo::Opaque { name, .. } => Some(name.as_str()),
            _ => None,
        };
        if let Some(n) = pointee {
            if interface_classes.contains(n) {
                return Some(n.to_string());
            }
        }
    }
    let wrapped = match ty {
        TypeInfo::Struct { name, .. } | TypeInfo::Opaque { name, .. } => Some(name.as_str()),
        _ => None,
    };
    if let Some(name) = wrapped {
        for wrap in [
            "std::unique_ptr<",
            "unique_ptr<",
            "std::shared_ptr<",
            "shared_ptr<",
        ] {
            if let Some(rest) = name.strip_prefix(wrap) {
                if let Some(end) = rest.find('>') {
                    let inner = rest[..end].trim().trim_end_matches('*').trim();
                    let simple = inner.split_whitespace().next().unwrap_or(inner);
                    if interface_classes.contains(simple) {
                        return Some(simple.to_string());
                    }
                }
            }
        }
    }
    None
}

/// Emit a SAW block that allocates `<param>_ptr` as a packed struct
/// whose interface-typed slots point to objects wired to their stub
/// vtables (matching the layout the bitcode reads via `getelementptr`).
pub fn emit_container_this(out: &mut String, param_name: &str, fields: &[FieldKind]) {
    out.push_str(&format!(
        "    // {param_name}: container class with interface fields — wire each vptr.\n",
    ));
    let mut slot_values: Vec<String> = Vec::with_capacity(fields.len());
    for (i, kind) in fields.iter().enumerate() {
        let slot_name = format!("{param_name}_f{i}");
        match kind {
            FieldKind::Interface(fname, iface) => {
                let safe_iface = sanitize_name(iface).to_lowercase();
                out.push_str(&format!(
                    "    {slot_name} <- alloc_{safe_iface}_this;  // {fname} : {iface}*\n",
                ));
                slot_values.push(slot_name);
            }
            FieldKind::Other(fname) => {
                out.push_str(&format!(
                    "    {slot_name} <- llvm_alloc (llvm_int 64);  // {fname} (opaque slot)\n",
                ));
                slot_values.push(slot_name);
            }
        }
    }
    let n = fields.len();
    out.push_str(&format!(
        "    {param_name}_ptr <- llvm_alloc (llvm_array {n} (llvm_int 64));\n",
    ));
    out.push_str(&format!(
        "    llvm_points_to {param_name}_ptr (llvm_packed_struct_value [{}]);\n",
        slot_values.join(", "),
    ));
}

/// Bit width for a class data-member field as emitted into a packed
/// struct value. Mirrors the width logic used in `havoc_params`.
fn ctor_field_width(fty: &TypeInfo) -> u32 {
    match fty {
        TypeInfo::SignedInt(w) | TypeInfo::UnsignedInt(w) => *w,
        TypeInfo::Bool => 1,
        _ => 64,
    }
}

/// Compute trailing padding (bytes) for a polymorphic class layout so
/// the packed struct (vptr + members) rounds up to 8-byte alignment,
/// matching clang's x86_64 ABI.
fn ctor_trailing_padding(fields: &[(String, TypeInfo, String)]) -> u32 {
    let mut bits = 64u32; // vptr
    for (_, fty, _) in fields {
        let w = ctor_field_width(fty);
        bits += w.max(8).next_multiple_of(8);
    }
    let bytes = bits.div_ceil(8);
    let aligned = bytes.next_multiple_of(8);
    aligned - bytes
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::constraints::TypeInfo;

    fn ifaces(names: &[&str]) -> HashSet<String> {
        names.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn field_interface_name_recognizes_raw_pointer() {
        let known = ifaces(&["IFoo"]);
        let ty = TypeInfo::Pointer(Box::new(TypeInfo::Opaque {
            name: "IFoo".into(),
            size_bytes: 0,
        }));
        assert_eq!(field_interface_name(&ty, &known), Some("IFoo".into()));
    }

    #[test]
    fn field_interface_name_recognizes_unique_ptr() {
        let known = ifaces(&["IBar"]);
        let ty = TypeInfo::Struct {
            name: "std::unique_ptr<IBar>".into(),
            size_bytes: Some(8),
            fields: vec![],
        };
        assert_eq!(field_interface_name(&ty, &known), Some("IBar".into()));
    }

    #[test]
    fn field_interface_name_ignores_unknown() {
        let known = ifaces(&["IFoo"]);
        let ty = TypeInfo::Pointer(Box::new(TypeInfo::Opaque {
            name: "INotKnown".into(),
            size_bytes: 0,
        }));
        assert_eq!(field_interface_name(&ty, &known), None);
    }

    #[test]
    fn container_layout_returns_none_for_pure_interface() {
        let known = ifaces(&["IService"]);
        let ty = TypeInfo::Pointer(Box::new(TypeInfo::Struct {
            name: "IService".into(),
            size_bytes: None,
            fields: vec![],
        }));
        assert!(container_layout_for(&ty, &known).is_none());
    }

    #[test]
    fn container_layout_picks_up_interface_field() {
        let known = ifaces(&["IFoo"]);
        let ty = TypeInfo::Pointer(Box::new(TypeInfo::Struct {
            name: "Holder".into(),
            size_bytes: Some(16),
            fields: vec![(
                "ptr".into(),
                TypeInfo::Pointer(Box::new(TypeInfo::Opaque {
                    name: "IFoo".into(),
                    size_bytes: 0,
                })),
            )],
        }));
        let kinds = container_layout_for(&ty, &known).expect("should pick up Holder");
        assert_eq!(kinds.len(), 1);
        assert!(matches!(&kinds[0], FieldKind::Interface(name, iface)
            if name == "ptr" && iface == "IFoo"));
    }
}
