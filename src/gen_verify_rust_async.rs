//! Async fn verification support for `gen-verify-rust --async`.
//!
//! When `--async` is set, `gen-verify-rust` targets the coroutine
//! **resume function** (mangled `_RNC…`) rather than the constructor shim.
//! The generated SAW script uses `mir_verify` (crucible-mir), which has
//! native coroutine / `TyCoroutine` support.
//!
//! ## How rustc lowers `async fn foo(x: u32) -> u32`
//!
//! ```text
//! _RNvCs…crate3foo        ← constructor: allocates coroutine, returns it
//! _RNCNvCs…crate3foo0B3_  ← resume: executes the state machine (our target)
//! ```
//!
//! The constructor proves nothing about the body — it just builds the future.
//! The resume function is what we verify.
//!
//! ## Nested coroutines (`_RNCNC…`)
//!
//! An `async { … }` block inside an `async fn` produces a doubly-nested
//! closure. We pick the least-nested resume (fewest `NC` pairs) so that
//! `--function foo` always resolves to the direct coroutine body for `foo`,
//! not to an inner sub-future.

use crate::gen_verify_rust_emit::RustVerifyParam;
use crate::parsers::llvm_ir::extract_functions;
use anyhow::{bail, Context, Result};

/// Information about the resolved async coroutine resume function.
#[derive(Debug)]
pub struct AsyncResumeInfo {
    /// Mangled name of the resume function (e.g. `_RNCNvCs…crate3foo0B3_`).
    pub resume_symbol: String,
    /// Mangled names of callee functions detected as sub-future `.await` polls.
    /// Each needs a `mir_unsafe_assume_spec` stub in the generated script.
    pub sub_await_stubs: Vec<String>,
    /// Parameters from the original (constructor) function — used to build
    /// the Cryptol bridge on the caller side.
    pub original_params: Vec<RustVerifyParam>,
    /// Total return bits from the original function (for meta.json).
    pub return_bits: u32,
}

/// Resolve the coroutine resume symbol for `function`.
///
/// Searches the LLVM IR for `_RNC`-prefixed functions whose mangled path
/// contains `<len><function>`. Among all matching closures, picks the
/// least-nested one (fewest consecutive `NC` pairs).
///
/// `original_params` and `return_bits` come from the constructor candidate
/// (resolved via the normal `resolve_target` path) and are carried through
/// so the Cryptol bridge uses the right integer widths.
pub fn resolve_resume_symbol(
    ir: &str,
    function: &str,
    original_params: Vec<RustVerifyParam>,
    return_bits: u32,
) -> Result<AsyncResumeInfo> {
    let funcs = extract_functions(ir, None).context("parsing LLVM IR for async resume target")?;
    let needle = format!("{}{}", function.len(), function);

    let candidates: Vec<_> = funcs
        .iter()
        .filter(|f| f.has_body)
        .filter(|f| f.name.starts_with("_RNC"))
        .filter(|f| f.name.contains(&needle))
        .collect();

    if candidates.is_empty() {
        bail!(
            "No coroutine resume symbol found for `{fn}` (looked for `_RNC`-prefixed \
             symbols containing `{needle}`).\n\
             Make sure:\n  \
             1. The function is declared `async fn`.\n  \
             2. `--llvm-ir` points at the disassembled `.bc` for the same crate.\n  \
             3. The crate was compiled with debug info / no LTO stripping.",
            fn = function,
        );
    }

    // Sort by (nc_depth, symbol_len) so we pick the immediate resume, not a
    // deeply nested sub-future closure.
    let nc_depth = |name: &str| -> usize {
        let bytes = name.as_bytes();
        let mut count = 0usize;
        let mut i = 0;
        while i + 1 < bytes.len() {
            if bytes[i] == b'N' && bytes[i + 1] == b'C' {
                count += 1;
                i += 2;
            } else {
                i += 1;
            }
        }
        count
    };

    let mut sorted = candidates;
    sorted.sort_by_key(|f| (nc_depth(&f.name), f.name.len()));
    let resume = sorted[0];

    // Detect sub-await stubs: other _RNC symbols called by the resume body.
    // These are the inner poll callees that need `mir_unsafe_assume_spec` stubs.
    let sub_await_stubs = detect_sub_await_stubs(ir, &resume.name);

    Ok(AsyncResumeInfo {
        resume_symbol: resume.name.clone(),
        sub_await_stubs,
        original_params,
        return_bits,
    })
}

