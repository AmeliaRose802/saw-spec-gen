// Adversarial coverage probe: a variadic function that WRITES through a
// pointer argument. The caller's correctness *appears* to depend on the
// pointee being left alone. If our extern-override emitter only feeds
// the overridden callee a *fresh* pointer (no `llvm_points_to` clobber
// against the pointer the caller actually passed), then SAW sees `side`
// keep its pre-call value of 1 and falsely proves the function equals
// `x + 1`.
//
// Expected verdict (after the override emitter is fixed to clobber
// pointee memory): DISPROVED — `side` becomes symbolic, `x + side` is
// not equal to `x + 1` in general.
//
// Today's verdict (before the fix): VERIFIED — the override returns
// without writing through `out`, `side` stays 1, the proof goes
// through. This is the false positive we want to demonstrate, then
// fix.

#include <cstdarg>
#include <cstdint>

// Variadic function that writes through `out`. SAW cannot symbolically
// execute it (uses `llvm.va_*`), so it must be overridden. A sound
// override clobbers `*out`; a naïve override (the current state) does
// not, and the caller's correctness silently depends on that gap.
static void log_and_capture(uint32_t* out, const char* fmt, ...) {
    va_list args;
    va_start(args, fmt);
    *out = (uint32_t)va_arg(args, int);
    va_end(args);
}

uint32_t add_one_disproved(uint32_t x) {
    uint32_t side = 1;
    log_and_capture(&side, "x=%u", x);
    return x + side;
}
