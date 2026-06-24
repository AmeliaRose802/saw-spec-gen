/*
DEMO: ArrayView rule 1 (saw_spec_gen-4po) — bind a Cryptol type
variable to a C++ pointer parameter's length.

The Cryptol spec for `count_bytes` is

    count_bytes_spec : {n}(fin n, n <= 8) => [n][8] -> [32]

i.e. it is *length-polymorphic* in `n`, with an upper bound of 8.
The C++ implementation reads exactly 8 bytes behind `s`. There is
NO `_In_reads_(8)` SAL macro on the parameter — saw-spec-gen reads
the upper bound `n <= 8` directly from the Cryptol signature and
allocates an 8-byte buffer automatically.

Expected RESULT: VERIFIED.
*/

#include <cstdint>

uint32_t count_bytes(const uint8_t* s) {
    uint32_t n = 0;
    for (uint32_t i = 0; i < 8; i++) {
        if (s[i] >= 0x30 && s[i] <= 0x39) {
            n += 1;
        }
    }
    return n;
}
