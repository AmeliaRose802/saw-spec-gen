/*
DEMO: count_digits over a fixed-length `const char*` buffer.

This is the simpler, "C-style string" baseline. The companion file
count_digits_string.cpp wraps the same operation in `std::string&`
and (currently) cannot be verified through gen-verify -- see
README.md for why.

The SAL annotation `_In_reads_(8)` tells the spec generator that
exactly 8 bytes behind `s` are readable. Without it, gen-verify
falls back to a single-byte allocation and SAW rejects every
non-zero offset load with "outside of the allocation".

Note: we include the SAW SAL shim (saw_sal.h) rather than the
real <sal.h>. The MSVC sal.h expands sized SAL macros to nothing,
so clang would not emit `AnnotateAttr` nodes for them and the
spec generator could not see the buffer length. The shim defines
each macro to `__attribute__((annotate(...)))` with the size
stringized into the attribute payload.

Expected RESULT: SAT.
*/

#include <cstdint>
#include "../vtable_havoc_spec_demos/saw_sal.h"

uint32_t count_digits(_In_reads_(8) const char* s) {
    uint32_t n = 0;
    for (uint32_t i = 0; i < 8; i++) {
        uint8_t b = static_cast<uint8_t>(s[i]);
        if (b >= 0x30 && b <= 0x39) {
            n += 1;
        }
    }
    return n;
}
