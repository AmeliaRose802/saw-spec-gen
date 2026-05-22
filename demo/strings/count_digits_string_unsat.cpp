/*
DEMO: count_digits over `const std::string&` -- BROKEN.

Companion to count_digits_string.cpp. Same logic-bug as the cstr
variant: the upper-bound check on the digit range is missing, so
any byte with value >= '0' is counted.

Verified against the same Cryptol spec (count_digits_string_spec
with MAX_LEN = 32) via the same hand-rolled SAW driver. SAW finds
a counterexample within the MAX_LEN unroll.

Expected RESULT: NOT VERIFIED -- disproved.
*/

#include <cstdint>
#include <string>

uint32_t count_digits(const std::string& s) {
    uint32_t n = 0;
    for (size_t i = 0; i < s.size(); i++) {
        uint8_t b = static_cast<uint8_t>(s[i]);
        // BUG: missing upper bound, so non-digit bytes >= '0' also count.
        if (b >= 0x30) {
            n += 1;
        }
    }
    return n;
}