/// Scan the IR for calls inside `resume_symbol` to other `_RNC`-prefixed
/// functions — these correspond to `.await` points that poll sub-futures.
fn detect_sub_await_stubs(ir: &str, resume_symbol: &str) -> Vec<String> {
    // Simple line-by-line scan: collect function bodies, then find calls.
    let mut in_target = false;
    let mut stubs = Vec::new();
    let mut seen = std::collections::BTreeSet::new();

    for line in ir.lines() {
        let t = line.trim();
        // Detect start of the resume function body.
        if t.starts_with("define ") && t.contains(&format!("@{resume_symbol}")) {
            in_target = true;
            continue;
        }
        // End of function body.
        if in_target && t == "}" {
            in_target = false;
            continue;
        }
        if !in_target {
            continue;
        }
        // Look for `call` / `invoke` lines referencing another _RNC symbol.
        if !t.contains("@_RNC") {
            continue;
        }
        if let Some(callee) = extract_call_target(t) {
            if callee != resume_symbol && seen.insert(callee.clone()) {
                stubs.push(callee);
            }
        }
    }
    stubs
}

/// Pull the callee symbol name out of a `call … @symbol(` line.
fn extract_call_target(line: &str) -> Option<String> {
    let at = line.find("@_RNC")?;
    let rest = &line[at + 1..];
    let end = rest.find(|c: char| !c.is_alphanumeric() && c != '_' && c != '$' && c != '.')?;
    Some(rest[..end].to_string())
}

