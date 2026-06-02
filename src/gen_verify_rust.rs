//! Implements the `gen-verify-rust` subcommand.
//!
//! Produces a SAW verification script + meta sidecar that lets
//! `verify-rust.ps1` prove a Rust function matches a hand-written
//! Cryptol spec. Replaces the inline `.ll`-regex + here-string
//! emission that previously lived in PowerShell.
//!
//! The C++ generator (`commands::gen_verify_cmd`) and this Rust
//! generator both go through [`crate::saw_emit::cryptol_bridge`] for
//! the Cryptol/LLVM type bridge so a `*_spec.cry` authored for one
//! runner type-checks under the other without surprise.

use anyhow::{anyhow, bail, Context, Result};
use serde_json::json;
use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

use crate::constraints::{FunctionInfo, TypeInfo};
use crate::parsers::llvm_ir::extract_functions;
use crate::saw_emit::cryptol_bridge::cryptol_arg_for;

/// Run the `gen-verify-rust` subcommand. Writes `verify_rust.saw`
/// and `verify_rust.meta.json` into `output`, and copies the Cryptol
/// spec next to them.
pub fn run(
    llvm_ir: &Path,
    bitcode: &Path,
    cryptol_spec: &Path,
    cryptol_fn: &str,
    function: &str,
    output: &Path,
) -> Result<()> {
    fs::create_dir_all(output)
        .with_context(|| format!("creating output dir {}", output.display()))?;

    let ir =
        fs::read_to_string(llvm_ir).with_context(|| format!("reading {}", llvm_ir.display()))?;

    let arts = resolve_target(&ir, function)?;

    let cry_name = cryptol_spec
        .file_name()
        .ok_or_else(|| anyhow!("--cryptol-spec has no file name"))?
        .to_string_lossy()
        .into_owned();
    let cry_dest = output.join(&cry_name);
    fs::copy(cryptol_spec, &cry_dest).with_context(|| {
        format!(
            "copying {} → {}",
            cryptol_spec.display(),
            cry_dest.display()
        )
    })?;

    let bc_name = bitcode
        .file_name()
        .ok_or_else(|| anyhow!("--bitcode has no file name"))?
        .to_string_lossy()
        .into_owned();

    let saw_path = output.join("verify_rust.saw");
    let saw_text = emit_saw_script(function, cryptol_fn, &bc_name, &cry_name, &arts);
    fs::write(&saw_path, saw_text).with_context(|| format!("writing {}", saw_path.display()))?;

    let meta_path = output.join("verify_rust.meta.json");
    let meta = json!({
        "schema_version": 1,
        "function": function,
        "cryptol_fn": cryptol_fn,
        "mangled_name": arts.mangled_name,
        "params": arts.params.iter().map(|p| json!({
            "index": p.index,
            "name": p.name,
            "bits": p.bits,
            "llvm_type": p.llvm_type,
        })).collect::<Vec<_>>(),
        "return_bits": arts.return_bits,
        "return_llvm_type": arts.return_llvm_type,
        "globals": arts.globals,
    });
    let mut meta_text = serde_json::to_string_pretty(&meta)?;
    meta_text.push('\n');
    fs::write(&meta_path, meta_text).with_context(|| format!("writing {}", meta_path.display()))?;

    // Friendly stdout summary mirroring the old PowerShell trace.
    println!("  → mangled symbol: {}", arts.mangled_name);
    let sig = arts
        .params
        .iter()
        .map(|p| p.llvm_type.clone())
        .collect::<Vec<_>>()
        .join(", ");
    println!("  → LLVM signature: ({sig})");
    println!("  → LLVM return type: {}", arts.return_llvm_type);
    if !arts.globals.is_empty() {
        println!("  → allocating {} mutable global(s):", arts.globals.len());
        for g in &arts.globals {
            println!("      {g}");
        }
    }
    println!("  → wrote {}", saw_path.display());
    println!("  → wrote {}", meta_path.display());
    Ok(())
}

/// A single integer parameter to verify (every supported Rust fn arg
/// today lowers to an `iN` at the LLVM boundary).
#[derive(Debug, Clone)]
pub struct RustVerifyParam {
    pub index: usize,
    /// Fresh-var name used in the SAW script (`x0`, `x1`, …). Stable
    /// across runs so the counterexample probe in `verify-rust.ps1`
    /// can correlate by index.
    pub name: String,
    pub bits: u32,
    pub llvm_type: String,
}

/// Everything the SAW + meta emitters need about the resolved target.
#[derive(Debug)]
pub struct RustVerifyArtifacts {
    pub mangled_name: String,
    pub params: Vec<RustVerifyParam>,
    pub return_bits: u32,
    pub return_llvm_type: String,
    pub globals: Vec<String>,
}

