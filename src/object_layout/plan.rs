//! Join source declarations, same-invocation layouts and the actual LLVM ABI.

use super::{capture, clang::normalize_name, derive, validate, *};
use crate::buffer_overrides::BufferOverrides;
use crate::constraints::{AllocType, FunctionInfo, SpecConstraint, TypeInfo};
use anyhow::{ensure, Context, Result};
use std::path::Path;

#[path = "plan_inputs.rs"]
mod inputs;

#[allow(clippy::too_many_arguments)]
pub fn prepare(
    ast: &crate::clang_ast::AstNode,
    ll: Option<&Path>,
    ir: &str,
    target: &FunctionInfo,
    spec: &mut SpecConstraint,
    overrides: &mut BufferOverrides,
    alias_sizes: &[String],
    output: &Path,
    cryptol_spec: &Path,
    cryptol_fn: &str,
    bitcode: &Path,
) -> Result<()> {
    let symbol = target
        .mangled_name
        .as_deref()
        .context("target has no symbol")?;
    let source = inputs::source_signature(ast, symbol)?;
    let facts = ll.map(|ll| capture::load(ll, ir)).transpose()?.flatten();
    let Some(facts) = facts else {
        ensure!(!spec.return_constraint.is_sret && !target.params.iter().any(|p| p.name == "this" || is_aggregate(&p.ty)),
            "compiler object layouts are unavailable; run verify-cpp to derive them from the same compilation (refusing a guessed object allocation)");
        ensure!(
            overrides.layout_config.modifies.is_none()
                && overrides.layout_config.offsets.is_empty()
                && overrides.layout_config.alignments.is_empty(),
            "named layout constraints require compiler layout facts"
        );
        validate_bindings(spec, overrides)?;
        for warning in super::config_keys::validate_keys(spec, overrides, &LayoutPlan::default())? {
            eprintln!("PROOF SCOPE WARNING: {warning}");
        }
        return Ok(());
    };
    let abi = inputs::abi_signature(ir, symbol)?;
    let cry_sig = crate::cryptol_sig::parse_signature(cryptol_spec, cryptol_fn);
    ensure!(
        abi.params.len() == target.params.len() + usize::from(abi.sret.is_some()),
        "unsupported aggregate ABI expansion: {} source parameters but {} LLVM arguments",
        target.params.len(),
        abi.params.len()
    );
    let mut plan = LayoutPlan {
        schema_version: 1,
        target_triple: facts.target_triple.clone(),
        data_layout: facts.data_layout.clone(),
        compiler: facts.compiler.clone(),
        command: facts.command.clone(),
        records: facts.records.clone(),
        llvm_types: facts.llvm_types.clone(),
        abi: if facts.target_triple.contains("msvc") {
            "msvc"
        } else {
            "itanium"
        }
        .into(),
        ..Default::default()
    };
    ensure!(
        super::data_layout::DataLayout::parse(&facts.data_layout)?.little_endian,
        "big-endian object projections are not supported; refusing host-endian guesses"
    );
    for (i, param) in target.params.iter().enumerate() {
        let source_type = if param.name == "this" {
            source.receiver.as_deref()
        } else {
            source.params.get(&param.name).map(String::as_str)
        };
        let source_type = source_type
            .map(inputs::source_object_name)
            .or_else(|| type_name(&param.ty));
        let source_name = source_type
            .as_deref()
            .and_then(|name| find_record(&facts, name));
        let Some(source_name) = source_name else {
            ensure!(param.name != "this" && !is_aggregate(&param.ty),
                "unresolved compiler object layout for parameter {} ({:?}); no guessed allocation is permitted", param.name, source_type);
            continue;
        };
        let index = i + usize::from(abi.sret.as_ref().is_some_and(|(pos, _, _)| *pos <= i));
        let ir_param = &abi.params[index];
        ensure!(ir_param.starts_with("ptr ") || ir_param == "ptr",
            "{}: register-coerced aggregate parameters need an ABI value projection; refusing pointer setup for {ir_param}", param.name);
        let mut layout = derive::derive(
            &facts,
            ir,
            &source_name,
            &param.name,
            &overrides.layout_config,
        )?;
        super::enums::apply(&mut layout, ast)?;
        if param.name != "this"
            && inputs::abstract_record(ast, &normalize_name(&source_name))
            && layout
                .fields
                .iter()
                .all(|f| f.is_pointer && f.path.starts_with("vptr_"))
        {
            ensure!(
                !overrides.is_out_buffer(&param.name)
                    && !overrides.has_in_buffer_size(&param.name)
                    && !overrides.cryptol_fn_out.contains_key(&param.name),
                "abstract interface {} cannot have a complete-object buffer override",
                param.name
            );
            plan.warnings.push(format!("{}: abstract interface {} uses the explicit vtable assumed-contract model; concrete implementation layout is not claimed", param.name, source_name));
            plan.abstract_objects.insert(param.name.clone(), layout);
            continue;
        }
        // A parameter's explicit alignment can strengthen, never weaken, the
        // source ABI requirement. Otherwise an oversized allocation can mask UB.
        if let Some(n) = attribute_number(ir_param, "align") {
            ensure!(
                n <= layout.alignment,
                "{}: LLVM parameter alignment exceeds Clang object alignment",
                param.name
            );
        }
        let mutable = overrides.is_out_buffer(&param.name)
            || spec.params[i].alloc_type == AllocType::AllocMutable;
        if overrides.cryptol_fn_out.contains_key(&param.name) {
            ensure!(
                mutable,
                "post-state binding {} requires a writable object",
                param.name
            );
            overrides.out_buffer_auto.insert(param.name.clone());
        }
        if let Some(post) = spec.params[i].out_postcond.clone() {
            overrides
                .cryptol_fn_out
                .entry(param.name.clone())
                .or_insert(post);
            overrides.out_buffer_auto.insert(param.name.clone());
            spec.params[i].out_postcond = None;
        }
        // Source enum constraints must not be an enumerator-only set for fixed
        // underlying C++ enums. No value restriction is guessed from enum names.
        for field in &layout.fields {
            if field.source_type.starts_with("enum ") {
                layout.validation.push(format!(
                    "{}: LLVM integer representation retained; no enumerator-only precondition",
                    field.path
                ));
            }
        }
        let mut object = ObjectPlan {
            region: param.name.clone(),
            layout,
            projection: overrides.layout_config.projection.clone(),
            mutable,
            argument_index: index,
            lowering: if source
                .params
                .get(&param.name)
                .is_some_and(|ty| !ty.contains('*') && !ty.contains('&'))
            {
                if ir_param.contains("byval(") {
                    "byval"
                } else {
                    "indirect_by_value"
                }
            } else {
                "pointer"
            }
            .into(),
            configured_shape: overrides.override_saw_type(&param.name),
            inferred_shape: None,
            asserted: Vec::new(),
            framed: Vec::new(),
            selectors: Default::default(),
        };
        if object.configured_shape.is_none()
            && object.projection == Projection::Bytes
            && overrides.cryptol_call_args(cryptol_fn).is_none()
        {
            if let Some(crate::cryptol_sig::CryType::Bitvector(bits)) =
                cry_sig.as_ref().and_then(|s| s.params.get(i))
            {
                if object.layout.fields.len() == 1
                    && object.layout.fields[0].offset == 0
                    && object.layout.fields[0].size == object.layout.size
                    && object.layout.fields[0].llvm_type == format!("i{bits}")
                {
                    object.inferred_shape = Some(format!("llvm_int {bits}"));
                }
            }
        }
        let has_post = overrides.cryptol_fn_out.contains_key(&param.name);
        for field in &object.layout.fields {
            if has_post && !field.is_pointer && !field.runtime {
                object.asserted.push(field.path.clone());
            }
            // Pointer fields never become Cryptol integer bits. Framing retains
            // their real allocation identity, even in byte projection mode.
            if field.is_pointer || field.runtime {
                object.framed.push(field.path.clone());
            }
        }
        validate::validate_object(&mut object, ir)?;
        if !has_post && mutable && object.lowering == "pointer" {
            plan.warnings.push(format!(
                "{}: mutable object has no semantic post-state contract",
                param.name
            ));
        }
        spec.params[i].saw_type = format!("llvm_alias \"{}\"", object.layout.llvm_type);
        plan.objects.insert(param.name.clone(), object);
    }
    if let Some((index, ir_name, ir_align)) = &abi.sret {
        let source_name = find_record(&facts, &inputs::source_object_name(&source.return_type))
            .or_else(|| type_name(&target.return_type).and_then(|s| find_record(&facts, &s)))
            .or_else(|| find_record(&facts, &strip_llvm_tag(ir_name)))
            .context("sret source type has no unique Clang record layout")?;
        let mut layout = derive::derive_with_llvm(
            &facts,
            ir,
            &source_name,
            "return",
            &overrides.layout_config,
            Some(ir_name),
        )?;
        super::enums::apply(&mut layout, ast)?;
        ensure!(
            &layout.llvm_type == ir_name,
            "sret LLVM type does not match Clang return type"
        );
        ensure!(
            ir_align.is_none_or(|a| a <= layout.alignment),
            "sret alignment disagrees with Clang layout"
        );
        ensure!(
            !layout.fields.iter().any(|f| f.is_pointer),
            "pointer-containing sret requires an explicit provenance-aware return contract"
        );
        let mut object = ObjectPlan {
            region: "return".into(),
            asserted: layout.fields.iter().map(|f| f.path.clone()).collect(),
            layout,
            projection: overrides.layout_config.projection.clone(),
            mutable: true,
            argument_index: *index,
            lowering: "sret".into(),
            configured_shape: None,
            inferred_shape: None,
            framed: Vec::new(),
            selectors: Default::default(),
        };
        validate::validate_object(&mut object, ir)?;
        spec.return_constraint.is_sret = true;
        spec.return_constraint.saw_type = format!("llvm_alias \"{}\"", object.layout.llvm_type);
        plan.objects.insert("return".into(), object);
    }
    validate_bindings(spec, overrides)?;
    let warnings = super::config_keys::validate_keys(spec, overrides, &plan)?;
    plan.warnings.extend(warnings);
    let lowered = validate::finish(
        &mut plan,
        &overrides.layout_config,
        &overrides.raw_preconds,
        overrides.sret_assert_bytes,
        alias_sizes,
    )?;
    overrides.raw_preconds = lowered;
    for object in plan.objects.values() {
        for field in object.layout.fields.iter().filter(|f| f.validity.is_some()) {
            if object.region != "return" {
                if let Some(index) = field
                    .validity
                    .as_deref()
                    .and_then(|v| v.strip_prefix("active_variant:"))
                {
                    plan.semantic_preconditions.push(format!(
                        "{}.{} == {index} (configured active alternative)",
                        object.region, field.path
                    ));
                    continue;
                }
                plan.validity_constraints.push(format!(
                    "valid({}.{}){}",
                    object.region,
                    field.path,
                    field
                        .guard
                        .as_ref()
                        .map(|g| format!(" when {g}"))
                        .unwrap_or_default()
                ));
            }
        }
        for note in &object.layout.validation {
            if note.starts_with("warning:") {
                plan.warnings.push(format!("{}: {note}", object.region));
            }
        }
    }
    for warning in &plan.warnings {
        eprintln!("PROOF SCOPE WARNING: {warning}");
    }
    // Record every boundary, including the ones that remain truly opaque after
    // typed wrapper execution. No implication that OS synchronization is proved.
    plan.abstraction_boundaries = crate::transform::extern_override_scan::scan_typed(ir, symbol)
        .into_iter()
        .map(|t| format!("{}: {:?}", t.symbol, t.reason))
        .collect();
    super::bitcode::validate(bitcode, ir, &plan, symbol)?;
    std::fs::create_dir_all(output)?;
    std::fs::write(
        output.join("layout-plan.json"),
        serde_json::to_vec_pretty(&plan)?,
    )?;
    overrides.layout_plan = Some(plan);
    Ok(())
}

