//! Exercise the public typed entry point with real IR file reads.

use super::*;
use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

struct IrFixture(PathBuf);

impl IrFixture {
    fn new(ir: &str) -> Self {
        static NEXT_ID: AtomicU64 = AtomicU64::new(0);
        loop {
            let path = std::env::temp_dir().join(format!(
                "saw-spec-gen-typed-overrides-{}-{}.ll",
                std::process::id(),
                NEXT_ID.fetch_add(1, Ordering::Relaxed)
            ));
            let mut file = match std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)
            {
                Ok(file) => file,
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(e) => panic!("could not create IR fixture: {e}"),
            };
            file.write_all(ir.as_bytes()).unwrap();
            return Self(path);
        }
    }
}

impl Drop for IrFixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

const MUTEX_WRAPPER: &str = "?lock@_Mutex_base@std@@QEAAXXZ";
const STL_HELPER: &str = "_ZNKSt6vectorIiSaIiEE4sizeEv";
const DEFINED_HELPERS: &str = r#"
define i64 @target(ptr %self) {
  call void @"?lock@_Mutex_base@std@@QEAAXXZ"(ptr %self)
  %size = call i64 @_ZNKSt6vectorIiSaIiEE4sizeEv(ptr %self)
  ret i64 %size
}
define linkonce_odr void @"?lock@_Mutex_base@std@@QEAAXXZ"(ptr %self) {
  %status = call i32 @_Mtx_lock(ptr %self)
  %ok = icmp eq i32 %status, 0
  br i1 %ok, label %locked, label %failed
locked:
  ret void
failed:
  call void @_Throw_Cpp_error(i32 %status)
  unreachable
}
define linkonce_odr i64 @_ZNKSt6vectorIiSaIiEE4sizeEv(ptr %self) {
  %size = load i64, ptr %self
  ret i64 %size
}
declare i32 @_Mtx_lock(ptr)
declare void @_Throw_Cpp_error(i32) noreturn
"#;

#[test]
fn typed_entry_point_attempts_defined_helpers_but_keeps_runtime_contracts() {
    let ir = IrFixture::new(DEFINED_HELPERS);
    let catalog = ContainerCatalog::default();
    let typed = scan_and_emit_typed(Some(&ir.0), "target", &[], &[], &catalog);
    assert_eq!(typed.override_names.len(), 2);
    assert!(!typed.snippet.contains(MUTEX_WRAPPER));
    assert!(!typed.snippet.contains(STL_HELPER));
    for symbol in ["_Mtx_lock", "_Throw_Cpp_error"] {
        assert!(typed
            .snippet
            .contains(&format!("llvm_unsafe_assume_spec m \"{symbol}\"")));
    }
    assert!(typed
        .snippet
        .contains("llvm_return (llvm_term {{ 0 : [32] }});"));
    assert!(typed.snippet.contains("llvm_postcond {{ False }};"));
    assert!(
        !typed.snippet.contains("p0_after"),
        "retain mutex pointer preservation"
    );
    let legacy = scan_and_emit(Some(&ir.0), "target", &[], &[], &catalog);
    assert!(legacy.snippet.contains(MUTEX_WRAPPER));
    if super::super::stl_overrides::matches(STL_HELPER) {
        assert!(legacy.snippet.contains(STL_HELPER));
    }
}

