//! Identifier sanitization helpers for filenames, SAW variable names,
//! and stub function names.

use crate::clang_ast::InterfaceMethod;
use crate::constraints::SpecConstraint;

/// Sanitize a name for use as a filename or SAW identifier.
///
/// Mangled symbols are first run through [`crate::mangle::humanize`] to
/// produce a short readable form, then a short hash is appended for
/// collision resistance. Non-mangled names go through
/// [`crate::mangle::sanitize_filename_chars`] and are truncated to 120
/// characters (with a hash suffix) so they fit Windows' `MAX_PATH`.
pub fn sanitize_name(name: &str) -> String {
    if let Some(human) = crate::mangle::humanize(name) {
        let safe = crate::mangle::sanitize_filename_chars(&human);
        let hash = crate::mangle::short_hash(name);
        return format!("{safe}_{hash}");
    }
    let sanitized = crate::mangle::sanitize_filename_chars(name);
    if sanitized.len() > 120 {
        let hash = crate::mangle::short_hash(&sanitized);
        format!("{}_{}", &sanitized[..120], hash)
    } else {
        sanitized
    }
}

/// Stable identifier for a [`SpecConstraint`] — used for filenames,
/// override variable names, and `include` statements.
///
/// The mangled name is preferred when distinct from the unmangled name,
/// so overloaded functions and same-named methods on different classes
/// produce distinct identifiers.
pub fn spec_safe_id(spec: &SpecConstraint) -> String {
    if let Some(ref m) = spec.mangled_name {
        if !m.is_empty() && m != &spec.function_name {
            return sanitize_name(m);
        }
    }
    sanitize_name(&spec.function_name)
}

/// `extern "C"`-friendly stub function name for a virtual method.
/// Matches the symbol emitted in `vtable_stubs.ll`.
pub fn stub_function_name(method: &InterfaceMethod) -> String {
    format!(
        "{}_{}_stub",
        sanitize_name(&method.class_name).to_lowercase(),
        sanitize_name(&method.method.name).to_lowercase(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::constraints::{ReturnConstraint, SpecConstraint};

    fn make_spec(name: &str, mangled: Option<&str>) -> SpecConstraint {
        SpecConstraint {
            function_name: name.into(),
            mangled_name: mangled.map(String::from),
            params: vec![],
            return_constraint: ReturnConstraint {
                saw_type: "// void".into(),
                value_constraints: vec![],
                is_sret: false,
                returns_pointer: false,
                sret_prestate: false,
            },
            can_throw: false,
            is_virtual: false,
            has_body: true,
            referenced_globals: vec![],
            postconditions: vec![],
        }
    }

    #[test]
    fn sanitize_passes_through_simple_identifiers() {
        assert_eq!(sanitize_name("simple"), "simple");
    }

    #[test]
    fn sanitize_replaces_cpp_punctuation() {
        assert_eq!(sanitize_name("my::func"), "my__func");
        assert_eq!(sanitize_name("ns::Class<int>"), "ns__Class_int_");
    }

    #[test]
    fn sanitize_truncates_long_names_with_hash_suffix() {
        let long = "a".repeat(200);
        let out = sanitize_name(&long);
        // 120 char prefix + "_" + hash → strictly longer than 120,
        // strictly shorter than original.
        assert!(out.len() > 120);
        assert!(out.len() < 200);
        assert!(out.starts_with(&"a".repeat(120)));
    }

    #[test]
    fn spec_safe_id_prefers_mangled_when_different() {
        let s = make_spec("log", Some("?log@Foo@@QEAAXXZ"));
        let id = spec_safe_id(&s);
        // Mangled name was used (contains "Foo" or hash); should NOT
        // collide with the plain "log" sanitization.
        assert_ne!(id, "log");
    }

    #[test]
    fn spec_safe_id_falls_back_to_unmangled_when_same() {
        let s = make_spec("plain", Some("plain"));
        assert_eq!(spec_safe_id(&s), "plain");
    }

    #[test]
    fn spec_safe_id_falls_back_to_unmangled_when_missing() {
        let s = make_spec("plain", None);
        assert_eq!(spec_safe_id(&s), "plain");
    }

    /// Two C++ overloads share the same friendly name but have distinct
    /// mangled symbols. `spec_safe_id` must produce DIFFERENT identifiers
    /// for each — otherwise their generated `.saw` files would collide and
    /// one overload's spec would silently overwrite the other on disk.
    #[test]
    fn spec_safe_id_distinguishes_overloads_by_mangled_name() {
        // C++ source equivalent:
        //   int    add(int, int);     // mangles to "_Z3addii"
        //   double add(double, double); // mangles to "_Z3adddd"
        let int_overload = make_spec("add", Some("_Z3addii"));
        let dbl_overload = make_spec("add", Some("_Z3adddd"));
        let int_id = spec_safe_id(&int_overload);
        let dbl_id = spec_safe_id(&dbl_overload);
        assert_ne!(
            int_id, dbl_id,
            "overloads with same friendly name must get distinct spec ids"
        );
        // Neither should collapse to the bare friendly name (which would be
        // the collision-prone fallback path).
        assert_ne!(int_id, "add");
        assert_ne!(dbl_id, "add");
    }

    /// Same method name on two different classes (`Foo::compute` vs
    /// `Bar::compute`) must also produce distinct ids. This is the same
    /// invariant as classic overloading, just driven by class scope rather
    /// than parameter list.
    #[test]
    fn spec_safe_id_distinguishes_same_method_on_different_classes() {
        let foo = make_spec("compute", Some("_ZNK3Foo7computeEv"));
        let bar = make_spec("compute", Some("_ZNK3Bar7computeEv"));
        assert_ne!(spec_safe_id(&foo), spec_safe_id(&bar));
    }
}
