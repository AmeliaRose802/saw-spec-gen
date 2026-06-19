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
    enabled: bool,
    cryptol_spec: &Path,
    cryptol_fn: &str,
    function: &str,
) {
    if !enabled {
        return;
    }
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
