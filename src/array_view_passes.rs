//! ArrayView pre-derive passes (saw_spec_gen-rng umbrella).
//!
//! Three small passes that mutate the `Vec<FunctionInfo>` produced
//! by [`crate::parsers::clang_ast`] before [`crate::constraints::
//! derive_constraints`] runs. Extracted from [`super::gen_verify::run`]
//! to keep that file under the project's 500-line limit.
//!
//! Order matters:
//!
//! 1. Struct-shape recognizer (rule 4, saw_spec_gen-26d): pairs
//!    adjacent `(T* buf, size_t len)` parameters and synthesizes
//!    `_In_reads_(len)`. Runs first so the binding pass can respect
//!    its output.
//! 2. Container-layout catalog (rule 5, saw_spec_gen-26d): loads
//!    the optional user TOML on top of the built-in defaults. The
//!    catalog is diagnostic-only for the scaffold; full lowering is
//!    tracked under the umbrella issue.
//! 3. Cryptol length binding (rule 1, saw_spec_gen-4po): parses the
//!    target function's Cryptol signature binders + predicate
//!    context and injects `_In_reads_(MAX)` for each `[n][T]` /
//!    `[K][T]` parameter that has no existing size annotation.

use crate::constraints::container_layouts::ContainerCatalog;
use crate::constraints::container_layouts_derive::derive_catalog_from_structs;
use crate::constraints::{length_binding, struct_shape_recognizer, FunctionInfo, TypeInfo};
use crate::parsers::cryptol_poly_sig;
use anyhow::{bail, Result};
use std::collections::HashMap;
use std::path::Path;

/// Apply the struct-shape recognizer to every function unless the
/// caller opted out. Logs one stderr line per parameter that
/// received a synthetic annotation.
pub(crate) fn apply_struct_shape_recognizer(all_functions: &mut [FunctionInfo], disabled: bool) {
    if disabled {
        return;
    }
    for fi in all_functions.iter_mut() {
        let added = struct_shape_recognizer::recognize_and_annotate(fi);
        for buf_name in &added {
            eprintln!(
                "info[saw-spec-gen]: struct-shape recognizer: function \
                 `{}` parameter `{}` paired with a sibling length \
                 parameter via the `(T* buf, size_t len)` pattern; \
                 use --no-struct-shape-recognizer to disable.",
                fi.name, buf_name,
            );
        }
    }
}

/// Build the container-layout catalog by merging three sources, in
/// order of increasing priority:
///
///   1. Built-in defaults ([`ContainerCatalog::with_defaults`]).
///   2. AST-derived layouts for every record the clang AST describes
///      that matches the `{data_ptr, size, [capacity]}` shape. This
///      is the saw_spec_gen-26d / 530 auto-derive path and obviates
///      the user-supplied TOML for stdlib and most project-local
///      container types. Skipped when `ast_structs` is empty.
///   3. The user TOML file (if `path` is `Some`).
///
/// The catalog is consumed by the functional STL override emitter
/// (saw_spec_gen-i47 / qms) which uses it to confirm a recognized
/// container shape before emitting a points-to-edit override.
pub(crate) fn load_container_catalog(
    path: Option<&Path>,
    ast_structs: &HashMap<String, Vec<(String, TypeInfo)>>,
) -> ContainerCatalog {
    let mut c = ContainerCatalog::with_defaults();
    if !ast_structs.is_empty() {
        let derived = derive_catalog_from_structs(ast_structs);
        let n = derived.len();
        c.extend_from(derived);
        if n > 0 {
            eprintln!(
                "info[saw-spec-gen]: container-layout auto-derive added {n} \
                 layout(s) from the clang AST (saw_spec_gen-26d)."
            );
        }
    }
    if let Some(p) = path {
        match c.load_toml_file(p) {
            Ok(n) => eprintln!(
                "info[saw-spec-gen]: loaded {n} container layout(s) from {}",
                p.display()
            ),
            Err(e) => eprintln!(
                "warning[saw-spec-gen]: failed to load --container-layouts {}: {e}",
                p.display()
            ),
        }
        eprintln!(
            "warning[saw-spec-gen]: --container-layouts is increasingly \
             redundant — the AST auto-derive (saw_spec_gen-26d/530) now \
             covers stdlib + most project-local container shapes. The \
             TOML surface itself is scheduled for deletion under \
             saw_spec_gen-0nf."
        );
    }
    c
}

