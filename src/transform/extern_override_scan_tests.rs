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
    let mg = scan_mutable_globals(ir);
    assert!(mg.all.contains("?g_counter@@3IA"));
    assert!(mg.all.contains("plain_mut"));
    assert!(!mg.all.contains("??_C@_04@"));
    assert!(!mg.all.contains("plain_const"));
}

#[test]
fn variadic_body_writing_global_is_attributed() {
    // log_inc has a body that stores `g_counter++`. After the scan,
    // the OverrideTarget for log_inc must carry `g_counter` in its
    // `globals_written` set so the emitter can clobber it.
    let ir = r#"
@g_counter = dso_local global i32 0, align 4
@g_other = dso_local global i32 0, align 4

define i32 @target() {
  %1 = call i32 (ptr, ...) @log_inc(ptr null, i32 1)
  ret i32 0
}

define i32 @log_inc(ptr %fmt, ...) {
  %1 = alloca ptr
  call void @llvm.va_start.p0(ptr %1)
  %2 = load i32, ptr @g_counter, align 4
  %3 = add i32 %2, 1
  store i32 %3, ptr @g_counter, align 4
  call void @llvm.va_end.p0(ptr %1)
  ret i32 0
}

declare void @llvm.va_start.p0(ptr)
declare void @llvm.va_end.p0(ptr)
"#;
    let targets = scan(ir, "target");
    let log_inc = targets
        .iter()
        .find(|t| t.symbol == "log_inc")
        .expect("variadic-body override must be flagged");
    assert_eq!(
        log_inc.globals_written,
        vec!["g_counter".to_string()],
        "log_inc body stores to g_counter; scanner must attribute that"
    );
    assert!(
        !log_inc.globals_written.iter().any(|g| g == "g_other"),
        "scanner must not falsely attribute g_other to log_inc"
    );
}

#[test]
fn printf_chain_clobbers_external_globals_via_opaque_callee() {
    // The printf chain (printf -> _vfprintf_l -> __stdio_common_vfprintf)
    // reaches __stdio_common_vfprintf which is declare-only. Because any
    // opaque callee could be a forward-declared function from another TU
    // that writes externally-visible globals, the scanner conservatively
    // attributes all such globals. The override for printf inherits this
    // because its body transitively calls the opaque function.
    let ir = r#"
@super_important = dso_local global i32 7, align 4

define i32 @add_one(i32 %0) {
  %1 = call i32 (ptr, ...) @printf(ptr null, i32 %0)
  %2 = load i32, ptr @super_important, align 4
  ret i32 %2
}

define linkonce_odr i32 @printf(ptr noundef %0, ...) {
  %2 = alloca ptr
  call void @llvm.va_start.p0(ptr %2)
  %3 = call i32 @_vfprintf_l(ptr noundef %0, ptr noundef %0, ptr noundef %0, ptr noundef %0)
  call void @llvm.va_end.p0(ptr %2)
  ret i32 %3
}

define linkonce_odr i32 @_vfprintf_l(ptr noundef %0, ptr noundef %1, ptr noundef %2, ptr noundef %3) {
  %5 = call i32 @__stdio_common_vfprintf(i64 noundef 0, ptr noundef %0, ptr noundef %1, ptr noundef %2, ptr noundef %3)
  ret i32 %5
}

declare void @llvm.va_start.p0(ptr)
declare void @llvm.va_end.p0(ptr)
declare i32 @__stdio_common_vfprintf(i64, ptr, ptr, ptr, ptr)
"#;
    let targets = scan(ir, "add_one");
    let printf = targets
        .iter()
        .find(|t| t.symbol == "printf")
        .expect("printf must be in the override set");
    assert!(
        printf
            .globals_written
            .contains(&"super_important".to_string()),
        "printf chain reaches an opaque declare -- must conservatively \
         clobber externally-visible globals; got {:?}",
        printf.globals_written,
    );
    let common = targets
        .iter()
        .find(|t| t.symbol == "__stdio_common_vfprintf")
        .expect("__stdio_common_vfprintf must be in the override set");
    assert!(
        common
            .globals_written
            .contains(&"super_important".to_string()),
        "declare-only extern must conservatively clobber externally-visible \
         globals; got {:?}",
        common.globals_written,
    );
}

