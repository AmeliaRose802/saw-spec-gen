//! Emit `llvm_unsafe_assume_spec` overrides for bitcode symbols that
//! [`crate::transform::extern_override_scan`] flagged as untractable.
//!
//! The emission contract is intentionally narrow:
//!   * one signature-based override per broken symbol,
//!   * SAW spec parameters cover only the **fixed** (non-vararg) prefix
//!     of the IR signature — empirically (see
//!     `tests/e2e/cases/08-overrides/bump/out_bump_verified/verify_probe3.saw`)
//!     SAW's override matcher accepts variadic call sites against a
//!     fixed-prefix spec, while including the vararg arguments fails
//!     with "Fresh variable(s) not reachable via points-tos",
//!   * the spec body havocs the return value — nothing reads it
//!     symbolically, the override exists purely to short-circuit the
//!     broken body,
//!   * each pointer parameter is adversarially clobbered in the post
//!     state via `llvm_points_to_at_type p (llvm_int 8) (llvm_term
//!     p_after)`. Without this SAW treats the pointee memory as
//!     preserved, producing false positives for callees that write
//!     through pointer args (`sprintf`, custom logging functions,
//!     out-parameter helpers). One byte is the widest universally-safe
//!     clobber: wider writes (`i32`, `i64`) fail with "Memory store
//!     failed" at call sites that pass a pointer to a narrower stack
//!     slot, and the opaque-ptr IR signature gives no way to discover
//!     the real pointee width.
//!   * every mutable global the IR declares **that the target's body
//!     transitively stores to** gets adversarially clobbered. The
//!     scanner ([`extern_override_scan::scan`]) walks the target's
//!     visible body closure and unions direct `store ..., ptr @G`
//!     instructions per [`OverrideTarget::globals_written`]. For
//!     `DeclareOnly` externs the body is invisible so the set is
//!     empty — we assume libc-style externs do not mutate user
//!     globals (the previous behaviour of clobbering every global on
//!     every extern produced false DISPROVED verdicts whenever any
//!     reachable function touched `printf`, see
//!     `tests/e2e/cases/02-havoc-coverage/concrete_type_safe/`).
//!     The variadic-body must-havoc case is still covered, see
//!     `tests/e2e/cases/08-overrides/variadic_global_clobber/`.
//!
//! Output shape per target:
//! ```text
//! // override: my_log  [variadic; body uses llvm.va_start]
//! let ov_my_log_spec = do {
//!     p0 <- llvm_fresh_pointer (llvm_int 8);
//!     p1 <- llvm_fresh_pointer (llvm_int 8);
//!     llvm_execute_func [p0, p1];
//!     p0_after <- llvm_fresh_var "p0_after" (llvm_int 8);
//!     llvm_points_to_at_type p0 (llvm_int 8) (llvm_term p0_after);
//!     p1_after <- llvm_fresh_var "p1_after" (llvm_int 8);
//!     llvm_points_to_at_type p1 (llvm_int 8) (llvm_term p1_after);
//!     rv <- llvm_fresh_var "rv" (llvm_int 32);
//!     llvm_return (llvm_term rv);
//! };
//! ov_my_log <- llvm_unsafe_assume_spec m "my_log" ov_my_log_spec;
//! ```

use crate::constraints::container_layouts::ContainerCatalog;
use crate::constraints::{GlobalVarInfo, TypeInfo};
use crate::parsers::llvm_ir::struct_defs;
use crate::transform::extern_override_scan::{self, BrokenReason, OverrideTarget};

use super::bitcode_overrides_functional::{
    emit_shared_ghost_decl, needs_vector_ghost, try_emit_functional, FunctionalLayouts,
};
use super::names::sanitize_name;

/// Result of running the emitter: a SAWScript snippet to splice into
/// `verify.saw`, plus the list of `ov_*` names the caller should append
/// to the `llvm_verify` override list.
pub struct EmittedBitcodeOverrides {
    pub snippet: String,
    pub override_names: Vec<String>,
}

impl EmittedBitcodeOverrides {
    pub fn is_empty(&self) -> bool {
        self.override_names.is_empty()
    }

