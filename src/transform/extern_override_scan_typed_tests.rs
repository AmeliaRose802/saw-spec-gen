//! Typed mode removes only broad defined-body overrides, never leaf contracts.

use super::*;

const MUTEX_HELPER: &str = "?_Verify_ownership_levels@_Mutex_base@std@@IEAA_NXZ";
const STL_HELPER: &str = "_ZNKSt6vectorIiSaIiEE4sizeEv";

const DEFINED_HELPERS: &str = r#"
@shared = global i32 0
@hidden = internal global i32 0
define i64 @target(ptr %self) {
  %ok = call i1 @"?_Verify_ownership_levels@_Mutex_base@std@@IEAA_NXZ"(ptr %self)
  %size = call i64 @_ZNKSt6vectorIiSaIiEE4sizeEv(ptr %self)
  ret i64 %size
}
define linkonce_odr i1 @"?_Verify_ownership_levels@_Mutex_base@std@@IEAA_NXZ"(ptr %self) {
  %status = call i32 @_Mtx_lock(ptr %self)
  %ok = icmp eq i32 %status, 0
  ret i1 %ok
}
define linkonce_odr i64 @_ZNKSt6vectorIiSaIiEE4sizeEv(ptr %self) {
  call void @allocator_leaf(ptr %self)
  %size = load i64, ptr %self
  ret i64 %size
}
declare i32 @_Mtx_lock(ptr)
declare void @allocator_leaf(ptr)
declare void @unreachable_external()
"#;

#[test]
fn typed_scan_attempts_defined_helpers_and_keeps_runtime_leaves() {
    let legacy = scan(DEFINED_HELPERS, "target");
    let mutex = legacy.iter().find(|t| t.symbol == MUTEX_HELPER).unwrap();
    assert_eq!(mutex.reason, BrokenReason::MsvcMutexHelper);
    // Respect the existing environment kill-switch without changing it.
    let stl = legacy.iter().find(|t| t.symbol == STL_HELPER);
    assert_eq!(
        stl.map(|t| t.reason),
        crate::emit::saw_emit::stl_overrides::matches(STL_HELPER)
            .then_some(BrokenReason::StlOverride)
    );
    let typed = scan_typed(DEFINED_HELPERS, "target");
    assert_eq!(
        typed.iter().map(|t| t.symbol.as_str()).collect::<Vec<_>>(),
        vec!["_Mtx_lock", "allocator_leaf"]
    );
    for leaf in &typed {
        assert_eq!(leaf.reason, BrokenReason::DeclareOnly);
        assert_eq!(leaf.fixed_param_ir_types, vec!["ptr"]);
        assert_eq!(leaf.globals_written, vec!["shared"]);
        assert_eq!(Some(leaf), legacy.iter().find(|t| t.symbol == leaf.symbol));
    }
}

#[test]
fn typed_scan_keeps_declarations_even_for_broad_registry_names() {
    for (symbol, ret) in [(MUTEX_HELPER, "i1"), (STL_HELPER, "i64")] {
        let ir = format!(
            "declare {ret} @\"{symbol}\"(ptr, ...)\n\
             define {ret} @target(ptr %p) {{\n\
               %rv = call {ret} (ptr, ...) @\"{symbol}\"(ptr %p, i32 7)\n\
               ret {ret} %rv\n}}\n"
        );
        let typed = scan_typed(&ir, "target");
        assert_eq!(typed, scan(&ir, "target"));
        assert_eq!(typed.len(), 1);
        assert_eq!(typed[0].symbol, symbol);
        assert_eq!(typed[0].reason, BrokenReason::DeclareOnly);
        assert!(typed[0].is_variadic);
        assert_eq!(typed[0].fixed_param_ir_types, vec!["ptr"]);
        assert_eq!(typed[0].return_ir_type, ret);
    }
}

#[test]
fn typed_scan_keeps_varargs_bodies_even_if_registered() {
    for (symbol, ret) in [(MUTEX_HELPER, "i1"), (STL_HELPER, "i64")] {
        let ir = format!(
            "@shared = global i32 0\n\
             define {ret} @target(ptr %p) {{\n\
               %rv = call {ret} (ptr, ...) @\"{symbol}\"(ptr %p, i32 7)\n\
               ret {ret} %rv\n}}\n\
             define {ret} @\"{symbol}\"(ptr %fmt, ...) {{\n\
               %ap = alloca ptr\n\
               call void @llvm.va_start.p0(ptr %ap)\n\
               call void @external_leaf()\n\
               call void @llvm.va_end.p0(ptr %ap)\n\
               ret {ret} 0\n}}\n\
             declare void @llvm.va_start.p0(ptr)\n\
             declare void @llvm.va_end.p0(ptr)\n\
             declare void @external_leaf()\n"
        );
        let typed = scan_typed(&ir, "target");
        assert_eq!(typed, scan(&ir, "target"));
        assert_eq!(typed.len(), 2, "keep the body and its opaque leaf");
        let body = typed.iter().find(|t| t.symbol == symbol).unwrap();
        assert_eq!(body.reason, BrokenReason::UsesVarargsIntrinsic);
        assert!(body.is_variadic);
        assert_eq!(body.fixed_param_ir_types, vec!["ptr"]);
        assert_eq!(body.return_ir_type, ret);
        assert_eq!(body.globals_written, vec!["shared"]);
        assert!(typed.iter().all(|t| !t.symbol.starts_with("llvm.")));
    }
}

#[test]
fn typed_scan_preserves_memcmp_metadata_and_excludes_the_target() {
    let ir = format!(
        "define i1 @\"{MUTEX_HELPER}\"(ptr %a, ptr %b) {{\n\
           %rv = call i32 @memcmp(ptr %a, ptr %b, i64 16)\n\
           %same = icmp eq i32 %rv, 0\n\
           ret i1 %same\n}}\n\
         declare i32 @memcmp(ptr, ptr, i64)\n"
    );
    let typed = scan_typed(&ir, MUTEX_HELPER);
    assert_eq!(typed, scan(&ir, MUTEX_HELPER));
    assert_eq!(typed.len(), 1);
    assert_eq!(typed[0].symbol, "memcmp");
    assert_eq!(typed[0].memcmp_const_len, Some(16));
    assert_eq!(typed[0].fixed_param_ir_types, vec!["ptr", "ptr", "i64"]);
}

#[test]
fn typed_scan_keeps_va_arg_helpers_without_varargs_intrinsic_calls() {
    for (symbol, ret) in [(MUTEX_HELPER, "i1"), (STL_HELPER, "i64")] {
        let ir = format!(
            "define {ret} @target(ptr %ap) {{\n\
               %rv = call {ret} @\"{symbol}\"(ptr %ap)\n\
               ret {ret} %rv\n}}\n\
             define {ret} @\"{symbol}\"(ptr %ap) {{\n\
               %rv = va_arg ptr %ap, {ret}\n\
               ret {ret} %rv\n}}\n"
        );
        let typed = scan_typed(&ir, "target");
        assert_eq!(typed, scan(&ir, "target"));
        assert_eq!(typed.len(), 1);
        assert_eq!(typed[0].symbol, symbol);
        assert_eq!(typed[0].reason, BrokenReason::UsesVarargsIntrinsic);
        assert!(!typed[0].is_variadic, "va_list passed as a fixed parameter");
    }
}
