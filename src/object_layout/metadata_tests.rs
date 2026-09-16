//! Pure evidence comparisons: no compiler, disassembler, SAW, or filesystem I/O.

use super::*;
use crate::object_layout::{ObjectLayout, ObjectPlan, Projection};

const TRIPLE: &str = "x86_64-unknown-linux-gnu";
const LAYOUT: &str = "e-p:64:64-i64:64-f64:64-n8:16:32:64-S128";
const PARAMS: &str = "ptr noundef align 8 dereferenceable(32) %obj, i32 %count";
const IR: &str = r#"target datalayout = "e-p:64:64-i64:64-f64:64-n8:16:32:64-S128"
target triple = "x86_64-unknown-linux-gnu"
%struct.Inner = type { i32, float, ptr }
%struct.Root = type { %struct.Inner, [2 x i64] }
%unused = type { i8 }
define dso_local void @target(ptr noundef align 8 dereferenceable(32) %obj, i32 %count) #0 {
entry:
  %slot = getelementptr inbounds %struct.Root, ptr %obj, i32 0, i32 0
  call void @helper(ptr %slot)
  ret void
}
define internal void @helper(ptr %p) #1 {
  %local = alloca %struct.Inner, align 8
  ret void
}
attributes #0 = { noinline optnone }
attributes #1 = { nounwind }
!llvm.ident = !{!0}
!0 = !{!"clang fixture"}
"#;

fn plan() -> LayoutPlan {
    let object = ObjectPlan {
        region: "obj".into(),
        layout: ObjectLayout {
            source_type: "Root".into(),
            llvm_type: "struct.Root".into(),
            allocation_type: "llvm_alias \"struct.Root\"".into(),
            size: 32,
            alignment: 8,
            llvm_alignment: 8,
            fields: vec![],
            padding: vec![],
            bases: vec![],
            validation: vec![],
            unresolved: vec![],
        },
        projection: Projection::Bytes,
        mutable: true,
        argument_index: 0,
        lowering: "pointer".into(),
        configured_shape: None,
        inferred_shape: None,
        asserted: vec![],
        framed: vec![],
        selectors: BTreeMap::new(),
    };
    LayoutPlan {
        schema_version: 1,
        target_triple: TRIPLE.into(),
        data_layout: LAYOUT.into(),
        llvm_types: BTreeMap::from([
            ("struct.Root".into(), "{ %struct.Inner, [2 x i64] }".into()),
            ("struct.Inner".into(), "{ i32, float, ptr }".into()),
        ]),
        objects: BTreeMap::from([("obj".into(), object)]),
        ..Default::default()
    }
}

fn reject(actual: &str, expected: &str, plan: &LayoutPlan, detail: &str) {
    let error = format!(
        "{:#}",
        check_ir(actual, expected, plan, "target").unwrap_err()
    );
    assert!(error.contains(detail), "expected {detail:?}, got {error}");
}

#[test]
fn accepts_matching_compiled_layout_evidence() {
    check_ir(IR, IR, &plan(), "target").unwrap();
}

#[test]
fn normalizes_whitespace_comments_quoting_and_ssa_parameter_names() {
    let actual = IR
        .replace("%struct.Root", "%\"struct.Root\"")
        .replace("%struct.Inner", "%\"struct.Inner\"")
        .replace("%obj", "%\"renamed argument\"")
        .replace(
            "define dso_local void @target(",
            "define\tdso_local\nvoid @\"target\"(\n",
        )
        .replace(
            "{ %\"struct.Inner\", [2 x i64] }",
            "{\n%\"struct.Inner\", [ 2 x i64 ]\n}",
        )
        .replace("%unused = type { i8 }", "%unused=type{i8}; ignored comment");
    check_ir(&actual, IR, &plan(), "target").unwrap();
}

#[test]
fn ignores_noalias_and_renumbered_non_abi_metadata() {
    let actual = IR
        .replace("ptr noundef", "ptr noalias noundef")
        .replace("#0", "#77")
        .replace("noinline optnone", "nounwind")
        .replace("!0", "!99")
        .replace("clang fixture", "another compiler identity");
    check_ir(&actual, IR, &plan(), "target").unwrap();
}

