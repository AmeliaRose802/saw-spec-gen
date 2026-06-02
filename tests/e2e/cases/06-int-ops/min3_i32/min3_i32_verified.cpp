/*
DEMO: Three-argument signed minimum.

Adds coverage that the existing `add_one` suite is missing:
  * three input parameters (vs one),
  * signed comparison (`<` on `int32_t` lowers to LLVM `icmp slt`).

Expected verdict: VERIFIED.
*/

#include <cstdint>

int32_t min3_i32(int32_t a, int32_t b, int32_t c) {
    int32_t m = a;
    if (b < m) m = b;
    if (c < m) m = c;
    return m;
}
