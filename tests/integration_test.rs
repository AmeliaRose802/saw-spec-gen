//! Integration tests for saw-spec-gen with MSP-like C++ and Rust inputs.

use std::process::Command;

fn saw_spec_gen_binary() -> std::path::PathBuf {
    let mut path = std::env::current_exe().unwrap();
    path.pop(); // remove test binary name
    path.pop(); // remove "deps"
    path.push("saw-spec-gen");
    if cfg!(windows) {
        path.set_extension("exe");
    }
    path
}

// ============================================================================
// C++ RequestValidator Integration Tests
// ============================================================================

#[test]
fn test_cpp_request_validator_generates_specs() {
    let output_dir = std::env::temp_dir().join("saw_spec_gen_integ_cpp");
    let _ = std::fs::remove_dir_all(&output_dir);

    let status = Command::new(saw_spec_gen_binary())
        .args([
            "from-clang-ast",
            "--input",
            "tests/fixtures/request_validator_cpp.json",
            "--output",
            output_dir.to_str().unwrap(),
        ])
        .status()
        .expect("Failed to run saw-spec-gen");

    assert!(status.success(), "saw-spec-gen exited with error");
    assert!(output_dir.join("auto_specs.saw").exists());

    // Check that specs were generated for all functions
    let index = std::fs::read_to_string(output_dir.join("auto_specs.saw")).unwrap();
    assert!(index.contains("ValidateRequest_auto_spec.saw"));
    assert!(index.contains("ValidateContentLength_auto_spec.saw"));
    assert!(index.contains("CopyHeaderData_auto_spec.saw"));
    assert!(index.contains("ComputeChecksum_auto_spec.saw"));
    assert!(index.contains("clamp_value_auto_spec.saw")); // Template specialization

    let _ = std::fs::remove_dir_all(&output_dir);
}

#[test]
fn test_cpp_validate_request_spec_content() {
    let output_dir = std::env::temp_dir().join("saw_spec_gen_integ_cpp_content");
    let _ = std::fs::remove_dir_all(&output_dir);

    Command::new(saw_spec_gen_binary())
        .args([
            "from-clang-ast",
            "--input",
            "tests/fixtures/request_validator_cpp.json",
            "--output",
            output_dir.to_str().unwrap(),
        ])
        .status()
        .unwrap();

    // ValidateRequest: const method with noexcept, const pointer param with _In_
    let spec = std::fs::read_to_string(output_dir.join("ValidateRequest_auto_spec.saw")).unwrap();
    assert!(spec.contains("LLVMSetup ()"));
    assert!(spec.contains("llvm_alloc_readonly")); // this is const, header is _In_
    assert!(!spec.contains("WARNING: Function may throw")); // noexcept

    // CopyHeaderData: non-const method with _In_ and _Out_writes_
    let spec = std::fs::read_to_string(output_dir.join("CopyHeaderData_auto_spec.saw")).unwrap();
    assert!(spec.contains("llvm_alloc_readonly")); // src is _In_
    assert!(spec.contains("llvm_alloc (")); // dst is _Out_writes_ (mutable)

    // ComputeChecksum: has NoThrowAttr and _In_reads_bytes_
    let spec = std::fs::read_to_string(output_dir.join("ComputeChecksum_auto_spec.saw")).unwrap();
    assert!(!spec.contains("WARNING: Function may throw"));

    let _ = std::fs::remove_dir_all(&output_dir);
}

#[test]
fn test_cpp_filter_functions() {
    let output_dir = std::env::temp_dir().join("saw_spec_gen_integ_cpp_filter");
    let _ = std::fs::remove_dir_all(&output_dir);

    let status = Command::new(saw_spec_gen_binary())
        .args([
            "from-clang-ast",
            "--input",
            "tests/fixtures/request_validator_cpp.json",
            "--output",
            output_dir.to_str().unwrap(),
            "--filter",
            "Validate",
        ])
        .status()
        .unwrap();

    assert!(status.success());
    let index = std::fs::read_to_string(output_dir.join("auto_specs.saw")).unwrap();
    assert!(index.contains("ValidateRequest"));
    assert!(index.contains("ValidateContentLength"));
    assert!(!index.contains("ComputeChecksum"));

    let _ = std::fs::remove_dir_all(&output_dir);
}

#[test]
fn test_cpp_struct_field_resolution() {
    let output_dir = std::env::temp_dir().join("saw_spec_gen_integ_cpp_struct");
    let _ = std::fs::remove_dir_all(&output_dir);

    Command::new(saw_spec_gen_binary())
        .args([
            "from-clang-ast",
            "--input",
            "tests/fixtures/request_validator_cpp.json",
            "--output",
            output_dir.to_str().unwrap(),
            "--filter",
            "ValidateRequest",
        ])
        .status()
        .unwrap();

    let spec = std::fs::read_to_string(output_dir.join("ValidateRequest_auto_spec.saw")).unwrap();

    // The struct types should be resolved, not just "Opaque"
    assert!(spec.contains("this")); // implicit this parameter

    let _ = std::fs::remove_dir_all(&output_dir);
}