fn validate_bindings(spec: &SpecConstraint, overrides: &BufferOverrides) -> Result<()> {
    for name in overrides.cryptol_fn_out.keys() {
        ensure!(
            spec.params.iter().any(|p| &p.name == name),
            "unknown post-state region {name}"
        );
        ensure!(
            overrides.is_out_buffer(name),
            "post-state {name} needs an out-buffer or compiler-derived mutable object"
        );
    }
    Ok(())
}

fn type_name(ty: &TypeInfo) -> Option<String> {
    match ty {
        TypeInfo::Pointer(inner) => type_name(inner),
        TypeInfo::Struct { name, .. } | TypeInfo::Opaque { name, .. } => Some(name.clone()),
        _ => None,
    }
}

fn is_aggregate(ty: &TypeInfo) -> bool {
    match ty {
        TypeInfo::Pointer(inner) => is_aggregate(inner),
        TypeInfo::Struct { .. } | TypeInfo::Option(_) | TypeInfo::Result(_, _) => true,
        TypeInfo::Opaque { name, .. } => {
            crate::constraints::saw_type::std_integer_typedef_bits(name).is_none()
        }
        _ => false,
    }
}

fn find_record(facts: &CompilerLayouts, name: &str) -> Option<String> {
    let name = normalize_name(&strip_llvm_tag(name));
    let exact: Vec<_> = facts
        .records
        .values()
        .filter(|r| normalize_name(&r.source_type) == name)
        .collect();
    if exact.len() == 1 {
        return Some(exact[0].source_type.clone());
    }
    let suffix = format!("::{name}");
    let matches: Vec<_> = facts
        .records
        .values()
        .filter(|r| normalize_name(&r.source_type).ends_with(&suffix))
        .collect();
    (matches.len() == 1).then(|| matches[0].source_type.clone())
}

