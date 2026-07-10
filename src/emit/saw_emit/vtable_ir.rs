//! LLVM IR text generation for vtable stubs.
//!
//! Extracted from `stubs.rs` — contains the `generate_llvm_ir_stubs`
//! entry point and its private helpers (`TargetAbi`, Itanium mangling,
//! originating-class resolution, per-method stub emission).

use super::names::sanitize_name;
use super::types::{
    ir_default_return, method_param_ir_pieces, sret_inner_ir_type, type_to_llvm_ir,
};
use crate::clang_ast::InterfaceMethod;
use std::collections::{BTreeMap, HashMap, HashSet};

/// Generate the LLVM IR text (`vtable_stubs.ll`) with stub functions and
/// concrete vtable globals for every class in `by_class`.
///
/// Always emits:
///   * one stub function per originating virtual method,
///   * an `@<class>_vtable = global [N x ptr] [...]` (used by the
///     per-class `alloc_<class>_this` SAW helper).
///
/// On Itanium-ABI targets, additionally emits:
///   * `@_ZTV<mangled> = linkonce_odr unnamed_addr constant
///     { [<N+2> x ptr] } { [<N+2> x ptr] [ptr null, ptr null, stub1,
///     stub2, ...] }, align 8` — a layout-matching override for the
///     compiler-emitted vtable that virtual constructors load from
///     (`store ptr getelementptr inbounds (..., @_ZTV<C>, ..., i32 2),
///     ptr %this`). When the stub bitcode is linked into the C++
///     bitcode via `llvm-link --override`, our stub `_ZTV<C>` wins and
///     virtual dispatch resolves to the stub function (which has a
///     havoc spec bound) instead of the real method.
pub fn generate_llvm_ir_stubs(
    by_class: &BTreeMap<String, Vec<&InterfaceMethod>>,
    classes_with_vdtor: &HashSet<String>,
    target_triple: Option<&str>,
) -> String {
    let abi = TargetAbi::from_triple(target_triple);
    let originating = compute_originating_classes(by_class);
    let resolve_stub_class = |m: &InterfaceMethod| -> String {
        if m.is_override {
            originating
                .get(&m.method.name)
                .cloned()
                .unwrap_or_else(|| m.class_name.clone())
        } else {
            m.class_name.clone()
        }
    };
    let stub_name_for = |m: &InterfaceMethod| -> String {
        let cls = resolve_stub_class(m);
        format!(
            "{}_{}_stub",
            sanitize_name(&cls).to_lowercase(),
            sanitize_name(&m.method.name).to_lowercase(),
        )
    };

    let mut out = String::new();
    out.push_str("; Auto-generated vtable stubs for SAW verification\n");
    out.push_str("; Load directly: m_stubs <- llvm_load_module \"vtable_stubs.ll\";\n");
    out.push_str("; Or assemble:   llvm-as vtable_stubs.ll -o vtable_stubs.bc\n");
    out.push_str(";\n");
    out.push_str("; SAW resolves indirect vtable calls through these:\n");
    out.push_str(";   this->vptr -> vtable[slot] -> stub function -> havoc spec\n\n");
    out.push_str(abi.datalayout());
    out.push('\n');
    out.push_str(&format!("target triple = \"{}\"\n\n", abi.triple()));

    // Every symbol a vtable global references must also be *defined* in
    // this same module, or the `.ll` won't assemble to `.bc` and SAW's
    // `llvm_load_module` aborts. Overrides normally reuse the
    // originating (base) class's stub, which is emitted when we visit
    // that base class. But an override whose originating class is absent
    // from `by_class` (an external / unresolved base — the parser's
    // `"Unknown"` fallback) resolves via `stub_name_for` back to the
    // derived class's own name, so no base stub ever gets emitted.
    // Track the stub symbols we've defined and emit one for every
    // referenced slot exactly once, so the vtable never dangles.
    let mut emitted_stubs: HashSet<String> = HashSet::new();

    for (class_name, class_methods) in by_class {
        let safe_class = sanitize_name(class_name).to_lowercase();
        let has_vdtor = classes_with_vdtor.contains(class_name.as_str());

        out.push_str(&format!("; ---- {class_name} vtable ----\n\n"));

        // MSVC ABI: a single deleting-destructor slot when the class has
        // a virtual dtor. Itanium would carry a pair (complete + deleting);
        // matching the wrong ABI shifts every method slot by one.
        if has_vdtor {
            match abi {
                TargetAbi::Msvc => {
                    out.push_str(&format!(
                        "define void @{safe_class}_deleting_dtor_stub(ptr %self, i32 %flags) {{\n  ret void\n}}\n\n"
                    ));
                }
                TargetAbi::Itanium => {
                    // Itanium has two dtor slots: D1 (complete object)
                    // at slot 0, D0 (deleting) at slot 1.
                    out.push_str(&format!(
                        "define void @{safe_class}_complete_dtor_stub(ptr %self) {{\n  ret void\n}}\n\n"
                    ));
                    out.push_str(&format!(
                        "define void @{safe_class}_deleting_dtor_stub(ptr %self) {{\n  ret void\n}}\n\n"
                    ));
                }
            }
        }

        for method in class_methods {
            // Emit each stub exactly once, keyed by its *resolved* name.
            // A non-override defines its own stub; an override reuses the
            // originating base's stub (defined when we visit that base).
            // The one case that would otherwise dangle is an override
            // whose base is absent from `by_class`: `stub_name_for`
            // resolves it to the derived class name, so define a concrete
            // (trivial) stub here to keep the vtable global well-formed.
            let stub_name = stub_name_for(method);
            if emitted_stubs.insert(stub_name.clone()) {
                emit_stub_for_method(&mut out, class_name, &stub_name, method);
            }
        }

        // MSVC-style flat vtable. Kept on every target — the
        // `alloc_<class>_this` SAW helper references this global via
        // `llvm_global_initializer "<class>_vtable"` and is portable
        // across ABIs.
        let msvc_dtor_slots = if has_vdtor { 1 } else { 0 };
        let msvc_slot_count = class_methods.len() + msvc_dtor_slots;
        out.push_str(&format!(
            "; Concrete vtable for {class_name} (MSVC-style, used by alloc_{safe_class}_this)\n"
        ));
        out.push_str(&format!(
            "@{safe_class}_vtable = global [{msvc_slot_count} x ptr] [\n"
        ));
        let mut first = true;
        if has_vdtor {
            out.push_str(&format!("  ptr @{safe_class}_deleting_dtor_stub"));
            first = false;
        }
        for method in class_methods {
            let stub_name = stub_name_for(method);
            if first {
                out.push_str(&format!("  ptr @{stub_name}"));
                first = false;
            } else {
                out.push_str(&format!(",\n  ptr @{stub_name}"));
            }
        }
        out.push_str("\n]\n\n");

        emit_itanium_ztv(
            &mut out,
            class_name,
            class_methods,
            has_vdtor,
            abi,
            &stub_name_for,
        );
    }
    out
}