#[test]
fn test_cpp_cryptol_generation() {
    let output_dir = std::env::temp_dir().join("saw_spec_gen_integ_cpp_cry");
    let _ = std::fs::remove_dir_all(&output_dir);

    let status = Command::new(saw_spec_gen_binary())
        .args([
            "from-clang-ast",
            "--input",
            "tests/fixtures/request_validator_cpp.json",
            "--output",
            output_dir.to_str().unwrap(),
            "--cryptol",
        ])
        .status()
        .unwrap();

    assert!(status.success());
    assert!(output_dir.join("auto_constraints.cry").exists());

    let _ = std::fs::remove_dir_all(&output_dir);
}

// ============================================================================
// Rust request_validator Integration Tests
// ============================================================================

#[test]
fn test_rust_request_validator_generates_specs() {
    let output_dir = std::env::temp_dir().join("saw_spec_gen_integ_rust");
    let _ = std::fs::remove_dir_all(&output_dir);

    let status = Command::new(saw_spec_gen_binary())
        .args([
            "from-mir-json",
            "--input",
            "tests/fixtures/request_validator_rust.json",
            "--output",
            output_dir.to_str().unwrap(),
        ])
        .status()
        .expect("Failed to run saw-spec-gen");

    assert!(status.success(), "saw-spec-gen exited with error");
    assert!(output_dir.join("auto_specs.saw").exists());

    let index = std::fs::read_to_string(output_dir.join("auto_specs.saw")).unwrap();
    assert!(index.contains("is_valid_metadata_header"));
    assert!(index.contains("is_valid_request_date"));
    assert!(index.contains("is_valid_claims_header"));
    assert!(index.contains("latch_local"));
    assert!(index.contains("latch_key_local"));
    assert!(index.contains("validate_request"));

    let _ = std::fs::remove_dir_all(&output_dir);
}

#[test]
fn test_rust_latch_local_spec_content() {
    let output_dir = std::env::temp_dir().join("saw_spec_gen_integ_rust_latch");
    let _ = std::fs::remove_dir_all(&output_dir);

    Command::new(saw_spec_gen_binary())
        .args([
            "from-mir-json",
            "--input",
            "tests/fixtures/request_validator_rust.json",
            "--output",
            output_dir.to_str().unwrap(),
        ])
        .status()
        .unwrap();

    // latch_local takes &mut LatchState, returns Result<LatchState, ValidationError>
    let spec = std::fs::read_to_string(output_dir.join("latch_local_auto_spec.saw")).unwrap();
    assert!(spec.contains("LLVMSetup ()"));
    assert!(spec.contains("llvm_alloc (")); // mutable ref

    let _ = std::fs::remove_dir_all(&output_dir);
}

#[test]
fn test_rust_enum_discriminant_constraints() {
    let output_dir = std::env::temp_dir().join("saw_spec_gen_integ_rust_enum");
    let _ = std::fs::remove_dir_all(&output_dir);

    Command::new(saw_spec_gen_binary())
        .args([
            "from-mir-json",
            "--input",
            "tests/fixtures/request_validator_rust.json",
            "--output",
            output_dir.to_str().unwrap(),
            "--cryptol",
        ])
        .status()
        .unwrap();

    // Cryptol constraints should have enum predicates
    let cry = std::fs::read_to_string(output_dir.join("auto_constraints.cry")).unwrap();
    assert!(cry.contains("LatchState"));
    assert!(cry.contains("ValidationError"));

    let _ = std::fs::remove_dir_all(&output_dir);
}

#[test]
fn test_rust_mir_verify_mode() {
    let output_dir = std::env::temp_dir().join("saw_spec_gen_integ_rust_mir");
    let _ = std::fs::remove_dir_all(&output_dir);

    let status = Command::new(saw_spec_gen_binary())
        .args([
            "from-mir-json",
            "--input",
            "tests/fixtures/request_validator_rust.json",
            "--output",
            output_dir.to_str().unwrap(),
            "--mir-verify",
        ])
        .status()
        .unwrap();

    assert!(status.success());

    let spec = std::fs::read_to_string(output_dir.join("latch_local_auto_spec.saw")).unwrap();
    assert!(spec.contains("MIRSetup ()"));
    assert!(spec.contains("mir_alloc"));
    assert!(spec.contains("mir_execute_func"));

    let index = std::fs::read_to_string(output_dir.join("auto_specs.saw")).unwrap();
    assert!(index.contains("Mode: MIR"));

    let _ = std::fs::remove_dir_all(&output_dir);
}

