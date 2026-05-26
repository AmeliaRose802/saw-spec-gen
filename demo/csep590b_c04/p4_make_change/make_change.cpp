/*
DEMO: Problem 4 (make_change) — reference (CSEP590B 26sp / c04 Part B).

C++ port of the homework's greedy-change Python program. The
original uses four data-dependent loops; SAW unrolls eagerly so we
bound `amount` to [0, 99] cents and use fixed-iteration loops with
guarded subtractions inside (same trick as bounded_loop/sum_first_n).

For amount in [0, 99]:
    q ≤ 3   (need ≤ 4 iters to cover q=3)
    d ≤ 2   (≤ 3 iters)
    n ≤ 1   (≤ 2 iters)
    p ≤ 4   (≤ 5 iters)

The result is packed one byte per denomination (q << 24 | d << 16 |
n << 8 | p), which is fine because q,d,n,p all fit in 8 bits for
this input range.

Expected verdict: SAT (VERIFIED) against `make_change_spec`.
*/

#include <cstdint>

uint32_t make_change(uint32_t amount) {
    if (amount > 99u) {
        return 0u;
    }
    uint32_t q = 0u, d = 0u, n = 0u, p = 0u;

    for (int i = 0; i < 4; i++) {
        if (amount >= 25u) { amount -= 25u; q = q + 1u; }
    }
    for (int i = 0; i < 3; i++) {
        if (amount >= 10u) { amount -= 10u; d = d + 1u; }
    }
    for (int i = 0; i < 2; i++) {
        if (amount >= 5u)  { amount -= 5u;  n = n + 1u; }
    }
    for (int i = 0; i < 5; i++) {
        if (amount >= 1u)  { amount -= 1u;  p = p + 1u; }
    }
    return (q << 24) | (d << 16) | (n << 8) | p;
}
