/*
DEMO: Problem 3 (count_groups) — BUGGY reference (CSEP590B 26sp / c04 Part A).

Literal C++ translation of the buggy Python pseudocode:

    def count_groups(x):
        assume(x >= 0)
        cnt = 0; in_run = 0; run_len = 0; i = 0
        while i < BITWIDTH:
            bit = (x >> i) & 1
            if bit == 1:
                in_run = 1
                run_len = run_len + 1
            else:
                if in_run == 1:
                    in_run = 0
                    if run_len >= 2:
                        cnt = cnt + 1
                        run_len = 0
            i = i + 1
        return cnt

Two bugs in this version:

  1. `run_len` is only reset to 0 *inside* the `if run_len >= 2` block.
     A short run of one '1' leaves run_len = 1, so the next group
     starts counting at length 2 immediately — short runs glue onto
     later runs and inflate the count.

  2. If the highest-order bit is part of a run, the loop ends with
     `in_run = 1` and the final group is never tallied.

Expected verdict: DISPROVED against `count_groups_spec`.
Counterexample from bug 1: x = 0b101 = 5 (spec says 0 groups,
buggy says 1).
*/

#include <cstdint>

uint32_t count_groups(uint32_t x) {
    uint32_t cnt = 0;
    uint32_t in_run = 0;
    uint32_t run_len = 0;
    for (int i = 0; i < 32; i++) {
        uint32_t bit = (x >> i) & 1u;
        if (bit == 1u) {
            in_run = 1u;
            run_len = run_len + 1u;
        } else {
            if (in_run == 1u) {
                in_run = 0u;
                if (run_len >= 2u) {
                    cnt = cnt + 1u;
                    run_len = 0u;
                }
            }
        }
    }
    return cnt;
}