    /// Convenience constructor for "we did not run the scan" / "the
    /// scan found nothing".
    pub fn empty() -> Self {
        Self {
            snippet: String::new(),
            override_names: Vec::new(),
        }
    }
}

/// End-to-end orchestrator used by `gen_verify`: read the LLVM IR file
/// (if any), scan it for unloadable callees reachable from
/// `target_symbol`, and emit overrides for everything not already
/// covered by an AST-derived spec. All file I/O failures degrade
/// gracefully to an empty result with a stderr warning — extern
/// overrides are best-effort coverage, never required for correctness
/// of the generator itself.
pub fn scan_and_emit(
    llvm_ir_path: Option<&std::path::Path>,
    target_symbol: &str,
    already_covered: &[String],
    all_globals: &[GlobalVarInfo],
    container_catalog: &ContainerCatalog,
) -> EmittedBitcodeOverrides {
    let Some(path) = llvm_ir_path else {
        return EmittedBitcodeOverrides::empty();
    };
    let ir_text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("warning: could not re-read LLVM IR for extern override scan: {e}");
            return EmittedBitcodeOverrides::empty();
        }
    };
    let targets = extern_override_scan::scan(&ir_text, target_symbol);
    // Restrict global-clobber to globals that the IR actually declares
    // as `global` (not `constant`) and whose width we can express as an
    // `llvm_int N` term. Each `OverrideTarget` carries its own
    // `globals_written` set (computed by `extern_override_scan::scan`):
    //   - DeclareOnly targets conservatively list all externally-visible
    //     mutable globals (any opaque callee could write them).
    //   - Defined bodies (UsesVarargsIntrinsic) list exactly the globals
    //     their transitive call-chain stores to.
    // `emit_one` filters `mutable_globals` against that set.
    let mg = extern_override_scan::scan_mutable_globals(&ir_text);
    let mutable_globals: Vec<GlobalVarInfo> = all_globals
        .iter()
        .filter(|g| mg.all.contains(g.mangled_name.as_str()))
        .filter(|g| global_width_bits(&g.ty).is_some())
        .cloned()
        .collect();
    // Pre-discover container layouts so the functional STL emitter can
    // dispatch on canonical method names. The discovery is gated by
    // the AST-derived `ContainerCatalog` (saw_spec_gen-qms): we only
    // emit a functional override for a container whose shape the
    // catalog has independently confirmed from the clang AST. This
    // means the catalog — not ad-hoc IR-string matching — is the
    // source of truth for which containers we model.
    let struct_table = struct_defs(&ir_text);
    let layouts = FunctionalLayouts::discover(&struct_table, container_catalog);
    let emitted = emit_overrides(&targets, already_covered, &mutable_globals, &layouts);
    if !emitted.is_empty() {
        eprintln!(
            "Bitcode override scan: emitting {} extern override(s)",
            emitted.override_names.len(),
        );
    }
    emitted
}

