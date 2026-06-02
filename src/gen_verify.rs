//! Driver for the `gen-verify` subcommand.
//!
//! Runs the full C++ → SAW verification pipeline:
//!   1. Parse clang AST (one or more files, merged into a synthetic TU)
//!   2. Derive target function + global / call-graph info
//!   3. Emit adversarial specs for externals, virtual methods, and
//!      direct in-module callees (compositional verification)
//!   4. Emit the top-level `verify.saw` script that wires everything together

use crate::alias_fallbacks::{apply_cli_overrides, dump_fallback_diagnostics};
use crate::spec_rewrite::{apply_alias_rewrites, collect_type_sizes};
use crate::type_resolve::resolve_spec_types_quiet;
use crate::{alias_fallbacks_ir, clang_ast, constraints, llvm_ir, saw_emit};
use anyhow::Result;
use std::collections::{HashMap, HashSet};
use std::path::Path;

#[allow(clippy::too_many_arguments)]
pub fn run(
    ast: &[std::path::PathBuf],
    bitcode: &Path,
    llvm_ir_path: Option<&Path>,
    cryptol_spec: &Path,
    cryptol_fn: &str,
    function: &str,
    output: &Path,
    alias_size_overrides: &[String],
    alias_enum_overrides: &[String],
    use_llvm_combine_modules: bool,
) -> Result<()> {
    if ast.is_empty() {
        anyhow::bail!("At least one --ast file is required");
    }
    let parsed_ast = if ast.len() == 1 {
        eprintln!("Reading clang AST from: {}", ast[0].display());
        clang_ast::parse_ast(&ast[0])?
    } else {
        eprintln!("Reading and merging {} clang ASTs:", ast.len());
        let mut parsed = Vec::with_capacity(ast.len());
        for p in ast {
            eprintln!("  - {}", p.display());
            parsed.push(clang_ast::parse_ast(p)?);
        }
        clang_ast::merge_asts(parsed)
    };
    let all_functions = clang_ast::extract_functions(&parsed_ast, None)?;
    eprintln!("Found {} functions", all_functions.len());

    // Optional LLVM IR: struct-size table + per-param `dereferenceable(N)`.
    // MSVC-clang fully qualifies struct symbols, so without the IR we can't
    // match short C++ names against `%\"struct.Foo::Bar::Baz\"`.
    let (ir_struct_sizes, ir_funcs) = llvm_ir::load_optional(llvm_ir_path)?;

    // Sniff the bitcode's `target triple = "..."` line so vtable stub
    // generation can pick the correct ABI layout. Without this, an
    // Itanium-compiled binary (Linux / macOS) would get MSVC-shaped
    // stubs that never line up with the compiler's `_ZTV<C> + 16`
    // pointer arithmetic in virtual constructors, and every dispatch
    // would fall through to the unmodified real method body — defeating
    // havoc-based verification.
    let target_triple = llvm_ir_path.and_then(llvm_ir::read_target_triple);

    // Find the target function (FunctionInfo with call graph)
    let target_fn = all_functions
        .iter()
        .filter(|f| f.name == function)
        .min_by_key(|f| if f.is_virtual { 1 } else { 0 })
        .ok_or_else(|| anyhow::anyhow!("Function '{}' not found in AST", function))?
        .clone();

    // Find the target function's spec
    let target_spec = {
        let specs = constraints::derive_constraints(&all_functions)?;
        let matches: Vec<_> = specs
            .into_iter()
            .filter(|s| s.function_name == function)
            .collect();
        matches
            .into_iter()
            .min_by_key(|s| if s.is_virtual { 1 } else { 0 })
    };
    let mut target_spec =
        target_spec.ok_or_else(|| anyhow::anyhow!("Function '{}' not found in AST", function))?;
    let target_mangled = target_spec
        .mangled_name
        .clone()
        .ok_or_else(|| anyhow::anyhow!("No mangled name for '{}'", function))?;

    // Resolve any opaque `llvm_alias` parameter / return types against the
    // LLVM IR struct table.  Without this, MSVC-clang output (which fully
    // qualifies struct names like `%"struct.Foo::Bar::Baz"`) fails to load
    // because SAW can't find a `struct.Baz` symbol.
    crate::type_resolve::resolve_spec_aliases(
        &mut target_spec,
        &ir_struct_sizes,
        llvm_ir_path.is_some(),
    );

    // If the LLVM IR contains exception-lower globals (@__exclow_error_*),
    // inject them into the target spec so that SAW allocates them.
    // (Deferred until after `all_globals` is built below — see the call
    // to `inject_exclow_globals` further down.)

    let mut all_globals = clang_ast::extract_all_globals(&parsed_ast)?;

    // Augment with mutable globals discovered in the LLVM IR that the
    // clang AST parser missed (function-local statics, compiler-
    // generated globals, etc.).  Without this, SAW aborts with
    // "Global symbol not allocated" when symbolically executing a body
    // that touches an IR-only global.
    if let Some(ir_path) = llvm_ir_path {
        if let Ok(ir_text) = std::fs::read_to_string(ir_path) {
            let extra =
                crate::transform::ir_globals::discover_ir_only_globals(&ir_text, &all_globals);
            if !extra.is_empty() {
                eprintln!(
                    "  discovered {} IR-only mutable global(s) not in clang AST",
                    extra.len(),
                );
                all_globals.extend(extra);
            }
        }
    }

    // Inject the exception-lower bookkeeping globals (@__exclow_error_*)
    // with the right TypeInfo and pre-state init values. Must run after
    // the AST + IR scans so the explicit `init_value: Some("0")` for the
    // error flag isn't shadowed by a duplicate entry from
    // `discover_ir_only_globals` (which would have `init_value: None`
    // because it can't parse the LLVM `false` literal).
    if let Some(ir_path) = llvm_ir_path {
        crate::transform::eh_globals::inject_exclow_globals(&mut all_globals, ir_path);
    }

    let all_specs = constraints::derive_constraints(&all_functions)?;

    // Warn about interfaces referenced by fields but missing from the merged
    // AST.  These cause `extract_virtual_methods` to miss the interface,
    // which in turn causes gen-verify to skip vtable stub generation and
    // emit a spec that fails to verify (the indirect calls have no overrides).
    let missing = clang_ast::detect_missing_interfaces(&parsed_ast);
    if !missing.is_empty() {
        eprintln!(
            "warning: {} interface(s) referenced by class fields but missing from AST(s):",
            missing.len(),
        );
        for m in &missing {
            eprintln!(
                "  - {}::{} : {}<{}> (interface AST not provided)",
                m.owning_class, m.field_name, m.wrapper, m.interface_name,
            );
        }
        eprintln!("  hint: pass additional --ast files containing each missing interface so");
        eprintln!("        gen-verify can synthesize vtable stubs for their virtual methods.");
    }

    std::fs::create_dir_all(output)?;

    // Walk the call graph transitively to find ALL external calls reachable
    // through any in-module function. Necessary because when SAW executes a
    // real method body, that method's external calls (e.g. printf) also need
    // overrides.
    let fn_by_mangled: HashMap<String, &constraints::FunctionInfo> = all_functions
        .iter()
        .filter_map(|f| f.mangled_name.as_ref().map(|m| (m.clone(), f)))
        .collect();
    let fn_by_name: HashMap<String, &constraints::FunctionInfo> =
        all_functions.iter().map(|f| (f.name.clone(), f)).collect();

    let mut visited: HashSet<String> = HashSet::new();
    let mut called_mangled: HashSet<String> = HashSet::new();
    let mut worklist: Vec<&constraints::FunctionInfo> = vec![&target_fn];
    while let Some(f) = worklist.pop() {
        let key = f.mangled_name.clone().unwrap_or_else(|| f.name.clone());
        if !visited.insert(key) {
            continue;
        }
        for c in &f.called_functions {
            called_mangled.insert(c.mangled_name.clone());
            // Recurse only if callee has a body AND isn't a system function.
            // System functions (stdio/ucrt/etc.) have inline bodies that rely
            // on LLVM intrinsics SAW can't resolve, so we treat them as
            // external instead.
            let callee = fn_by_mangled
                .get(&c.mangled_name)
                .or_else(|| fn_by_name.get(&c.name))
                .copied();
            if let Some(callee) = callee {
                if callee.has_body && !callee.is_system {
                    worklist.push(callee);
                }
            }
        }
    }

    // Find which called functions are treated as external. Skip clang
    // `__builtin_*` intrinsics — they aren't real bitcode symbols. Skip
    // virtuals — they're handled via vtable stubs.
    let external_calls: Vec<&constraints::SpecConstraint> = all_specs
        .iter()
        .filter(|s| {
            let fn_info = s
                .mangled_name
                .as_ref()
                .and_then(|m| fn_by_mangled.get(m))
                .or_else(|| fn_by_name.get(&s.function_name))
                .copied();
            let is_treated_external = match fn_info {
                Some(f) => !f.has_body || f.is_system,
                None => !s.has_body,
            };
            if !is_treated_external {
                return false;
            }
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
        .collect();

    // Extract interface info first so external spec generation can detect
    // functions whose return type is an interface pointer.
    let vmethods = clang_ast::extract_virtual_methods(&parsed_ast, None)?;
    let has_interfaces = !vmethods.is_empty();
    let interface_classes: HashSet<String> =
        vmethods.iter().map(|m| m.class_name.clone()).collect();

    // Pointer-to-interface helper. Abstract classes (only virtual methods,
    // no fields) parse as Opaque rather than Struct, so we accept both.
    let interface_of = |ty: &constraints::TypeInfo| -> Option<String> {
        if let constraints::TypeInfo::Pointer(inner) = ty {
            let name_opt = match inner.as_ref() {
                constraints::TypeInfo::Struct { name, .. } => Some(name.as_str()),
                constraints::TypeInfo::Opaque { name, .. } => Some(name.as_str()),
                _ => None,
            };
            if let Some(name) = name_opt {
                if interface_classes.contains(name) {
                    return Some(name.to_string());
                }
            }
        }
        None
    };

    // External specs (only those actually called)
    if !external_calls.is_empty() {
        let experimental_dir = output.join("specs_experimental");
        std::fs::create_dir_all(&experimental_dir)?;
        for spec in &external_calls {
            if spec.function_name == "operator new"
                || spec.mangled_name.as_deref() == Some("??2@YAPEAX_K@Z")
            {
                let mangled = spec.mangled_name.as_deref().unwrap_or("??2@YAPEAX_K@Z");
                saw_emit::emit_operator_new_spec(mangled, &experimental_dir)?;
                continue;
            }

            let fn_info = all_functions
                .iter()
                .find(|f| f.name == spec.function_name && f.mangled_name == spec.mangled_name);
            let iface_return = fn_info.and_then(|f| interface_of(&f.return_type));

            if let Some(iface_name) = iface_return {
                saw_emit::emit_interface_factory_spec(
                    spec,
                    &iface_name,
                    &all_globals,
                    &experimental_dir,
                )?;
                continue;
            }

            saw_emit::emit_single_experimental_spec(spec, &all_globals, &experimental_dir)?;
        }
        eprintln!("Generated {} external function specs", external_calls.len());
    }

    // Vtable stubs + interface overrides
    let mut ctors = Vec::new();
    if has_interfaces {
        let class_names: Vec<String> = interface_classes.iter().cloned().collect();
        ctors = clang_ast::extract_constructors(&parsed_ast, &class_names)?;
        // Drop ctors whose mangled symbol is absent from the LLVM IR —
        // pure-virtual interface ctors are typically never emitted, and
        // binding `llvm_unsafe_assume_spec` against a missing symbol
        // makes SAW fail with "Could not find definition for function".
        if !ir_funcs.is_empty() {
            clang_ast::filter_ctors_by_ir_symbols(&mut ctors, &ir_funcs);
        }
        // Trust the AST. `classes_with_virtual_dtor` already propagates
        // virtual-dtor-ness through inheritance, so a derived class that
        // doesn't redeclare `~T()` still gets the slot when its base has
        // one. If the AST is incomplete (filtered ast-dump etc.), MSVC's
        // real bitcode will reflect the *declared* layout — we must
        // match that exactly, or SAW will dispatch every method to the
        // wrong vtable entry. An incorrect over-eager dtor slot is at
        // least as bad as a missing one: it shifts every method by one
        // and SAW will resolve `log(this, msg)` through a `(ptr, i32)`
        // dtor stub, producing a function-handle type mismatch.
        let classes_with_vdtor = clang_ast::classes_with_virtual_dtor(&parsed_ast);
        let missing_dtor: Vec<&str> = interface_classes
            .iter()
            .filter(|c| !classes_with_vdtor.contains(c.as_str()))
            .map(|c| c.as_str())
            .collect();
        if !missing_dtor.is_empty() {
            let mut sorted = missing_dtor.clone();
            sorted.sort_unstable();
            eprintln!(
                "note: emitting vtable for {} polymorphic class(es) without a virtual dtor slot \
                 (no `virtual ~T()` declared, transitively):",
                sorted.len(),
            );
            for c in &sorted {
                eprintln!("  - {c}");
            }
            eprintln!(
                "      If your real bitcode disagrees with this layout, add \
                 `virtual ~T() = default;` to the interface header."
            );
        }
        saw_emit::emit_interface_stubs(
            &vmethods,
            &ctors,
            &all_globals,
            &classes_with_vdtor,
            output,
            Some(cryptol_fn),
            target_triple.as_deref(),
        )?;
        eprintln!(
            "Generated {} havoc specs + {} constructor overrides",
            vmethods.len(),
            ctors.len(),
        );
    }

    // Try to assemble `vtable_stubs.ll` → `vtable_stubs.bc` so the
    // generated script is directly runnable. SAW's `llvm_load_module`
    // only accepts bitcode; a text `.ll` produces "Invalid magic number"
    // at load time. If no assembler (llvm-as / clang) is on PATH we fall
    // back to referencing the `.ll` and the emitted script will warn the
    // user to assemble it manually.
    //
    // After assembly, by default we go one step further and pre-link
    // main.bc + vtable_stubs.bc into a single code.combined.bc using
    // llvm-link. The emitted verify.saw then loads one module and skips
    // SAW's `llvm_combine_modules` primitive, which only exists in
    // post-v1.5 SAW (master / forks). Pass --use-llvm-combine-modules
    // to keep the old two-module emission for users on a custom SAW.
    let stubs_status = if has_interfaces {
        let assembled = saw_emit::assemble_vtable_stubs(output);
        match &assembled {
            saw_emit::AssembledStubs::Bitcode {
                bc_filename,
                assembler,
            } => {
                eprintln!(
                    "Assembled vtable stubs to {} via `{assembler}`",
                    bc_filename,
                );
            }
            saw_emit::AssembledStubs::TextOnly { ll_filename } => {
                eprintln!(
                    "warning: could not find llvm-as / clang on PATH — \
                     {ll_filename} was NOT assembled to bitcode.",
                );
                eprintln!("         SAW's llvm_load_module rejects text IR. Run one of:",);
                eprintln!(
                    "           llvm-as {} -o {}/vtable_stubs.bc",
                    output.join(ll_filename).display(),
                    output.display(),
                );
                eprintln!(
                    "           clang -c -emit-llvm {} -o {}/vtable_stubs.bc",
                    output.join(ll_filename).display(),
                    output.display(),
                );
                eprintln!("         before invoking saw on verify.saw.",);
            }
            saw_emit::AssembledStubs::LinkedBitcode { .. } => { /* unreachable here */ }
            saw_emit::AssembledStubs::NoStubs => {}
        }
        // Default: pre-link with llvm-link so the script doesn't need
        // SAW's `llvm_combine_modules` (post-v1.5 only).
        let final_status = if use_llvm_combine_modules {
            assembled
        } else {
            let linked = saw_emit::link_stubs_with_main(bitcode, output, assembled);
            match &linked {
                saw_emit::AssembledStubs::LinkedBitcode {
                    combined_filename,
                    linker,
                } => {
                    eprintln!(
                        "Pre-linked main + vtable stubs into {} via `{linker}` \
                         (verify.saw will not need llvm_combine_modules).",
                        combined_filename,
                    );
                }
                saw_emit::AssembledStubs::Bitcode { .. } => {
                    eprintln!(
                        "warning: llvm-link not found on PATH; falling back to \
                         llvm_combine_modules in the emitted script. Stock SAW \
                         v1.5 will not be able to run that — install llvm-link \
                         (ships with LLVM) or pass --use-llvm-combine-modules \
                         to silence this warning.",
                    );
                }
                _ => {}
            }
            linked
        };
        final_status
    } else {
        saw_emit::AssembledStubs::NoStubs
    };

    // In-module sub-callees with a body are NOT havoc-overridden — SAW
    // executes their real bodies during symbolic execution. The two
    // discrimination layers we keep:
    //
    //   * `external_calls` (built earlier via callgraph::external_callees)
    //     filters to `!has_body`, so genuinely undefined callees — printf,
    //     libc, externs from other TUs — still get an adversarial spec.
    //   * vtable dispatches and constructors are handled by the vtable
    //     stub / interface-override layers; nothing to do here.
    //
    // What this loop used to do — havoc *every* body-having in-module
    // callee — is wrong for callees defined inline in headers (e.g. a
    // helper in `my_header.hpp` that gets inlined into the TU still ends
    // up with `has_body == true`). Havoc'ing it produced spurious
    // DISPROVED counterexamples for code that just called a trivial
    // helper. Empty list = no compositional havoc step emitted.
    let sub_callee_specs: Vec<constraints::SpecConstraint> = Vec::new();

    // Apply the same `llvm_alias` resolution / dereferenceable fallback to
    // every spec that will be emitted as an override (externals only now —
    // sub-callees are empty above).  Without this the override files
    // contain `llvm_alias "ShortName"` references that SAW can't load
    // against the bitcode's mangled struct table.
    let mut external_calls_owned: Vec<constraints::SpecConstraint> =
        external_calls.iter().map(|s| (*s).clone()).collect();
    for spec in &mut external_calls_owned {
        resolve_spec_types_quiet(spec, &ir_struct_sizes);
    }

    // Bitcode-driven extern override scan. The AST pipeline above only
    // sees declarations the path filter kept; anything filtered as a
    // system header (e.g. `printf` in MSVC's `<cstdio>`) is invisible
    // even when the TU actually calls it, and SAW then aborts on
    // `internal: error: in printf`. A second pass on the LLVM IR text
    // covers declare-only externs and variadic functions whose body
    // uses `llvm.va_*` intrinsics Crucible-LLVM can't simulate.
    // AST-derived overrides take precedence — we pass their symbols
    // as `already_covered` so the bitcode scan doesn't double-emit.
    // See `src/emit/saw_emit/bitcode_overrides.rs` for the contract.
    let already_covered: Vec<String> = external_calls_owned
        .iter()
        .filter_map(|s| {
            s.mangled_name
                .clone()
                .or_else(|| Some(s.function_name.clone()))
        })
        .collect();
    let bitcode_overrides = saw_emit::scan_and_emit_bitcode_overrides(
        llvm_ir_path,
        &target_mangled,
        &already_covered,
        &all_globals,
    );

    saw_emit::emit_verification_script(
        bitcode,
        cryptol_spec,
        cryptol_fn,
        function,
        &target_mangled,
        &target_spec,
        &target_fn,
        &vmethods,
        has_interfaces,
        &external_calls_owned,
        &sub_callee_specs,
        &all_globals,
        &ctors,
        &stubs_status,
        &bitcode_overrides,
        output,
    )?;

    // Post-processing: rewrite unresolved `llvm_alias "X"` references into
    // concrete SAW types (structs → byte arrays, enums → `llvm_int N`).
    let mut fallbacks = collect_type_sizes(&all_functions);
    // Seed enum_bits from every EnumDecl in the AST so forward-declared
    // enums like `LatchResult` still get the `llvm_int <bits>` fallback.
    for (name, bits) in clang_ast::collect_all_enum_bits(&parsed_ast) {
        fallbacks.enum_bits.entry(name).or_insert(bits);
    }
    if !ir_funcs.is_empty() {
        alias_fallbacks_ir::add_ir_deref_fallbacks(&mut fallbacks, &all_functions, &ir_funcs);
    }
    // CLI overrides take priority over inferred sizes.
    apply_cli_overrides(&mut fallbacks, alias_size_overrides, alias_enum_overrides)?;
    // SAW_SPEC_GEN_DEBUG_FALLBACKS=1 to see resolved fallback sizes.
    if std::env::var_os("SAW_SPEC_GEN_DEBUG_FALLBACKS").is_some() {
        dump_fallback_diagnostics(&fallbacks);
    }
    apply_alias_rewrites(output, &ir_struct_sizes, &fallbacks);

    eprintln!("Generated verification script in {}", output.display());
    eprintln!("Run with: saw {}/verify.saw", output.display());
    Ok(())
}
