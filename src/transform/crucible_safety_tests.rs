//! Tests for `super` ([`crate::transform::crucible_safety`]).
//! Extracted from `crucible_safety.rs` to keep that file under the
//! 500 non-whitespace line limit.

use super::*;

const PURE_ARITH: &str = r#"
define i32 @add_one(i32 %x) {
  %1 = add i32 %x, 1
  ret i32 %1
}
"#;

const STD_MAX: &str = r#"
define linkonce_odr dso_local noundef nonnull align 4 ptr @_ZSt3maxIjERKT_S2_S2_(ptr %0, ptr %1) {
  %3 = alloca ptr, align 8
  %4 = alloca ptr, align 8
  %5 = alloca ptr, align 8
  store ptr %0, ptr %4, align 8
  store ptr %1, ptr %5, align 8
  %6 = load ptr, ptr %4, align 8
  %7 = load i32, ptr %6, align 4
  %8 = load ptr, ptr %5, align 8
  %9 = load i32, ptr %8, align 4
  %10 = icmp ult i32 %7, %9
  br i1 %10, label %11, label %13

11:
  %12 = load ptr, ptr %5, align 8
  store ptr %12, ptr %3, align 8
  br label %15

13:
  %14 = load ptr, ptr %4, align 8
  store ptr %14, ptr %3, align 8
  br label %15

15:
  %16 = load ptr, ptr %3, align 8
  ret ptr %16
}
"#;

const WITH_INVOKE: &str = r#"
define i32 @throws() personality ptr @__gxx_personality_v0 {
  %1 = invoke i32 @some_call() to label %ok unwind label %bad
ok:
  ret i32 %1
bad:
  %lp = landingpad { ptr, i32 } cleanup
  resume { ptr, i32 } %lp
}
"#;

const WITH_ATOMIC: &str = r#"
define i32 @increment(ptr %p) {
  %1 = atomicrmw add ptr %p, i32 1 seq_cst
  ret i32 %1
}
"#;

const WITH_PRINTF: &str = r#"
declare i32 @printf(ptr, ...)

define i32 @hello() {
  %1 = call i32 (ptr, ...) @printf(ptr null)
  ret i32 %1
}
"#;

const RECURSIVE: &str = r#"
define i32 @fact(i32 %n) {
  %1 = icmp eq i32 %n, 0
  br i1 %1, label %base, label %rec
base:
  ret i32 1
rec:
  %2 = sub i32 %n, 1
  %3 = call i32 @fact(i32 %2)
  %4 = mul i32 %n, %3
  ret i32 %4
}
"#;

const SAFE_INTRINSIC_USE: &str = r#"
define i32 @abs_wrap(i32 %x) {
  %1 = call i32 @llvm.abs.i32(i32 %x, i1 false)
  ret i32 %1
}
declare i32 @llvm.abs.i32(i32, i1)
"#;

const TRANSITIVELY_SAFE: &str = r#"
define i32 @inner(i32 %x) {
  %1 = add i32 %x, 1
  ret i32 %1
}
define i32 @outer(i32 %x) {
  %1 = call i32 @inner(i32 %x)
  ret i32 %1
}
"#;

const INDIRECT_CALL: &str = r#"
define i32 @via_fp(ptr %fp, i32 %x) {
  %1 = call i32 %fp(i32 %x)
  ret i32 %1
}
"#;

#[test]
fn pure_arithmetic_is_safe() {
    let mut a = SafetyAnalyzer::new(PURE_ARITH);
    let r = a.is_safe("add_one");
    assert!(r.safe, "expected safe, got {:?}", r);
}

#[test]
fn std_max_body_is_safe() {
    let mut a = SafetyAnalyzer::new(STD_MAX);
    let r = a.is_safe("_ZSt3maxIjERKT_S2_S2_");
    assert!(r.safe, "expected safe, got {:?}", r);
}

#[test]
fn invoke_is_unsafe() {
    let mut a = SafetyAnalyzer::new(WITH_INVOKE);
    let r = a.is_safe("throws");
    assert!(!r.safe);
    let why = r.first_unsafe_inst.unwrap();
    assert!(
        why.contains("invoke") || why.contains("EH") || why.contains("exception"),
        "expected EH reason, got {why}",
    );
}

#[test]
fn atomic_is_unsafe() {
    let mut a = SafetyAnalyzer::new(WITH_ATOMIC);
    let r = a.is_safe("increment");
    assert!(!r.safe);
    assert!(r.first_unsafe_inst.unwrap().contains("atomic"));
}

#[test]
fn printf_is_unsafe_transitively() {
    let mut a = SafetyAnalyzer::new(WITH_PRINTF);
    let r = a.is_safe("hello");
    assert!(!r.safe);
    let why = r.first_unsafe_inst.unwrap();
    // printf has no body in this module — the transitive check surfaces
    // it as "no body".
    assert!(
        why.contains("printf") || why.contains("no body"),
        "got {why}",
    );
}