/// Emit overrides for `targets`, skipping any whose symbol is already
/// covered by an AST-derived override (whose name we get from
/// `already_covered`). The skip-set avoids duplicate `ov_NAME` bindings
/// which SAW rejects at script parse time. `layouts` carries the
/// discovered per-container layouts (currently only basic_string);
/// recognized STL methods get a functional override instead of the
/// default adversarial-havoc spec.
pub fn emit_overrides(
    targets: &[OverrideTarget],
    already_covered: &[String],
    mutable_globals: &[GlobalVarInfo],
    layouts: &FunctionalLayouts,
) -> EmittedBitcodeOverrides {
    let mut out = String::new();
    let mut names = Vec::new();
    let mut seen_unsafe_ids: std::collections::HashSet<String> = std::collections::HashSet::new();

    if targets.is_empty() {
        return EmittedBitcodeOverrides {
            snippet: out,
            override_names: names,
        };
    }

    out.push_str(
        "// Bitcode-derived extern overrides (saw-spec-gen extern_override_scan)\n\
         // These cover symbols the clang-AST path filter dropped (system\n\
         // headers) and variadic functions whose body uses llvm.va_*\n\
         // intrinsics that Crucible-LLVM cannot symbolically execute.\n",
    );

    // Vector functional overrides share a single SAW ghost variable.
    // It must be declared exactly once, before any spec that
    // references it. Pre-scan the target list and emit the
    // declaration at the top of the snippet when needed.
    if needs_vector_ghost(targets.iter().map(|t| t.symbol.as_str())) {
        emit_shared_ghost_decl(&mut out);
    }

    for t in targets {
        if already_covered.iter().any(|n| n == &t.symbol) {
            continue;
        }
        let safe = sanitize_name(&t.symbol);
        let ov_name = format!("ov_{safe}");
        if !seen_unsafe_ids.insert(ov_name.clone()) {
            continue;
        }
        // STL functional path: regardless of the BrokenReason
        // (DeclareOnly for libstdc++ template headers, StlOverride
        // for user-instantiated STL bodies, UsesVarargsIntrinsic for
        // some I/O wrappers), if the mangled name matches a curated
        // functional model, emit a points-to-edit / ghost-coupled
        // spec rather than the default havoc. The two-state coupling
        // (e.g. `resize(n); size() == n`, `push_back(v); back() ==
        // v`) is what flips the `gap_disproved` cases. The
        // classifier is conservative — anything it doesn't
        // recognize falls through to `emit_one` below.
        if try_emit_functional(&mut out, &t.symbol, &safe, &ov_name, layouts) {
            names.push(ov_name);
            continue;
        }
        emit_one(&mut out, t, &safe, &ov_name, mutable_globals);
        names.push(ov_name);
    }

    EmittedBitcodeOverrides {
        snippet: out,
        override_names: names,
    }
}