#[test]
fn rejects_wrong_target_even_when_all_names_and_types_match() {
    for (old, new, message) in [
        (TRIPLE, "aarch64-unknown-linux-gnu", "triple mismatch"),
        ("p:64:64", "p:32:32", "datalayout mismatch"),
        ("e-p:", "E-p:", "datalayout mismatch"),
        ("i64:64-f64:64", "f64:64-i64:64", "datalayout mismatch"),
    ] {
        reject(&IR.replace(old, new), IR, &plan(), message);
    }
}

#[test]
fn target_properties_are_required_and_unique_in_both_inputs() {
    for key in ["triple", "datalayout"] {
        let prefix = format!("target {key}");
        let missing = IR
            .lines()
            .filter(|l| !l.starts_with(&prefix))
            .collect::<Vec<_>>()
            .join("\n");
        reject(&missing, IR, &plan(), key);
        reject(IR, &missing, &plan(), key);
        let line = IR.lines().find(|l| l.starts_with(&prefix)).unwrap();
        reject(&format!("{IR}{line}\n"), IR, &plan(), "duplicate target");
    }
    for empty in ["", "; empty bitcode placeholder\n"] {
        reject(empty, IR, &plan(), "compiled bitcode lacks target");
    }
}

#[test]
fn rejects_stale_plan_target_and_definition_snapshots() {
    let mut stale = plan();
    stale.target_triple = "x86_64-pc-windows-msvc".into();
    reject(IR, IR, &stale, "layout plan target triple");
    let mut stale = plan();
    stale.data_layout = "e-p:32:32".into();
    reject(IR, IR, &stale, "layout plan target datalayout");
    let mut stale = plan();
    stale
        .llvm_types
        .insert("struct.Inner".into(), "{ i32, float, i64 }".into());
    reject(IR, IR, &stale, "layout plan snapshot");
}

#[test]
fn rejects_size_packing_field_order_float_and_pointer_mismatches() {
    for (old, new) in [
        ("[2 x i64]", "[3 x i64]"),
        (
            "{ %struct.Inner, [2 x i64] }",
            "<{ %struct.Inner, [2 x i64] }>",
        ),
        ("{ i32, float, ptr }", "{ float, i32, ptr }"),
        ("{ i32, float, ptr }", "{ i32, i32, ptr }"),
        ("{ i32, float, ptr }", "{ i32, float, i64 }"),
        ("{ i32, float, ptr }", "{ i32, float, ptr addrspace(1) }"),
        ("{ i32, float, ptr }", "opaque"),
        ("%unused = type { i8 }", "%unused = type { i16 }"),
    ] {
        reject(&IR.replace(old, new), IR, &plan(), "LLVM type");
    }
}

#[test]
fn permits_removed_unused_types_and_added_compiler_only_types() {
    let actual = IR.replace("%unused = type { i8 }\n", "") + "%eh.lowered = type { ptr, i32 }\n";
    check_ir(&actual, IR, &plan(), "target").unwrap();
}

#[test]
fn rejects_missing_nested_types_and_does_not_suffix_match_aliases() {
    let actual = IR.replace("%struct.Inner = type { i32, float, ptr }\n", "");
    reject(
        &actual,
        IR,
        &plan(),
        "missing required LLVM type %struct.Inner",
    );
    reject(
        &IR.replace("struct.Root", "struct.Root.1"),
        IR,
        &plan(),
        "missing allocation type %struct.Root",
    );
}

#[test]
fn requires_even_unreferenced_planned_aliases_and_empty_allocation_fallbacks() {
    let expected = IR.replace(
        "getelementptr inbounds %struct.Root",
        "getelementptr inbounds i8",
    );
    let actual = expected.replace("%struct.Root = type { %struct.Inner, [2 x i64] }\n", "");
    reject(
        &actual,
        &expected,
        &plan(),
        "obj: compiled bitcode is missing allocation type",
    );
    let mut fallback = plan();
    fallback
        .objects
        .get_mut("obj")
        .unwrap()
        .layout
        .allocation_type
        .clear();
    reject(&actual, &expected, &fallback, "missing allocation type");
}