#[test]
fn store_to_constant_global_is_not_attributed() {
    // Even if a body syntactically `store`s to a global symbol, only
    // entries in the module's mutable-global set are reported. This
    // mirrors the constant filter `scan_mutable_globals` already
    // enforces and prevents the emitter from trying to clobber a
    // `constant` global (which SAW rejects).
    let ir = r#"
@g_const = constant i32 0, align 4
@g_mut = global i32 0, align 4

define i32 @target() {
  %1 = call i32 (ptr, ...) @writer(ptr null)
  ret i32 0
}

define i32 @writer(ptr %fmt, ...) {
  %1 = alloca ptr
  call void @llvm.va_start.p0(ptr %1)
  store i32 1, ptr @g_const, align 4
  store i32 2, ptr @g_mut, align 4
  call void @llvm.va_end.p0(ptr %1)
  ret i32 0
}

declare void @llvm.va_start.p0(ptr)
declare void @llvm.va_end.p0(ptr)
"#;
    let targets = scan(ir, "target");
    let writer = targets
        .iter()
        .find(|t| t.symbol == "writer")
        .expect("variadic writer must be flagged");
    assert_eq!(
        writer.globals_written,
        vec!["g_mut".to_string()],
        "scanner must filter out @g_const and keep @g_mut"
    );
}

// ── Linkage / opaque-extern tests ──────────────────────────────────

#[test]
fn internal_global_excluded_from_externally_visible() {
    let ir = r#"
@pub_mut = dso_local global i32 0, align 4
@priv_mut = internal global i32 0, align 4
@file_mut = private global i32 0, align 4
@ext_const = constant i32 9
"#;
    let mg = scan_mutable_globals(ir);
    assert!(mg.all.contains("pub_mut"));
    assert!(mg.all.contains("priv_mut"));
    assert!(mg.all.contains("file_mut"));
    assert!(mg.externally_visible.contains("pub_mut"));
    assert!(
        !mg.externally_visible.contains("priv_mut"),
        "internal globals can't be reached from other TUs"
    );
    assert!(
        !mg.externally_visible.contains("file_mut"),
        "private globals can't be reached from other TUs"
    );
}

#[test]
fn opaque_user_extern_clobbers_external_globals() {
    // `user_lib_fn` is DeclareOnly and NOT a system function.
    // It could `extern uint32_t g_pub;` in its own TU and write it.
    // The scanner must conservatively attribute all externally-visible
    // mutable globals to its override.
    let ir = r#"
@g_pub = dso_local global i32 0, align 4
@g_internal = internal global i32 0, align 4

define i32 @target(i32 %0) {
  call void @user_lib_fn(i32 %0)
  %1 = load i32, ptr @g_pub, align 4
  ret i32 %1
}

declare void @user_lib_fn(i32)
"#;
    let targets = scan(ir, "target");
    let ulf = targets
        .iter()
        .find(|t| t.symbol == "user_lib_fn")
        .expect("user_lib_fn must be in override set");
    assert!(
        ulf.globals_written.contains(&"g_pub".to_string()),
        "opaque user extern must conservatively clobber external globals"
    );
    assert!(
        !ulf.globals_written.contains(&"g_internal".to_string()),
        "internal-linkage global unreachable from other TUs"
    );
}

