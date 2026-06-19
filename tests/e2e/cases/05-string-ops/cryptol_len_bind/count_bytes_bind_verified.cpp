/*
DEMO: ArrayView rule 1 (saw_spec_gen-4po) — bind a Cryptol type
variable to a C++ pointer parameter's length.

The Cryptol spec for `count_bytes` is

    count_bytes_spec : {n}(fin n, n <= 8) => [n][8] -> [32]

i.e. it is *length-polymorphic* in `n`, with an upper bound of 8.
The C++ implementation reads exactly 8 bytes behind `s`. There is
NO `_In_reads_(8)` SAL macro on the parameter on purpose — the
test demonstrates that saw-spec-gen's `--bind-cryptol-lengths`
flag extracts the upper bound `n <= 8` from the Cryptol signature
and allocates an 8-byte buffer instead of the 1-byte fallback.

Without the flag, gen-verify would emit an unsized `_In_z_`/TODO
block, allocate `llvm_int 8`, and SAW would reject the read at
offset 1 with "outside of the allocation" — DISPROVED.

Expected RESULT: VERIFIED (with `--bind-cryptol-lengths`).
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
