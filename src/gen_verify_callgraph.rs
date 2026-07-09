//! Call-graph walk + external-call filtering for [`crate::gen_verify`].
//!
//! Extracted to keep `gen_verify.rs` under the 500 non-whitespace line
//! limit (see `.github/copilot-instructions.md`). The logic is:
//!
//!   1. Build name → `FunctionInfo` maps so the worklist can resolve
//!      both mangled and pretty names.
//!   2. Walk the call graph from `target_fn`, descending into bodies
//!      that look Crucible-safe (see [`crate::transform::crucible_safety`])
//!      or are non-system. Skipped system callees still get their
//!      mangled name recorded in `called_mangled` so the external
//!      filter below can find them.
//!   3. Filter `all_specs` down to the calls the script actually needs
//!      adversarial overrides for — externals, system callees whose
//!      bodies the safety check rejected, etc.
//!
//! Both `should_recurse` and `crucible_safe` honour the
//! `SAW_SPEC_GEN_NO_SYSTEM_RECURSION=1` kill-switch.

use crate::constraints;
use crate::emit::saw_emit::stl_overrides;
use crate::transform::crucible_safety::SafetyAnalyzer;
use std::collections::{HashMap, HashSet};

/// Walk the callgraph from `target_fn` and return the subset of
/// `all_specs` that the verification script needs adversarial
/// overrides for.
pub fn collect_external_call_specs<'s, 'f>(
    target_fn: &'f constraints::FunctionInfo,
    all_functions: &'f [constraints::FunctionInfo],
    all_specs: &'s [constraints::SpecConstraint],
    safety: &mut SafetyAnalyzer<'_>,
    no_system_recursion: bool,
) -> Vec<&'s constraints::SpecConstraint> {
    let fn_by_mangled: HashMap<String, &'f constraints::FunctionInfo> = all_functions
        .iter()
        .filter_map(|f| f.mangled_name.as_ref().map(|m| (m.clone(), f)))
        .collect();
    let fn_by_name: HashMap<String, &'f constraints::FunctionInfo> =
        all_functions.iter().map(|f| (f.name.clone(), f)).collect();

    let called_mangled = walk_callgraph(
        target_fn,
        &fn_by_mangled,
        &fn_by_name,
        safety,
        no_system_recursion,
    );

    filter_external_specs(
        all_specs,
        &fn_by_mangled,
        &fn_by_name,
        &called_mangled,
        safety,
        no_system_recursion,
    )
}

/// Return `true` when any virtual method in `vmethods` is reachable as a
/// call target from `target_fn`.
///
/// The clang AST records a virtual dispatch (`p->foo()`,
/// `obj.foo()`) as a `called_function` whose `mangled_name` is the
/// statically-resolved method's symbol, so a target that never
/// dispatches virtually reaches none of the interface methods. In that
/// case the whole vtable-stub machinery (stub module, `_vtable`
/// globals, `interface_overrides.saw`, the pre-linked `code.combined.bc`
/// load path) is dead weight — worse, it can reference stub symbols the
/// target never needs and block the generated script from running. This
/// gate lets [`crate::gen_verify`] skip stub emission entirely for
/// non-virtual targets even when the translation unit incidentally
/// contains polymorphic types.
///
/// Emission is all-or-nothing: dropping *some* methods of a reachable
/// class would shift its remaining vtable slots and mis-resolve
/// dispatch, so once any interface method is reachable the caller keeps
/// the full method set.
pub fn any_interface_reachable(
    target_fn: &constraints::FunctionInfo,
    all_functions: &[constraints::FunctionInfo],
    vmethods: &[crate::clang_ast::InterfaceMethod],
) -> bool {
    if vmethods.is_empty() {
        return false;
    }
    let reached = reachable_call_targets(target_fn, all_functions);
    vmethods.iter().any(|m| {
        m.method
            .mangled_name
            .as_deref()
            .is_some_and(|mn| reached.contains(mn))
    })
}

/// Return the virtual methods for which vtable stubs should be emitted.
///
/// Keeps `all_vmethods` when the target reaches any virtual dispatch
/// (see [`any_interface_reachable`]); otherwise drops the lot and logs a
/// note, since emitting stubs for polymorphic types the target never
/// touches yields an unusable script (issue #57).
pub fn select_reachable_vmethods(
    target_fn: &constraints::FunctionInfo,
    all_functions: &[constraints::FunctionInfo],
    all_vmethods: Vec<crate::clang_ast::InterfaceMethod>,
    function: &str,
) -> Vec<crate::clang_ast::InterfaceMethod> {
    if any_interface_reachable(target_fn, all_functions, &all_vmethods) {
        return all_vmethods;
    }
    if !all_vmethods.is_empty() {
        eprintln!(
            "note: target '{function}' reaches no virtual dispatch; \
             skipping vtable-stub emission for {} polymorphic method(s) \
             in the translation unit",
            all_vmethods.len(),
        );
    }
    Vec::new()
}

