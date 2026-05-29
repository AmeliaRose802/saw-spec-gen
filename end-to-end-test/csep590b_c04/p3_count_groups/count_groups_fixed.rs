// DEMO: Problem 3 (count_groups) — FIXED reference (Rust mirror of
// count_groups_fixed.cpp).
//
// Always resets run_len, and flushes a trailing run that extends
// through bit 31.
//
// Expected verdict: VERIFIED against `count_groups_spec`.

fn count_groups(x: u32) -> u32 {
    let mut cnt: u32 = 0;
    let mut in_run: u32 = 0;
    let mut run_len: u32 = 0;
    let mut i: u32 = 0;
    while i < 32 {
        let bit = (x >> i) & 1;
        if bit == 1 {
            in_run = 1;
            run_len = run_len.wrapping_add(1);
        } else if in_run == 1 {
            if run_len >= 2 {
                cnt = cnt.wrapping_add(1);
            }
            in_run = 0;
            run_len = 0;
        }
        i = i.wrapping_add(1);
    }
    if in_run == 1 && run_len >= 2 {
        cnt = cnt.wrapping_add(1);
    }
    cnt
}