#[test]
fn test_rust_async_function_handling() {
    let output_dir = std::env::temp_dir().join("saw_spec_gen_integ_rust_async");
    let _ = std::fs::remove_dir_all(&output_dir);

    Command::new(saw_spec_gen_binary())
        .args([
            "from-mir-json",
            "--input",
            "tests/fixtures/request_validator_rust.json",
            "--output",
            output_dir.to_str().unwrap(),
            "--filter",
            "validate_request",
        ])
        .status()
        .unwrap();

    // validate_request is async, should generate a spec with coroutine return type
    let spec = std::fs::read_to_string(output_dir.join("validate_request_auto_spec.saw")).unwrap();
    assert!(spec.contains("LLVMSetup ()"));

    let _ = std::fs::remove_dir_all(&output_dir);
}

// ============================================================================
// gen-verify-rust async auto-detection integration tests
// ============================================================================

/// `gen-verify-rust` must auto-detect an `async fn` from the LLVM IR
/// (no extra flag required) and emit a `mir_verify` script targeting
/// the `_RNC`-prefixed coroutine resume symbol.
#[test]
fn test_gen_verify_rust_async_auto_detect() {
    let output_dir = std::env::temp_dir().join("saw_spec_gen_integ_gvr_async_detect");
    let _ = std::fs::remove_dir_all(&output_dir);

    // Dummy bitcode file (gen-verify-rust uses only its filename for the script).
    let bc_path = std::env::temp_dir().join("async_add_one_dummy.bc");
    let _ = std::fs::write(&bc_path, b"BC");

    let status = Command::new(saw_spec_gen_binary())
        .args([
            "gen-verify-rust",
            "--llvm-ir",
            "tests/fixtures/async_add_one.ll",
            "--bitcode",
            bc_path.to_str().unwrap(),
            "--cryptol-spec",
            "tests/fixtures/async_add_one_spec.cry",
            "--cryptol-fn",
            "add_one_spec",
            "--function",
            "add_one",
            "--output",
            output_dir.to_str().unwrap(),
        ])
        .status()
        .expect("Failed to run saw-spec-gen");

    assert!(status.success(), "gen-verify-rust exited with error");

    // Script must exist.
    let saw_path = output_dir.join("verify_rust.saw");
    assert!(saw_path.exists(), "verify_rust.saw not generated");

    let saw = std::fs::read_to_string(&saw_path).unwrap();

    // Must be a mir_verify script (async path), not llvm_verify (sync path).
    assert!(
        saw.contains("mir_verify"),
        "missing mir_verify in async SAW script:\n{saw}"
    );
    assert!(
        !saw.contains("llvm_verify"),
        "async script must not use llvm_verify:\n{saw}"
    );

    // Must target the _RNC resume symbol.
    assert!(
        saw.contains("_RNCNvCs1234_8async_fn7add_one0Bc_"),
        "missing _RNC resume symbol in mir_verify call:\n{saw}"
    );

    let _ = std::fs::remove_file(&bc_path);
    let _ = std::fs::remove_dir_all(&output_dir);
}

/// meta.json must carry `"async": true` and `"resume_symbol"` when the
/// async path is taken.
#[test]
fn test_gen_verify_rust_async_meta_json() {
    let output_dir = std::env::temp_dir().join("saw_spec_gen_integ_gvr_async_meta");
    let _ = std::fs::remove_dir_all(&output_dir);

    let bc_path = std::env::temp_dir().join("async_add_one_meta_dummy.bc");
    let _ = std::fs::write(&bc_path, b"BC");

    Command::new(saw_spec_gen_binary())
        .args([
            "gen-verify-rust",
            "--llvm-ir",
            "tests/fixtures/async_add_one.ll",
            "--bitcode",
            bc_path.to_str().unwrap(),
            "--cryptol-spec",
            "tests/fixtures/async_add_one_spec.cry",
            "--cryptol-fn",
            "add_one_spec",
            "--function",
            "add_one",
            "--output",
            output_dir.to_str().unwrap(),
        ])
        .status()
        .unwrap();

    let meta_path = output_dir.join("verify_rust.meta.json");
    assert!(meta_path.exists(), "verify_rust.meta.json not generated");

    let meta = std::fs::read_to_string(&meta_path).unwrap();

    assert!(
        meta.contains("\"async\": true"),
        "meta.json must have async=true: {meta}"
    );
    assert!(
        meta.contains("_RNC"),
        "meta.json resume_symbol must contain _RNC: {meta}"
    );
    assert!(
        meta.contains("\"function\": \"add_one\""),
        "meta.json must record the source function name: {meta}"
    );

    let _ = std::fs::remove_file(&bc_path);
    let _ = std::fs::remove_dir_all(&output_dir);
}

