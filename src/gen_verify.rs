//! Driver for the `gen-verify` subcommand.
//!
//! Runs the full C++ → SAW verification pipeline:
//!   1. Parse clang AST (one or more files, merged into a synthetic TU)
//!   2. Derive target function + global / call-graph info
//!   3. Emit adversarial specs for externals, virtual methods, and
//!      direct in-module callees (compositional verification)
//!   4. Emit the top-level `verify.saw` script that wires everything together

use crate::alias_fallbacks::{apply_cli_overrides, dump_fallback_diagnostics};
use crate::gen_verify_callgraph::collect_external_call_specs;
use crate::spec_rewrite::{apply_alias_rewrites, collect_type_sizes};
use crate::transform::crucible_safety::SafetyAnalyzer;
use crate::type_resolve::resolve_spec_types_quiet;
use crate::{alias_fallbacks_ir, clang_ast, constraints, inventory, llvm_ir, saw_emit};
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
    spec_only_on_missing: bool,
    buffer_overrides: &crate::buffer_overrides::BufferOverrides,
    bind_cryptol_lengths: bool,
    no_struct_shape_recognizer: bool,
    container_layouts: Option<&Path>,
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
    let mut all_functions = clang_ast::extract_functions(&parsed_ast, None)?;
    eprintln!("Found {} functions", all_functions.len());

    // ArrayView pre-derive passes (saw_spec_gen-rng umbrella).
    // Order matters: struct-shape recognizer first so the binding
    // pass can respect its output, then the container catalog
    // (diagnostic-only for the scaffold), then the Cryptol length
    // binding for the target function. See `src/array_view_passes.rs`.
    crate::array_view_passes::apply_struct_shape_recognizer(
        &mut all_functions,
        no_struct_shape_recognizer,
    );
    // The catalog is built (and the user warned) but no emitter pass
    // consumes it yet. Wiring is tracked under saw_spec_gen-qms; the
    // user-facing auto-derive replacement (so no TOML is ever needed)
    // is saw_spec_gen-530.
    let _container_catalog = crate::array_view_passes::load_container_catalog(container_layouts);
    crate::array_view_passes::apply_cryptol_length_binding(
        &mut all_functions,
        bind_cryptol_lengths,
        cryptol_spec,
        cryptol_fn,
        function,
    );

    // Optional LLVM IR: struct-size table + per-param `dereferenceable(N)`.
    // MSVC-clang fully qualifies struct symbols, so without the IR we can't
    // match short C++ names against `%\"struct.Foo::Bar::Baz\"`.
    let (ir_struct_sizes, ir_funcs) = llvm_ir::load_optional(llvm_ir_path)?;

    // Also load the IR text (cheap re-read) so the Crucible-safety
    // analyzer can walk header-only STL template bodies. Without this
    // the blanket `is_system` gate sweeps `std::max`, `std::min`,
    // `std::pair::first` etc. into the adversarial-override fallback
    // even though clang fully instantiates their bodies.
    let ir_text = llvm_ir_path
        .and_then(|p| llvm_ir::parse_llvm_ir(p).ok())
        .unwrap_or_default();
    let mut safety = SafetyAnalyzer::new(&ir_text);
    let no_system_recursion = std::env::var_os("SAW_SPEC_GEN_NO_SYSTEM_RECURSION").is_some();

    // Sniff the bitcode's `target triple = "..."` line so vtable stub
    // generation can pick the correct ABI layout. Without this, an
    // Itanium-compiled binary (Linux / macOS) would get MSVC-shaped
    // stubs that never line up with the compiler's `_ZTV<C> + 16`
    // pointer arithmetic in virtual constructors, and every dispatch
    // would fall through to the unmodified real method body — defeating
    // havoc-based verification.
    let target_triple = llvm_ir_path.and_then(llvm_ir::read_target_triple);

    // Find the target function (FunctionInfo with call graph)
    let target_fn_opt = all_functions
        .iter()
        .filter(|f| f.name == function)
        .min_by_key(|f| if f.is_virtual { 1 } else { 0 })
        .cloned();
    let target_fn = match target_fn_opt {
        Some(f) => f,
        None => {
            if spec_only_on_missing {
                return emit_spec_only_result(
                    output,
                    cryptol_fn,
                    function,
                    "No matching C++ implementation symbol found in clang AST — \
                     this is a Cryptol-only helper used by other models, with \
                     no implementation to verify against at this layer.",
                );
            }
            anyhow::bail!("Function '{}' not found in AST", function);
        }
    };

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
    let mut target_spec = match target_spec {
        Some(s) => s,
        None => {
            if spec_only_on_missing {
                return emit_spec_only_result(
                    output,
                    cryptol_fn,
                    function,
                    "Function found in AST but no constraint spec could be derived — \
                     treating as Cryptol-only helper.",
                );
            }
            anyhow::bail!("Function '{}' not found in AST", function);
        }
    };
    let target_mangled = match target_spec.mangled_name.clone() {
        Some(m) => m,
        None => {
            if spec_only_on_missing {
                return emit_spec_only_result(
                    output,
                    cryptol_fn,
                    function,
                    "No mangled name for function — likely an inline / template / \
                     header-only helper with no out-of-line symbol to verify.",
                );
            }
            anyhow::bail!("No mangled name for '{}'", function);
        }
    };

    // Resolve any opaque `llvm_alias` parameter / return types against the
    // LLVM IR struct table.  Without this, MSVC-clang output (which fully
    // qualifies struct names like `%"struct.Foo::Bar::Baz"`) fails to load
    // because SAW can't find a `struct.Baz` symbol.
    crate::type_resolve::resolve_spec_aliases(
        &mut target_spec,
        &ir_struct_sizes,
        llvm_ir_path.is_some(),
    );

    // Correct sret misclassification: small trivially-copyable structs
    // on MSVC are returned in registers, not via sret pointer.
    constraints::correct_sret_from_ir(&mut target_spec, &ir_funcs);

    // Detect sret pre-state threading from Cryptol arity.
    saw_emit::cryptol_bridge::detect_sret_prestate(&mut target_spec, cryptol_spec, cryptol_fn);

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
    let ir_struct_defs = if let Some(ir_path) = llvm_ir_path {
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
            llvm_ir::struct_defs(&ir_text)
        } else {
            HashMap::new()
        }
    } else {
        HashMap::new()
    };

    // Inject the exception-lower bookkeeping globals (@__exclow_error_*)
    // with the right TypeInfo and pre-state init values. Must run after
    // the AST + IR scans so the explicit `init_value: Some("0")` for the
    // error flag isn't shadowed by a duplicate entry from
    // `discover_ir_only_globals` (which would have `init_value: None`
    // because it can't parse the LLVM `false` literal).
    if let Some(ir_path) = llvm_ir_path {
        crate::transform::eh_globals::inject_exclow_globals(&mut all_globals, ir_path);
    }

    let mut all_specs = constraints::derive_constraints(&all_functions)?;
    for spec in &mut all_specs {
        constraints::correct_sret_from_ir(spec, &ir_funcs);
    }

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

    // Walk the call graph transitively to find ALL external calls
    // reachable through any in-module function, and pre-decide which
    // system callees have Crucible-safe bodies. See
    // [`crate::gen_verify_callgraph`] for the full logic and the
    // `SAW_SPEC_GEN_NO_SYSTEM_RECURSION=1` kill-switch.
    let external_calls = collect_external_call_specs(
        &target_fn,
        &all_functions,
        &all_specs,
        &mut safety,
        no_system_recursion,
    );

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

    let stubs_status =
        assemble_and_link_stubs(has_interfaces, use_llvm_combine_modules, bitcode, output);

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
        &ir_struct_defs,
        output,
        buffer_overrides,
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

    let source_file = inventory::resolve_cpp_source_file(&parsed_ast, function, &target_mangled);
    inventory::emit_fragment(
        output,
        function,
        "cpp",
        Some(target_mangled.clone()),
        source_file,
        cryptol_fn,
        inventory::build_models_note(
            !buffer_overrides.max_len_preconds_ordered().is_empty(),
            false,
        ),
    )?;

    eprintln!("Generated verification script in {}", output.display());
    eprintln!("Run with: saw {}/verify.saw", output.display());
    Ok(())
}

use crate::gen_verify_helpers::{assemble_and_link_stubs, emit_spec_only_result};