fn emit_one(
    out: &mut String,
    t: &OverrideTarget,
    safe: &str,
    ov_name: &str,
    mutable_globals: &[GlobalVarInfo],
) {
    // Filter the module-wide mutable-global list down to just the
    // globals in this target's `globals_written` set. For DeclareOnly
    // targets this is all externally-visible mutable globals (opaque
    // callees could write any of them). For UsesVarargsIntrinsic
    // targets it's the precise union of direct stores from the
    // visible body closure.
    let globals_to_clobber: Vec<&GlobalVarInfo> = mutable_globals
        .iter()
        .filter(|g| t.globals_written.iter().any(|w| w == &g.mangled_name))
        .collect();
    let reason_tag = match t.reason {
        BrokenReason::DeclareOnly => "declare-only",
        BrokenReason::UsesVarargsIntrinsic => "body uses llvm.va_*",
        BrokenReason::StlOverride => "stl-override",
    };
    let variadic_tag = if t.is_variadic { "; variadic" } else { "" };
    out.push_str(&format!(
        "\n// override: {sym}  [{reason}{variadic}]\n",
        sym = t.symbol,
        reason = reason_tag,
        variadic = variadic_tag,
    ));
    out.push_str(&format!("let {safe}_spec = do {{\n"));

    // Per-parameter fresh setup. We also remember which params are
    // pointers so we can emit an adversarial post-state clobber after
    // `llvm_execute_func` — without it, SAW treats pointee memory as
    // preserved across the call, which produces false positives for
    // callees that write through their pointer arguments (e.g. a
    // variadic `log(uint32_t* out, const char* fmt, ...)` that stashes
    // a value into `*out`). See
    // `tests/e2e/cases/08-overrides/variadic_clobber/add_one_disproved.cpp`
    // for the regression: pre-fix that test falsely VERIFIED, post-fix
    // it DISPROVES on a counterexample with `p_after` = some non-1 byte.
    let mut arg_refs: Vec<String> = Vec::new();
    let mut ptr_param_vars: Vec<String> = Vec::new();
    for (i, ir_ty) in t.fixed_param_ir_types.iter().enumerate() {
        let varname = format!("p{i}");
        match ir_param_setup(ir_ty, &varname) {
            ParamSetup::Pointer => {
                out.push_str(&format!(
                    "    {varname} <- llvm_fresh_pointer (llvm_int 8);\n"
                ));
                arg_refs.push(varname.clone());
                ptr_param_vars.push(varname);
            }
            ParamSetup::Int(bits) => {
                out.push_str(&format!(
                    "    {varname} <- llvm_fresh_var \"{varname}\" (llvm_int {bits});\n"
                ));
                arg_refs.push(format!("llvm_term {varname}"));
            }
            ParamSetup::Unsupported => {
                // Fall back to an opaque byte pointer + warn. Better to
                // produce *some* override than to skip the symbol and
                // leave the caller broken.
                eprintln!(
                    "warning: bitcode override for `{sym}` param #{i}: \
                     IR type `{ir_ty}` is not a primitive int / ptr — \
                     substituting opaque ptr. SAW may reject this \
                     override if the real call site has a richer type.",
                    sym = t.symbol,
                );
                out.push_str(&format!(
                    "    {varname} <- llvm_fresh_pointer (llvm_int 8);\n"
                ));
                arg_refs.push(varname.clone());
                ptr_param_vars.push(varname);
            }
        }
    }

    out.push_str(&format!(
        "    llvm_execute_func [{}];\n",
        arg_refs.join(", "),
    ));

    // Adversarial post-state: for each pointer parameter, claim that
    // the callee may have written one symbolic byte through it. We use
    // `llvm_int 8` (one byte) deliberately — wider claims (e.g. i32 or
    // i64) fail with "Memory store failed" at call sites that pass a
    // pointer to a narrower stack slot, and SAW gives us no way to
    // discover the pointee width from the opaque-ptr IR signature. One
    // byte of havoc is sufficient to invalidate any caller that relies
    // on a specific pointee value, and it is always alignment-safe.
    if !ptr_param_vars.is_empty() {
        for var in &ptr_param_vars {
            out.push_str(&format!(
                "    {var}_after <- llvm_fresh_var \"{var}_after\" (llvm_int 8);\n",
            ));
            out.push_str(&format!(
                "    llvm_points_to_at_type {var} (llvm_int 8) (llvm_term {var}_after);\n",
            ));
        }
    }

    // Adversarial post-state: clobber every mutable global the IR
    // declares **that this target's body actually stores to** (per
    // `OverrideTarget::globals_written`, computed in
    // `extern_override_scan::scan`). We mirror the pre-state
    // allocation that `emit_equiv_spec_body` does for
    // `target_spec.referenced_globals`, and the AST-side
    // `emit_global_havoc` pattern in `havoc.rs`.
    //
    // Two regressions guard this granularity:
    //   * `tests/e2e/cases/08-overrides/variadic_global_clobber/`
    //     covers the must-havoc direction: a `log_inc` variadic body
    //     that writes `g_counter++` must clobber `g_counter` in the
    //     override, otherwise the override preserves the pre-call
    //     value and the caller falsely VERIFIES.
    //   * `tests/e2e/cases/02-havoc-coverage/concrete_type_safe/
    //     add_one_verified.cpp` covers the must-NOT-havoc direction:
    //     `printf` (whose MSVC body uses `llvm.va_*` and so lands in
    //     the override set on Windows) cannot write user globals,
    //     so havocing `super_important` here would falsely DISPROVE
    //     a function that only reads the global.
    for g in globals_to_clobber {
        let Some(bits) = global_width_bits(&g.ty) else {
            continue;
        };
        let post = format!("{}_after", sanitize_name(&g.name));
        out.push_str(&format!(
            "    {post} <- llvm_fresh_var \"{post}\" (llvm_int {bits});\n",
        ));
        out.push_str(&format!(
            "    llvm_points_to (llvm_global \"{}\") (llvm_term {post});\n",
            g.mangled_name,
        ));
    }

    // Return slot.
    match ir_return_setup(&t.return_ir_type) {
        ReturnSetup::Void => {
            // No `llvm_return`.
        }
        ReturnSetup::Int(bits) => {
            // Known threading status primitives (`_Mtx_lock`/`_Mtx_unlock`
            // and friends) return `_Thrd_result`, whose success value
            // `_Thrd_success` is `0`. A fresh-symbolic return lets the
            // solver pick a failure code, which sends `_Mutex_base::lock`
            // down its `_Throw_Cpp_error` → LLVM `unreachable` path and
            // fails the subgoal. Pin the success sentinel so the
            // lock/unlock pair is transparent to a lock-guarded body.
            if let Some(sentinel) = super::status_primitives::success_sentinel(&t.symbol) {
                out.push_str(&format!(
                    "    // status primitive: pin success sentinel \
                     ({sentinel} = _Thrd_success) instead of a fresh return\n"
                ));
                out.push_str(&format!(
                    "    llvm_return (llvm_term {{{{ {sentinel} : [{bits}] }}}});\n"
                ));
            } else {
                out.push_str(&format!(
                    "    rv <- llvm_fresh_var \"rv\" (llvm_int {bits});\n"
                ));
                out.push_str("    llvm_return (llvm_term rv);\n");
            }
        }
        ReturnSetup::ByteArray(n) => {
            out.push_str(&format!(
                "    rv <- llvm_fresh_var \"rv\" (llvm_array {n} (llvm_int 8));\n"
            ));
            out.push_str("    llvm_return (llvm_term rv);\n");
        }
        ReturnSetup::Pointer => {
            out.push_str("    rv <- llvm_fresh_pointer (llvm_int 8);\n");
            out.push_str("    llvm_return rv;\n");
        }
        ReturnSetup::Unsupported => {
            eprintln!(
                "warning: bitcode override for `{sym}`: return IR type `{ret}` \
                 is not a primitive int / ptr / void — emitting opaque ptr return. \
                 SAW may reject this override; supply a hand-written spec to fix.",
                sym = t.symbol,
                ret = t.return_ir_type,
            );
            out.push_str("    rv <- llvm_fresh_pointer (llvm_int 8);\n");
            out.push_str("    llvm_return rv;\n");
        }
    }

    out.push_str("};\n");
    out.push_str(&format!(
        "{ov_name} <- llvm_unsafe_assume_spec m \"{sym}\" {safe}_spec;\n",
        sym = t.symbol,
    ));
}

