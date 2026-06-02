// DEMO: 8-bit popcount (Rust mirror of popcount_u8_verified.cpp).
// Static bound 8 → fully unrolled by SAW. Exercises u8 inputs.

fn popcount_u8(x: u8) -> u32 {
    let mut n: u32 = 0;
    let mut i: u32 = 0;
    while i < 8 {
        n += ((x >> i) & 1) as u32;
        i += 1;
    }
    n
}