/// The generated `mir_verify` script must contain `BEGIN_PROOF` and `VERIFIED`
/// so the harness correctly classifies a SAW run as VERIFIED.
#[test]
fn test_gen_verify_rust_async_saw_proof_tokens() {
    let output_dir = std::env::temp_dir().join("saw_spec_gen_integ_gvr_async_tokens");
    let _ = std::fs::remove_dir_all(&output_dir);

    let bc_path = std::env::temp_dir().join("async_add_one_tokens_dummy.bc");
    let _ = std::fs::write(&bc_path, b"BC");

    Command::new(saw_spec_gen_binary())
        .args([
            "gen-verify-rust",
            "--llvm-ir",
            "tests/fixtures/async_add_one.ll",
            "--bitcode",
            bc_path.to_str().unwrap(),
            "--cryptol-spec",
            "tests/fixtures/async_add_one_spec.cry",
            "--cryptol-fn",
            "add_one_spec",
            "--function",
            "add_one",
            "--output",
            output_dir.to_str().unwrap(),
        ])
        .status()
        .unwrap();

    let saw = std::fs::read_to_string(output_dir.join("verify_rust.saw")).unwrap();

    assert!(
        saw.contains("BEGIN_PROOF add_one"),
        "missing BEGIN_PROOF token:\n{saw}"
    );
    assert!(
        saw.contains("PROVED add_one"),
        "missing PROVED token:\n{saw}"
    );
    assert!(saw.contains("VERIFIED"), "missing VERIFIED token:\n{saw}");

    let _ = std::fs::remove_file(&bc_path);

    let _ = std::fs::remove_dir_all(&output_dir);
}

/// A sync (non-async) function in the same IR must still produce an
/// `llvm_verify` script, not a `mir_verify` script.  Ensures that
/// async detection doesn't over-fire.
#[test]
fn test_gen_verify_rust_sync_unaffected_by_async_detection() {
    // Write a minimal IR with only a sync function (no _RNC symbol).
    let ir = "\
define i32 @_RNvCs0_4test7add_one(i32 %x) unnamed_addr {
entry:
  ret i32 0
}
";
    let tmp = std::env::temp_dir().join("saw_spec_gen_integ_gvr_sync_check");
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();

    let ll_path = tmp.join("sync.ll");
    std::fs::write(&ll_path, ir).unwrap();
    let bc_path = tmp.join("sync.bc");
    std::fs::write(&bc_path, b"BC").unwrap();
    let cry_path = tmp.join("spec.cry");
    std::fs::write(
        &cry_path,
        "add_one_spec : [32] -> [32]\nadd_one_spec x = x + 1\n",
    )
    .unwrap();
    let out = tmp.join("out");

    let status = Command::new(saw_spec_gen_binary())
        .args([
            "gen-verify-rust",
            "--llvm-ir",
            ll_path.to_str().unwrap(),
            "--bitcode",
            bc_path.to_str().unwrap(),
            "--cryptol-spec",
            cry_path.to_str().unwrap(),
            "--cryptol-fn",
            "add_one_spec",
            "--function",
            "add_one",
            "--output",
            out.to_str().unwrap(),
        ])
        .status()
        .unwrap();

    assert!(status.success(), "sync gen-verify-rust must succeed");

    let saw = std::fs::read_to_string(out.join("verify_rust.saw")).unwrap();
    assert!(
        saw.contains("llvm_verify"),
        "sync path must use llvm_verify:\n{saw}"
    );
    assert!(
        !saw.contains("mir_verify"),
        "sync path must not use mir_verify:\n{saw}"
    );

    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn test_rust_filter_functions() {
    let output_dir = std::env::temp_dir().join("saw_spec_gen_integ_rust_filter");
    let _ = std::fs::remove_dir_all(&output_dir);

    let status = Command::new(saw_spec_gen_binary())
        .args([
            "from-mir-json",
            "--input",
            "tests/fixtures/request_validator_rust.json",
            "--output",
            output_dir.to_str().unwrap(),
            "--filter",
            "latch",
        ])
        .status()
        .unwrap();

    assert!(status.success());
    let index = std::fs::read_to_string(output_dir.join("auto_specs.saw")).unwrap();
    assert!(index.contains("latch_local"));
    assert!(index.contains("latch_key_local"));
    assert!(!index.contains("is_valid_metadata_header"));

    let _ = std::fs::remove_dir_all(&output_dir);
}
