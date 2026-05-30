/*
DEMO: BUGGY 32-bit byte-order reversal — last lane mask is wrong.

The high byte is shifted to position 0 using a 16-bit (rather than
24-bit) shift, so the top input byte ends up in the wrong lane. SAW
finds a counterexample with any input whose top byte differs from
its byte-3 position.

Expected verdict: DISPROVED.
*/

#include <cstdint>

uint32_t byte_swap_u32(uint32_t x) {
    return ((x >> 16) & 0x000000FFu)   // BUG: was (x >> 24) & 0x000000FFu
         | ((x >> 8)  & 0x0000FF00u)
         | ((x << 8)  & 0x00FF0000u)
         | ((x << 24) & 0xFF000000u);
}
