# C++ gold reference vs two Rust ports — saturating add

A demo of using formal equivalence to catch a *subtle* bug in a Rust
port of a C++ reference function. The wrong version looks plausible,
even clever, and passes every "obvious" unit test a reviewer would
think to write. SAW catches it in a fraction of a second with a
concrete counterexample.

## The setup

`sat_add.cpp` is the **gold reference** — written the straightforward,
unoptimised way:

```cpp
uint32_t sat_add(uint32_t a, uint32_t b) {
    uint32_t sum = a + b;
    if (sum < a) {
        // overflow: the wrapped result is smaller than what we started with
        return UINT32_MAX;
    }
    return sum;
}
```

What is *saturating add*? Normal `u32` addition wraps around on
overflow (`u32::MAX + 1 == 0`). Saturating add instead **clamps** to
`u32::MAX` — useful for counters that mustn't roll back to zero,
durations that mustn't go negative, etc.

We want to port this to Rust. Two attempts:

### `sat_add_verified.rs` — the correct port

A direct translation of the C++:

```rust
fn sat_add(a: u32, b: u32) -> u32 {
    let sum = a.wrapping_add(b);
    if sum < a {
        u32::MAX
    } else {
        sum
    }
}
```

### `sat_add_disproved.rs` — the *clever* port

Same function, but "optimised" to remove the branch:

```rust
fn sat_add(a: u32, b: u32) -> u32 {
    let sum = a.wrapping_add(b);
    let overflow = (sum ^ a ^ b) >> 31;
    sum | overflow.wrapping_neg()
}
```

The author of this version is convinced it works. The reasoning sounds
solid: `(sum ^ a ^ b) >> 31` extracts a single bit from the top of an
XOR, `wrapping_neg` turns that bit into an all-ones or all-zeros mask,
and ORing the mask into `sum` forces the result to `u32::MAX` exactly
when overflow happened. It's a Hacker's-Delight-style trick, written
cleanly. What could go wrong?

## What goes wrong

The MSB of `(sum ^ a ^ b)` is the **carry *into* bit 31** of the adder.
For unsigned overflow detection you need the **carry *out of* bit 31**.
Those two bits coincide in many cases — enough that every obvious unit
test passes — but they disagree on a specific class of inputs: when
both `a` and `b` have their top bit set and the sum's top bit is clear,
or symmetrically, when neither has the top bit set but the sum does.

Things the "clever" version gets right:

| Input                              | Expected     | Buggy result   |
| ---------------------------------- | ------------ | -------------- |
| `sat_add(0, 0)`                    | `0`          | `0` ✓          |
| `sat_add(1, 2)`                    | `3`          | `3` ✓          |
| `sat_add(u32::MAX, 0)`             | `u32::MAX`   | `u32::MAX` ✓   |
| `sat_add(0, u32::MAX)`             | `u32::MAX`   | `u32::MAX` ✓   |
| `sat_add(u32::MAX, 1)`             | `u32::MAX`   | `u32::MAX` ✓   |
| `sat_add(u32::MAX, u32::MAX)`      | `u32::MAX`   | `u32::MAX` ✓   |

Things it silently breaks:

| Input                              | Expected     | Buggy result               |
| ---------------------------------- | ------------ | -------------------------- |
| `sat_add(0x7FFFFFFF, 1)`           | `0x80000000` | `0xFFFFFFFF` (false saturate) |
| `sat_add(0x40000000, 0x40000000)`  | `0x80000000` | `0xFFFFFFFF` (false saturate) |
| `sat_add(0x80000000, 0x80000000)`  | `u32::MAX`   | `0` (missed overflow!)      |

Two of those wrongly saturate inputs that don't actually overflow, and
one *misses* a real overflow entirely.

## The Cryptol spec

The same spec for both ports:

```cryptol
sat_add_spec : [32] -> [32] -> [32]
sat_add_spec a b = if carry then 0xFFFFFFFF else sum
  where
    sum   = a + b
    carry = sum < a
```

## Running the demo

From the repo root:

```powershell
# The correct port — should print EQUIVALENT
.\verify-equiv.ps1 `
    -CppFile     e2e-tests\04-cpp-rust-equivalence\sat_add_optimized\sat_add.cpp `
    -RustFile    e2e-tests\04-cpp-rust-equivalence\sat_add_optimized\sat_add_verified.rs `
    -CryptolSpec e2e-tests\04-cpp-rust-equivalence\sat_add_optimized\sat_add_spec.cry `
    -CryptolFn   sat_add_spec `
    -Function    sat_add

# The "clever" port — should print NOT EQUIVALENT with a concrete counterexample
.\verify-equiv.ps1 `
    -CppFile     e2e-tests\04-cpp-rust-equivalence\sat_add_optimized\sat_add.cpp `
    -RustFile    e2e-tests\04-cpp-rust-equivalence\sat_add_optimized\sat_add_disproved.rs `
    -CryptolSpec e2e-tests\04-cpp-rust-equivalence\sat_add_optimized\sat_add_spec.cry `
    -CryptolFn   sat_add_spec `
    -Function    sat_add
```

## What SAW prints for the buggy version

```
  ── C++ side ────────────────────────────────────────
    Verdict: VERIFIED ✓
  ── Rust side ───────────────────────────────────────
    Verdict: DISPROVED ✗
    Counterexample input:
      a = 2365857345
      b = 2465980863
    Expected (sat_add_spec(2365857345, 2465980863)) = 4294967295
    Actual   (sat_add(2365857345, 2465980863))      = 536870912

  RESULT: NOT EQUIVALENT
```

The real sum here is `4,831,838,208`, which overflows `u32::MAX` by
`536,870,913`. The spec saturates to `0xFFFF_FFFF`; the buggy Rust
silently returns `536,870,912` — half a billion off, with no panic, no
debug-mode trap, nothing to warn the caller. SAW found this input in
under a second by symbolically exploring *every possible* 32-bit `(a,
b)` pair.

## Why this matters

Saturating arithmetic shows up in real production code: timeouts,
retry counters, traffic-shaping caps, fixed-point DSP, image
processing. The buggy version of this function would likely:

* **Pass code review.** The bit-trick looks like a textbook
  optimisation. The comment is confident and references a respected
  source.
* **Pass the obvious unit tests.** Every edge case a human typically
  remembers to write — zero, one, max, max+1, max+max — comes out
  right.
* **Pass most fuzz testing.** A random-input fuzzer would have to draw
  inputs from a fairly specific bit-pattern distribution before it hit
  the bug; uniform `u32` sampling will mostly miss it.
* **Fail subtly in production.** When the bug fires it returns the
  *wrong magnitude entirely* — often by hundreds of millions, not a
  near-miss. A counter would skip from "near overflow" straight back
  to a small positive number — exactly the failure mode saturating
  arithmetic is supposed to prevent.

The same workflow scales to any pair of functions where you have a
trusted reference and want to be sure a "more efficient" rewrite
hasn't silently changed behaviour on some input you didn't think to
test.
