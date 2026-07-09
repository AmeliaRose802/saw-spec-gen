//! Unit tests for [`super`] (`llvm_spec`), split out to keep the
//! generator module under the 500 non-whitespace line limit.

use super::super::writer::VOID_SAW_TYPE;
use super::*;
use crate::constraints::{ParamConstraint, ReturnConstraint};

fn make_spec(
    name: &str,
    params: Vec<ParamConstraint>,
    return_type: &str,
    can_throw: bool,
) -> SpecConstraint {
    SpecConstraint {
        function_name: name.into(),
        mangled_name: None,
        params,
        return_constraint: ReturnConstraint {
            saw_type: return_type.into(),
            value_constraints: vec![],
            is_sret: false,
            returns_pointer: false,
            sret_prestate: false,
        },
        can_throw,
        is_virtual: false,
        has_body: true,
        referenced_globals: vec![],
        postconditions: vec![],
    }
}

fn make_param(name: &str, alloc: AllocType, ty: &str, unchanged: bool) -> ParamConstraint {
    ParamConstraint {
        name: name.into(),
        alloc_type: alloc,
        saw_type: ty.into(),
        preconditions: vec![],
        unchanged_after: unchanged,
        dereferenceable_size: None,
        out_postcond: None,
    }
}

#[test]
fn test_generate_llvm_spec_readonly() {
    let spec = make_spec(
        "test_fn",
        vec![make_param(
            "x",
            AllocType::AllocReadonly,
            "llvm_int 32",
            true,
        )],
        VOID_SAW_TYPE,
        false,
    );
    let output = generate_saw_spec(&spec, &spec.referenced_globals);
    assert!(output.contains("LLVMSetup ()"));
    assert!(output.contains("llvm_alloc_readonly"));
    assert!(output.contains("llvm_fresh_var"));
    assert!(output.contains("llvm_execute_func"));
}

#[test]
fn test_generate_llvm_spec_mutable() {
    let spec = make_spec(
        "mutate_fn",
        vec![make_param(
            "buf",
            AllocType::AllocMutable,
            "llvm_int 64",
            false,
        )],
        VOID_SAW_TYPE,
        false,
    );
    let output = generate_saw_spec(&spec, &spec.referenced_globals);
    assert!(output.contains("llvm_alloc (llvm_int 64)"));
    assert!(!output.contains("llvm_alloc_readonly"));
}

#[test]
fn test_generate_llvm_spec_freshvar() {
    let spec = make_spec(
        "add",
        vec![
            make_param("a", AllocType::FreshVar, "llvm_int 32", false),
            make_param("b", AllocType::FreshVar, "llvm_int 32", false),
        ],
        "llvm_int 32",
        false,
    );
    let output = generate_saw_spec(&spec, &spec.referenced_globals);
    assert!(output.contains("a <- llvm_fresh_var \"a\" (llvm_int 32)"));
    assert!(output.contains("llvm_term a, llvm_term b"));
}

#[test]
fn test_generate_llvm_spec_return() {
    let spec = make_spec("get_val", vec![], "llvm_int 32", false);
    let output = generate_saw_spec(&spec, &spec.referenced_globals);
    assert!(output.contains("ret <- llvm_fresh_var \"ret\" (llvm_int 32)"));
    assert!(output.contains("llvm_return (llvm_term ret)"));
    assert!(output.contains("ACTION REQUIRED"));
    assert!(output.contains("llvm_verify"));
    assert!(!output.contains("llvm_unsafe_assume_spec"));
}

#[test]
fn test_generate_llvm_spec_void_return() {
    let spec = make_spec("noop", vec![], VOID_SAW_TYPE, false);
    let output = generate_saw_spec(&spec, &spec.referenced_globals);
    assert!(!output.contains("llvm_return"));
}

#[test]
fn test_generate_llvm_spec_can_throw() {
    let spec = make_spec("risky", vec![], VOID_SAW_TYPE, true);
    let output = generate_saw_spec(&spec, &spec.referenced_globals);
    assert!(output.contains("WARNING: Function may throw"));
}

