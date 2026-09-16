//! Validate C++ configuration keys after compiler object/ABI inference.

use crate::buffer_overrides::BufferOverrides;
use crate::constraints::{AllocType, SpecConstraint};
use crate::object_layout::{LayoutConfig, LayoutPlan};
use anyhow::{ensure, Context, Result};
use std::collections::BTreeSet;

/// Check actual target parameters, not just objects that happened to be found.
/// The caller adds returned assumptions to the plan's proof-scope warnings.
/// Shape/offset/alignment and alias-size assertions remain with `validate`.
pub fn validate_keys(
    spec: &SpecConstraint,
    overrides: &BufferOverrides,
    plan: &LayoutPlan,
) -> Result<Vec<String>> {
    let keys: BTreeSet<_> = overrides
        .in_buffers
        .keys()
        .chain(&overrides.in_buffer_auto)
        .chain(overrides.out_buffers.keys())
        .chain(&overrides.out_buffer_auto)
        .collect();
    let mut warnings = Vec::new();
    for name in keys {
        let param = spec
            .params
            .iter()
            .find(|p| &p.name == name)
            .with_context(|| {
                format!("unknown buffer override parameter {name}: not an actual target parameter")
            })?;
        // Manual + auto on the same side is valid (output inference adds auto).
        // Crossing the input/output boundary would silently change mutability.
        ensure!(
            !(overrides.has_in_buffer_size(name) && overrides.is_out_buffer(name)),
            "{name}: conflicting input and output buffer overrides"
        );
        let object = plan.objects.contains_key(name);
        ensure!(
            object || param.alloc_type != AllocType::FreshVar,
            "{name}: buffer override cannot apply to a scalar FreshVar parameter without a compiler object"
        );
        if !object {
            let allocation = overrides.override_saw_type(name).map_or_else(
                || format!("inferred scalar/pointee shape {}", param.saw_type),
                |shape| format!("manual extent/shape {shape}"),
            );
            warnings.push(format!(
                "{name}: unvalidated raw allocation assumption ({allocation}); no compiler-derived C++ aggregate extent is known; proof is limited to caller-provided storage with that shape and extent, not a validated object layout"
            ));
        }
    }
    validate_active_members(&overrides.layout_config, plan)?;
    Ok(warnings)
}

