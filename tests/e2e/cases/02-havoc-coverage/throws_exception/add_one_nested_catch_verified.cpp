// DEMO: nested try/catch — inner catch swallows, outer never fires.
//
// A helper throws on `x == 42`. The inner try/catch catches `int`
// and swallows it (volatile write, then falls through). The outer
// try/catch catches `...` (catch-all) and would return 0, but it is
// never reached because the inner catch already handled the
// exception.
//
// For all inputs the function returns `x + 1`:
//   * x != 42 → no throw, straight to `return x + 1`.
//   * x == 42 → inner catch swallows, falls through to
//     `return x + 1`.
//
// Expected RESULT: VERIFIED by z3 (requires exception-lower pass).
//
// This tests that the exception-lower pass correctly routes the
// exception to the innermost matching handler and does not
// accidentally fall through to the outer catch-all.

#include <cstdint>

static volatile uint32_t g_inner_observed;

static uint32_t maybe_throw(uint32_t x) {
    if (x == 42u) {
        throw 1;
    }
    return 0;
}

uint32_t add_one(uint32_t x) {
    try {
        try {
            (void)maybe_throw(x);
        } catch (int) {
            // Inner catch swallows the exception harmlessly.
            g_inner_observed = x;
        }
    } catch (...) {
        // Outer catch-all — should never fire because the inner
        // catch already matched. If it did fire, we'd return 0
        // (wrong), and SAW would disprove the spec.
        return 0;
    }
    return x + 1;
}
