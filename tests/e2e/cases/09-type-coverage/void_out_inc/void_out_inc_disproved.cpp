/*
DISPROVED variant of void_out_inc — the implementation writes BEYOND
the end of the caller-allocated buffer. The SAW harness allocates a
single `uint32_t` for the `out` pointer, so any access past offset 0
is out-of-bounds and SAW's memory-safety checker reports DISPROVED.

This demonstrates that even though the auto-generator does not yet
encode the functional postcondition for `_Out_` parameters, void-return
functions are NOT a verification dead-spot: memory-safety bugs are
still caught.
*/

#include <cstdint>
#include <sal.h>

void void_out_inc(uint32_t x, _Out_ uint32_t* out) {
    // Off-by-100 write — definitely past the end of the SAW allocation.
    *(out + 100) = x + 1;
}
