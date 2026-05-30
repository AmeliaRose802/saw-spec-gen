/*
DEMO: 32-bit byte-order reversal via shift+mask (big <-> little endian).

The shift+mask formula reorders the four bytes (b3 b2 b1 b0) into
(b0 b1 b2 b3). SAW proves bit-for-bit equivalence with the Cryptol
`split + reverse + #` reference.

Coverage: logical shifts (`>>`, `<<`) and bitwise OR over `uint32_t` —
not directly exercised by earlier tests.

Expected verdict: VERIFIED.
*/

#include <cstdint>

uint32_t byte_swap_u32(uint32_t x) {
    return ((x >> 24) & 0x000000FFu)
         | ((x >> 8)  & 0x0000FF00u)
         | ((x << 8)  & 0x00FF0000u)
         | ((x << 24) & 0xFF000000u);
}