#[test]
fn test_emit_saw_specs_creates_files() {
    let dir = std::env::temp_dir().join("saw_spec_gen_test_emit");
    let _ = fs::remove_dir_all(&dir);
    let specs = vec![make_spec("test_fn", vec![], VOID_SAW_TYPE, false)];
    emit_saw_specs(&specs, &dir, false).unwrap();
    assert!(dir.join("test_fn_auto_spec.saw").exists());
    assert!(dir.join("auto_specs.saw").exists());
    let index = fs::read_to_string(dir.join("auto_specs.saw")).unwrap();
    assert!(index.contains("include \"test_fn_auto_spec.saw\""));
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn test_generate_postconditions() {
    let spec = SpecConstraint {
        function_name: "read_fn".into(),
        mangled_name: None,
        params: vec![make_param(
            "data",
            AllocType::AllocReadonly,
            "llvm_int 32",
            true,
        )],
        return_constraint: ReturnConstraint {
            saw_type: VOID_SAW_TYPE.into(),
            value_constraints: vec![],
            is_sret: false,
            returns_pointer: false,
            sret_prestate: false,
        },
        can_throw: false,
        is_virtual: false,
        has_body: true,
        referenced_globals: vec![],
        postconditions: vec!["llvm_points_to data_ptr (llvm_term data_before)".into()],
    };
    let output = generate_saw_spec(&spec, &spec.referenced_globals);
    assert!(output.contains("Postconditions"));
    assert!(output.contains("data_before"));
}

#[test]
fn unspecified_spec_pins_status_primitive_sentinel() {
    // A declared-only mutex primitive must get a pinned success
    // sentinel return (0 = _Thrd_success), not a fresh symbolic one.
    let mut spec = make_spec(
        "_Mtx_lock",
        vec![make_param(
            "m",
            AllocType::AllocMutable,
            "llvm_int 32",
            false,
        )],
        "llvm_int 32",
        false,
    );
    spec.has_body = false;
    let output = generate_unspecified_spec(&spec, &spec.referenced_globals);
    assert!(
        output.contains("llvm_return (llvm_term {{ 0 : [32] }})"),
        "expected pinned sentinel return, got:\n{output}"
    );
    assert!(
        !output.contains("ret <- llvm_fresh_var \"ret\""),
        "sentinel primitive must not emit a fresh symbolic return"
    );
    assert!(output.contains("_Thrd_success"));
}

#[test]
fn unspecified_spec_leaves_ordinary_return_symbolic() {
    // A non-primitive external keeps its fresh symbolic return.
    let mut spec = make_spec("compute_checksum", vec![], "llvm_int 32", false);
    spec.has_body = false;
    let output = generate_unspecified_spec(&spec, &spec.referenced_globals);
    assert!(output.contains("ret <- llvm_fresh_var \"ret\" (llvm_int 32)"));
    assert!(!output.contains("_Thrd_success"));
}

#[test]
fn unspecified_spec_sret_includes_result_ptr_in_execute_func() {
    // A sub-callee that returns std::string by value (sret ABI) must have
    // `result_ptr` as the first argument in `llvm_execute_func`. Without
    // this fix, SAW fails with "Argument 1 unspecified".
    let mut spec = SpecConstraint {
        function_name: "canonicalizePayload".into(),
        mangled_name: Some(
            "?canonicalizePayload@sdep@@YA?AV?$basic_string@DU?$char_traits@D@std@@\
             V?$allocator@D@2@@std@@AEBUDeviceRequest@1@@Z"
                .into(),
        ),
        params: vec![make_param(
            "request",
            AllocType::AllocReadonly,
            "llvm_alias \"%struct.Req\"",
            true,
        )],
        return_constraint: ReturnConstraint {
            saw_type: "llvm_alias \"%class.std__basic_string\"".into(),
            value_constraints: vec![],
            is_sret: true,
            returns_pointer: false,
            sret_prestate: false,
        },
        can_throw: false,
        is_virtual: false,
        has_body: false,
        referenced_globals: vec![],
        postconditions: vec![],
    };
    spec.has_body = false;
    let output = generate_unspecified_spec(&spec, &spec.referenced_globals);
    // The hidden sret pointer must be allocated in the pre-state.
    assert!(
        output.contains("result_ptr <- llvm_alloc (llvm_alias \"%class.std__basic_string\")"),
        "expected result_ptr alloc, got:\n{output}",
    );
    // result_ptr must be the FIRST argument in llvm_execute_func.
    let exec_line = output
        .lines()
        .find(|l| l.contains("llvm_execute_func"))
        .expect("missing llvm_execute_func");
    let result_pos = exec_line
        .find("result_ptr")
        .expect("result_ptr not in exec args");
    let request_pos = exec_line
        .find("request_ptr")
        .expect("request_ptr not in exec args");
    assert!(
        result_pos < request_pos,
        "result_ptr must precede request_ptr in llvm_execute_func: {exec_line}",
    );
    // Post-state must write via llvm_points_to, not llvm_return.
    assert!(
        output.contains("llvm_points_to result_ptr (llvm_term ret)"),
        "expected llvm_points_to result_ptr, got:\n{output}",
    );
    assert!(
        !output.contains("llvm_return (llvm_term ret)"),
        "sret spec must not emit llvm_return for the struct return, got:\n{output}",
    );
}

#[test]
fn unspecified_spec_sret_with_no_params_still_has_result_ptr() {
    // Even when there are no explicit parameters, sret must still supply
    // result_ptr as the sole argument to llvm_execute_func.
    let spec = SpecConstraint {
        function_name: "makeDefault".into(),
        mangled_name: None,
        params: vec![],
        return_constraint: ReturnConstraint {
            saw_type: "llvm_alias \"%struct.Point\"".into(),
            value_constraints: vec![],
            is_sret: true,
            returns_pointer: false,
            sret_prestate: false,
        },
        can_throw: false,
        is_virtual: false,
        has_body: false,
        referenced_globals: vec![],
        postconditions: vec![],
    };
    let output = generate_unspecified_spec(&spec, &spec.referenced_globals);
    assert!(
        output.contains("llvm_execute_func [result_ptr]"),
        "expected exactly result_ptr in exec args, got:\n{output}",
    );
}
