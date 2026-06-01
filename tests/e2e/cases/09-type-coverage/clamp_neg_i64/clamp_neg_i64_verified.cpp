/*
DEMO: Clamp-negative-to-zero, i64 ("long").

Returns 0 when x is strictly negative, x otherwise. Exercises:
  * `int64_t` parameter + return — the largest signed integer width;
    earlier suites topped out at i32 (`min3_i32`).
  * Signed-less-than at 64 bits (lowers to `icmp slt i64`).

Expected verdict: VERIFIED.
*/

#include <cstdint>

int64_t clamp_neg_i64(int64_t x) {
    return x < 0 ? (int64_t)0 : x;
}
