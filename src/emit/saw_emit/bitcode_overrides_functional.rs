//! Functional SAW spec emission for STL container overrides.
//!
//! Where [`super::bitcode_overrides`] emits adversarial havoc specs
//! for unknown extern calls, this module emits *functional* specs for
//! a curated set of standard-library methods whose semantics are
//! simple enough to model field-for-field (basic_string) or via SAW
//! ghost state (vector). The actual emission logic lives in:
//!
//! * [`super::bitcode_overrides_functional_string`] —
//!   `std::basic_string::{ctor, dtor, size, resize, data}` using
//!   integer field indices via `llvm_elem`.
//! * [`super::bitcode_overrides_functional_vector`] —
//!   `std::vector::{ctor, dtor, push_back(T&&), back}` using SAW
//!   ghost state to couple writes and reads across calls.
//!
//! Together these flip the two `gap_disproved` cases under
//! `tests/e2e/cases/10-stl-coverage/{string_size_havoc,
//! vector_back_havoc}/` from DISPROVED to VERIFIED.

use std::collections::HashMap;

use crate::constraints::container_layouts::ContainerCatalog;
use crate::parsers::llvm_ir::IrStructDef;

use super::bitcode_overrides_functional_string::{
    discover_string_layout, emit_string_override, StringLayout,
};
use super::bitcode_overrides_functional_vector::{
    element_bits_from_mangled, emit_vector_ghost_decl, emit_vector_override,
};

/// Which canonical STL method does this mangled symbol denote? We
/// only recognize methods whose semantics fit a one-line functional
/// spec — anything fancier falls through to the havoc emitter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StlMethod {
    /// `basic_string::basic_string()` — default ctor.
    BasicStringCtorDefault,
    /// `basic_string::~basic_string()` — destructor.
    BasicStringDtor,
    /// `basic_string::size()` — read-only size accessor.
    BasicStringSize,
    /// `basic_string::resize(size_type)` — write-only size mutator.
    BasicStringResize,
    /// Read-only byte-view accessor with no explicit index argument
    /// (e.g. `data()`, `c_str()`).
    BasicStringByteViewNoArg,
    /// Read-only byte-view accessor with an explicit index argument
    /// (e.g. `operator[]`, `at()`).
    BasicStringByteViewIndexArg,
    /// `basic_string::basic_string(const char*)` — ctor from C-string.
    BasicStringCtorFromCStr,
    /// `basic_string::basic_string(const basic_string&)` — copy ctor.
    BasicStringCtorCopy,
    /// `basic_string::assign(const char*)` — assign from C-string.
    BasicStringAssignCStr,
    /// `basic_string::assign(const basic_string&)` — assign from another string.
    BasicStringAssignStr,
    /// `basic_string::operator=(const char*)` — assign-op from C-string.
    BasicStringOpEqCStr,
    /// `basic_string::operator=(const basic_string&)` — copy-assign.
    BasicStringOpEqStr,
    /// `basic_string::empty()` — returns `sz == 0`.
    BasicStringEmpty,
    /// Size-neutral scalar query (e.g. `capacity()`) modeled as a
    /// fresh symbolic i64 return.
    BasicStringSizeNeutralScalarQuery,
    /// Size-neutral mutator with one size-like argument (e.g.
    /// `reserve(size_type)`), modeled with no post-state writes.
    BasicStringSizeNeutralMutator,
    /// `vector::vector()` — default ctor.
    VectorCtorDefault,
    /// `vector::~vector()` — destructor.
    VectorDtor,
    /// `vector::push_back(T&&)` — rvalue overload.
    VectorPushBackRvalue,
    /// `vector::back()` — non-const reference accessor.
    VectorBack,
}

impl StlMethod {
    /// Is this method part of the `std::vector` family? Used by the
    /// dispatcher to decide whether to emit the shared ghost
    /// declaration at the top of the script.
    pub fn is_vector(self) -> bool {
        matches!(
            self,
            StlMethod::VectorCtorDefault
                | StlMethod::VectorDtor
                | StlMethod::VectorPushBackRvalue
                | StlMethod::VectorBack
        )
    }
}

/// Classify a mangled symbol against the supported method registry.
pub fn classify(mangled: &str) -> Option<StlMethod> {
    if is_basic_string(mangled) {
        return classify_basic_string(mangled);
    }
    if is_vector(mangled) {
        return classify_vector(mangled);
    }
    None
}

/// Returns `true` when `mangled` has a curated functional model in the
/// bitcode override scan (i.e., `classify` would return `Some`). Used by
/// `gen_verify` to exclude these symbols from the AST-derived
/// `already_covered` list so the bitcode scan can emit the correct
/// functional override instead of a generic havoc spec.
pub fn is_stl_functional(mangled: &str) -> bool {
    classify(mangled).is_some()
}

fn is_basic_string(mangled: &str) -> bool {
    mangled.contains("St7__cxx1112basic_string")
        || mangled.contains("St12basic_string")
        || mangled.contains("NSt3__112basic_string")
        || mangled.contains("?$basic_string@")
}

fn is_vector(mangled: &str) -> bool {
    mangled.contains("St6vector") || mangled.contains("?$vector@")
}

