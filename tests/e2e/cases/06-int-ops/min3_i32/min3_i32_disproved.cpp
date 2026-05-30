/*
DEMO: BUGGY three-argument signed minimum.

The third comparison is inverted (`>` instead of `<`), so the function
sometimes returns the larger of `m` and `c`. SAW will find a
counterexample with `c` strictly greater than `min(a, b)`.

Expected verdict: DISPROVED.
*/

#include <cstdint>

int32_t min3_i32(int32_t a, int32_t b, int32_t c) {
    int32_t m = a;
    if (b < m) m = b;
    if (c > m) m = c;   // BUG: comparison inverted.
    return m;
}