/// Walk the IR text and resolve the mangled symbol whose source name
/// is `function`, plus its full signature + the module's mutable
/// globals. Mirrors the heuristic that used to live in
/// `verify-rust.ps1` step 2:
///
/// 1. Pull every `define …` line via [`extract_functions`].
/// 2. Keep those whose mangled symbol ends with `<len><function>` —
///    the trailing length-prefixed segment of Rust v0 mangling.
/// 3. Keep those with an integer-only `(iN, …) -> iN` signature so
///    instantiated drop glue / vtable shims that the user's crate
///    happens to monomorphize get filtered out.
/// 4. Pick the shortest remaining symbol; bail on a tie.
fn resolve_target(ir: &str, function: &str) -> Result<RustVerifyArtifacts> {
    let funcs = extract_functions(ir, None).context("parsing LLVM IR functions")?;
    let suffix = format!("{}{}", function.len(), function);

    let name_matches: Vec<&FunctionInfo> = funcs
        .iter()
        .filter(|f| f.has_body && f.name.ends_with(&suffix))
        .collect();
    if name_matches.is_empty() {
        bail!(
            "Could not find a defined function whose mangled name ends with '{}'.\n\
             Make sure --llvm-ir points at the disassembled bitcode for the same\n\
             crate that defines `{}`.",
            suffix,
            function
        );
    }

    let mut int_matches: Vec<(&FunctionInfo, Vec<u32>, u32)> = Vec::new();
    for f in name_matches.iter().copied() {
        let Some(bits) = collect_int_bits(&f.params) else {
            continue;
        };
        let Some(ret) = type_int_bits(&f.return_type) else {
            continue;
        };
        int_matches.push((f, bits, ret));
    }
    if int_matches.is_empty() {
        let names = name_matches
            .iter()
            .map(|f| format!("  {}", f.name))
            .collect::<Vec<_>>()
            .join("\n");
        bail!(
            "Found symbol(s) ending in '{}' but none have an integer-only \
             (iN, ...) -> iN signature compatible with this verifier.\n\
             Candidates were:\n{}",
            suffix,
            names
        );
    }
    int_matches.sort_by_key(|(f, ..)| f.name.len());
    if int_matches.len() >= 2 && int_matches[0].0.name.len() == int_matches[1].0.name.len() {
        let shortest = int_matches[0].0.name.len();
        let tied = int_matches
            .iter()
            .take_while(|(f, ..)| f.name.len() == shortest)
            .map(|(f, ..)| format!("  {}", f.name))
            .collect::<Vec<_>>()
            .join("\n");
        bail!(
            "Ambiguous '{}': multiple matching symbols of equal length. \
             Qualify the function name (e.g. `mod::{}`) or rename one.\n\
             Tied candidates:\n{}",
            function,
            function,
            tied
        );
    }

    let (chosen, arg_bits, ret_bits) = int_matches.into_iter().next().unwrap();
    let params: Vec<RustVerifyParam> = arg_bits
        .iter()
        .enumerate()
        .map(|(i, &bits)| RustVerifyParam {
            index: i,
            name: format!("x{i}"),
            bits,
            llvm_type: format!("i{bits}"),
        })
        .collect();
    let globals = scan_globals(ir);
    Ok(RustVerifyArtifacts {
        mangled_name: chosen.name.clone(),
        params,
        return_bits: ret_bits,
        return_llvm_type: format!("i{ret_bits}"),
        globals,
    })
}

fn collect_int_bits(params: &[crate::constraints::ParamInfo]) -> Option<Vec<u32>> {
    params.iter().map(|p| type_int_bits(&p.ty)).collect()
}

/// Width in bits of an `iN`-shaped [`TypeInfo`], or `None` for any
/// other type (struct/ptr/array/void/etc.).
fn type_int_bits(t: &TypeInfo) -> Option<u32> {
    match t {
        TypeInfo::Bool => Some(1),
        TypeInfo::SignedInt(n) | TypeInfo::UnsignedInt(n) => Some(*n),
        _ => None,
    }
}

/// Scan the IR for mutable `global` *definitions* (skipping immutable
/// `constant` items, external decls, and MSVC `__imp_*` import
/// thunks). Returns names in source order, de-duplicated.
fn scan_globals(ir: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut seen = BTreeSet::new();
    for line in ir.lines() {
        let line = line.trim_start();
        if !line.starts_with('@') {
            continue;
        }
        let Some(eq_pos) = line.find('=') else {
            continue;
        };
        // The `@<name>` token: stops at whitespace or `=`. Preserve quoting.
        let head = line[1..eq_pos].trim();
        if head.is_empty() {
            continue;
        }
        let mut name = head.to_string();
        if name.starts_with('"') && name.ends_with('"') && name.len() >= 2 {
            name = name[1..name.len() - 1].to_string();
        }
        if name.contains("__imp_") {
            continue;
        }
        // What follows the `=` must be `(linkage-words…)? global ` —
        // not `constant`, not `alias`, not `=` continuation, etc.
        let tail = line[eq_pos + 1..].trim_start();
        if !contains_global_keyword(tail) {
            continue;
        }
        if seen.insert(name.clone()) {
            out.push(name);
        }
    }
    out
}