/// Apply the Cryptol length binding (rule 1) to the target function.
/// No-op when `enabled` is false or when the Cryptol signature
/// cannot be parsed.
pub(crate) fn apply_cryptol_length_binding(
    all_functions: &mut [FunctionInfo],
    cryptol_spec: &Path,
    cryptol_fn: &str,
    function: &str,
) {
    let poly_sig = match cryptol_poly_sig::parse_poly_signature(cryptol_spec, cryptol_fn) {
        Some(s) => s,
        None => {
            eprintln!(
                "warning[saw-spec-gen]: --bind-cryptol-lengths: could not parse \
                 Cryptol signature for `{cryptol_fn}`; falling back to the \
                 untyped derivation path."
            );
            return;
        }
    };
    let fi = match all_functions.iter_mut().find(|f| f.name == function) {
        Some(f) => f,
        None => {
            eprintln!(
                "warning[saw-spec-gen]: --bind-cryptol-lengths: target \
                 function `{function}` not found in AST; nothing to bind."
            );
            return;
        }
    };
    let bindings = length_binding::bind_lengths(&poly_sig, fi);
    if bindings.is_empty() {
        eprintln!(
            "warning[saw-spec-gen]: --bind-cryptol-lengths: no bindings \
             for `{function}` (Cryptol arity {} vs C/Rust arity {}); \
             leaving annotations unchanged.",
            poly_sig.params.len(),
            fi.params.len(),
        );
        return;
    }
    let warnings = length_binding::apply_to_function(fi, &bindings);
    eprintln!(
        "info[saw-spec-gen]: --bind-cryptol-lengths: injected \
         {} length binding(s) for `{function}` from Cryptol \
         signature `{cryptol_fn}`.",
        bindings.len()
    );
    for w in &warnings {
        eprintln!("{w}");
    }
}

/// Infer `max_len_precond` entries from the Cryptol signature's upper
/// bounds for struct-shape-recognized `(buf, len)` pairs, and merge
/// them into `overrides`. User-supplied preconditions win: a length
/// parameter already named in `overrides.max_len_preconds` is left
/// untouched. Runs after [`apply_struct_shape_recognizer`] and
/// [`apply_cryptol_length_binding`] so the synthetic `InReadsParam`
/// annotations are present.
pub(crate) fn apply_inferred_len_preconds(
    overrides: &mut crate::buffer_overrides::BufferOverrides,
    all_functions: &[FunctionInfo],
    cryptol_spec: &Path,
    cryptol_fn: &str,
    function: &str,
) {
    let Some(sig) = cryptol_poly_sig::parse_poly_signature(cryptol_spec, cryptol_fn) else {
        return;
    };
    let Some(fi) = all_functions.iter().find(|f| f.name == function) else {
        return;
    };
    let existing: std::collections::HashSet<String> = overrides
        .max_len_preconds
        .iter()
        .map(|(n, _)| n.clone())
        .collect();
    let mut to_add: Vec<(String, u64)> = Vec::new();
    for (name, k) in length_binding::infer_len_preconds(&sig, fi) {
        if existing.contains(&name) || to_add.iter().any(|(n, _)| *n == name) {
            continue;
        }
        eprintln!(
            "info[saw-spec-gen]: inferred `llvm_precond {{{{ {name} <= {k} }}}}` for \
             `{function}` from the Cryptol upper bound on `{cryptol_fn}` \
             (no --max-len-precond needed)."
        );
        to_add.push((name, k));
    }
    overrides.max_len_preconds.extend(to_add);
}