fn classify_basic_string(m: &str) -> Option<StlMethod> {
    // From-cstr ctor must precede default ctor (both start with C1/C2).
    if has_suffix_any(m, &["C1EPKc", "C2EPKc", "EC1EPKc", "EC2EPKc"]) {
        return Some(StlMethod::BasicStringCtorFromCStr);
    }
    if has_suffix_any(m, &["C1Ev", "C2Ev", "EC1Ev", "EC2Ev"]) {
        return Some(StlMethod::BasicStringCtorDefault);
    }
    // Copy ctor: const-ref to same type (substitution number varies).
    if m.contains("C1ERKS") || m.contains("C2ERKS") || m.contains("EC1ERKS") {
        return Some(StlMethod::BasicStringCtorCopy);
    }
    if has_suffix_any(m, &["D1Ev", "D2Ev", "ED1Ev", "ED2Ev"]) {
        return Some(StlMethod::BasicStringDtor);
    }
    if has_suffix_any(m, &["4sizeEv", "E4sizeEv", "6lengthEv", "E6lengthEv"]) {
        return Some(StlMethod::BasicStringSize);
    }
    if has_suffix_any(m, &["6resizeEm", "E6resizeEm", "6resizeEj", "E6resizeEj"]) {
        return Some(StlMethod::BasicStringResize);
    }
    // Family: byte-view accessors. These methods expose character
    // storage but do not change the tracked size field.
    if has_suffix_any(m, &["4dataEv", "E4dataEv", "5c_strEv", "E5c_strEv"]) {
        return Some(StlMethod::BasicStringByteViewNoArg);
    }
    if has_suffix_any(
        m,
        &[
            "ixEm", "EixEm", "ixEj", "EixEj", "2atEm", "E2atEm", "2atEj", "E2atEj",
        ],
    ) {
        return Some(StlMethod::BasicStringByteViewIndexArg);
    }
    if has_suffix_any(m, &["6assignEPKc", "E6assignEPKc"]) {
        return Some(StlMethod::BasicStringAssignCStr);
    }
    if m.contains("6assignERKS") {
        return Some(StlMethod::BasicStringAssignStr);
    }
    if has_suffix_any(m, &["aSEPKc", "EaSEPKc"]) {
        return Some(StlMethod::BasicStringOpEqCStr);
    }
    if m.contains("aSERKS") {
        return Some(StlMethod::BasicStringOpEqStr);
    }
    if has_suffix_any(m, &["5emptyEv", "E5emptyEv"]) {
        return Some(StlMethod::BasicStringEmpty);
    }
    if has_suffix_any(m, &["8capacityEv", "E8capacityEv"]) {
        return Some(StlMethod::BasicStringSizeNeutralScalarQuery);
    }
    if has_suffix_any(
        m,
        &["7reserveEm", "E7reserveEm", "7reserveEj", "E7reserveEj"],
    ) {
        return Some(StlMethod::BasicStringSizeNeutralMutator);
    }
    None
}

fn classify_vector(m: &str) -> Option<StlMethod> {
    if has_suffix_any(m, &["C1Ev", "C2Ev", "EC1Ev", "EC2Ev"]) {
        return Some(StlMethod::VectorCtorDefault);
    }
    if has_suffix_any(m, &["D1Ev", "D2Ev", "ED1Ev", "ED2Ev"]) {
        return Some(StlMethod::VectorDtor);
    }
    if has_suffix_any(m, &["E4backEv"]) {
        return Some(StlMethod::VectorBack);
    }
    if has_suffix_any(
        m,
        &[
            "E9push_backEOj",
            "E9push_backEOi",
            "E9push_backEOm",
            "E9push_backEOl",
        ],
    ) {
        return Some(StlMethod::VectorPushBackRvalue);
    }
    None
}

fn has_suffix_any(s: &str, suffixes: &[&str]) -> bool {
    suffixes.iter().any(|suf| s.ends_with(suf))
}

/// Discovered container layouts the emitter needs. Currently only
/// `basic_string` requires struct-table introspection; the vector
/// model is layout-agnostic (ghost-state coupled).
#[derive(Debug, Clone, Default)]
pub struct FunctionalLayouts {
    pub string: Option<StringLayout>,
}

impl FunctionalLayouts {
    /// Run every supported container's layout-discovery routine
    /// against the IR struct table, gated by the AST-derived
    /// container catalog (saw_spec_gen-qms): we only emit functional
    /// overrides for containers whose shape the catalog independently
    /// confirms from the clang AST. This keeps the catalog \u2014 not
    /// hand-rolled IR string matching \u2014 the source of truth for
    /// which container types we model functionally.
    pub fn discover(
        struct_defs: &HashMap<String, IrStructDef>,
        catalog: &ContainerCatalog,
    ) -> Self {
        // `basic_string` is the only IR-introspected container today.
        // The catalog seeds defaults for `std::string` so the lookup
        // succeeds out-of-the-box; AST auto-derive also contributes
        // `std::__cxx11::basic_string` for libstdc++ targets.
        let string = if catalog.lookup("std::string").is_some()
            || catalog.lookup("std::__cxx11::basic_string").is_some()
        {
            discover_string_layout(struct_defs)
        } else {
            None
        };
        Self { string }
    }
}

