/*
DEMO: ArrayView rule 1 (saw_spec_gen-4po) — same allocation shape
as `count_bytes_bind_verified.cpp`, but with a deliberate value
bug. The Cryptol spec counts ASCII digits (`0x30`..`0x39`); this
C++ implementation widens the upper bound to `0x40` so the byte
`'@'` (0x40) is wrongly counted as a digit.

saw-spec-gen reads `n <= 8` off the Cryptol signature and allocates
an 8-byte buffer — every read in the loop succeeds. The proof fails
because the symbolic solver finds an input (e.g. `s = "@\0\0\0\0\0\0\0"`)
where the C++ returns 1 but the spec returns 0.

Expected RESULT: DISPROVED.
*/

#include <cstdint>

uint32_t count_bytes(const uint8_t* s) {
    uint32_t n = 0;
    for (uint32_t i = 0; i < 8; i++) {
        // BUG: upper bound is 0x40 instead of 0x39 — '@' counts as a digit.
        if (s[i] >= 0x30 && s[i] <= 0x40) {
            n += 1;
        }
    }
    return n;
}
