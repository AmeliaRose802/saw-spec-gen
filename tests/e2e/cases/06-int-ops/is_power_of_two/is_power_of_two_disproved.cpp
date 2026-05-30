/*
DEMO: BUGGY power-of-two predicate — missing the `x != 0` guard.

For x == 0, `0 & (0 - 1) == 0 & 0xFFFFFFFF == 0`, so the bit trick
alone returns 1. The spec says 0. SAW disproves with x = 0.

Expected verdict: DISPROVED.
*/

#include <cstdint>

uint32_t is_power_of_two(uint32_t x) {
    return ((x & (x - 1)) == 0) ? 1u : 0u;   // BUG: no x != 0 guard.
}