/// Emit the `mir_verify` SAWScript for async fn verification.
///
/// The generated script:
/// 1. Loads the MIR module (`.linked-mir.json`).
/// 2. Optionally emits `mir_unsafe_assume_spec` stubs for `.await` sub-polls.
/// 3. Declares a spec that:
///    a. Creates fresh variables for the original function arguments.
///    b. Allocates the coroutine struct at the initial discriminant (0).
///    c. Allocates a `Context` (just an address for leaf async fns).
///    d. Calls `mir_execute_func [self_ref, cx_ref]`.
///    e. Asserts `Poll::Ready(model(args))` on return.
/// 4. Invokes `mir_verify`.
pub fn emit_async_mir_saw_script(
    function: &str,
    cryptol_fn: &str,
    mir_module: &str,
    cry_name: &str,
    info: &AsyncResumeInfo,
) -> String {
    let mut buf = String::new();

    // Header
    buf.push_str(&format!(
        "// Auto-generated by `saw-spec-gen gen-verify-rust --async`.\n\
         // Verifies that the async fn `{function}` (resume body) matches\n\
         // the Cryptol spec `{cryptol_fn}`.\n\
         //\n\
         // Resume symbol  : {resume}\n\
         //\n\
         // Run with `mir_load_module` using the `.linked-mir.json` produced\n\
         // by `cargo-saw-build` or `mir-json --link-mir`.\n\n\
         m <- mir_load_module \"{mir_module}\";\n\n\
         import \"{cry_name}\";\n\n",
        resume = info.resume_symbol,
    ));

    // .await stubs (if any sub-polls detected)
    if !info.sub_await_stubs.is_empty() {
        buf.push_str("// mir_unsafe_assume_spec stubs for .await sub-poll callees.\n");
        for stub in &info.sub_await_stubs {
            let safe = sanitize_ident(stub);
            buf.push_str(&format!(
                "let {safe}_stub_spec = do {{\n\
                 \x20   // TODO: tighten — currently havocs all state.\n\
                 \x20   mir_execute_func [];\n\
                 }};\n\
                 {safe}_ov <- mir_unsafe_assume_spec m \"{stub}\" {safe}_stub_spec;\n\n",
            ));
        }
    }

    // Spec body
    buf.push_str(&format!("let {function}_async_spec = do {{\n"));

    // Fresh variables for the original function's captured arguments
    let mut cry_args: Vec<String> = Vec::new();
    for p in &info.original_params {
        let mir_ty = bits_to_mir_type(p.bits);
        buf.push_str(&format!(
            "    {name} <- mir_fresh_var \"{name}\" ({mir_ty});\n",
            name = p.name,
        ));
        cry_args.push(p.name.clone());
    }
    if !info.original_params.is_empty() {
        buf.push('\n');
    }

    // Coroutine struct allocation
    buf.push_str(&format!(
        "    // Allocate the coroutine struct at the initial discriminant\n\
         \x20   // (0 = not-yet-started state).\n\
         \x20   // NOTE: Replace the ADT path with the fully-qualified name from\n\
         \x20   //       `mir_find_adt m` — e.g. `\"my_crate::decide::{{coroutine#0}}\"`.\n\
         \x20   self_ref <- mir_alloc (mir_find_adt m \"{function}::{{coroutine#0}}\");\n\n"
    ));

    // Context allocation
    buf.push_str(
        "    // Allocate a waker Context — only its address is used by a leaf\n\
         \x20   // async fn that never drives a sub-future's poll.\n\
         \x20   cx_ref <- mir_alloc (mir_find_adt m \"core::task::wake::Context\");\n\n",
    );

    // execute_func — Pin<&mut Coroutine> is transparent at MIR level
    if info.sub_await_stubs.is_empty() {
        buf.push_str(
            "    // Pin<&mut Coroutine> is erased at MIR level; pass self_ref directly.\n\
             \x20   mir_execute_func [self_ref, cx_ref];\n\n",
        );
    } else {
        let ov_names: Vec<_> = info
            .sub_await_stubs
            .iter()
            .map(|s| format!("{}_ov", sanitize_ident(s)))
            .collect();
        buf.push_str(&format!(
            "    // Pin<&mut Coroutine> is erased at MIR level; pass self_ref directly.\n\
             \x20   // Sub-poll overrides: {ovs}\n\
             \x20   mir_execute_func [self_ref, cx_ref];\n\n",
            ovs = ov_names.join(", "),
        ));
    }

    // Return: Poll::Ready(cryptol_result)
    let cry_call = build_cryptol_call(cryptol_fn, &cry_args);
    buf.push_str(&format!(
        "    // Assert Poll::Ready(result) == model(args).\n\
         \x20   // `Ready` is the Cryptol constructor for Poll::Ready (discriminant 0).\n\
         \x20   // If your spec returns T directly instead of Poll<T>, remove `Ready`.\n\
         \x20   mir_return (mir_term {{{{ Ready ({cry_call}) }}}});\n"
    ));

    buf.push_str("};\n\n");

    // Override list for mir_verify
    let ov_list = if info.sub_await_stubs.is_empty() {
        "[]".to_string()
    } else {
        let names: Vec<_> = info
            .sub_await_stubs
            .iter()
            .map(|s| format!("{}_ov", sanitize_ident(s)))
            .collect();
        format!("[{}]", names.join(", "))
    };

    // Proof invocation
    buf.push_str(&format!(
        "print \"BEGIN_PROOF {function}\";\n\
         mir_verify m \"{resume}\" {ov_list} false {function}_async_spec z3;\n\
         print \"PROVED {function}\";\n\
         print \"VERIFIED\";\n",
        resume = info.resume_symbol,
    ));

    buf
}

/// Map LLVM integer bit width to a MIR type expression in SAWScript.
fn bits_to_mir_type(bits: u32) -> String {
    match bits {
        1 => "mir_bool".to_string(),
        8 => "mir_u8".to_string(),
        16 => "mir_u16".to_string(),
        32 => "mir_u32".to_string(),
        64 => "mir_u64".to_string(),
        128 => "mir_u128".to_string(),
        n => format!("mir_u{n}"),
    }
}

/// Build a Cryptol function application string, e.g. `foo_spec x0 x1`.
fn build_cryptol_call(cryptol_fn: &str, args: &[String]) -> String {
    if args.is_empty() {
        return cryptol_fn.to_string();
    }
    format!("{cryptol_fn} {}", args.join(" "))
}