/// Convenience: classify, look up layout, and emit. Returns `true`
/// when a functional override was emitted (caller must skip the
/// havoc emission), `false` when nothing was emitted.
pub fn try_emit_functional(
    out: &mut String,
    symbol: &str,
    safe: &str,
    ov_name: &str,
    layouts: &FunctionalLayouts,
) -> bool {
    let Some(method) = classify(symbol) else {
        return false;
    };
    if method.is_vector() {
        let Some(bits) = element_bits_from_mangled(symbol) else {
            return false;
        };
        emit_vector_override(out, method, bits, symbol, safe, ov_name);
        return true;
    }
    let Some(layout) = layouts.string.as_ref() else {
        return false;
    };
    emit_string_override(out, method, layout, symbol, safe, ov_name);
    true
}

/// Does the target set include at least one vector method we'd emit a
/// functional override for? The dispatcher uses this to decide
/// whether to emit the shared `declare_ghost_state` binding.
pub fn needs_vector_ghost(symbols: impl IntoIterator<Item = impl AsRef<str>>) -> bool {
    symbols.into_iter().any(|s| {
        let m = s.as_ref();
        classify(m)
            .map(|k| k.is_vector() && element_bits_from_mangled(m).is_some())
            .unwrap_or(false)
    })
}

/// Emit the shared ghost-state declaration (delegates to the vector
/// module). Caller is responsible for guarding this with
/// [`needs_vector_ghost`].
pub fn emit_shared_ghost_decl(out: &mut String) {
    emit_vector_ghost_decl(out);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_recognises_basic_string_size() {
        let m = "_ZNKSt7__cxx1112basic_stringIcSt11char_traitsIcESaIcEE4sizeEv";
        assert_eq!(classify(m), Some(StlMethod::BasicStringSize));
    }

    #[test]
    fn classify_recognises_basic_string_resize() {
        let m = "_ZNSt7__cxx1112basic_stringIcSt11char_traitsIcESaIcEE6resizeEm";
        assert_eq!(classify(m), Some(StlMethod::BasicStringResize));
    }

    #[test]
    fn classify_recognises_basic_string_default_ctor_and_dtor() {
        assert_eq!(
            classify("_ZNSt7__cxx1112basic_stringIcSt11char_traitsIcESaIcEEC1Ev"),
            Some(StlMethod::BasicStringCtorDefault),
        );
        assert_eq!(
            classify("_ZNSt7__cxx1112basic_stringIcSt11char_traitsIcESaIcEED1Ev"),
            Some(StlMethod::BasicStringDtor),
        );
    }

    #[test]
    fn classify_recognises_vector_methods() {
        assert_eq!(
            classify("_ZNSt6vectorIjSaIjEEC2Ev"),
            Some(StlMethod::VectorCtorDefault),
        );
        assert_eq!(
            classify("_ZNSt6vectorIjSaIjEED2Ev"),
            Some(StlMethod::VectorDtor),
        );
        assert_eq!(
            classify("_ZNSt6vectorIjSaIjEE9push_backEOj"),
            Some(StlMethod::VectorPushBackRvalue),
        );
        assert_eq!(
            classify("_ZNSt6vectorIjSaIjEE4backEv"),
            Some(StlMethod::VectorBack),
        );
    }

    #[test]
    fn classify_skips_unsupported_methods() {
        assert_eq!(
            classify("_ZNSt7__cxx1112basic_stringIcSt11char_traitsIcESaIcEE6appendEPKc"),
            None,
        );
        assert_eq!(classify("_ZNKSt6vectorIjSaIjEE4sizeEv"), None);
        assert_eq!(classify("_Z3fooi"), None);
    }

    #[test]
    fn needs_vector_ghost_true_when_any_vector_method_present() {
        let syms = vec![
            "_ZNKSt7__cxx1112basic_stringIcSt11char_traitsIcESaIcEE4sizeEv",
            "_ZNSt6vectorIjSaIjEE9push_backEOj",
        ];
        assert!(needs_vector_ghost(&syms));
    }

    #[test]
    fn needs_vector_ghost_false_for_string_only() {
        let syms = vec![
            "_ZNSt7__cxx1112basic_stringIcSt11char_traitsIcESaIcEEC1Ev",
            "_ZNKSt7__cxx1112basic_stringIcSt11char_traitsIcESaIcEE4sizeEv",
        ];
        assert!(!needs_vector_ghost(&syms));
    }

    #[test]
    fn needs_vector_ghost_false_for_unsupported_vector_element() {
        let syms = vec!["_ZNSt6vectorIN3foo3BarESaIS1_EEC2Ev"];
        assert!(!needs_vector_ghost(&syms));
    }

    #[test]
    fn try_emit_functional_skips_unknown_symbol() {
        let mut out = String::new();
        let layouts = FunctionalLayouts::default();
        assert!(!try_emit_functional(
            &mut out, "_Z3fooi", "f", "ov_f", &layouts,
        ));
        assert!(out.is_empty());
    }
}