fn emit_itanium_ztv(
    out: &mut String,
    class_name: &str,
    class_methods: &[&InterfaceMethod],
    has_vdtor: bool,
    abi: TargetAbi,
    stub_name_for: &dyn Fn(&InterfaceMethod) -> String,
) {
    if !matches!(abi, TargetAbi::Itanium) {
        return;
    }
    let safe_class = sanitize_name(class_name).to_lowercase();
    let itanium_dtor_slots = if has_vdtor { 2 } else { 0 };
    let itanium_slot_count = 2 + itanium_dtor_slots + class_methods.len();
    let mangled = itanium_mangle_class_name(class_name);
    let symbol = format!("_ZTV{mangled}");
    out.push_str(&format!(
        "; Itanium-ABI vtable override for {class_name} ({symbol}).\n"
    ));
    out.push_str(
        "; Replaces the compiler-emitted vtable when linked via `llvm-link --override`.\n",
    );
    out.push_str(&format!(
        "@{symbol} = linkonce_odr unnamed_addr constant {{ [{itanium_slot_count} x ptr] }} {{ [{itanium_slot_count} x ptr] [\n"
    ));
    out.push_str("  ptr null,\n");
    out.push_str("  ptr null");
    if has_vdtor {
        out.push_str(&format!(
            ",\n  ptr @{safe_class}_complete_dtor_stub,\n  ptr @{safe_class}_deleting_dtor_stub"
        ));
    }
    for method in class_methods {
        let stub_name = stub_name_for(method);
        out.push_str(&format!(",\n  ptr @{stub_name}"));
    }
    out.push_str("\n] }, align 8\n\n");
}

/// LLVM target ABI flavour.  Selected from the input bitcode's
/// `target triple` line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TargetAbi {
    /// Windows-MSVC C++ ABI. Flat vtable, single deleting-dtor slot,
    /// MSVC-style mangled vtable symbols (`??_7C@@6B@`).
    Msvc,
    /// Itanium C++ ABI (Linux / macOS / *BSD / MinGW). Vtable has two
    /// leader slots (offset-to-top, RTTI), two dtor slots (D1, D0),
    /// `_ZTV<mangled>` mangled symbol.
    Itanium,
}

impl TargetAbi {
    fn from_triple(triple: Option<&str>) -> Self {
        match triple {
            Some(t) if t.contains("windows-msvc") => TargetAbi::Msvc,
            Some(_) => TargetAbi::Itanium,
            None => TargetAbi::Msvc,
        }
    }

