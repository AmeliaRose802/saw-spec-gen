// DEMO: a function that throws a C++ exception on a "bad" input.
//
// add_one(x) is supposed to compute x + 1. For the magic input
// x == 42 we instead raise an int via `throw 1`. There is no
// try/catch here — the exception simply propagates out of add_one.
//
// What does clang generate, and what does SAW do with it?
//
// On the MSVC ABI (the only ABI clang supports for
// x86_64-pc-windows-msvc), `throw 1` lowers to:
//
//     call void @_CxxThrowException(ptr %ex, ptr @_TI1H)
//     unreachable
//
// `_TI1H` is a "throw info" global referencing a chain of MSVC
// type-descriptor globals (`@"_CT??_R0H@84"`, `@_CTA1H`, ...) whose
// initializers use `trunc (sub (ptrtoint @A) (ptrtoint @B)) to i32`
// — the MSVC PE convention for image-relative addresses.
//
// SAW (post the crucible-llvm fix for ptrtoint-subtraction in global
// initializers) now loads this bitcode cleanly: unfoldable
// `ptrtoint`-difference fields are materialized as undef integers,
// which is sound here because the metadata is only ever read by the
// OS unwinder, never by user code.
//
// gen-verify emits an auto-spec for the external `_CxxThrowException`
// that lets it return normally with havoced memory, but the
// `unreachable` immediately after the throw call prunes that path.
// SAW therefore proves the spec on the non-throwing branch only —
// a partial-correctness property: "if add_one returns, it returns
// x + 1."
//
// The Cryptol spec in add_one_spec.cry is a total mathematical
// function (defined on every input), so the equivalence proof
// against it fails at exactly the input on which add_one does *not*
// return: the counterexample x = 42. Expected RESULT: UNSAT
// (DISPROVED with counterexample x = 42).

#include <cstdint>

uint32_t add_one(uint32_t x) {
    if (x == 42u) {
        throw 1;
    }
    return x + 1;
}
