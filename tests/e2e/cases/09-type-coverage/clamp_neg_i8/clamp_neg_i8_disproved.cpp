/*
DEMO: BUGGY clamp-negative-to-zero, i8.

Inverted comparison: clamps positive values to 0 instead of negatives.
SAW disproves with any positive x (e.g. x = 1: impl returns 0, spec says 1).

Expected verdict: DISPROVED.
*/

#include <cstdint>

int8_t clamp_neg_i8(int8_t x) {
    return x > 0 ? (int8_t)0 : x;   // BUG: comparison inverted.
}
