# CSEP590B 26sp — Coding Assignment 4 demos

C++ and Rust ports of the five problems from
[CSEP590B 26sp Coding Assignment 4](https://courses.cs.washington.edu/courses/csep590b/26sp/coding/c04/),
adapted into this repo's SAW verification pipeline. Every problem is
checked end-to-end against a Cryptol reference spec via z3.

The original assignment is split into:

  * **Part A — counterexample finding.** The provided pseudocode is
    buggy; the task is to make a symbolic-execution engine spit out
    a concrete counterexample.
  * **Part B — invariant finding.** The provided pseudocode is
    correct; the task is to discharge proof obligations with loop
    invariants.

Here we adapt both halves to bit-precise z3-backed equivalence
checking:

  * For Part A problems we ship **both** the literal buggy port
    (DISPROVED by SAW, the counterexample plays the role of the
    homework's expected output) and a **fixed** implementation
    (VERIFIED).
  * For Part B problems we ship a **reference** implementation
    bounded so SAW can fully unroll, against a closed-form Cryptol
    spec (no loop-invariant hole required — the bounded loop is
    handled by SAW's existing pipeline).

## Problems

| # | Problem | Cryptol fn | C++ args / return | Rust args / return |
|---|---------|------------|-------------------|--------------------|
| 1 | Clamped subtraction      | `clamp_sub_spec`      | `int32_t(int32_t,int32_t)` | `fn(i32,i32) -> i32` |
| 2 | Safe multiplication      | `safe_mul_spec`       | `int32_t(int32_t,int32_t)` | `fn(i32,i32) -> i32` |
| 3 | Count groups             | `count_groups_spec`   | `uint32_t(uint32_t)`       | `fn(u32) -> u32` |
| 4 | Make change (bounded)    | `make_change_spec`    | `uint32_t(uint32_t)`       | `fn(u32) -> u32` |
| 5 | Newton's isqrt (bounded) | `isqrt_spec`          | `uint32_t(uint32_t)`       | `fn(u32) -> u32` |

Output for Problem 4 is packed as one byte per coin
(`q << 24 | d << 16 | n << 8 | p`); see
[`p4_make_change/make_change_spec.cry`](p4_make_change/make_change_spec.cry).

### Part B input bounds

Part B problems use loops whose iteration counts depend on the
input. SAW unrolls eagerly, so we constrain the inputs and bound
the loops:

  * Problem 4: `amount` ∈ [0, 99] cents; out-of-range returns 0.
    Per-denomination loop bounds: 4 / 3 / 2 / 5 iterations.
  * Problem 5: `n` ∈ [1, 255]; out-of-range returns 0. Newton's
    method is unrolled 10 times (converges in ≤ 6 for this range);
    once `y ≥ x` the guarded body becomes a no-op.

## Running individual demos

```powershell
# C++ side
.\verify.ps1 `
    -CppFile     demo\csep590b_c04\p1_clamp_sub\clamp_sub_fixed.cpp `
    -CryptolSpec demo\csep590b_c04\p1_clamp_sub\clamp_sub_spec.cry `
    -CryptolFn   clamp_sub_spec `
    -Function    clamp_sub

# Rust side
.\verify-rust.ps1 `
    -RustFile    demo\csep590b_c04\p1_clamp_sub\clamp_sub_fixed.rs `
    -CryptolSpec demo\csep590b_c04\p1_clamp_sub\clamp_sub_spec.cry `
    -CryptolFn   clamp_sub_spec `
    -Function    clamp_sub
```

## Running via the test harness

All ten demos (5 problems × {buggy, fixed} for Part A; 5 problems
× 1 reference for Part B; both C++ and Rust ports) are registered
in [`tests/saw_demos/cases.psd1`](../../tests/saw_demos/cases.psd1)
under the tag `csep590b_c04`:

```powershell
pwsh tests\saw_demos\Run-SawDemos.ps1 -Tag csep590b_c04
```

## Notes

  * Problem 2's buggy Rust port differs from the buggy C++ port:
    the literal `if c / a != b` round-trip pattern, when ported to
    Rust via `wrapping_div`, hits crucible-llvm's poison-value path
    on the `sdiv i32::MIN, -1` corner and aborts the simulator
    instead of yielding a clean DISPROVED. The Rust buggy port
    therefore omits the overflow check entirely — a simpler bug
    that still trips the spec — while the C++ port retains the
    literal Python translation. See the per-file headers for
    details.

  * Newton's-isqrt divisor (`n / x`) relies on `x ≥ 1` as a
    loop-carried invariant. That invariant is inductive and SAW
    discharges it during bounded-loop unrolling without a hint.
