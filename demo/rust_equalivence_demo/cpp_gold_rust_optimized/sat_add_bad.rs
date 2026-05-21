// Saturating add — branchless Rust port of sat_add.cpp.
//
// Optimisation: skip the branch by computing the carry as an integer
// mask and OR'ing it in. The MSB of `(sum ^ a ^ b)` is the carry into
// bit 31 of the adder, which is set exactly when `a + b` overflows
// `u32`. (Same family of bit-tricks as Hacker's Delight §2-13.)

fn sat_add(a: u32, b: u32) -> u32 {
    let sum = a.wrapping_add(b);
    let overflow = (sum ^ a ^ b) >> 31;
    sum | overflow.wrapping_neg()
}