    fn triple(self) -> &'static str {
        match self {
            TargetAbi::Msvc => "x86_64-pc-windows-msvc",
            TargetAbi::Itanium => "x86_64-unknown-linux-gnu",
        }
    }

    fn datalayout(self) -> &'static str {
        match self {
            TargetAbi::Msvc =>
                "target datalayout = \"e-m:w-p270:32:32-p271:32:32-p272:64:64-i64:64-i128:128-f80:128-n8:16:32:64-S128\"",
            TargetAbi::Itanium =>
                "target datalayout = \"e-m:e-p270:32:32-p271:32:32-p272:64:64-i64:64-i128:128-f80:128-n8:16:32:64-S128\"",
        }
    }
}

/// Mangle a C++ class name into the Itanium-ABI form that follows the
/// `_ZTV` prefix.  Handles top-level (`BadProcessor` → `12BadProcessor`)
/// and `::`-namespaced (`foo::Bar` → `N3foo3BarE`) cases.
fn itanium_mangle_class_name(class_name: &str) -> String {
    let name = class_name
        .trim_start_matches("class ")
        .trim_start_matches("struct ")
        .trim();
    if name.contains("::") {
        let mut out = String::from("N");
        for part in name.split("::") {
            out.push_str(&part.len().to_string());
            out.push_str(part);
        }
        out.push('E');
        out
    } else {
        format!("{}{}", name.len(), name)
    }
}

/// Build `method_name → originating_class` for use when redirecting
/// override vtable slots.
fn compute_originating_classes(
    by_class: &BTreeMap<String, Vec<&InterfaceMethod>>,
) -> HashMap<String, String> {
    let mut originating: HashMap<String, String> = HashMap::new();
    for methods in by_class.values() {
        for m in methods {
            if !m.is_override {
                originating
                    .entry(m.method.name.clone())
                    .or_insert_with(|| m.class_name.clone());
            }
        }
    }
    originating
}