#[test]
fn recursive_is_unsafe() {
    let mut a = SafetyAnalyzer::new(RECURSIVE);
    let r = a.is_safe("fact");
    assert!(!r.safe);
    assert!(
        r.first_unsafe_inst.unwrap().contains("recursive"),
        "expected recursion diagnostic",
    );
}

#[test]
fn allow_listed_intrinsic_is_safe() {
    let mut a = SafetyAnalyzer::new(SAFE_INTRINSIC_USE);
    let r = a.is_safe("abs_wrap");
    assert!(r.safe, "expected safe, got {:?}", r);
}

#[test]
fn transitive_safe_chain_is_safe() {
    let mut a = SafetyAnalyzer::new(TRANSITIVELY_SAFE);
    let r = a.is_safe("outer");
    assert!(r.safe, "expected safe, got {:?}", r);
}

#[test]
fn indirect_call_is_unsafe() {
    let mut a = SafetyAnalyzer::new(INDIRECT_CALL);
    let r = a.is_safe("via_fp");
    assert!(!r.safe);
    assert!(
        r.first_unsafe_inst.unwrap().contains("indirect"),
        "expected indirect-call diagnostic",
    );
}

#[test]
fn missing_symbol_is_unsafe() {
    let mut a = SafetyAnalyzer::new(PURE_ARITH);
    let r = a.is_safe("nonexistent_symbol");
    assert!(!r.safe);
    assert!(r.first_unsafe_inst.unwrap().contains("no body"));
}

#[test]
fn label_lines_are_skipped() {
    // No code in the bodies above triggers regressions if labels
    // misparse as opcodes, but this pin guards the strip helper.
    assert_eq!(strip_inst_prefix("entry:"), "");
    assert_eq!(strip_inst_prefix("  15:"), "");
    assert_eq!(strip_inst_prefix("  ; just a comment"), "");
    assert_eq!(strip_inst_prefix("  %1 = add i32 %x, 1"), "add i32 %x, 1");
    assert_eq!(
        strip_inst_prefix("  store i32 %x, ptr %p"),
        "store i32 %x, ptr %p",
    );
}

