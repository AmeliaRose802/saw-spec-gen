/*
DEMO: count_digits over a fixed-length `const char*` buffer -- BROKEN.

Companion to count_digits_cstr.cpp. The implementation has a subtle
logic bug: it checks `b >= '0'` but forgets the upper bound, so every
byte with value >= 0x30 (any digit, letter, or punctuation in the
upper half of ASCII) is counted -- not just the actual digits
0x30..0x39.

SAW finds the smallest counterexample distinguishing this from the
spec: any 8-byte input containing at least one byte in
[0x3A, 0xFF] (e.g. ':', 'A', or any non-digit ASCII).

Expected RESULT: DISPROVED (disproved -- spec disagrees with impl).
*/

#include <cstdint>
#include "../../02-havoc-coverage/saw_sal.h"

uint32_t count_digits(_In_reads_(8) const char* s) {
    uint32_t n = 0;
    for (uint32_t i = 0; i < 8; i++) {
        uint8_t b = static_cast<uint8_t>(s[i]);
        // BUG: missing upper bound, so non-digit bytes >= '0' also count.
        if (b >= 0x30) {
            n += 1;
        }
    }
    return n;
}
