/*
DEMO: Problem 2 (safe_mul) — BUGGY reference (CSEP590B 26sp / c04 Part A).

The homework's buggy Python pseudocode:

    def safe_mul(a, b):
        if a == 0 or b == 0:
            result = 0
        else:
            c = a * b
            if c // a != b:
                result = 0
            else:
                result = c
        return result

Why a literal C++ port doesn't work as a SAW demo:

  - `int32_t a * int32_t b` lowers to LLVM `mul nsw i32 ...`, which
    poisons the result on signed overflow.
  - `int32_t c / a` lowers to LLVM `sdiv i32`, whose `(INT_MIN, -1)`
    case is also UB.
  - SAW would disprove on those UB witnesses long before it ever
    surfaced the homework's logic bug.
  - And even if we hand-rolled `bvSDiv` over `uint32_t` (we tried),
    the resulting SMT formula is too hard for z3 in a reasonable
    time — symbolic 32-bit divide is notoriously expensive.

What the homework's bug *actually does*:

  The Python `//` operator over the engine's i32 bitvector model is
  `bvSDiv`, which wraps `INT_MIN / -1` to `INT_MIN` instead of
  erroring. So for `(a, b) = (-1, INT_MIN)`:
     c           = INT_MIN  (after the wrapped multiply)
     c // a      = bvSDiv(INT_MIN, -1) = INT_MIN
     c // a != b ?  INT_MIN != INT_MIN ?  false
  The round-trip check fails to fire, the code returns the wrapped
  `c = INT_MIN`, the spec says the right answer is 0. Bug.

This port encodes that *outcome* directly, without doing any divide:
use a correct `int64_t`-widening overflow check, but **explicitly
exempt the `(c == INT_MIN, a == -1)` corner** — i.e. the one place
the bvSDiv-based round-trip is silently OK with an overflow.

The LLVM IR contains only:
  - `mul i32` (modular, via uint32_t cast)
  - `mul nsw i64` (overflow-free: every i32*i32 fits in i64)
  - sign-extends, comparisons, branches

No `sdiv`, no `nsw` on the i32 multiply, no UB anywhere. SAW finds
the counterexample (a = -1, b = INT_MIN) in a fraction of a second
and reports a genuine value disagreement (spec 0 vs code
2147483648), so the poison-detection NOTE block correctly stays
silent.
*/

#include <climits>
#include <cstdint>

int32_t safe_mul(int32_t a, int32_t b) {
    if (a == 0 || b == 0) {
        return 0;
    }

    // Modular signed multiply via unsigned (defined wrap, no `mul nsw`).
    int32_t c = (int32_t)((uint32_t)a * (uint32_t)b);

    // *Correct* overflow check: widen to i64 (where i32*i32 cannot
    // overflow) and test against the i32 range.
    int64_t mathematical = (int64_t)a * (int64_t)b;
    bool real_overflow =
        (mathematical > (int64_t)INT32_MAX) ||
        (mathematical < (int64_t)INT32_MIN);

    // BUG: exempt the bvSDiv wrap corner from the overflow check.
    // This mirrors what `c // a != b` *does* in the Python engine at
    // `(a, b) = (-1, INT_MIN)`: `bvSDiv(INT_MIN, -1) = INT_MIN`, which
    // equals `b`, so the round-trip silently passes despite a real
    // overflow having occurred.
    bool engine_says_overflow =
        real_overflow && !(c == INT32_MIN && a == -1);

    if (engine_says_overflow) {
        return 0;
    }
    return c;
}
