/*
DEMO: Clamp-negative-to-zero, i8.

Returns 0 when x is strictly negative, x otherwise. Exercises:
  * `int8_t` parameter + return — width not seen by the existing
    integer suites (popcount_u8 covered u8, min3_i32 covered i32).
  * Signed-less-than at 8 bits (lowers to `icmp slt i8`).

Expected verdict: VERIFIED.
*/

#include <cstdint>

int8_t clamp_neg_i8(int8_t x) {
    return x < 0 ? (int8_t)0 : x;
}