/// Emit one stub function definition.
fn emit_stub_for_method(
    out: &mut String,
    class_name: &str,
    stub_name: &str,
    method: &InterfaceMethod,
) {
    let param_strs = method_param_ir_pieces(&method.method);
    let (ret_ir, params_ir) = match sret_inner_ir_type(&method.method.return_type) {
        Some(inner) => {
            let mut parts: Vec<String> = Vec::new();
            if let Some(this_p) = param_strs.first() {
                parts.push(this_p.clone());
            }
            parts.push(format!("ptr sret({inner}) %retptr"));
            for p in param_strs.iter().skip(1) {
                parts.push(p.clone());
            }
            ("void".to_string(), parts.join(", "))
        }
        None => (
            type_to_llvm_ir(&method.method.return_type),
            param_strs.join(", "),
        ),
    };
    out.push_str(&format!(
        "; {class_name}::{} [{}]\n",
        method.method.name,
        if method.is_pure {
            "pure virtual"
        } else {
            "virtual"
        },
    ));
    out.push_str(&format!("define {ret_ir} @{stub_name}({params_ir}) {{\n"));
    out.push_str(&format!("  {}\n", ir_default_return(&ret_ir)));
    out.push_str("}\n\n");
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::constraints::{FunctionInfo, Mutability, Nullability, ParamInfo, TypeInfo};

    fn make_iface_method(class: &str, name: &str, ret: TypeInfo, offset: u64) -> InterfaceMethod {
        InterfaceMethod {
            class_name: class.into(),
            method: FunctionInfo {
                name: name.into(),
                mangled_name: None,
                params: vec![ParamInfo {
                    name: "this".into(),
                    ty: TypeInfo::Pointer(Box::new(TypeInfo::Opaque {
                        name: "Self".into(),
                        size_bytes: 0,
                    })),
                    mutability: Mutability::Readonly,
                    nullable: Nullability::NonNull,
                    annotations: vec![],
                }],
                return_type: ret,
                can_throw: false,
                is_virtual: true,
                has_body: false,
                is_system: false,
                annotations: vec![],
                referenced_globals: vec![],
                called_functions: vec![],
            },
            is_pure: true,
            is_override: false,
            source_offset: offset,
        }
    }

    #[test]
    fn test_itanium_emits_ztv_override_with_leader_slots() {
        let methods = [
            make_iface_method("BadProcessor", "validate", TypeInfo::Void, 100),
            make_iface_method("BadProcessor", "audit", TypeInfo::Void, 200),
        ];
        let mut by_class = BTreeMap::new();
        by_class.insert(
            "BadProcessor".to_string(),
            methods.iter().collect::<Vec<_>>(),
        );
        let ll =
            generate_llvm_ir_stubs(&by_class, &HashSet::new(), Some("x86_64-unknown-linux-gnu"));
        assert!(
            ll.contains("target triple = \"x86_64-unknown-linux-gnu\""),
            "expected Itanium triple, got:\n{ll}",
        );
        assert!(
            ll.contains("@badprocessor_vtable"),
            "MSVC-style helper vtable should still be emitted",
        );
        assert!(
            ll.contains("@_ZTV12BadProcessor = linkonce_odr unnamed_addr constant { [4 x ptr] }"),
            "expected Itanium `_ZTV12BadProcessor` override with 4 slots, got:\n{ll}",
        );
        let ztv_section = ll
            .split("@_ZTV12BadProcessor")
            .nth(1)
            .expect("ZTV body missing");
        let leader_block =
            &ztv_section[..ztv_section.find("], align").unwrap_or(ztv_section.len())];
        assert!(
            leader_block.matches("ptr null").count() >= 2,
            "expected two `ptr null` leader slots, got:\n{leader_block}",
        );
    }

    #[test]
    fn test_msvc_default_omits_ztv_override() {
        let methods = [make_iface_method(
            "BadProcessor",
            "validate",
            TypeInfo::Void,
            100,
        )];
        let mut by_class = BTreeMap::new();
        by_class.insert(
            "BadProcessor".to_string(),
            methods.iter().collect::<Vec<_>>(),
        );
        let ll = generate_llvm_ir_stubs(&by_class, &HashSet::new(), None);
        assert!(ll.contains("target triple = \"x86_64-pc-windows-msvc\""));
        assert!(
            !ll.contains("_ZTV"),
            "MSVC mode should not emit Itanium `_ZTV` symbols:\n{ll}"
        );
    }

    #[test]
    fn test_override_with_absent_base_defines_every_referenced_stub() {
        // Regression for docs/07: a class whose virtual methods are all
        // `override`s of a base that is NOT present in `by_class` (an
        // external/unresolved base — the parser's `"Unknown"` fallback).
        // `stub_name_for` resolves each override back to the derived
        // class's own name, so no base stub gets emitted. Before the fix
        // the vtable global referenced `@unknown__*_stub` symbols that
        // were never defined and the `.ll` failed to assemble.
        let mut destroy = make_iface_method("Unknown", "_destroy", TypeInfo::Void, 100);
        destroy.is_override = true;
        let mut delete_this = make_iface_method("Unknown", "_delete_this", TypeInfo::Void, 200);
        delete_this.is_override = true;
        let methods = [destroy, delete_this];
        let mut by_class = BTreeMap::new();
        by_class.insert("Unknown".to_string(), methods.iter().collect::<Vec<_>>());
        let ll = generate_llvm_ir_stubs(&by_class, &HashSet::new(), None);

        // Every `ptr @sym` slot inside the vtable global must have a
        // matching `define ... @sym(` in the same module.
        let tail = ll
            .split("_vtable = global")
            .nth(1)
            .expect("vtable global missing");
        let body = &tail[..tail.find("\n]\n").unwrap_or(tail.len())];
        let mut slots = 0;
        for slot in body.split("ptr @").skip(1) {
            let sym: String = slot
                .chars()
                .take_while(|c| c.is_alphanumeric() || *c == '_')
                .collect();
            assert!(!sym.is_empty(), "empty vtable slot symbol in:\n{ll}");
            assert!(
                ll.contains(&format!("@{sym}(")),
                "vtable references @{sym} but no definition was emitted:\n{ll}",
            );
            slots += 1;
        }
        assert_eq!(slots, 2, "expected two vtable slots, got:\n{ll}");
    }

    #[test]
    fn test_itanium_mangle_class_name_simple_and_namespaced() {
        assert_eq!(itanium_mangle_class_name("BadProcessor"), "12BadProcessor");
        assert_eq!(itanium_mangle_class_name("foo::Bar"), "N3foo3BarE");
        assert_eq!(
            itanium_mangle_class_name("class BadProcessor"),
            "12BadProcessor"
        );
    }

    #[test]
    fn test_target_abi_from_triple_picks_correct_flavour() {
        assert_eq!(
            TargetAbi::from_triple(Some("x86_64-pc-windows-msvc")),
            TargetAbi::Msvc,
        );
        assert_eq!(
            TargetAbi::from_triple(Some("x86_64-unknown-linux-gnu")),
            TargetAbi::Itanium,
        );
        assert_eq!(
            TargetAbi::from_triple(Some("aarch64-apple-darwin")),
            TargetAbi::Itanium,
        );
        assert_eq!(
            TargetAbi::from_triple(Some("x86_64-pc-windows-gnu")),
            TargetAbi::Itanium,
        );
        assert_eq!(TargetAbi::from_triple(None), TargetAbi::Msvc);
    }
}
