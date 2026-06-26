/*
DEMO: String content verification — to_lower_ascii (verified).

`to_lower_ascii` converts a 6-byte input buffer to ASCII lowercase:
bytes in ['A'..'Z'] (0x41–0x5A) get +0x20; all other bytes are
copied unchanged. The result is written into `dst`.

This test verifies *content-level string transformation*: the
postcondition asserts that every output byte equals the correct
lowercase form of the corresponding input byte, for all possible
6-byte inputs. SAW verifies this universally (over all 2^48 inputs).

The `_In_reads_(6)` and `_Out_writes_(6)` SAL annotations tell
saw-spec-gen to allocate the correct buffers. The `_Out_writes_(6)`
annotation also triggers auto-detection of `dst_post` in the Cryptol
spec, so no CLI flags are needed.

Expected RESULT: VERIFIED.
*/

#include <cstdint>
#include <cstddef>
#include "../../02-havoc-coverage/saw_sal.h"

std::uint32_t to_lower_ascii(_Out_writes_(6) std::uint8_t* dst,
                              _In_reads_(6) const std::uint8_t* src) noexcept {
    for (std::uint32_t i = 0; i < 6; ++i) {
        std::uint8_t b = src[i];
        if (b >= 0x41 && b <= 0x5A) {
            dst[i] = static_cast<std::uint8_t>(b + 0x20);
        } else {
            dst[i] = b;
        }
    }
    return 6;
}
