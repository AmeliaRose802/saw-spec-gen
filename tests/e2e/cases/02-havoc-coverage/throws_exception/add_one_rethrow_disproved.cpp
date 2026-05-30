// DEMO: catch-and-rethrow — the exception escapes the function.
//
// A helper throws on `x == 42`. The caller catches `int`, logs a
// side-effect, then re-throws with `throw;`. Because the re-throw
// propagates out of `add_one`, clang emits an `unreachable` after
// the `_CxxThrowException` / `__cxa_rethrow` call. SAW (after the
// exception-lower pass) sees that the throw path never reaches the
// `return` statement, so the function is partial — it doesn't
// return `x + 1` for `x == 42`.
//
// Expected RESULT: DISPROVED with counterexample x = 42.
//
// This complements `add_one_throws_caught_verified.cpp` (where the
// catch swallows the exception) by showing that `throw;` in a catch
// body makes the function partial again — same as no catch at all.

#include <cstdint>

static volatile uint32_t g_rethrow_observed;

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
        // Log an observable, then re-throw — exception escapes.
        g_rethrow_observed = x;
        throw;
    }
    return x + 1;
}
