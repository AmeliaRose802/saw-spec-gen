// User-defined variadic function — proves the bitcode override scan
// handles arbitrary `...` callees, not just libc printf-family symbols.
//
// `user_log` uses `va_start` / `va_end` so SAW cannot symbolically
// execute its body. The scan flags it as
// `BrokenReason::UsesVarargsIntrinsic` and emits a signature-based
// override matching only the fixed `const char* fmt` prefix; SAW's
// matcher accepts the variadic call site against that prefix.
//
// `user_log` is intentionally a plain (mangled) C++ function — the
// scan reads symbol names verbatim from the LLVM IR, so the override
// works regardless of whether the symbol is `extern "C"` or
// MSVC-mangled (see extern_override_scan_tests.rs for the mangled-name
// coverage).

#include <cstdarg>
#include <cstdint>

static int user_log(const char* fmt, ...) {
    va_list args;
    va_start(args, fmt);
    // Touch the va_list so the body cannot be trivially DCE'd away.
    int n = va_arg(args, int);
    va_end(args);
    return n;
}

uint32_t bump_with_log(uint32_t x) {
    user_log("bump %u", x);
    return x + 1;
}
