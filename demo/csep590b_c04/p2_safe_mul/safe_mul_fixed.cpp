/*
DEMO: Problem 2 (safe_mul) — FIXED reference.

Multiply in a wider type (int64_t), check whether the result falls
in the signed-i32 range, return 0 on overflow.

Expected verdict: SAT (VERIFIED) against `safe_mul_spec`.
*/

#include <climits>
#include <cstdint>

int32_t safe_mul(int32_t a, int32_t b) {
    int64_t c = (int64_t)a * (int64_t)b;
    if (c > (int64_t)INT32_MAX || c < (int64_t)INT32_MIN) {
        return 0;
    }
    return (int32_t)c;
}
