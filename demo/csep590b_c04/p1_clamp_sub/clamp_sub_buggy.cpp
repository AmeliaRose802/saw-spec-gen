/*
DEMO: Problem 1 (clamp_sub) — BUGGY reference (CSEP590B 26sp / c04 Part A).

Faithful port of the buggy Python pseudocode:

    def clamp_sub(a, b):
        if a > 0 and b < 0 and a - b < 0:
            result = INT_MAX
        elif a < 0 and b > 0 and a - b > 0:
            result = INT_MIN
        else:
            result = a - b
        return result

IMPORTANT — why this differs from a one-to-one source translation:

A literal `int32_t a - int32_t b` compiles to LLVM `sub nsw i32 ...`,
making any signed-overflow input pair (e.g. a = 2, b = INT_MIN)
evaluate to *poison* under LLVM semantics. SAW finds those poison
witnesses first, which masks the actual logic bug the homework
wants the student to discover. The Python pseudocode has no
analogue — Python ints don't overflow.

To expose the homework's *logic* bug (the `a == 0, b == INT_MIN`
case where the strict `a > 0` / `a < 0` guards both miss), we
compute the subtraction over `uint32_t` (modular wrap is defined,
no nsw) and only reinterpret the result as signed afterwards. The
buggy branching logic — `a > 0`, `b < 0`, `r < 0` — is then
performed on the same two's-complement bit patterns the Python
program would see at runtime, but without the UB obstacle.

Expected verdict: UNSAT (DISPROVED) against `clamp_sub_spec`.
SAW finds counterexample a = 0, b = INT_MIN:
    a > 0   is false   (a is 0, not > 0)
    a < 0   is false
    fallthrough returns the wrapped r = 0 - INT_MIN = INT_MIN.
    The spec says INT_MAX.
*/

#include <climits>
#include <cstdint>

int32_t clamp_sub(int32_t a, int32_t b) {
    // Modular two's-complement subtract via unsigned (defined wrap,
    // no `sub nsw` flag, no poison).
    int32_t r = (int32_t)((uint32_t)a - (uint32_t)b);

    int32_t result;
    if (a > 0 && b < 0 && r < 0) {
        result = INT32_MAX;
    } else if (a < 0 && b > 0 && r > 0) {
        result = INT32_MIN;
    } else {
        result = r;
    }
    return result;
}
