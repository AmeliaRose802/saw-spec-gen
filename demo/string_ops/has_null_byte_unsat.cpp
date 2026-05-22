/*
DEMO: SWAR null-byte detection -- BROKEN variant.

Identical to has_null_byte_sat.cpp except the implementer "simplified"
the formula by dropping the `& ~w` mask:

    has  = (w - 0x0101010101010101) & 0x8080808080808080;

Without that mask, a byte lane whose original value already has the
high bit set will look like a null byte: e.g. byte = 0x80 gives
0x80 - 1 = 0x7F (top bit off) -- still safe by luck -- but byte = 0x81
gives 0x81 - 1 = 0x80 (top bit on), so the broken formula reports
"has a zero byte" even though there isn't one.

SAW will pick exactly one such adversarial input as the
counterexample. Treated as a string operation: any 8-byte sequence
containing a byte >= 0x81 (most non-ASCII text) is flagged as
NUL-terminated, which would make a strlen built on this primitive
walk off the end of any buffer holding UTF-8 or binary data.

Expected RESULT: UNSAT (disproved by SAW with a concrete byte pattern).
*/

#include <cstdint>

uint32_t has_null_byte(uint64_t w) {
    constexpr uint64_t LO = 0x0101010101010101ULL;
    constexpr uint64_t HI = 0x8080808080808080ULL;
    // BUG: missing `& ~w` -- high-bit-set bytes are mis-detected as nulls.
    return ((w - LO) & HI) != 0 ? 1u : 0u;
}
