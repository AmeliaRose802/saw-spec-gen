/*
DEMO: void-return function that writes `x + 1` through an `_Out_` pointer.

Coverage: a TARGET function whose return type is `void` (existing
void-returning callsites are all on *interface methods* that get
havoc'd; no top-level target has tested this lowering path).

Expected verdict captured by experiment — see cases.psd1.
*/

#include <cstdint>
#include <sal.h>

void void_out_inc(uint32_t x, _Out_ uint32_t* out) {
    *out = x + 1;
}
