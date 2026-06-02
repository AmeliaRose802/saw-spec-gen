// DEMO: nested try/catch — inner catch swallows, outer never fires.
//
// An inline throw fires on `x == 42`. The inner try/catch catches
// `BadInput` and swallows it (volatile write, then falls through).
// The outer try/catch catches `...` (catch-all) and would return 0,
// but it is never reached because the inner catch already handled
// the exception.
//
// The throw is inline (not in a helper) because exception-lower's
// per-function error-flag mechanism does not propagate across call
// boundaries when SAW executes the callee inline. The exception
// type is a struct (not `int`) so that the typeinfo is defined
// in-module, avoiding SAW's limitation with external typeinfo
// symbols like `@_ZTIi`.
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

struct BadInput {};

static volatile uint32_t g_inner_observed;

uint32_t add_one(uint32_t x) {
    try {
        try {
            if (x == 42u) {
                throw BadInput{};
            }
        } catch (const BadInput&) {
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
