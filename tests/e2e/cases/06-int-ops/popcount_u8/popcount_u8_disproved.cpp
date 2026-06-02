/*
DEMO: BUGGY 8-bit popcount — off-by-one on the loop bound.

Stops at i < 7 instead of i < 8, missing the top bit. SAW disproves
with x = 0x80 (only the missed bit set): impl returns 0, spec says 1.

Expected verdict: DISPROVED.
*/

#include <cstdint>

uint32_t popcount_u8(uint8_t x) {
    uint32_t n = 0;
    for (uint32_t i = 0; i < 7; i++) {   // BUG: was i < 8.
        n += static_cast<uint32_t>((x >> i) & 1u);
    }
    return n;
}
