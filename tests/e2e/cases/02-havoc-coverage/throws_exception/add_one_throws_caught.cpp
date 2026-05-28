// DEMO: a function that throws a C++ exception on a "bad" input,
// but catches it locally and still returns the correct value.
//
// add_one(x) computes x + 1. On x == 42 it raises `throw 1` inside
// a nested helper, which the outer add_one catches via `catch (int)`.
// The catch handler does an observable side effect (a `printf`) but
// does NOT alter the return value: add_one always returns x + 1.
//
// ─────────────────────────────────────────────────────────────────────
//  ⚠️  CURRENT SAW LIMITATION (MSVC funclet EH parser gap)
// ─────────────────────────────────────────────────────────────────────
//
// On `x86_64-pc-windows-msvc`, clang lowers `try` / `catch` to MSVC
// funclet-based EH:
//
//     %0 = invoke ... @maybe_throw(i32 %x)
//             to label %cont unwind label %catch.dispatch
//     catch.dispatch:
//       %cs = catchswitch within none [label %catch] unwind to caller
//     catch:
//       %cp = catchpad within %cs [ptr @"??_R0H@8", i32 0, ptr null]
//       call void @printf(ptr @.str, ...) [ "funclet"(token %cp) ]
//       catchret from %cp to label %cont
//     cont:
//       ret i32 (x + 1)
//
// SAW's bitcode parser does not currently understand
// `FUNC_CODE_CATCHSWITCH` (or the matching `FUNC_CODE_CATCHPAD` /
// `FUNC_CODE_CATCHRET` / `FUNC_CODE_CLEANUPPAD` / `FUNC_CODE_CLEANUPRET`
// opcodes). `llvm_load_module` aborts with:
//
//     parseField: unable to parse record field 3 of record
//     Record {recordCode = 52, ...}
//         FUNC_CODE_CATCHSWITCH
//         @"?add_one@@YAII@Z"
//
// before symbolic execution starts. The companion
// [`add_one_throws.cpp`](add_one_throws.cpp) works only because a
// *propagating* `throw` lowers to a plain `call _CxxThrowException`
// + `unreachable` — no funclet record types are emitted. The moment
// you add a matching `catch`, clang has to emit the funclet plumbing
// and the parser refuses.
//
// ─────────────────────────────────────────────────────────────────────
//  What this demo would prove once the parser gap is filled
// ─────────────────────────────────────────────────────────────────────
//
// The `_CxxThrowException` call inside `maybe_throw` is still
// followed by `unreachable`, so the throw path is pruned where it
// is raised. The catch funclet would be reached only via the
// unwinder, which SAW does not model — so SAW would symbolically
// execute the no-throw branch of `maybe_throw` (a no-op) and fall
// through to `return x + 1`. The Cryptol spec is total `x + 1`, so
// the equivalence would hold on every input.
//
// Expected (aspirational) RESULT: SAT — VERIFIED by z3.
//
// Why this is a useful complement to add_one_throws.cpp:
//
// - add_one_throws.cpp shows that a *propagating* `throw` makes
//   equivalence to a total spec fail (counterexample x = 42).
// - This file shows that adding a local `catch` that preserves the
//   return value should be enough to recover equivalence — the
//   spec doesn't care about the printf side effect, only about
//   the returned bits.
//
// To unblock this demo, one of the following has to land upstream:
//
//   (a) llvm-pretty-bc-parser learns to read the funclet opcodes
//       (`FUNC_CODE_CATCHSWITCH`, `_CATCHPAD`, `_CATCHRET`,
//       `_CLEANUPPAD`, `_CLEANUPRET`). The existing
//       `saw/seh-bitcode-only` branch is the start of this work.
//   (b) An exception-lower pre-pass for the MSVC ABI rewrites
//       catchswitch/catchpad to a switch on a sentinel — analogous
//       to the existing Itanium pass in
//       saw-tools/exception-lower/.
//
// Until either lands, this file is intentionally NOT wired into the
// SAW regression suite (`tests/saw_demos/cases.psd1`). It is kept
// as a worked example of what the canonical "throw caught
// harmlessly" demo will look like — and as a concrete reproducer
// for the parser gap.

#include <cstdint>
#include <cstdio>

// Out-of-line helper so the throw stays in its own function and
// clang actually emits an `invoke` + funclet at the call site.
static uint32_t maybe_throw(uint32_t x) {
    if (x == 42u) {
        throw 1;
    }
    return 0;
}

uint32_t add_one(uint32_t x) {
    try {
        (void)maybe_throw(x);
    } catch (int) {
        // Observable but irrelevant to the return value.
        std::printf("caught harmless throw on x=42\n");
    }
    return x + 1;
}