/// Derivation must have consumed every selection, including nested/empty ones.
/// A matching root or flattened leaf is not evidence that a union key was used.
/// Also callable by `validate::finish`, which has no parameter constraints.
pub fn validate_active_members(config: &LayoutConfig, plan: &LayoutPlan) -> Result<()> {
    for (key, member) in &config.active_members {
        let prefix = format!("{key}: selected actual compiler union member ");
        ensure!(
            plan.objects.values().any(|object| object
                .layout
                .validation
                .iter()
                .any(|note| note.starts_with(&prefix))),
            "active_members {key}={member:?}: unused or invalid selection; no matching compiler union member validation note"
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::constraints::{AllocType::*, ParamConstraint, ReturnConstraint};
    use crate::object_layout::{derive, validate, CompilerLayouts, ObjectPlan};
    use serde_json::{json, Value};
    use std::collections::BTreeMap;

    const DL: &str = "e-p:64:64-i64:64";
    const IR: &str = concat!(
        "target datalayout = \"e-p:64:64-i64:64\"\n",
        "%union.U = type { i32 }\n%struct.Host = type { %union.U }\n",
        "%union.Storage = type { i32 }\n",
        "%\"class.std::optional<int>\" = type { %union.Storage, i8 }\n"
    );

    fn spec(params: &[(&str, AllocType)]) -> SpecConstraint {
        SpecConstraint {
            function_name: "target".into(),
            mangled_name: Some("target".into()),
            params: params
                .iter()
                .map(|(name, alloc)| ParamConstraint {
                    name: (*name).into(),
                    alloc_type: alloc.clone(),
                    saw_type: "llvm_int 32".into(),
                    preconditions: Vec::new(),
                    unchanged_after: *alloc == AllocReadonly,
                    dereferenceable_size: None,
                    out_postcond: None,
                })
                .collect(),
            return_constraint: ReturnConstraint {
                saw_type: "llvm_int 32".into(),
                value_constraints: Vec::new(),
                is_sret: false,
                returns_pointer: false,
                sret_prestate: false,
            },
            can_throw: false,
            is_virtual: false,
            has_body: true,
            postconditions: Vec::new(),
            referenced_globals: Vec::new(),
        }
    }

    fn buffers(input: &[&str], output: &[&str]) -> BufferOverrides {
        BufferOverrides::from_cli(
            &input.iter().map(|s| (*s).into()).collect::<Vec<_>>(),
            &output.iter().map(|s| (*s).into()).collect::<Vec<_>>(),
            &[],
            &[],
            &[],
            &[],
            &[],
        )
        .unwrap()
    }

    fn raw(target: &SpecConstraint, overrides: &BufferOverrides) -> Result<Vec<String>> {
        validate_keys(target, overrides, &LayoutPlan::default())
    }

    fn active(key: &str, member: &str) -> LayoutConfig {
        LayoutConfig {
            active_members: BTreeMap::from([(key.into(), member.into())]),
            ..LayoutConfig::default()
        }
    }

    fn member(source: &str, name: &str, offset: usize) -> Value {
        json!({"name": name, "source_type": source, "offset": offset, "bit_offset": null,
            "bit_width": null, "is_base": false, "is_empty": false, "children": []})
    }

    fn record(source: &str, size: usize, union: bool, members: Vec<Value>) -> Value {
        json!({"source_type": source, "size": size, "alignment": 4,
            "is_union": union, "members": members})
    }

    fn compiler_facts() -> CompilerLayouts {
        serde_json::from_value(json!({
            "schema_version": 1, "target_triple": "x86_64-pc-windows-msvc",
            "data_layout": DL, "compiler": "unit test fixture", "command": [], "irgen_types": {},
            "llvm_types": {"union.U": "{ i32 }", "struct.Host": "{ %union.U }",
                "union.Storage": "{ i32 }", "class.std::optional<int>": "{ %union.Storage, i8 }"},
            "records": {
                "U": record("union U", 4, true, vec![member("int", "small", 0), member("int", "other", 0)]),
                "Host": record("struct Host", 4, false, vec![member("union U", "choice", 0)]),
                "Storage": record("union Storage", 4, true, vec![member("char", "_Dummy", 0), member("int", "_Value", 0)]),
                "std::optional<int>": record("class std::optional<int>", 8, false,
                    vec![member("union Storage", "", 0), member("bool", "_Has_value", 4)])
            }
        }))
        .unwrap()
    }

    fn derived(source: &str, region: &str, config: &LayoutConfig) -> LayoutPlan {
        let facts = compiler_facts();
        let layout = derive::derive(&facts, IR, source, region, config).unwrap();
        let mut object: ObjectPlan = serde_json::from_value(json!({
            "region": region, "layout": layout, "projection": "bytes", "mutable": true,
            "argument_index": 0, "lowering": "pointer", "configured_shape": null,
            "inferred_shape": null, "asserted": [], "framed": [], "selectors": {}
        }))
        .unwrap();
        validate::validate_object(&mut object, IR).unwrap();
        LayoutPlan {
            data_layout: DL.into(),
            objects: BTreeMap::from([(region.into(), object)]),
            ..LayoutPlan::default()
        }
    }

    #[test]
    fn unknown_parameters_fail_in_all_four_buffer_key_sets() {
        let target = spec(&[("p", AllocMutable)]);
        for name in ["missing", "return", "p.field"] {
            for shape in ["4", "auto"] {
                let entry = format!("{name}={shape}");
                for overrides in [buffers(&[&entry], &[]), buffers(&[], &[&entry])] {
                    let error = raw(&target, &overrides).unwrap_err().to_string();
                    assert!(error.contains(&format!("unknown buffer override parameter {name}")));
                }
            }
        }
    }

    #[test]
    fn a_plan_object_is_not_a_substitute_for_an_actual_parameter() {
        let plan = derived("U", "ghost", &active("ghost", "small"));
        assert!(validate_keys(&spec(&[]), &buffers(&["ghost=4"], &[]), &plan).is_err());
    }

    #[test]
    fn conflicting_input_and_output_membership_is_rejected() {
        let target = spec(&[("p", AllocMutable)]);
        for input in ["p=4", "p=auto"] {
            for output in ["p=4", "p=auto"] {
                let error = raw(&target, &buffers(&[input], &[output])).unwrap_err();
                assert!(error.to_string().contains("conflicting input and output"));
            }
        }
    }

    #[test]
    fn inferred_auto_membership_on_the_same_side_is_not_a_conflict() {
        let config = active("this.choice", "small");
        let plan = derived("Host", "this", &config);
        let target = spec(&[("this", AllocMutable)]);
        for mut overrides in [
            buffers(&["this=4", "this=auto"], &[]),
            buffers(&[], &["this=4", "this=auto"]),
        ] {
            overrides.layout_config = config.clone();
            assert!(validate_keys(&target, &overrides, &plan)
                .unwrap()
                .is_empty());
        }
        let mut overrides = buffers(&["this=4"], &[]);
        overrides.out_buffer_auto.insert("this".into());
        assert!(validate_keys(&target, &overrides, &plan).is_err());
    }

    #[test]
    fn scalar_fresh_variables_reject_manual_and_auto_buffer_flags() {
        let target = spec(&[("n", FreshVar)]);
        for entry in ["n=4", "n=auto"] {
            for overrides in [buffers(&[entry], &[]), buffers(&[], &[entry])] {
                let error = raw(&target, &overrides).unwrap_err();
                assert!(error.to_string().contains("scalar FreshVar"));
            }
        }
        assert!(raw(&target, &BufferOverrides::default())
            .unwrap()
            .is_empty());
    }

    #[test]
    fn compiler_objects_can_replace_legacy_fresh_variable_classification() {
        let config = active("this.choice", "small");
        let plan = derived("Host", "this", &config);
        let mut overrides = buffers(&[], &["this=auto"]);
        overrides.layout_config = config;
        let target = spec(&[("this", FreshVar)]);
        assert!(validate_keys(&target, &overrides, &plan)
            .unwrap()
            .is_empty());
    }

    #[test]
    fn raw_manual_extents_warn_instead_of_rejecting_scalar_pointers() {
        let target = spec(&[("p", AllocReadonly), ("q", AllocMutable)]);
        for shape in ["4", "i32", "2xi16", "{i16,i16}", "struct:Raw"] {
            let overrides = buffers(&[&format!("p={shape}")], &[&format!("q={shape}")]);
            let warnings = raw(&target, &overrides).unwrap();
            assert_eq!(warnings.len(), 2);
            for (name, warning) in ["p", "q"].iter().zip(&warnings) {
                assert!(warning.starts_with(&format!("{name}:")));
                assert!(warning.contains("unvalidated raw allocation assumption"));
                assert!(warning.contains("manual extent/shape"));
                assert!(warning.contains("no compiler-derived C++ aggregate extent"));
                assert!(warning.contains("proof is limited to caller-provided storage"));
            }
        }
    }

    #[test]
    fn raw_auto_allocations_are_explicitly_scoped_and_warnings_are_deduplicated() {
        let target = spec(&[("p", AllocReadonly)]);
        let warnings = raw(&target, &buffers(&["p=auto"], &[])).unwrap();
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("inferred scalar/pointee shape llvm_int 32"));
        let warnings = raw(&target, &buffers(&["p=4", "p=auto"], &[])).unwrap();
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("manual extent/shape"));
    }

    #[test]
    fn active_keys_require_the_exact_consumed_union_path() {
        let config = active("this.choice", "small");
        let plan = derived("Host", "this", &config);
        let target = spec(&[("this", AllocMutable)]);
        let note = "this.choice: selected actual compiler union member small";
        assert!(plan.objects["this"]
            .layout
            .validation
            .contains(&note.into()));
        for key in [
            "this.no_such_union",
            "this",
            "this.choice2",
            "this.choice.small",
            "this.choice[0]",
            "missing.choice",
        ] {
            let mut overrides = BufferOverrides {
                layout_config: config.clone(),
                ..Default::default()
            };
            overrides
                .layout_config
                .active_members
                .insert(key.into(), "small".into());
            let error = validate_keys(&target, &overrides, &plan).unwrap_err();
            assert!(error.to_string().contains(key), "{error}");
        }
    }

    #[test]
    fn selection_evidence_requires_the_exact_compiler_note_prefix() {
        let config = active("this.choice", "small");
        let mut plan = derived("Host", "this", &config);
        for note in [
            "this.choice2: selected actual compiler union member small",
            "this.choice: optional flag small identified from compiler members",
            "this.choice: selected actual compiler union member",
        ] {
            plan.objects.get_mut("this").unwrap().layout.validation = vec![note.into()];
            assert!(validate_active_members(&config, &plan).is_err());
        }
        plan.objects
            .get_mut("this")
            .unwrap()
            .layout
            .validation
            .clear();
        assert!(validate_active_members(&config, &plan).is_err());
    }

    #[test]
    fn nonexistent_members_are_rejected_during_derivation() {
        for member in ["", "smal", "small.extra", "missing"] {
            let config = active("this.choice", member);
            assert!(derive::derive(&compiler_facts(), IR, "Host", "this", &config).is_err());
        }
    }

    #[test]
    fn root_return_and_empty_selection_notes_do_not_require_flattened_leaves() {
        for region in ["this", "return"] {
            let config = active(region, "small");
            let mut plan = derived("U", region, &config);
            validate_active_members(&config, &plan).unwrap();
            // The key validator must also work when a selected empty member
            // produces no scalar leaves; consumption notes are the evidence.
            plan.objects.get_mut(region).unwrap().layout.fields.clear();
            validate_active_members(&config, &plan).unwrap();
        }
    }

    #[test]
    fn optional_payload_selection_is_automatic_and_needs_no_config_key() {
        let plan = derived("std::optional<int>", "this", &LayoutConfig::default());
        let target = spec(&[("this", AllocReadonly)]);
        let fields = &plan.objects["this"].layout.fields;
        assert!(fields.iter().any(|f| f.path == "value"));
        let warnings = validate_keys(&target, &BufferOverrides::default(), &plan).unwrap();
        assert!(warnings.is_empty());
        for key in ["this", "this.value", "this.no_such_union"] {
            assert!(validate_active_members(&active(key, "_Value"), &plan).is_err());
        }
    }

    #[test]
    fn pipeline_keeps_compiler_shape_offset_and_alias_assertions_in_finish() {
        let config = active("this.choice", "small");
        let mut plan = derived("Host", "this", &config);
        let mut overrides = buffers(&[], &["this=4"]);
        overrides.layout_config = config;
        let target = spec(&[("this", AllocMutable)]);
        overrides
            .layout_config
            .offsets
            .insert("this.choice.small".into(), 0);
        let object = plan.objects.get_mut("this").unwrap();
        object.configured_shape = overrides.override_saw_type("this");
        validate::validate_object(object, IR).unwrap();
        assert!(validate_keys(&target, &overrides, &plan)
            .unwrap()
            .is_empty());
        let config = &mut overrides.layout_config;
        validate::finish(&mut plan, config, &[], None, &["Host=4".into()]).unwrap();
        assert!(validate::finish(&mut plan, config, &[], None, &["Host=5".into()]).is_err());
        config.offsets.insert("this.no_such_field".into(), 0);
        assert!(validate::finish(&mut plan, config, &[], None, &[]).is_err());
        let object = plan.objects.get_mut("this").unwrap();
        object.configured_shape = Some("llvm_array 5 (llvm_int 8)".into());
        assert!(validate::validate_object(object, IR).is_err());
    }
}