/// Return `true` if the first non-identifier token in `tail` is the
/// `global` keyword (modulo any leading linkage / attribute words).
fn contains_global_keyword(tail: &str) -> bool {
    for tok in tail.split_whitespace() {
        // Identifier-shaped tokens are linkage/attr words to skip.
        let is_word = tok.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
        if !is_word {
            return false;
        }
        if tok == "global" {
            return true;
        }
        if tok == "constant" || tok == "alias" || tok == "ifunc" {
            return false;
        }
        // else: linkage/visibility/attr (private, internal, dso_local,
        // unnamed_addr, …) → keep scanning.
    }
    false
}

fn emit_saw_script(
    function: &str,
    cryptol_fn: &str,
    bc_name: &str,
    cry_name: &str,
    arts: &RustVerifyArtifacts,
) -> String {
    let mut buf = String::new();
    buf.push_str(&format!(
        "// Auto-generated by `saw-spec-gen gen-verify-rust`.\n\
         // Prove that the Rust {function} (compiled to LLVM bitcode by rustc)\n\
         // is extensionally equal to the Cryptol spec {cryptol_fn}.\n\
         //\n\
         // Mangled symbol resolved from the .ll: {mangled}\n\n\
         m <- llvm_load_module \"{bc_name}\";\n\n\
         import \"{cry_name}\";\n\n",
        mangled = arts.mangled_name,
    ));
    buf.push_str(&format!("let {function}_equiv_spec = do {{\n"));
    for g in &arts.globals {
        buf.push_str(&format!(
            "    llvm_alloc_global \"{g}\";\n    llvm_points_to (llvm_global \"{g}\") (llvm_global_initializer \"{g}\");\n"
        ));
    }
    for p in &arts.params {
        buf.push_str(&format!(
            "    {name} <- llvm_fresh_var \"{name}\" (llvm_int {bits});\n",
            name = p.name,
            bits = p.bits,
        ));
    }
    let exec_args = arts
        .params
        .iter()
        .map(|p| format!("llvm_term {}", p.name))
        .collect::<Vec<_>>()
        .join(", ");
    buf.push_str(&format!("    llvm_execute_func [{exec_args}];\n"));
    let cry_call = if arts.params.is_empty() {
        cryptol_fn.to_string()
    } else {
        let args = arts
            .params
            .iter()
            .map(|p| cryptol_arg_for(&p.name, &bits_to_typeinfo(p.bits)))
            .collect::<Vec<_>>()
            .join(" ");
        format!("{cryptol_fn} {args}")
    };
    buf.push_str(&format!(
        "    llvm_return (llvm_term {{{{ {cry_call} }}}});\n"
    ));
    buf.push_str("};\n\n");
    buf.push_str(&format!(
        "llvm_verify m \"{}\" [] true {}_equiv_spec z3;\n\
         print \"VERIFIED\";\n",
        arts.mangled_name, function,
    ));
    buf
}

fn bits_to_typeinfo(bits: u32) -> TypeInfo {
    match bits {
        1 => TypeInfo::Bool,
        n => TypeInfo::UnsignedInt(n),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_simple_unary_function() {
        let ir = "\
define internal i32 @_RNvCs1234_8mycrate7add_one(i32 %x) unnamed_addr {
entry:
  ret i32 1
}
";
        let arts = resolve_target(ir, "add_one").unwrap();
        assert_eq!(arts.mangled_name, "_RNvCs1234_8mycrate7add_one");
        assert_eq!(arts.params.len(), 1);
        assert_eq!(arts.params[0].bits, 32);
        assert_eq!(arts.return_bits, 32);
        assert!(arts.globals.is_empty());
    }

    #[test]
    fn bool_arg_is_bridged_in_saw_script() {
        let ir = "\
define i32 @_RNvCs0_4test4bf32(i1 %b) {
entry:
  ret i32 0
}
";
        let arts = resolve_target(ir, "bf32").unwrap();
        assert_eq!(arts.params[0].bits, 1);
        let saw = emit_saw_script("bf32", "bf32_spec", "x.bc", "x.cry", &arts);
        assert!(saw.contains("(x0 ! 0)"), "missing Bit bridge:\n{saw}");
    }

    #[test]
    fn skips_drop_glue_via_signature_filter() {
        // A monomorphized drop_in_place::<add_one> shim would match
        // the `7add_one` suffix but have a ptr arg, which fails the
        // integer-only filter — leaving the user's fn as the unique
        // candidate.
        let ir = "\
define internal void @_RINvNtCs0_4core3ptr13drop_in_placeNvCs1_8mycrate7add_oneEB7_(ptr %0) {
entry:
  ret void
}
define internal i32 @_RNvCs1_8mycrate7add_one(i32 %x) {
entry:
  ret i32 1
}
";
        let arts = resolve_target(ir, "add_one").unwrap();
        assert_eq!(arts.mangled_name, "_RNvCs1_8mycrate7add_one");
    }

    #[test]
    fn picks_up_mutable_globals_only() {
        let ir = "\
@COUNTER = internal global i32 0
@MAX = internal constant i32 42
@__imp_foo = external global ptr
define i32 @_RNvCs0_2cc3get() {
entry:
  ret i32 0
}
";
        let arts = resolve_target(ir, "get").unwrap();
        assert_eq!(arts.globals, vec!["COUNTER".to_string()]);
    }
}
