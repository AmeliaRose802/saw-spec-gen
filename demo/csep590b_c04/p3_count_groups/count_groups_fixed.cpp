/*
DEMO: Problem 3 (count_groups) — FIXED reference.

Corrected version of the buggy algorithm:

  * Always reset `run_len = 0` when a 0-bit closes a run, regardless
    of whether the closing run was counted.
  * After the loop, flush a trailing run-of-length-2+ that ran into
    bit 31 without being closed by a 0-bit.

Loop bound is the compile-time constant 32, so SAW fully unrolls.

Expected verdict: SAT (VERIFIED) against `count_groups_spec`.
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
                if (run_len >= 2u) {
                    cnt = cnt + 1u;
                }
                in_run = 0u;
                run_len = 0u;     // fix: always reset
            }
        }
    }
    // fix: flush any trailing run that extended through bit 31
    if (in_run == 1u && run_len >= 2u) {
        cnt = cnt + 1u;
    }
    return cnt;
}
