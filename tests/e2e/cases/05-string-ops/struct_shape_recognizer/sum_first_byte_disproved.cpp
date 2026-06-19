/*
DEMO: ArrayView rule 4 (saw_spec_gen-26d) — same `(T* buf, size_t
len)` shape as `sum_first_byte_verified.cpp`, but with a deliberate
value bug. The Cryptol spec returns `buf[0]` when `len > 0`; this
C++ implementation returns `buf[0] + 1` instead.

The struct-shape recognizer still sizes `buf` to its `len` sibling
(default behavior, no flag needed), so the read `buf[0]` succeeds.
The proof fails because the symbolic solver finds an input (e.g.
`buf = [0]`, `len = 1`) where the spec returns 0 but the C++
returns 1.

Expected RESULT: DISPROVED.
*/

#include <cstddef>
#include <cstdint>

uint32_t sum_first_byte(const uint8_t* buf, size_t len) {
    if (len == 0) {
        return 0;
    }
    // BUG: returns buf[0] + 1 instead of buf[0].
    return static_cast<uint32_t>(buf[0]) + 1u;
}