/// Post-derive pass: for every `AllocMutable` parameter in the target
/// function's `SpecConstraint` that has an `_Out_writes_(N)` annotation
/// AND a matching `<param>_post` function in the Cryptol spec file,
/// populate `param.out_postcond` with the Cryptol function name.
///
/// This eliminates the need for `--out-buffer-param` and
/// `--cryptol-fn-out` CLI flags when the source uses SAL annotations
/// and follows the `<param>_post` naming convention.
pub(crate) fn apply_out_postcond_autodetect(
    target_spec: &mut crate::constraints::SpecConstraint,
    target_fn: &FunctionInfo,
    cryptol_spec: &Path,
) {
    use crate::constraints::types::{AllocType, Annotation};
    let cry_text = match std::fs::read_to_string(cryptol_spec) {
        Ok(t) => t,
        Err(_) => return,
    };
    for (i, param) in target_spec.params.iter_mut().enumerate() {
        if param.alloc_type != AllocType::AllocMutable || param.out_postcond.is_some() {
            continue;
        }
        let has_out_writes = target_fn
            .params
            .get(i)
            .map(|p| {
                p.annotations
                    .iter()
                    .any(|a| matches!(a, Annotation::OutWrites(_) | Annotation::OutWritesParam(_)))
            })
            .unwrap_or(false);
        if !has_out_writes {
            continue;
        }
        let post_fn = format!("{}_post", param.name);
        if cryptol_fn_exists(&cry_text, &post_fn) {
            param.out_postcond = Some(post_fn);
        }
    }
}

/// Infer a non-static C++ method receiver as the logical state parameter.
///
/// Mutable receivers default to `this=auto`, so the generated spec uses
/// `this_pre` and can wire post-state Cryptol functions without an explicit
/// `--out-buffer-param this=...`. Const receivers stay read-only unless the
/// user explicitly overrides them with `--out-buffer-param this=...`.
pub(crate) fn apply_receiver_state_inference(
    target_spec: &mut crate::constraints::SpecConstraint,
    target_fn: &FunctionInfo,
    overrides: &mut crate::buffer_overrides::BufferOverrides,
) -> Result<()> {
    use crate::constraints::{AllocType, Mutability};

    if overrides.is_out_buffer("this") || overrides.has_in_buffer_size("this") {
        return Ok(());
    }
    let Some(receiver) = target_fn.params.first().filter(|p| p.name == "this") else {
        if let Some(post_fn) = overrides.cryptol_fn_out.get("this") {
            bail!(
                "receiver inference for `{}` could not bind `this` to `{post_fn}`: \
                 target has no implicit receiver parameter. Use an explicit \
                 --out-buffer-param NAME=... / --cryptol-fn-out NAME=... mapping instead.",
                target_fn.name
            );
        }
        return Ok(());
    };
    let Some(receiver_spec) = target_spec.params.first() else {
        return Ok(());
    };
    if receiver_spec.alloc_type != AllocType::AllocMutable
        || receiver.mutability == Mutability::Readonly
    {
        if let Some(post_fn) = overrides.cryptol_fn_out.get("this") {
            bail!(
                "receiver inference for `{}` refused to make `this` writable for `{post_fn}`: \
                 the method receiver is read-only. Pass an explicit \
                 --out-buffer-param this=... only if you intentionally want to override that.",
                target_fn.name
            );
        }
        return Ok(());
    }
    overrides.out_buffer_auto.insert("this".into());
    eprintln!(
        "info[saw-spec-gen]: inferred mutable receiver `this` as the state parameter for `{}` \
         (no --out-buffer-param this=auto needed).",
        target_fn.name
    );
    Ok(())
}