/// Convert a mangled symbol to a safe SAWScript identifier by replacing
/// non-alphanumeric characters with underscores.
fn sanitize_ident(mangled: &str) -> String {
    let s: String = mangled
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    // Trim leading underscores that could make the ident look private.
    s.trim_start_matches('_').to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gen_verify_rust_emit::RustVerifyParam;

    fn make_params(bits: &[u32]) -> Vec<RustVerifyParam> {
        bits.iter()
            .enumerate()
            .map(|(i, &b)| RustVerifyParam {
                index: i,
                name: format!("x{i}"),
                bits: b,
                llvm_type: format!("i{b}"),
                range: None,
            })
            .collect()
    }

    // ──────────────────────────────────────────────────────────────────────
    // resolve_resume_symbol tests
    // ──────────────────────────────────────────────────────────────────────

    #[test]
    fn finds_resume_symbol_for_async_fn() {
        let ir = "\
define i64 @_RNvCs1234_6decide6decide(i32 %req) unnamed_addr {
entry:
  ret i64 0
}
define void @_RNCNvCs1234_6decide6decide0Bc_(ptr %self, ptr %cx) unnamed_addr {
entry:
  ret void
}
";
        let params = make_params(&[32]);
        let info = resolve_resume_symbol(ir, "decide", params, 32).unwrap();
        assert_eq!(info.resume_symbol, "_RNCNvCs1234_6decide6decide0Bc_");
    }

    #[test]
    fn skips_constructor_shim() {
        let ir = "\
define i32 @_RNvCs0_4test7add_one(i32 %x) {
entry:
  ret i32 0
}
define void @_RNCNvCs0_4test7add_one0B3_(ptr %s, ptr %c) {
entry:
  ret void
}
";
        // resolve_resume_symbol should find the _RNC symbol, not the constructor.
        let info = resolve_resume_symbol(ir, "add_one", make_params(&[32]), 32).unwrap();
        assert!(
            info.resume_symbol.starts_with("_RNC"),
            "expected _RNC prefix, got: {}",
            info.resume_symbol
        );
    }

    #[test]
    fn error_when_no_resume_symbol() {
        let ir = "\
define i32 @_RNvCs0_4test7add_one(i32 %x) {
entry:
  ret i32 0
}
";
        let err = resolve_resume_symbol(ir, "add_one", make_params(&[32]), 32);
        assert!(err.is_err(), "expected error for missing resume symbol");
        let msg = format!("{:#}", err.unwrap_err());
        assert!(msg.contains("No coroutine resume symbol"), "got: {msg}");
    }

    #[test]
    fn picks_least_nested_resume() {
        // _RNCNv... has 1 NC pair; _RNCNCNv... has 2 NC pairs.
        // Should pick the shallower one.
        let ir = "\
define void @_RNCNvCs0_4test6decide0B3_(ptr %s, ptr %c) {
entry:
  ret void
}
define void @_RNCNCNvCs0_4test6decide0B5_(ptr %s, ptr %c) {
entry:
  ret void
}
";
        let info = resolve_resume_symbol(ir, "decide", make_params(&[32]), 32).unwrap();
        assert_eq!(info.resume_symbol, "_RNCNvCs0_4test6decide0B3_");
    }

    #[test]
    fn detects_sub_await_stubs() {
        // The resume function calls another _RNC symbol → sub-await stub.
        let ir = "\
define void @_RNCNvCs0_4test3foo0B3_(ptr %s, ptr %c) {
entry:
  call void @_RNCNvCs0_4test3bar0B3_(ptr %s, ptr %c)
  ret void
}
define void @_RNCNvCs0_4test3bar0B3_(ptr %s, ptr %c) {
entry:
  ret void
}
";
        let info = resolve_resume_symbol(ir, "foo", make_params(&[]), 0).unwrap();
        assert_eq!(info.sub_await_stubs, vec!["_RNCNvCs0_4test3bar0B3_"]);
    }

    // ──────────────────────────────────────────────────────────────────────
    // emit_async_mir_saw_script tests
    // ──────────────────────────────────────────────────────────────────────

    fn simple_info(_fn_name: &str, resume: &str) -> AsyncResumeInfo {
        AsyncResumeInfo {
            resume_symbol: resume.to_string(),
            sub_await_stubs: vec![],
            original_params: make_params(&[32]),
            return_bits: 32,
        }
    }

    #[test]
    fn emits_mir_load_module() {
        let info = simple_info("decide", "_RNCNvCs0_6decide6decide0Bc_");
        let saw = emit_async_mir_saw_script(
            "decide",
            "decide_spec",
            "decide.linked-mir.json",
            "decide.cry",
            &info,
        );
        assert!(saw.contains("mir_load_module \"decide.linked-mir.json\""));
    }

    #[test]
    fn emits_mir_verify_with_resume_symbol() {
        let info = simple_info("decide", "_RNCNvCs0_6decide6decide0Bc_");
        let saw = emit_async_mir_saw_script(
            "decide",
            "decide_spec",
            "decide.linked-mir.json",
            "decide.cry",
            &info,
        );
        assert!(
            saw.contains("mir_verify m \"_RNCNvCs0_6decide6decide0Bc_\""),
            "missing mir_verify with resume symbol:\n{saw}"
        );
    }

    #[test]
    fn emits_begin_proof_before_mir_verify() {
        let info = simple_info("decide", "_RNCNvCs0_6decide6decide0Bc_");
        let saw = emit_async_mir_saw_script("decide", "decide_spec", "m.json", "s.cry", &info);
        let begin_idx = saw.find("BEGIN_PROOF decide").unwrap();
        let verify_idx = saw.find("mir_verify").unwrap();
        assert!(
            begin_idx < verify_idx,
            "BEGIN_PROOF must come before mir_verify"
        );
    }

    #[test]
    fn emits_proved_and_verified() {
        let info = simple_info("decide", "_RNCNvCs0_6decide6decide0Bc_");
        let saw = emit_async_mir_saw_script("decide", "decide_spec", "m.json", "s.cry", &info);
        assert!(saw.contains("PROVED decide"));
        assert!(saw.contains("VERIFIED"));
    }

    #[test]
    fn emits_fresh_var_for_each_arg() {
        let mut info = simple_info("decide", "_RNCNvCs0_6decide6decide0Bc_");
        info.original_params = make_params(&[32, 8]);
        let saw = emit_async_mir_saw_script("decide", "spec", "m.json", "s.cry", &info);
        assert!(saw.contains("mir_fresh_var \"x0\" (mir_u32)"));
        assert!(saw.contains("mir_fresh_var \"x1\" (mir_u8)"));
    }

    #[test]
    fn emits_coroutine_alloc_comment() {
        let info = simple_info("decide", "_RNCNvCs0_6decide6decide0Bc_");
        let saw = emit_async_mir_saw_script("decide", "spec", "m.json", "s.cry", &info);
        assert!(
            saw.contains("coroutine#0"),
            "missing coroutine#0 ADT placeholder:\n{saw}"
        );
    }

    #[test]
    fn emits_ready_wrapper_in_return() {
        let info = simple_info("decide", "_RNCNvCs0_6decide6decide0Bc_");
        let saw = emit_async_mir_saw_script("decide", "decide_spec", "m.json", "s.cry", &info);
        assert!(
            saw.contains("Ready (decide_spec x0)"),
            "missing Poll::Ready wrapper:\n{saw}"
        );
    }

    #[test]
    fn emits_sub_await_stubs_when_present() {
        let info = AsyncResumeInfo {
            resume_symbol: "_RNCNvCs0_4test3foo0B3_".to_string(),
            sub_await_stubs: vec!["_RNCNvCs0_4test3bar0B3_".to_string()],
            original_params: make_params(&[32]),
            return_bits: 32,
        };
        let saw = emit_async_mir_saw_script("foo", "foo_spec", "m.json", "s.cry", &info);
        assert!(
            saw.contains("mir_unsafe_assume_spec"),
            "missing stub:\n{saw}"
        );
        assert!(
            saw.contains("RNCNvCs0_4test3bar0B3_"),
            "missing stub symbol:\n{saw}"
        );
    }

    #[test]
    fn sanitize_ident_replaces_non_alnum() {
        assert_eq!(sanitize_ident("_RNCNvCs0_foo"), "RNCNvCs0_foo");
        assert_eq!(sanitize_ident("foo::bar"), "foo__bar");
    }

    #[test]
    fn bits_to_mir_type_maps_known_widths() {
        assert_eq!(bits_to_mir_type(1), "mir_bool");
        assert_eq!(bits_to_mir_type(8), "mir_u8");
        assert_eq!(bits_to_mir_type(32), "mir_u32");
        assert_eq!(bits_to_mir_type(64), "mir_u64");
    }
}
