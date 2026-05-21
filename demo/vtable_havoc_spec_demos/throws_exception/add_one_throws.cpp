// DEMO: a function that throws a C++ exception on a "bad" input.
//
// add_one(x) is supposed to compute x + 1. For the magic input
// x == 42, instead of returning a value it raises an int. There
// is no try/catch — the exception propagates out of add_one.
//
// What does SAW see?
//
// clang lowers `throw 1` (on the MSVC ABI, with -fno-rtti) to
//
//     call void @_CxxThrowException(ptr %ex, ptr @_TI1H)
//     unreachable
//
// `_CxxThrowException` is an external void function. gen-verify
// emits an auto-spec under specs_experimental/ that lets it return
// normally with havoced memory. But the LLVM `unreachable` right
// after the call tells SAW that any normal-return path is infeasible
// — so SAW prunes the x == 42 path entirely.
//
// As a consequence, SAW only checks the x != 42 branch, which
// trivially returns x + 1, and reports the function as **equivalent**
// to the Cryptol spec. That's correct under partial-correctness
// equivalence ("if the function returns, it returns x + 1") but it
// is *not* the property a programmer who wrote `throw 1` probably
// has in mind.

#include <cstdint>

uint32_t add_one(uint32_t x) {
    if (x == 42u) {
        throw 1;
    }
    return x + 1;
}
