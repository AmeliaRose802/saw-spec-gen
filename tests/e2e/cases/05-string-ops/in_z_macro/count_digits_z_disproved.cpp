/*
DEMO: ArrayView rule 2 (saw_spec_gen-5mt) — same `_In_z_(8)`
allocation shape as `count_digits_z_verified.cpp`, but with a
deliberate value bug. The Cryptol spec counts ASCII digits
(`0x30`..`0x39`); this C++ implementation flips the comparison so
non-digits (every byte outside `0x30`..`0x39`) are counted instead.

The `_In_z_(8)` macro still allocates an 8-byte buffer, so every
read in the loop succeeds. The proof fails because the symbolic
solver finds an input (e.g. `s = "0\0\0\0\0\0\0\0"`) where the
spec returns 1 but the C++ returns 7.

Expected RESULT: DISPROVED.
*/

#include <cstdint>
#include "../../02-havoc-coverage/saw_sal.h"

uint32_t count_digits_z(_In_z_(8) const char* s) {
    uint32_t n = 0;
    for (uint32_t i = 0; i < 8; i++) {
        uint8_t b = static_cast<uint8_t>(s[i]);
        // BUG: counts non-digits instead of digits.
        if (b < 0x30 || b > 0x39) {
            n += 1;
        }
    }
    return n;
}
