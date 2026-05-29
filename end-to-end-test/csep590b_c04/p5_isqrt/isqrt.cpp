/*
DEMO: Problem 5 (isqrt_newton) — reference (CSEP590B 26sp / c04 Part B).

C++ port of the homework's Newton's-method integer-square-root
program. The reference uses a data-dependent `while y < x` loop;
SAW needs a static bound, so we:

  * Restrict `n` to [1, 255] (out-of-contract values return the
    sentinel 0).
  * Unroll a fixed 10 iterations. Newton's method on n <= 255
    converges in at most 6 steps; once `y >= x` the guarded body
    becomes a no-op so the extra iterations are harmless.

x stays >= 1 by induction (x starts at n >= 1, and each update sets
x = y where y = (x_prev + ...)/2 >= 1), so the `udiv n, x` SAW sees
is well-defined on every reachable path.

Expected verdict: VERIFIED against `isqrt_spec`.
*/

#include <cstdint>

uint32_t isqrt(uint32_t n) {
    if (n < 1u || n > 255u) {
        return 0u;
    }
    uint32_t x = n;
    uint32_t y = (x + 1u) / 2u;
    for (int i = 0; i < 10; i++) {
        if (y < x) {
            x = y;
            y = (x + n / x) / 2u;
        }
    }
    return x;
}