#[test]
fn allows_exact_inline_cg_storage_without_inventing_a_module_alias() {
    let ir = format!("target triple = \"{TRIPLE}\"\ntarget datalayout = \"{LAYOUT}\"\ndefine void @target(ptr %obj) {{ ret void }}\n");
    let mut inline = plan();
    inline.llvm_types.clear();
    let object = inline.objects.get_mut("obj").unwrap();
    object.layout.llvm_type = "struct.InlineOnly".into();
    object.layout.allocation_type = "llvm_struct_type [llvm_int 32]".into();
    object.layout.size = 4;
    object.layout.alignment = 4;
    object.layout.llvm_alignment = 4;
    check_ir(&ir, &ir, &inline, "target").unwrap();
    inline
        .objects
        .get_mut("obj")
        .unwrap()
        .layout
        .allocation_type
        .clear();
    reject(
        &ir,
        &ir,
        &inline,
        "missing allocation type %struct.InlineOnly",
    );
}

#[test]
fn rejects_wrong_allocation_alias_and_argument_index() {
    let mut wrong = plan();
    wrong.objects.get_mut("obj").unwrap().layout.allocation_type =
        "llvm_alias \"struct.Inner\"".into();
    reject(IR, IR, &wrong, "allocation alias disagrees");
    for (index, message) in [(1, "not a compiled pointer"), (9, "out of range")] {
        let mut wrong = plan();
        wrong.objects.get_mut("obj").unwrap().argument_index = index;
        reject(IR, IR, &wrong, message);
    }
}

#[test]
fn checks_reachable_callee_types_without_a_planned_object() {
    let expected = IR.replace(
        "%local = alloca %struct.Inner",
        "%local = alloca %callee.only",
    ) + "%callee.only = type { i64 }\n";
    let actual = expected
        .replace("%callee.only = type { i64 }\n", "")
        .replace("  %local = alloca %callee.only, align 8\n", "");
    let mut no_objects = plan();
    no_objects.objects.clear();
    reject(
        &actual,
        &expected,
        &no_objects,
        "missing required LLVM type %callee.only",
    );
}

#[test]
fn follows_global_aliases_and_global_storage_references() {
    let expected = IR
        .replace("@helper(ptr %slot)", "@dispatch(ptr %slot)")
        .replace(
            "%local = alloca %struct.Inner",
            "%local = alloca %callee.only",
        )
        + "%callee.only = type { i64 }\n@dispatch = alias void (ptr), ptr @helper\n";
    let actual = expected
        .replace("%callee.only = type { i64 }\n", "")
        .replace("  %local = alloca %callee.only, align 8\n", "");
    reject(
        &actual,
        &expected,
        &plan(),
        "missing required LLVM type %callee.only",
    );
    let expected = IR.replace(
        "call void @helper(ptr %slot)",
        "call void @helper(ptr @storage)",
    ) + "%global.only = type { i64 }\n@storage = external global %global.only\n";
    let actual = expected
        .replace("%global.only = type { i64 }\n", "")
        .replace("global %global.only", "global i64");
    reject(
        &actual,
        &expected,
        &plan(),
        "missing required LLVM type %global.only",
    );
}

#[test]
fn ignores_unreachable_removed_types_and_terminates_on_recursive_calls() {
    let expected = format!("{IR}%dead.type = type {{ i64 }}\ndefine void @dead() {{\n %x = alloca %dead.type\n ret void\n}}\n");
    check_ir(IR, &expected, &plan(), "target").unwrap();
    let recursive = IR.replace(
        "%local = alloca %struct.Inner, align 8",
        "call void @target(ptr %p, i32 0)",
    );
    check_ir(&recursive, &recursive, &plan(), "target").unwrap();
}

#[test]
fn compares_raw_abi_types_and_attributes_without_lossy_typeinfo_conversion() {
    for (old, new) in [
        (
            "define dso_local void @target",
            "define dso_local i32 @target",
        ),
        (
            "define dso_local void @target",
            "define dso_local fastcc void @target",
        ),
        ("ptr noundef align 8", "i64 noundef align 8"),
        ("ptr noundef align 8", "ptr noundef align 4"),
        ("dereferenceable(32)", "dereferenceable(16)"),
        ("i32 %count", "float %count"),
        ("i32 %count", "i32 signext %count"),
        ("i32 %count", "i32 %count, ..."),
        (") #0 {", ") addrspace(1) #0 {"),
    ] {
        reject(&IR.replace(old, new), IR, &plan(), "ABI mismatch");
    }
}

