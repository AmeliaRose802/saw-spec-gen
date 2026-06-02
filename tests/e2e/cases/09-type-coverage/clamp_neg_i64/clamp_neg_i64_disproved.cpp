/*
DEMO: BUGGY clamp-negative-to-zero, i64 — comparison inverted.
SAW disproves with any positive x.

Expected verdict: DISPROVED.
*/

#include <cstdint>

int64_t clamp_neg_i64(int64_t x) {
    return x > 0 ? (int64_t)0 : x;   // BUG: was x < 0.
}
