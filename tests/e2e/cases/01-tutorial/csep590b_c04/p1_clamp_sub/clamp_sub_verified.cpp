/*
DEMO: Problem 1 (clamp_sub) — FIXED reference.

Saturating subtraction implemented via the classical two's-complement
overflow rule:

    r = a - b                       (modular i32 subtract)
    overflow iff sign(a) != sign(b) AND sign(r) != sign(a)
    on overflow:
        if a >= 0    -> +overflow   -> return INT_MAX
        else         -> -overflow   -> return INT_MIN
    else return r

Expected verdict: VERIFIED against `clamp_sub_spec`.
*/

#include <climits>
#include <cstdint>

int32_t clamp_sub(int32_t a, int32_t b) {
    // Cast to unsigned so the subtraction wraps cleanly (modular)
    // without invoking signed-overflow UB at the C++ level.
    uint32_t ua = (uint32_t)a;
    uint32_t ub = (uint32_t)b;
    uint32_t ur = ua - ub;

    uint32_t a_sign = (ua >> 31) & 1u;
    uint32_t b_sign = (ub >> 31) & 1u;
    uint32_t r_sign = (ur >> 31) & 1u;

    bool overflow = (a_sign != b_sign) && (r_sign != a_sign);
    if (overflow) {
        return (a_sign == 0u) ? INT32_MAX : INT32_MIN;
    }
    return (int32_t)ur;
}
