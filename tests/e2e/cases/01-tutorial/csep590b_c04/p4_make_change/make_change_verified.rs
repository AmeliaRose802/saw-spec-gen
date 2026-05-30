// DEMO: Problem 4 (make_change) — reference (Rust mirror of
// make_change_verified.cpp).
//
// Bounded-loop port of the homework's greedy-change algorithm.
// Input is clamped to [0, 99] cents so SAW can fully unroll the
// four denomination loops. Output is packed one byte per coin:
//   bits 31..24 : quarters
//   bits 23..16 : dimes
//   bits 15.. 8 : nickels
//   bits  7.. 0 : pennies
//
// Expected verdict: VERIFIED against `make_change_spec`.

fn make_change(amount: u32) -> u32 {
    if amount > 99 {
        return 0;
    }
    let mut amt = amount;
    let mut q: u32 = 0;
    let mut d: u32 = 0;
    let mut n: u32 = 0;
    let mut p: u32 = 0;

    let mut i: u32 = 0;
    while i < 4 {
        if amt >= 25 { amt = amt.wrapping_sub(25); q = q.wrapping_add(1); }
        i = i.wrapping_add(1);
    }
    let mut i: u32 = 0;
    while i < 3 {
        if amt >= 10 { amt = amt.wrapping_sub(10); d = d.wrapping_add(1); }
        i = i.wrapping_add(1);
    }
    let mut i: u32 = 0;
    while i < 2 {
        if amt >= 5 { amt = amt.wrapping_sub(5); n = n.wrapping_add(1); }
        i = i.wrapping_add(1);
    }
    let mut i: u32 = 0;
    while i < 5 {
        if amt >= 1 { amt = amt.wrapping_sub(1); p = p.wrapping_add(1); }
        i = i.wrapping_add(1);
    }
    (q << 24) | (d << 16) | (n << 8) | p
}