fn strip_llvm_tag(name: &str) -> String {
    ["struct.", "class.", "union."]
        .iter()
        .find_map(|p| name.strip_prefix(p))
        .unwrap_or(name)
        .into()
}

fn attribute_number(param: &str, attribute: &str) -> Option<usize> {
    param
        .split_once(&format!(" {attribute} "))?
        .1
        .split_whitespace()
        .next()?
        .parse()
        .ok()
}

/// Record emitted assumptions as well as the bitcode scan's candidate leaves.
/// This includes AST, vtable and user-composed contracts; none are hidden merely
/// because they originated outside the bitcode-driven override registry.
pub fn record_emitted_boundaries(output: &Path) -> Result<()> {
    let path = output.join("layout-plan.json");
    if !path.exists() {
        return Ok(());
    }
    let mut plan: LayoutPlan = serde_json::from_slice(&std::fs::read(&path)?)?;
    let mut pending = vec![output.to_path_buf()];
    let mut assumptions = std::collections::BTreeSet::new();
    while let Some(dir) = pending.pop() {
        for entry in std::fs::read_dir(dir)? {
            let entry = entry?;
            let kind = entry.file_type()?;
            if kind.is_dir() {
                pending.push(entry.path());
            } else if kind.is_file() && entry.path().extension().is_some_and(|e| e == "saw") {
                let text = std::fs::read_to_string(entry.path())?;
                for line in text
                    .lines()
                    .map(str::trim)
                    .filter(|line| !line.starts_with("//"))
                {
                    if let Some((_, call)) = line.split_once("llvm_unsafe_assume_spec") {
                        assumptions.insert(format!(
                            "emitted assumption: llvm_unsafe_assume_spec{}",
                            call
                        ));
                    }
                }
            }
        }
    }
    plan.abstraction_boundaries.extend(assumptions);
    plan.abstraction_boundaries.sort();
    plan.abstraction_boundaries.dedup();
    std::fs::write(path, serde_json::to_vec_pretty(&plan)?)?;
    Ok(())
}
