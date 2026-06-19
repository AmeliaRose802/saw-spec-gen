/*
DEMO: ArrayView rule 2 (saw_spec_gen-5mt) — `_In_z_(MAX)` SAL macro
for null-terminated input strings.

The new macro `_In_z_(N)` declares that `s` is a NUL-terminated C
string with at most `N` readable bytes. saw-spec-gen treats this
as the same allocation shape as `_In_reads_(N)` (so the C++ body
can safely read `s[0..N-1]`) and additionally emits a TODO note
pointing at the `findNul` Cryptol helper in
`lib/cryptol/saw_strings.cry` so callers can add a precondition
when they need the NUL semantic.

Here we deliberately use a fixed-length read loop bounded by the
SAL count (8 bytes); the function never looks past byte 7 even if
a NUL appears earlier. Cryptol-side, the spec is identical to the
length-bounded count_digits — the NUL terminator is irrelevant for
the value semantics in this test.

Expected RESULT: VERIFIED.
*/

#include <cstdint>
#include "../../02-havoc-coverage/saw_sal.h"

uint32_t count_digits_z(_In_z_(8) const char* s) {
    uint32_t n = 0;
    for (uint32_t i = 0; i < 8; i++) {
        uint8_t b = static_cast<uint8_t>(s[i]);
        if (b >= 0x30 && b <= 0x39) {
            n += 1;
        }
    }
    return n;
}
