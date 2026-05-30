// DEMO: catch-and-rethrow — the exception escapes the function.
//
// The throw is inline (not in a helper), so after the exception-lower
// pass the error-flag branch is visible to SAW in `add_one` itself.
// The catch arm clears the flag, logs a side-effect, then re-throws
// with `throw;` — which sets the error flag again and returns a
// sentinel. The re-throw path returns `0` (not `x + 1`), so SAW
// disproves equivalence to the total spec at `x == 42`.
//
// Expected RESULT: DISPROVED with counterexample x = 42.
//
// This complements `add_one_throws_caught_verified.cpp` (where the
// catch swallows the exception) by showing that `throw;` in a catch
// body makes the function partial again — same as no catch at all.

#include <cstdint>

static volatile uint32_t g_rethrow_observed;

uint32_t add_one(uint32_t x) {
    try {
        if (x == 42u) {
            throw 1;
        }
    } catch (int) {
        // Log an observable, then re-throw — exception escapes.
        g_rethrow_observed = x;
        throw;
    }
    return x + 1;
}