#[test]
fn extract_function_symbol_handles_quoted_and_bare() {
    assert_eq!(
        extract_function_symbol("define i32 @foo(i32) {").as_deref(),
        Some("foo"),
    );
    assert_eq!(
        extract_function_symbol(r#"define i32 @"_ZSt3maxIjE"(ptr, ptr) {"#).as_deref(),
        Some("_ZSt3maxIjE"),
    );
}

#[test]
fn opaque_leaves_are_opt_in_and_invalidate_transitive_cache() {
    let ir = r#"
%Mutex = type { i32, i32 }
define void @target(ptr %mutex) {
  call void @"?lock@_Mutex_base@std@@QEAAXXZ"(ptr %mutex)
  ret void
}
define linkonce_odr void @"?lock@_Mutex_base@std@@QEAAXXZ"(ptr %self) {
  %status = call i32 @_Mtx_lock(ptr %self)
  %ok = icmp eq i32 %status, 0
  br i1 %ok, label %locked, label %failed
locked:
  %count = getelementptr %Mutex, ptr %self, i32 0, i32 1
  %before = load i32, ptr %count, align 4
  %after = add i32 %before, 1
  store i32 %after, ptr %count, align 4
  ret void
failed:
  call void @_Throw_Cpp_error(i32 %status)
  unreachable
}
declare i32 @_Mtx_lock(ptr)
declare void @_Throw_Cpp_error(i32) noreturn
"#;
    let mut analyzer = SafetyAnalyzer::new(ir);
    for symbol in ["target", "_Mtx_lock", "unknown_leaf"] {
        assert!(!analyzer.is_safe(symbol).safe, "legacy check: {symbol}");
    }
    analyzer.allow_opaque_leaves();
    for symbol in [
        "target",
        "?lock@_Mutex_base@std@@QEAAXXZ",
        "_Mtx_lock",
        "_Throw_Cpp_error",
        "unknown_leaf",
    ] {
        let report = analyzer.is_safe(symbol);
        assert!(report.safe, "traversal only: {symbol}: {report:?}");
    }
    assert!(!analyzer.is_safe("llvm.va_start.p0").safe);
    assert!(analyzer.allow_opaque_leaves().is_safe("target").safe);
    assert!(!SafetyAnalyzer::new(ir).is_safe("target").safe);
}

#[test]
fn diamond_and_repeated_calls_restore_the_visited_stack() {
    let ir = r#"
define void @target() {
  call void @left()
  call void @right()
  call void @leaf()
  call void @leaf()
  ret void
}
define void @left() {
  call void @leaf()
  ret void
}
define void @right() {
  call void @leaf()
  ret void
}
define void @leaf() {
  ret void
}
"#;
    let mut visited = HashSet::from(["ancestor".to_string()]);
    let original = visited.clone();
    // Separate one-shot analyzers must not mistake completed siblings
    // left in the caller's stack for recursion.
    for symbol in ["target", "left", "right", "leaf"] {
        let report = is_crucible_safe(symbol, ir, &mut visited);
        assert!(report.safe, "{symbol}: {report:?}");
        assert_eq!(visited, original);
    }
    let mut analyzer = SafetyAnalyzer::new(ir);
    analyzer.allow_opaque_leaves();
    assert!(analyzer.check("target", &mut visited).safe);
    assert_eq!(visited, original);
}

#[test]
fn failed_checks_restore_the_visited_stack() {
    for (ir, symbol) in [
        (WITH_INVOKE, "throws"),
        (WITH_ATOMIC, "increment"),
        (WITH_PRINTF, "hello"),
        (RECURSIVE, "fact"),
        (PURE_ARITH, "missing"),
    ] {
        let mut visited = HashSet::from(["ancestor".to_string()]);
        let original = visited.clone();
        assert!(!is_crucible_safe(symbol, ir, &mut visited).safe);
        assert_eq!(visited, original, "unwind failed for {symbol}");
    }
}

#[test]
fn cached_safety_cannot_hide_an_active_recursion_entry() {
    let mut analyzer = SafetyAnalyzer::new(PURE_ARITH);
    assert!(analyzer.is_safe("add_one").safe);
    let mut visited = HashSet::from(["add_one".to_string()]);
    let original = visited.clone();
    let report = analyzer.check("add_one", &mut visited);
    assert!(!report.safe);
    assert!(report.first_unsafe_inst.unwrap().contains("recursive"));
    assert_eq!(visited, original, "must not pop the caller's own entry");
}

#[test]
fn opaque_mode_keeps_unsafe_opcodes_intrinsics_and_recursion_unsafe() {
    for (ir, symbol) in [
        (WITH_INVOKE, "throws"),
        (WITH_ATOMIC, "increment"),
        (INDIRECT_CALL, "via_fp"),
        (RECURSIVE, "fact"),
    ] {
        let mut analyzer = SafetyAnalyzer::new(ir);
        assert!(!analyzer.allow_opaque_leaves().is_safe(symbol).safe);
    }
    for (inst, reason) in [
        (
            "%old = cmpxchg ptr %p, i32 0, i32 1 seq_cst seq_cst",
            "atomic",
        ),
        ("fence seq_cst", "atomic"),
        ("%arg = va_arg ptr %p, i32", "varargs"),
        ("call void @llvm.va_start.p0(ptr %p)", "intrinsic"),
        ("call void asm sideeffect \"\", \"\"()", "inline asm"),
        ("unsupported_opcode", "unknown opcode"),
    ] {
        let ir = format!(
            "declare void @external()\n\
             define void @helper(ptr %p) {{\n\
               call void @external()\n{inst}\nret void\n}}\n"
        );
        let mut analyzer = SafetyAnalyzer::new(&ir);
        let report = analyzer.allow_opaque_leaves().is_safe("helper");
        assert!(!report.safe, "must reject {inst}");
        assert!(report.first_unsafe_inst.unwrap().contains(reason));
    }
}

#[test]
fn opaque_mode_rejects_mutual_recursion_and_unwinds() {
    let ir = r#"
declare void @external()
define void @a() {
  call void @external()
  call void @b()
  ret void
}
define void @b() {
  call void @a()
  ret void
}
"#;
    let mut analyzer = SafetyAnalyzer::new(ir);
    analyzer.allow_opaque_leaves();
    let mut visited = HashSet::new();
    for symbol in ["a", "b"] {
        let report = analyzer.check(symbol, &mut visited);
        assert!(!report.safe);
        assert!(report.first_unsafe_inst.unwrap().contains("recursive"));
        assert!(visited.is_empty());
    }
}

#[test]
fn opaque_mode_checks_the_callee_not_its_global_arguments() {
    let ir = r#"
@state = global i32 0
define void @target() {
  call void @"unsafe_helper"(ptr @state)
  ret void
}
define void @unsafe_helper(ptr %p) {
  fence seq_cst
  ret void
}
define void @indirect(ptr %fp) {
  call void %fp(ptr @state)
  ret void
}
"#;
    let mut analyzer = SafetyAnalyzer::new(ir);
    analyzer.allow_opaque_leaves();
    for (symbol, reason) in [("target", "atomic"), ("indirect", "indirect")] {
        let report = analyzer.is_safe(symbol);
        assert!(
            !report.safe,
            "must inspect {symbol}, not treat @state as a leaf"
        );
        assert!(report.first_unsafe_inst.unwrap().contains(reason));
    }
}
