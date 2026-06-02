/*
DEMO: Data-dependent loop with a STATIC upper bound (so SAW terminates).

KEY INSIGHT: SAW's default symbolic execution does NOT do path-sat
pruning on loop back-edges.  A loop whose exit condition depends on a
symbolic input (`i <= n`) will be unrolled forever — even if the
function has an early-return `if (n > MAX_N) return 0`, because SAW
doesn't propagate that fact back into the loop's termination check.

The fix is to give the loop a CONSTANT bound (MAX_N) and put the
data-dependent piece inside the body as a guard:

   for (i = 1; i <= MAX_N; i++)        // static, unrolled MAX_N times
       if (i <= n) total += i;         // data-dependent guard

The loop still does `n` semantic iterations of useful work, but SAW
only has to symbolically execute MAX_N=10 fixed iterations.

The invariant SAW implicitly checks on every unrolled iteration is:
   total == (min(i-1, n) * (min(i-1, n) + 1)) / 2

After MAX_N iterations the post-condition collapses to the Cryptol
closed-form spec  n*(n+1)/2  for n in [0, MAX_N], else 0.
*/

#include <cstdint>

constexpr uint32_t MAX_N = 10;

uint32_t sum_first_n(uint32_t n) {
    if (n > MAX_N) {
        return 0;          // out-of-contract input → sentinel
    }
    uint32_t total = 0;
    // STATIC bound: SAW unrolls exactly MAX_N times, no symbolic divergence.
    for (uint32_t i = 1; i <= MAX_N; i++) {
        if (i <= n) {
            total += i;    // guarded data-dependent update
        }
    }
    return total;          // == n*(n+1)/2 for n in [0, MAX_N]
}
