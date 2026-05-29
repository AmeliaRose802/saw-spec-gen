// DEMO: Bounded-loop verification (Rust mirror of add_one_verified.cpp).
//
// This `add_one` does not use Rust's built-in `+` on the full word.
// Instead it builds the result bit-by-bit using a 32-iteration
// ripple-carry adder:
//
//   - read one bit of x,
//   - XOR it with the running carry to get the result bit,
//   - AND the two to propagate the carry to the next position.
//
// The loop bound is a compile-time constant (32) so SAW fully unrolls
// it.  The Cryptol spec (`add_one_spec.cry`) is the mathematical
// statement `x + 1` over 32-bit words, and SAW (via z3) must prove the
// unrolled bit-level circuit is equivalent to that high-level
// definition.
//
// Build/verify:
//   .\verify-rust.ps1 `
//       -RustFile    demos\01-tutorial\bounded_loop\add_one_verified.rs `
//       -CryptolSpec demos\01-tutorial\bounded_loop\add_one_spec.cry `
//       -CryptolFn   add_one_spec `
//       -Function    add_one
//
// Notes on Rust-specific quirks for SAW:
//   - The pipeline compiles with `-C overflow-checks=off` so the `<<`
//     at i == 31 wraps cleanly (Cryptol-style modular arithmetic) and
//     no panic shims are emitted.
//   - The function is private; rustc's `-C link-dead-code=yes` keeps
//     the body in the bitcode so we don't need `#[no_mangle]`.

fn add_one(x: u32) -> u32 {
    let mut result: u32 = 0;
    let mut carry: u32 = 1; // adding the constant 1

    let mut i: u32 = 0;
    while i < 32 {
        let bit_x = (x >> i) & 1;
        let sum = bit_x ^ carry; // result bit at position i
        let next = bit_x & carry; // carry into position i+1
        result |= sum << i;
        carry = next;
        i += 1;
    }

    result
}