#[test]
fn typed_entry_point_keeps_declared_helpers_and_variadic_body_emission() {
    let ir = IrFixture::new(
        r#"
define void @target(ptr %p) {
  %ok = call i1 @"?_Verify_ownership_levels@_Mutex_base@std@@IEAA_NXZ"(ptr %p)
  %rv = call i32 (ptr, ...) @logger(ptr %p, i32 7)
  ret void
}
declare i1 @"?_Verify_ownership_levels@_Mutex_base@std@@IEAA_NXZ"(ptr)
define i32 @logger(ptr %fmt, ...) {
  %ap = alloca ptr
  call void @llvm.va_start.p0(ptr %ap)
  call void @llvm.va_end.p0(ptr %ap)
  ret i32 0
}
declare void @llvm.va_start.p0(ptr)
declare void @llvm.va_end.p0(ptr)
"#,
    );
    let catalog = ContainerCatalog::default();
    let typed = scan_and_emit_typed(Some(&ir.0), "target", &[], &[], &catalog);
    let legacy = scan_and_emit(Some(&ir.0), "target", &[], &[], &catalog);
    assert_eq!(typed.override_names.len(), 2);
    assert_eq!(typed.override_names, legacy.override_names);
    assert_eq!(typed.snippet, legacy.snippet);
    assert!(typed.snippet.contains("[declare-only]"));
    assert!(typed.snippet.contains("{{ 1 : [1] }}"));
    assert!(typed.snippet.contains("[body uses llvm.va_*; variadic]"));
    assert!(typed.snippet.contains("llvm_execute_func [p0];"));
    assert!(typed.snippet.contains("p0_after <- llvm_fresh_var"));
}

fn global(name: &str, ty: TypeInfo) -> GlobalVarInfo {
    GlobalVarInfo {
        name: name.to_string(),
        mangled_name: name.to_string(),
        ty,
        init_value: None,
        has_static_initializer: false,
    }
}

#[test]
fn typed_entry_point_keeps_coverage_global_filters_and_memcmp_ownership() {
    let ir = IrFixture::new(
        r#"
@state = global i32 0
@hidden = internal global i32 0
@immutable = constant i32 0
@pointer_state = global ptr null
define void @target(ptr %a, ptr %b) {
  call void @external(ptr %a)
  call void @covered(ptr %a)
  %rv = call i32 @memcmp(ptr %a, ptr %b, i64 16)
  ret void
}
declare void @external(ptr)
declare void @covered(ptr)
declare i32 @memcmp(ptr, ptr, i64)
"#,
    );
    let globals = [
        global("state", TypeInfo::UnsignedInt(32)),
        global("hidden", TypeInfo::UnsignedInt(32)),
        global("immutable", TypeInfo::UnsignedInt(32)),
        global("absent", TypeInfo::UnsignedInt(32)),
        global(
            "pointer_state",
            TypeInfo::Pointer(Box::new(TypeInfo::UnsignedInt(8))),
        ),
    ];
    let catalog = ContainerCatalog::default();
    let covered = ["covered".to_string(), "memcmp".to_string()];
    let typed = scan_and_emit_typed(Some(&ir.0), "target", &covered, &globals, &catalog);
    let legacy = scan_and_emit(Some(&ir.0), "target", &covered, &globals, &catalog);
    assert_eq!(typed.override_names.len(), 2);
    assert_eq!(typed.override_names, legacy.override_names);
    assert_eq!(typed.snippet, legacy.snippet);
    assert!(!typed
        .snippet
        .contains("llvm_unsafe_assume_spec m \"covered\""));
    assert!(typed
        .snippet
        .contains("llvm_unsafe_assume_spec m \"memcmp\""));
    assert!(typed.snippet.contains("llvm_global \"state\""));
    for name in ["hidden", "immutable", "absent", "pointer_state"] {
        assert!(!typed.snippet.contains(&format!("llvm_global \"{name}\"")));
    }
}

#[test]
fn typed_entry_point_retains_best_effort_missing_ir_handling() {
    let directory = std::env::temp_dir();
    let catalog = ContainerCatalog::default();
    // A directory cannot be read as an IR text file on either platform.
    for path in [None, Some(directory.as_path())] {
        for output in [
            scan_and_emit(path, "target", &[], &[], &catalog),
            scan_and_emit_typed(path, "target", &[], &[], &catalog),
        ] {
            assert!(output.is_empty());
            assert!(output.snippet.is_empty());
        }
    }
}
