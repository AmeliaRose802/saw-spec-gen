// DEMO: Data-dependent bounded loop (Rust mirror of sum_first_n_verified.cpp).
//
// `sum_first_n(n)` returns 1 + 2 + ... + n.  The loop trip count is the
// *symbolic* input `n`, but we cap it at MAX_N so SAW can unroll every
// admissible path.  The Cryptol spec is the closed-form Gauss formula
//   sum_first_n_spec n = n * (n + 1) / 2     (with a sentinel for n > 10)
//
// The implicit loop invariant SAW must discover (via unrolling) is:
//   after k iterations,  total == k*(k+1)/2  AND  i == k + 1
// When the loop exits at k == n the post-condition collapses to the
// Cryptol spec.
//
// Why this is interesting:
//   - The loop bound `n` is symbolic — not a compile-time constant.
//   - SAW must show the invariant holds for *every* admissible n in
//     [0, MAX_N], not just one concrete value.
//   - The closed-form spec uses multiplication and division; the impl
//     uses repeated addition.  z3 must prove these equivalent.
//
// Build/verify:
//   .\verify-rust.ps1 `
//       -RustFile    e2e-tests\01-tutorial\bounded_loop\sum_first_n_verified.rs `
//       -CryptolSpec e2e-tests\01-tutorial\bounded_loop\sum_first_n_spec.cry `
//       -CryptolFn   sum_first_n_spec `
//       -Function    sum_first_n

const MAX_N: u32 = 10;

fn sum_first_n(n: u32) -> u32 {
    if n > MAX_N {
        return 0; // out-of-contract input → sentinel value
    }
    let mut total: u32 = 0;
    let mut i: u32 = 1;
    while i <= n {
        total += i; // loop invariant: total == i*(i-1)/2 at top of body
        i += 1;
    }
    total // total == n*(n+1)/2
}
