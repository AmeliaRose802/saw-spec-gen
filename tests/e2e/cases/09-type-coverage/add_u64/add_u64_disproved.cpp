/*
DEMO: BUGGY 64-bit unsigned add — subtracts instead.
SAW disproves with a=0, b=1 (impl returns 0 - 1 = 2^64-1, spec says 1).

Expected verdict: DISPROVED.
*/

#include <cstdint>

uint64_t add_u64(uint64_t a, uint64_t b) {
    return a - b;   // BUG: subtraction instead of addition.
}