/// Collect the set of call-target symbols (mangled names) reachable from
/// `target_fn` by descending through every in-module callee that has a
/// body.
///
/// Unlike [`collect_external_call_specs`] this ignores crucible-safety
/// gating — the goal is a *complete* reachability set so callers can
/// decide whether the target dispatches to any virtual method.
fn reachable_call_targets(
    target_fn: &constraints::FunctionInfo,
    all_functions: &[constraints::FunctionInfo],
) -> HashSet<String> {
    let fn_by_mangled: HashMap<&str, &constraints::FunctionInfo> = all_functions
        .iter()
        .filter_map(|f| f.mangled_name.as_deref().map(|m| (m, f)))
        .collect();
    let fn_by_name: HashMap<&str, &constraints::FunctionInfo> =
        all_functions.iter().map(|f| (f.name.as_str(), f)).collect();

    let mut visited: HashSet<String> = HashSet::new();
    let mut reached: HashSet<String> = HashSet::new();
    let mut worklist: Vec<&constraints::FunctionInfo> = vec![target_fn];
    while let Some(f) = worklist.pop() {
        let key = f.mangled_name.clone().unwrap_or_else(|| f.name.clone());
        if !visited.insert(key) {
            continue;
        }
        for c in &f.called_functions {
            if !c.mangled_name.is_empty() {
                reached.insert(c.mangled_name.clone());
            }
            if !c.name.is_empty() {
                reached.insert(c.name.clone());
            }
            let callee = fn_by_mangled
                .get(c.mangled_name.as_str())
                .or_else(|| fn_by_name.get(c.name.as_str()))
                .copied();
            if let Some(callee) = callee {
                worklist.push(callee);
            }
        }
    }
    reached
}

/// Transitive worklist walk from `target_fn`. Returns the set of
/// mangled-name strings that were observed as call targets anywhere
/// in the reachable subgraph (whether or not the walk descended into
/// them — system callees still need adversarial overrides emitted).
fn walk_callgraph<'f>(
    target_fn: &'f constraints::FunctionInfo,
    fn_by_mangled: &HashMap<String, &'f constraints::FunctionInfo>,
    fn_by_name: &HashMap<String, &'f constraints::FunctionInfo>,
    safety: &mut SafetyAnalyzer<'_>,
    no_system_recursion: bool,
) -> HashSet<String> {
    let mut visited: HashSet<String> = HashSet::new();
    let mut called_mangled: HashSet<String> = HashSet::new();
    let mut worklist: Vec<&constraints::FunctionInfo> = vec![target_fn];
    while let Some(f) = worklist.pop() {
        let key = f.mangled_name.clone().unwrap_or_else(|| f.name.clone());
        if !visited.insert(key) {
            continue;
        }
        for c in &f.called_functions {
            called_mangled.insert(c.mangled_name.clone());
            let callee = fn_by_mangled
                .get(&c.mangled_name)
                .or_else(|| fn_by_name.get(&c.name))
                .copied();
            if let Some(callee) = callee {
                if should_recurse(callee, safety, no_system_recursion) {
                    worklist.push(callee);
                }
            }
        }
    }
    called_mangled
}

fn filter_external_specs<'s, 'f>(
    all_specs: &'s [constraints::SpecConstraint],
    fn_by_mangled: &HashMap<String, &'f constraints::FunctionInfo>,
    fn_by_name: &HashMap<String, &'f constraints::FunctionInfo>,
    called_mangled: &HashSet<String>,
    safety: &mut SafetyAnalyzer<'_>,
    no_system_recursion: bool,
) -> Vec<&'s constraints::SpecConstraint> {
    all_specs
        .iter()
        .filter(|s| {
            let fn_info = s
                .mangled_name
                .as_ref()
                .and_then(|m| fn_by_mangled.get(m))
                .or_else(|| fn_by_name.get(&s.function_name))
                .copied();
            let is_treated_external = match fn_info {
                Some(f) => {
                    !f.has_body
                        || stl_override_matches(f)
                        || (f.is_system && !crucible_safe(f, safety, no_system_recursion))
                }
                None => !s.has_body || stl_overrides::matches(&s.function_name),
            };
            if !is_treated_external {
                return false;
            }
            // Skip virtual methods; they're handled by vtable stub system.
            if s.is_virtual {
                return false;
            }
            if let Some(f) = fn_info {
                if f.is_virtual {
                    return false;
                }
            }
            if s.function_name.starts_with("__builtin_") {
                return false;
            }
            if let Some(ref m) = s.mangled_name {
                if m.starts_with("__builtin_") {
                    return false;
                }
                called_mangled.contains(m)
            } else {
                false
            }
        })
        .collect()
}