#[test]
fn opaque_declare_clobbers_external_globals() {
    // Any declare-only callee -- even __stdio_common_vfprintf --
    // could be a forward-declared function from another TU that
    // writes externally-visible globals. No heuristic exemption.
    let ir = r#"
@g_pub = dso_local global i32 0, align 4

define i32 @target(i32 %0) {
  %1 = call i32 @__stdio_common_vfprintf(i64 0, ptr null, ptr null, ptr null, ptr null)
  %2 = load i32, ptr @g_pub, align 4
  ret i32 %2
}

declare i32 @__stdio_common_vfprintf(i64, ptr, ptr, ptr, ptr)
"#;
    let targets = scan(ir, "target");
    let common = targets
        .iter()
        .find(|t| t.symbol == "__stdio_common_vfprintf")
        .expect("__stdio_common_vfprintf must be in override set");
    assert!(
        common.globals_written.contains(&"g_pub".to_string()),
        "declare-only extern must conservatively clobber externally-visible \
         globals; got {:?}",
        common.globals_written,
    );
}

#[test]
fn variadic_calling_opaque_user_extern_inherits_clobber() {
    // user_log is variadic (UsesVarargsIntrinsic) and calls
    // `user_helper` which is DeclareOnly and not a system function.
    // The BFS should attribute all externally-visible globals via
    // the opaque user callee.
    let ir = r#"
@g_state = dso_local global i32 0, align 4

define i32 @target(i32 %0) {
  %1 = call i32 (ptr, ...) @user_log(ptr null, i32 %0)
  %2 = load i32, ptr @g_state, align 4
  ret i32 %2
}

define i32 @user_log(ptr %fmt, ...) {
  %1 = alloca ptr
  call void @llvm.va_start.p0(ptr %1)
  call void @user_helper()
  call void @llvm.va_end.p0(ptr %1)
  ret i32 0
}

declare void @llvm.va_start.p0(ptr)
declare void @llvm.va_end.p0(ptr)
declare void @user_helper()
"#;
    let targets = scan(ir, "target");
    let log = targets
        .iter()
        .find(|t| t.symbol == "user_log")
        .expect("user_log must be in override set");
    assert!(
        log.globals_written.contains(&"g_state".to_string()),
        "variadic calling opaque user extern must inherit clobber; \
         got {:?}",
        log.globals_written,
    );
}

#[test]
fn msvc_mutex_helper_defined_in_module_is_flagged() {
    // `_Verify_ownership_levels` is defined in-module with `linkonce_odr`
    // linkage (not `declare`-only). The existing scan path skips defined
    // bodies that don't use va_* intrinsics and aren't in the STL override
    // list. The `MsvcMutexHelper` classification ensures these get an
    // override so SAW's simulator never steps into their typed
    // `_Mutex_base` reads, which would cause "Error during memory load"
    // when the struct is modeled as flat symbolic bytes.
    let ir = r#"
define i32 @target(ptr %self) {
  %1 = call i1 @"?_Verify_ownership_levels@_Mutex_base@std@@IEAA_NXZ"(ptr %self)
  ret i32 0
}

define linkonce_odr i1 @"?_Verify_ownership_levels@_Mutex_base@std@@IEAA_NXZ"(ptr noundef %0) {
  %2 = load i32, ptr %0, align 4
  ret i1 true
}
"#;
    let targets = scan(ir, "target");
    let helper = targets
        .iter()
        .find(|t| t.symbol == "?_Verify_ownership_levels@_Mutex_base@std@@IEAA_NXZ")
        .expect("defined-in-module _Verify_ownership_levels must be in override set");
    assert_eq!(
        helper.reason,
        BrokenReason::MsvcMutexHelper,
        "must be classified as MsvcMutexHelper, not skipped"
    );
    assert_eq!(helper.return_ir_type, "i1");
    assert!(!helper.is_variadic);
}

#[test]
fn non_mutex_defined_function_is_still_skipped() {
    // A plain defined function without va_* and not in any allowlist
    // must NOT be added to the override set — only the target function
    // itself can be verified, not every helper.
    let ir = r#"
define i32 @target() {
  %1 = call i32 @helper()
  ret i32 %1
}
define i32 @helper() {
  ret i32 42
}
"#;
    let targets = scan(ir, "target");
    assert!(
        targets.iter().all(|t| t.symbol != "helper"),
        "plain defined helper must not be overridden"
    );
}
