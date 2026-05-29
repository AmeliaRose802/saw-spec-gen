// DEMO: Problem 1 (clamp_sub) — FIXED reference (Rust mirror of
// clamp_sub_verified.cpp).
//
// Two's-complement saturating subtraction via the classical
// (sign(a) != sign(b)) /\ (sign(r) != sign(a)) overflow predicate.
//
// Expected verdict: VERIFIED against `clamp_sub_spec`.

fn clamp_sub(a: i32, b: i32) -> i32 {
    let ua: u32 = a as u32;
    let ub: u32 = b as u32;
    let ur: u32 = ua.wrapping_sub(ub);

    let a_sign = (ua >> 31) & 1;
    let b_sign = (ub >> 31) & 1;
    let r_sign = (ur >> 31) & 1;

    let overflow = (a_sign != b_sign) && (r_sign != a_sign);
    if overflow {
        if a_sign == 0 { i32::MAX } else { i32::MIN }
    } else {
        ur as i32
    }
}