#[test]
fn preserves_sret_position_type_and_alignment() {
    let signature = "ptr %obj, ptr noalias sret(%struct.Root) align 8 %result";
    let expected = IR.replace(PARAMS, signature);
    let mut sret = plan();
    let mut object = sret.objects["obj"].clone();
    object.region = "return".into();
    object.argument_index = 1;
    object.lowering = "sret".into();
    sret.objects.insert("return".into(), object);
    check_ir(&expected, &expected, &sret, "target").unwrap();
    for new in [
        "ptr noalias sret(%struct.Root) align 8 %result, ptr %obj",
        "ptr %obj, ptr noalias sret(%struct.Inner) align 8 %result",
        "ptr %obj, ptr noalias sret(%struct.Root) align 4 %result",
    ] {
        reject(
            &expected.replace(signature, new),
            &expected,
            &sret,
            "ABI mismatch",
        );
    }
    sret.objects.get_mut("return").unwrap().argument_index = 0;
    reject(&expected, &expected, &sret, "planned sret type/position");
}

#[test]
fn preserves_byval_types_and_rejects_unplanned_abi_lowerings() {
    let expected = IR.replace(PARAMS, "ptr byval(%struct.Root) align 8 %obj");
    let mut byval = plan();
    byval.objects.get_mut("obj").unwrap().lowering = "byval".into();
    check_ir(&expected, &expected, &byval, "target").unwrap();
    reject(
        &expected.replace("byval(%struct.Root)", "byval(%struct.Inner)"),
        &expected,
        &byval,
        "ABI mismatch",
    );
    reject(&expected, &expected, &plan(), "unplanned sret/byval");
    byval.objects.get_mut("obj").unwrap().lowering = "sret".into();
    reject(&expected, &expected, &byval, "planned sret type/position");
}

#[test]
fn quoted_symbols_and_hexadecimal_name_escapes_are_atomic() {
    let name = "target; (%not_a_type)";
    let ir = IR.replace("@target", &format!("@\"{name}\""));
    check_ir(&ir, &ir, &plan(), name).unwrap();
    let actual = IR.replace("%struct.Root", r#"%"struct.R\6Fot""#);
    check_ir(&actual, IR, &plan(), "target").unwrap();
    let decoy = IR.replace("@target", "@target_extra")
        + "@text = private constant [8 x i8] c\"@target\\00\"\n; define void @target() {}\n";
    reject(&decoy, IR, &plan(), "no compiled bitcode signature");
}

#[test]
fn requires_definitions_not_just_matching_declarations() {
    let start = IR.find("define dso_local void @target").unwrap();
    let end = start + IR[start..].find("\n}\n").unwrap() + 3;
    let declaration = format!(
        "{}declare dso_local void @target({PARAMS})\n{}",
        &IR[..start],
        &IR[end..]
    );
    reject(&declaration, IR, &plan(), "definition in both modules");
    reject(IR, &declaration, &plan(), "definition in both modules");
}

#[test]
fn rejects_ambiguous_or_incomplete_evidence_instead_of_skipping_it() {
    for extra in [
        "%struct.Root = type { i8 }\n",
        "%\"struct.Root\" = type { i8 }\n",
        "define void @target() { ret void }\n",
        "%broken = type { i32\n",
        "%broken = type <{ i32 }\n",
        "!9 = !{!\"bad\\ZZescape\"}\n",
    ] {
        assert!(check_ir(&format!("{IR}{extra}"), IR, &plan(), "target").is_err());
        assert!(check_ir(IR, &format!("{IR}{extra}"), &plan(), "target").is_err());
    }
    let grouped = IR.replace("ptr noundef", "ptr #12 noundef");
    reject(&grouped, &grouped, &plan(), "grouped ABI attributes");
    reject(
        &IR.replace("i32 %count)", "i32 %count,)"),
        IR,
        &plan(),
        "trailing LLVM parameter comma",
    );
}
