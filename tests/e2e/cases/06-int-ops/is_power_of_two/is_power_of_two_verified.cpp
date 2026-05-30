/*
DEMO: Power-of-two predicate via the classic `x & (x - 1) == 0` bit trick.

  * Returns 1 when x is exactly 2^k for 0 <= k < 32,
  * Returns 0 otherwise.
  * Notably handles x == 0 correctly (the bit trick alone would say
    "yes" because 0 & (0 - 1) == 0).

Coverage: bit-trick predicate over a single uint32 with a 0/1-valued
return — pattern absent from earlier tests.

Expected verdict: VERIFIED.
*/

#include <cstdint>

uint32_t is_power_of_two(uint32_t x) {
    return (x != 0 && (x & (x - 1)) == 0) ? 1u : 0u;
}