enum ParamSetup {
    Pointer,
    Int(u32),
    Unsupported,
}

enum ReturnSetup {
    Void,
    Int(u32),
    ByteArray(u32),
    Pointer,
    Unsupported,
}

fn ir_param_setup(ir_ty: &str, _var: &str) -> ParamSetup {
    if ir_ty == "ptr" {
        return ParamSetup::Pointer;
    }
    if let Some(bits_str) = ir_ty.strip_prefix('i') {
        if let Ok(bits) = bits_str.parse::<u32>() {
            return ParamSetup::Int(bits);
        }
    }
    ParamSetup::Unsupported
}

/// Width in bits to use for an `llvm_fresh_var ... (llvm_int N)` term
/// when clobbering a mutable global in an extern-override post-state.
/// Returns `None` for shapes we don't know how to express as a single
/// scalar (pointers, structs, byte arrays etc.) — those globals are
/// silently skipped rather than risk emitting a malformed override.
fn global_width_bits(ty: &TypeInfo) -> Option<u32> {
    match ty {
        TypeInfo::SignedInt(w) | TypeInfo::UnsignedInt(w) => Some(*w),
        TypeInfo::Bool => Some(1),
        _ => None,
    }
}

fn ir_return_setup(ir_ty: &str) -> ReturnSetup {
    if ir_ty == "void" {
        return ReturnSetup::Void;
    }
    if ir_ty == "ptr" {
        return ReturnSetup::Pointer;
    }
    if let Some(bits_str) = ir_ty.strip_prefix('i') {
        if let Ok(bits) = bits_str.parse::<u32>() {
            return ReturnSetup::Int(bits);
        }
    }
    // Handle `[N x i8]` — MSVC sometimes returns small aggregates in this form.
    // Only `i8` element arrays are handled: MSVC's ABI expresses opaque
    // small-struct returns as byte arrays (`[N x i8]`) which SAW accepts via
    // `llvm_array N (llvm_int 8)`.  Other element types (e.g. `[4 x i32]`)
    // are not covered because their SAW representation differs and an incorrect
    // override would silently pass the spec while masking real type mismatches;
    // hand-written specs should cover those cases.
    if let Some(inner) = ir_ty.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
        if let Some((n_str, elem)) = inner.split_once(" x ") {
            if elem.trim() == "i8" {
                if let Ok(n) = n_str.trim().parse::<u32>() {
                    return ReturnSetup::ByteArray(n);
                }
            }
        }
    }
    ReturnSetup::Unsupported
}

#[cfg(test)]
#[path = "bitcode_overrides_tests.rs"]
mod tests;
