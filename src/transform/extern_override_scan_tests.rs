//! Tests for [`super`] — extracted to keep the parent under the
//! 500 NWS-line limit.

use super::*;

const BUMP_IR: &str = r#"
define dso_local noundef i32 @"?bump@@YAII@Z"(i32 noundef %0) #0 {
  %2 = alloca i32, align 4
  store i32 %0, ptr %2, align 4
  %3 = load i32, ptr %2, align 4
  %4 = call i32 (ptr, ...) @printf(ptr noundef @str, i32 noundef %3)
  %5 = add i32 %3, 1
  ret i32 %5
}

define linkonce_odr dso_local i32 @printf(ptr noundef %0, ...) #0 comdat {
  %2 = alloca ptr, align 8
  call void @llvm.va_start.p0(ptr %2)
  %3 = call i32 @_vfprintf_l(ptr noundef %0, ptr noundef %0, ptr noundef %0, ptr noundef %0)
  call void @llvm.va_end.p0(ptr %2)
  ret i32 %3
}

define linkonce_odr dso_local i32 @_vfprintf_l(ptr noundef %0, ptr noundef %1, ptr noundef %2, ptr noundef %3) #0 comdat {
  %5 = call i32 @__stdio_common_vfprintf(i64 noundef 0, ptr noundef %0, ptr noundef %1, ptr noundef %2, ptr noundef %3)
  ret i32 %5
}

define linkonce_odr dso_local ptr @__local_stdio_printf_options() #3 comdat {
  ret ptr null
}

declare void @llvm.va_start.p0(ptr) #1
declare void @llvm.va_end.p0(ptr) #1
declare dso_local ptr @__acrt_iob_func(i32 noundef) #2
declare dso_local i32 @__stdio_common_vfprintf(i64 noundef, ptr noundef, ptr noundef, ptr noundef, ptr noundef) #2
"#;

#[test]
fn scan_bump_finds_printf_as_varargs_user() {
    let targets = scan(BUMP_IR, "?bump@@YAII@Z");
    let by_sym: std::collections::HashMap<_, _> =
        targets.iter().map(|t| (t.symbol.as_str(), t)).collect();

    let printf = by_sym
        .get("printf")
        .expect("printf reachable from bump must land in the broken set");
    assert_eq!(printf.reason, BrokenReason::UsesVarargsIntrinsic);
    assert!(
        printf.is_variadic,
        "printf signature ends in `, ...)`; scanner must flag it"
    );
    // Override emission uses ONLY the fixed prefix; the `...` tail must
    // not show up here. SAW's matcher relies on this contract.
    assert_eq!(printf.fixed_param_ir_types, vec!["ptr"]);
    assert_eq!(printf.return_ir_type, "i32");
}

#[test]
fn scan_bump_finds_declare_only_externs() {
    let targets = scan(BUMP_IR, "?bump@@YAII@Z");
    let by_sym: std::collections::HashMap<_, _> =
        targets.iter().map(|t| (t.symbol.as_str(), t)).collect();

    let vfprintf = by_sym
        .get("__stdio_common_vfprintf")
        .expect("declare-only ucrt extern must be flagged");
    assert_eq!(vfprintf.reason, BrokenReason::DeclareOnly);
    assert!(!vfprintf.is_variadic);
    assert_eq!(
        vfprintf.fixed_param_ir_types,
        vec!["i64", "ptr", "ptr", "ptr", "ptr"]
    );
    assert_eq!(vfprintf.return_ir_type, "i32");
}

#[test]
fn scan_skips_target_function_itself() {
    let targets = scan(BUMP_IR, "?bump@@YAII@Z");
    // The function under verification is never an override target —
    // SAW would refuse to verify a body it has already assumed.
    assert!(targets.iter().all(|t| t.symbol != "?bump@@YAII@Z"));
}

#[test]
fn scan_skips_llvm_intrinsics() {
    let targets = scan(BUMP_IR, "?bump@@YAII@Z");
    // Even though printf calls `llvm.va_start.p0` and it's declare-only,
    // we do NOT emit an override for it — the probe experiment showed
    // intrinsic overrides leave the caller's body broken at the
    // va_arg site. Overriding the variadic caller is what fixes it.
    assert!(targets.iter().all(|t| !t.symbol.starts_with("llvm.")));
}

#[test]
fn scan_skips_unreachable_functions() {
    let ir = r#"
define i32 @target() {
  ret i32 0
}
declare i32 @never_called(i32)
"#;
    let targets = scan(ir, "target");
    assert!(
        targets.is_empty(),
        "never_called is in the bitcode but not reachable from target — skip"
    );
}

#[test]
fn scan_skips_non_varargs_defined_functions() {
    // __local_stdio_printf_options has a body and uses no va_intrinsic.
    // It's not in the broken set — SAW can execute it.
    let ir = r#"
define i32 @target() {
  %1 = call ptr @__local_stdio_printf_options()
  ret i32 0
}
define linkonce_odr dso_local ptr @__local_stdio_printf_options() {
  ret ptr null
}
"#;
    let targets = scan(ir, "target");
    assert!(
        targets.is_empty(),
        "defined non-varargs callees are tractable; should not be overridden"
    );
}

#[test]
fn user_defined_variadic_is_flagged() {
    // The user's own variadic function — same treatment as printf.
    let ir = r#"
define i32 @target() {
  %1 = call i32 (ptr, ...) @user_log(ptr null, i32 42)
  ret i32 %1
}
define i32 @user_log(ptr %fmt, ...) {
  %1 = alloca ptr
  call void @llvm.va_start.p0(ptr %1)
  call void @llvm.va_end.p0(ptr %1)
  ret i32 0
}
declare void @llvm.va_start.p0(ptr)
declare void @llvm.va_end.p0(ptr)
"#;
    let targets = scan(ir, "target");
    let log = targets
        .iter()
        .find(|t| t.symbol == "user_log")
        .expect("user-defined variadic must be flagged exactly like printf");
    assert_eq!(log.reason, BrokenReason::UsesVarargsIntrinsic);
    assert!(log.is_variadic);
    assert_eq!(log.fixed_param_ir_types, vec!["ptr"]);
    assert_eq!(log.return_ir_type, "i32");
}

#[test]
fn quoted_symbols_round_trip_correctly() {
    let ir = r#"
define i32 @"?foo@@YAHXZ"() {
  %1 = call i32 @"?bar@@YAHXZ"()
  ret i32 %1
}
declare i32 @"?bar@@YAHXZ"()
"#;
    let targets = scan(ir, "?foo@@YAHXZ");
    assert_eq!(targets.len(), 1);
    assert_eq!(targets[0].symbol, "?bar@@YAHXZ");
}

#[test]
fn void_return_is_preserved_as_void_token() {
    let ir = r#"
define i32 @target() {
  call void @sink(i32 0)
  ret i32 0
}
declare void @sink(i32)
"#;
    let targets = scan(ir, "target");
    assert_eq!(targets.len(), 1);
    assert_eq!(targets[0].return_ir_type, "void");
}

#[test]
fn scan_mutable_globals_picks_global_not_constant() {
    let ir = r#"
@"?g_counter@@3IA" = dso_local global i32 0, align 4
@"??_C@_04@" = linkonce_odr dso_local unnamed_addr constant [5 x i8] c"x=%u\00", align 1
@plain_mut = global i32 7
@plain_const = constant i32 9
"#;
    let muts = scan_mutable_globals(ir);
    assert!(muts.contains("?g_counter@@3IA"));
    assert!(muts.contains("plain_mut"));
    assert!(!muts.contains("??_C@_04@"));
    assert!(!muts.contains("plain_const"));
}
