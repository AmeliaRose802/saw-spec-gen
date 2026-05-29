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
