/*
DEMO: 64-bit unsigned wrapping add ("unsigned long" / "unsigned long long").

Returns `a + b`, wrapping on overflow (well-defined in C++ for
unsigned arithmetic). Exercises:
  * `uint64_t` parameter + return — the largest unsigned integer
    width; earlier suites topped out at uint32 (`byte_swap_u32`).
  * Two-argument unsigned addition at 64 bits (lowers to `add i64`).

Expected verdict: VERIFIED.
*/

#include <cstdint>

uint64_t add_u64(uint64_t a, uint64_t b) {
    return a + b;
}