/// Return true if `fn_name :` appears as a top-level definition in `text`.
fn cryptol_fn_exists(text: &str, fn_name: &str) -> bool {
    let prefix = format!("{fn_name} :");
    let prefix_ns = format!("{fn_name}:");
    text.lines().any(|l| {
        let t = l.trim_start();
        t.starts_with(prefix.as_str()) || t.starts_with(prefix_ns.as_str())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::buffer_overrides::BufferOverrides;
    use crate::constraints::{
        AllocType, Nullability, ParamConstraint, ParamInfo, ReturnConstraint, SpecConstraint,
    };

    fn receiver_target(
        mutability: crate::constraints::Mutability,
    ) -> (FunctionInfo, SpecConstraint) {
        let target_fn = FunctionInfo {
            name: "activate".into(),
            mangled_name: Some("?activate@KeyStore@@QEAAHXZ".into()),
            params: vec![ParamInfo {
                name: "this".into(),
                ty: TypeInfo::Pointer(Box::new(TypeInfo::Opaque {
                    name: "class.KeyStore".into(),
                    size_bytes: 16,
                })),
                mutability,
                nullable: Nullability::NonNull,
                annotations: vec![],
            }],
            return_type: TypeInfo::SignedInt(32),
            can_throw: false,
            is_virtual: false,
            has_body: true,
            is_system: false,
            annotations: vec![],
            referenced_globals: vec![],
            called_functions: vec![],
        };
        let target_spec = SpecConstraint {
            function_name: "activate".into(),
            mangled_name: target_fn.mangled_name.clone(),
            params: vec![ParamConstraint {
                name: "this".into(),
                alloc_type: AllocType::AllocMutable,
                saw_type: "llvm_alias \"class.KeyStore\"".into(),
                preconditions: vec![],
                unchanged_after: false,
                dereferenceable_size: None,
                out_postcond: None,
            }],
            return_constraint: ReturnConstraint {
                saw_type: "llvm_int 32".into(),
                value_constraints: vec![],
                is_sret: false,
                returns_pointer: false,
                sret_prestate: false,
            },
            can_throw: false,
            is_virtual: false,
            has_body: true,
            postconditions: vec![],
            referenced_globals: vec![],
        };
        (target_fn, target_spec)
    }

    #[test]
    fn infers_mutable_receiver_as_auto_out_buffer() {
        let (target_fn, mut target_spec) = receiver_target(crate::constraints::Mutability::Mutable);
        let mut overrides =
            BufferOverrides::from_cli(&[], &[], &["this=activate_post".into()], &[], &[], &[])
                .unwrap();

        apply_receiver_state_inference(&mut target_spec, &target_fn, &mut overrides).unwrap();

        assert!(overrides.out_buffer_auto.contains("this"));
        assert_eq!(
            overrides.cryptol_fn_out.get("this").map(String::as_str),
            Some("activate_post")
        );
    }

    #[test]
    fn const_receiver_stays_readonly_without_override() {
        let (target_fn, mut target_spec) =
            receiver_target(crate::constraints::Mutability::Readonly);
        let mut overrides = BufferOverrides::default();

        apply_receiver_state_inference(&mut target_spec, &target_fn, &mut overrides).unwrap();

        assert!(!overrides.out_buffer_auto.contains("this"));
    }

    #[test]
    fn const_receiver_poststate_binding_requires_explicit_override() {
        let (target_fn, mut target_spec) =
            receiver_target(crate::constraints::Mutability::Readonly);
        let mut overrides =
            BufferOverrides::from_cli(&[], &[], &["this=activate_post".into()], &[], &[], &[])
                .unwrap();

        let err = apply_receiver_state_inference(&mut target_spec, &target_fn, &mut overrides)
            .unwrap_err();

        assert!(format!("{err:#}").contains("read-only"));
        assert!(format!("{err:#}").contains("--out-buffer-param this=..."));
    }

    #[test]
    fn this_poststate_binding_requires_member_receiver() {
        let target_fn = FunctionInfo {
            name: "activate".into(),
            mangled_name: Some("_Z8activatePi".into()),
            params: vec![ParamInfo {
                name: "state".into(),
                ty: TypeInfo::Pointer(Box::new(TypeInfo::SignedInt(32))),
                mutability: crate::constraints::Mutability::Mutable,
                nullable: Nullability::Nullable,
                annotations: vec![],
            }],
            return_type: TypeInfo::SignedInt(32),
            can_throw: false,
            is_virtual: false,
            has_body: true,
            is_system: false,
            annotations: vec![],
            referenced_globals: vec![],
            called_functions: vec![],
        };
        let mut target_spec = SpecConstraint {
            function_name: "activate".into(),
            mangled_name: target_fn.mangled_name.clone(),
            params: vec![ParamConstraint {
                name: "state".into(),
                alloc_type: AllocType::AllocMutable,
                saw_type: "llvm_int 32".into(),
                preconditions: vec![],
                unchanged_after: false,
                dereferenceable_size: None,
                out_postcond: None,
            }],
            return_constraint: ReturnConstraint {
                saw_type: "llvm_int 32".into(),
                value_constraints: vec![],
                is_sret: false,
                returns_pointer: false,
                sret_prestate: false,
            },
            can_throw: false,
            is_virtual: false,
            has_body: true,
            postconditions: vec![],
            referenced_globals: vec![],
        };
        let mut overrides =
            BufferOverrides::from_cli(&[], &[], &["this=activate_post".into()], &[], &[], &[])
                .unwrap();

        let err = apply_receiver_state_inference(&mut target_spec, &target_fn, &mut overrides)
            .unwrap_err();

        assert!(format!("{err:#}").contains("implicit receiver parameter"));
    }
}