/// Should the worklist descend into `callee`'s body?
///
/// Always recurse for non-system callees with a body. For system
/// callees (libstdc++ / libc++ / MSVC STL / CRT headers) recurse only
/// when the body passes [`SafetyAnalyzer`] — that allows header-only
/// STL templates like `std::max<u32>` (pure arithmetic + comparisons)
/// to be symbolically executed instead of havoced. The
/// `SAW_SPEC_GEN_NO_SYSTEM_RECURSION=1` kill-switch restores the
/// pre-`crucible_safety` behaviour for regression bisecting.
///
/// In addition, the `saw_spec_gen-jpp` STL functional-override
/// registry ([`crate::emit::saw_emit::stl_overrides`]) short-circuits
/// to "do not recurse" for any symbol it matches, regardless of
/// safety-analyzer verdict. This stops the walker from diving into
/// `std::vector` / `std::unique_ptr` / `std::basic_string` bodies
/// whose libstdc++ allocator helpers crash Crucible with
/// `Cannot mux LLVM values`.
pub fn should_recurse(
    callee: &constraints::FunctionInfo,
    safety: &mut SafetyAnalyzer<'_>,
    no_system: bool,
) -> bool {
    if !callee.has_body {
        return false;
    }
    if stl_override_matches(callee) {
        return false;
    }
    if !callee.is_system {
        return true;
    }
    crucible_safe(callee, safety, no_system)
}

/// Does `callee` have a Crucible-safe body? See [`should_recurse`].
pub fn crucible_safe(
    callee: &constraints::FunctionInfo,
    safety: &mut SafetyAnalyzer<'_>,
    no_system: bool,
) -> bool {
    if stl_override_matches(callee) {
        // The override registry trumps the safety analyzer — even a
        // syntactically Crucible-safe vector method must be treated
        // as external so the adversarial spec emitter havocs its
        // return value and `this`-pointer side effects.
        return false;
    }
    if no_system {
        return false;
    }
    let key = callee
        .mangled_name
        .as_deref()
        .unwrap_or(callee.name.as_str());
    safety.is_safe(key).safe
}

/// Convenience predicate over the STL override registry. Checks the
/// mangled name first (more specific) and falls back to the
/// demangled/pretty name (helps when the AST source only carries the
/// pretty form). See `saw_spec_gen-jpp`.
fn stl_override_matches(callee: &constraints::FunctionInfo) -> bool {
    if let Some(m) = callee.mangled_name.as_deref() {
        if stl_overrides::matches(m) {
            return true;
        }
    }
    stl_overrides::matches(callee.name.as_str())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clang_ast::InterfaceMethod;
    use crate::constraints::{CalledFunction, FunctionInfo, TypeInfo};

    fn func(name: &str, mangled: &str, calls: &[(&str, &str)]) -> FunctionInfo {
        FunctionInfo {
            name: name.into(),
            mangled_name: Some(mangled.into()),
            params: vec![],
            return_type: TypeInfo::Void,
            can_throw: false,
            is_virtual: false,
            has_body: true,
            is_system: false,
            annotations: vec![],
            referenced_globals: vec![],
            called_functions: calls
                .iter()
                .map(|(n, m)| CalledFunction {
                    name: (*n).into(),
                    mangled_name: (*m).into(),
                    has_body: false,
                })
                .collect(),
        }
    }

    fn vmethod(class: &str, name: &str, mangled: &str) -> InterfaceMethod {
        InterfaceMethod {
            class_name: class.into(),
            method: func(name, mangled, &[]),
            is_pure: false,
            is_override: false,
            source_offset: 0,
        }
    }

    #[test]
    fn reachable_when_target_dispatches_to_vmethod() {
        let target = func("activate", "?activate@@", &[("log", "?log@ILog@@")]);
        let all = vec![target.clone()];
        let vmethods = vec![vmethod("ILog", "log", "?log@ILog@@")];
        assert!(any_interface_reachable(&target, &all, &vmethods));
    }

    #[test]
    fn reachable_transitively_through_in_module_callee() {
        let target = func("activate", "?activate@@", &[("helper", "?helper@@")]);
        let helper = func("helper", "?helper@@", &[("log", "?log@ILog@@")]);
        let all = vec![target.clone(), helper];
        let vmethods = vec![vmethod("ILog", "log", "?log@ILog@@")];
        assert!(any_interface_reachable(&target, &all, &vmethods));
    }

    #[test]
    fn not_reachable_when_target_ignores_polymorphic_types() {
        // Issue #57: the TU has a virtual method but `activate` never
        // dispatches to it — no stubs should be emitted.
        let target = func("activate", "?activate@@", &[("strlen", "strlen")]);
        let all = vec![target.clone()];
        let vmethods = vec![vmethod("ILog", "log", "?log@ILog@@")];
        assert!(!any_interface_reachable(&target, &all, &vmethods));
    }

    #[test]
    fn empty_vmethods_is_not_reachable() {
        let target = func("activate", "?activate@@", &[]);
        let all = vec![target.clone()];
        assert!(!any_interface_reachable(&target, &all, &[]));
    }
}
