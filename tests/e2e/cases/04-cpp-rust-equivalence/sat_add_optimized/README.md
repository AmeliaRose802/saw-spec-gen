# `sat_add` — C++ gold reference vs Rust port

Cross-language equivalence catching a subtle bug in a "clever" Rust
port of a saturating add. The buggy version passes every obvious unit
test and looks like a Hacker's-Delight optimisation; SAW finds a
counterexample in under a second.

## The setup

Gold reference, [`sat_add.cpp`](sat_add.cpp) — straightforward overflow
check:

```cpp
uint32_t sat_add(uint32_t a, uint32_t b) {
    uint32_t sum = a + b;
    return (sum < a) ? UINT32_MAX : sum;
}
```

Two Rust ports:

[`sat_add_verified.rs`](sat_add_verified.rs) — direct translation:

```rust
fn sat_add(a: u32, b: u32) -> u32 {
    let sum = a.wrapping_add(b);
    if sum < a { u32::MAX } else { sum }
}
```

[`sat_add_disproved.rs`](sat_add_disproved.rs) — "optimised" branchless:

```rust
fn sat_add(a: u32, b: u32) -> u32 {
    let sum = a.wrapping_add(b);
    let overflow = (sum ^ a ^ b) >> 31;
    sum | overflow.wrapping_neg()
}
```

## Why the clever port is wrong

`(sum ^ a ^ b) >> 31` extracts the carry *into* bit 31, not the carry
*out of* bit 31 needed for unsigned overflow. The two coincide for
many inputs (so every obvious unit test passes) but disagree when
both top bits are set and the sum's top bit is clear (or
symmetrically):

| Input                              | Expected     | Buggy result |
| ---------------------------------- | ------------ | ------------ |
| `sat_add(0x7FFFFFFF, 1)`           | `0x80000000` | `0xFFFFFFFF` (false saturate) |
| `sat_add(0x40000000, 0x40000000)`  | `0x80000000` | `0xFFFFFFFF` (false saturate) |
| `sat_add(0x80000000, 0x80000000)`  | `u32::MAX`   | `0` (missed overflow!) |

## The Cryptol spec ([`sat_add_spec.cry`](sat_add_spec.cry))

```cryptol
sat_add_spec : [32] -> [32] -> [32]
sat_add_spec a b = if carry then 0xFFFFFFFF else sum
  where sum = a + b; carry = sum < a
```

## Running

```powershell
# Correct port — EQUIVALENT
.\verify-equiv.ps1 -CppFile  tests\e2e\cases\04-cpp-rust-equivalence\sat_add_optimized\sat_add.cpp `
                   -RustFile tests\e2e\cases\04-cpp-rust-equivalence\sat_add_optimized\sat_add_verified.rs `
                   -CryptolSpec tests\e2e\cases\04-cpp-rust-equivalence\sat_add_optimized\sat_add_spec.cry `
                   -CryptolFn sat_add_spec -Function sat_add

# "Clever" port — NOT EQUIVALENT with concrete counterexample
.\verify-equiv.ps1 -CppFile  tests\e2e\cases\04-cpp-rust-equivalence\sat_add_optimized\sat_add.cpp `
                   -RustFile tests\e2e\cases\04-cpp-rust-equivalence\sat_add_optimized\sat_add_disproved.rs `
                   -CryptolSpec tests\e2e\cases\04-cpp-rust-equivalence\sat_add_optimized\sat_add_spec.cry `
                   -CryptolFn sat_add_spec -Function sat_add
```

SAW output for the buggy version:

```
  ── C++ side ──  Verdict: VERIFIED ✓
  ── Rust side ── Verdict: DISPROVED ✗
    Counterexample: a = 2365857345, b = 2465980863
    Expected = 4294967295   Actual = 536870912

  RESULT: NOT EQUIVALENT
```

## Why this matters

The buggy version would (a) pass code review (looks like a textbook
bit-trick), (b) pass typical hand-written unit tests (zero, one, max,
max+1, max+max all happen to come out right), (c) mostly pass uniform
random fuzzing (the trigger pattern is narrow), and (d) fail
*subtly* in production — the wrong result is often off by hundreds of
millions, exactly the failure mode saturating arithmetic was supposed
to prevent.
