/*
DEMO: String content verification — to_lower_ascii (disproved).

Same signature as to_lower_ascii_verified.cpp, but this version has a
bug: it applies the lowercase shift to bytes in ['a'..'z'] (0x61–0x7A)
instead of ['A'..'Z'] (0x41–0x5A). This is a "double-lowercase" bug:
bytes already in lowercase get shifted a second time into control
characters, while uppercase letters pass through unchanged.

SAW finds a counterexample immediately: e.g. input 'A' (0x41) is
copied unchanged, but the spec expects 'a' (0x61).

The `_In_reads_(6)` and `_Out_writes_(6)` annotations trigger the
auto-postcondition from `dst_post` — no CLI flags needed.

Expected RESULT: DISPROVED.
*/

#include <cstdint>
#include <cstddef>
#include "../../02-havoc-coverage/saw_sal.h"

std::uint32_t to_lower_ascii(_Out_writes_(6) std::uint8_t* dst,
                              _In_reads_(6) const std::uint8_t* src) noexcept {
    for (std::uint32_t i = 0; i < 6; ++i) {
        std::uint8_t b = src[i];
        // Bug: checks lowercase range instead of uppercase range.
        if (b >= 0x61 && b <= 0x7A) {
            dst[i] = static_cast<std::uint8_t>(b + 0x20); // double-shifts lowercase
        } else {
            dst[i] = b; // leaves uppercase unchanged
        }
    }
    return 6;
}
