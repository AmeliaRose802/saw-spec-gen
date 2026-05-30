/*
DEMO: max of two i16 values.

Returns the larger of `a` and `b`. Exercises:
  * `int16_t` parameter + return — width not seen by the existing
    integer suites.
  * Signed-greater-than at 16 bits (lowers to `icmp sgt i16`).

Expected verdict: VERIFIED.
*/

#include <cstdint>

int16_t max_i16(int16_t a, int16_t b) {
    return a > b ? a : b;
}
