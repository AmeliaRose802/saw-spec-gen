// Adversarial coverage probe #2: a variadic function that writes to a
// MUTABLE GLOBAL. Like `variadic_clobber/add_one_disproved.cpp`, this
// case fails (DISPROVED) only when the override emitter is adversarial
// enough — here, when it havocs every mutable global in its post-state.
//
// Before the global-clobber fix: SAW's `llvm_unsafe_assume_spec`
// override has no post-condition on `g_counter`, so SAW treats it as
// preserved across the call. `g_counter` keeps its pre-call value of
// 1, the function returns `x + 1`, the Cryptol spec is `x + 1`, the
// proof FALSELY succeeds.
//
// After the fix: every mutable global gets a post-state
// `llvm_points_to (llvm_global ...) (llvm_term <fresh>)`, mirroring
// the AST-derived havoc.rs / factory.rs emitters. `g_counter` becomes
// symbolic, the function returns `x + <symbolic>`, the proof correctly
// DISPROVES.

#include <cstdarg>
#include <cstdint>

uint32_t g_counter = 0;

static void log_inc(const char* fmt, ...) {
    va_list args;
    va_start(args, fmt);
    // Variadic body — touches the va_list AND writes a global. The
    // global write is the side effect we want the override to model.
    (void)va_arg(args, int);
    g_counter++;
    va_end(args);
}

uint32_t add_one_disproved(uint32_t x) {
    g_counter = 1;
    log_inc("x=%u", x);
    return x + g_counter;
}
