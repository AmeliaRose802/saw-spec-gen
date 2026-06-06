/*
DEMO: ArrayView rule 4 (saw_spec_gen-26d) — struct-shape recognizer
auto-annotates `(T* buf, size_t len)` parameter pairs.

The function below takes the canonical "buffer + length" shape with
NO explicit SAL macro and NO Cryptol-binding flag. saw-spec-gen's
struct-shape recognizer notices the pattern and synthesizes
`_In_reads_(len)` on `buf`, which the rest of the pipeline turns
into a `DEFAULT_PARAMREF_MAX_LEN`-byte buffer allocation plus a
`llvm_precond {{ len <= MAX }}` bound.

Without the recognizer (legacy behavior, opt-in via
`--no-struct-shape-recognizer`): 1-byte fallback alloc → DISPROVED.
With the recognizer (default): VERIFIED.

Expected RESULT: VERIFIED.
*/

#include <cstddef>
#include <cstdint>

uint32_t sum_first_byte(const uint8_t* buf, size_t len) {
    // We only ever touch buf[0]; the length parameter is just a
    // capacity hint. The proof needs at least a 1-byte buffer, which
    // the recognizer guarantees by sizing `buf` to its length sibling.
    if (len == 0) {
        return 0;
    }
    return static_cast<uint32_t>(buf[0]);
}
