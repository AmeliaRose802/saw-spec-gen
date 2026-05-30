/*
DEMO: 8-bit popcount via a fully-unrolled bounded loop.

The loop bound (8) is a compile-time constant, so SAW unrolls it
exactly 8 times. Each iteration extracts bit `i` of `x` and adds it
to the running count.

Coverage:
  * `uint8_t` input parameter — width not exercised by earlier tests.
  * Static bounded loop with a per-iteration shift+mask.

Expected verdict: VERIFIED.
*/

#include <cstdint>

uint32_t popcount_u8(uint8_t x) {
    uint32_t n = 0;
    for (uint32_t i = 0; i < 8; i++) {
        n += static_cast<uint32_t>((x >> i) & 1u);
    }
    return n;
}
