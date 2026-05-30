/*
DEMO: 16-bit byte swap.

Returns `x` with its two bytes swapped. Exercises:
  * `uint16_t` parameter + return — width not seen by the existing
    integer suites.
  * Composition of left + right shifts at 16-bit width (lowers to
    `shl i16` / `lshr i16` followed by `or i16`).

Expected verdict: VERIFIED.
*/

#include <cstdint>

uint16_t swap_bytes_u16(uint16_t x) {
    return (uint16_t)((x >> 8) | (uint16_t)(x << 8));
}
