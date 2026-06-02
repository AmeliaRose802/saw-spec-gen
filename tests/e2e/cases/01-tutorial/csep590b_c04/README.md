# CSEP590B 26sp — Coding Assignment 4 (end-to-end tests)

C++ and Rust ports of the five problems from
[CSEP590B 26sp C04](https://courses.cs.washington.edu/courses/csep590b/26sp/coding/c04/),
end-to-end against a Cryptol reference spec via z3.

The assignment is split into:

* **Part A — counterexample finding.** Pseudocode is buggy; the task
  is to elicit a concrete counterexample. We ship both the literal
  buggy port (`*_disproved.{cpp,rs}` — DISPROVED, the CEX plays the
  role of the homework's expected output) and a fixed implementation
  (`*_verified.{cpp,rs}` — VERIFIED).
* **Part B — invariant finding.** Pseudocode is correct; the task is
  to discharge proof obligations. We ship a reference implementation
  (`*_verified.{cpp,rs}`) bounded so SAW fully unrolls, against a
  closed-form Cryptol spec — no loop-invariant hint required.

## Problems

| # | Problem | Cryptol fn | C++ / Rust signature |
|---|---------|------------|----------------------|
| 1 | Clamped subtraction      | `clamp_sub_spec`     | `(int32, int32) -> int32` |
| 2 | Safe multiplication      | `safe_mul_spec`      | `(int32, int32) -> int32` |
| 3 | Count groups             | `count_groups_spec`  | `uint32 -> uint32` |
| 4 | Make change (bounded)    | `make_change_spec`   | `uint32 -> uint32` (packed `q<<24 \| d<<16 \| n<<8 \| p`) |
| 5 | Newton's isqrt (bounded) | `isqrt_spec`         | `uint32 -> uint32` |

Part B input bounds (SAW unrolls eagerly, so we constrain):

* Problem 4: `amount` ∈ [0, 99] cents; out-of-range returns 0.
  Per-denomination loop bounds 4 / 3 / 2 / 5.
* Problem 5: `n` ∈ [1, 255]; out-of-range returns 0. Newton's method
  unrolled 10 times (converges ≤ 6 in this range).

## Running

```powershell
# Single C++ problem
.\verify.ps1 -CppFile     tests\e2e\cases\01-tutorial\csep590b_c04\p1_clamp_sub\clamp_sub_verified.cpp `
             -CryptolSpec tests\e2e\cases\01-tutorial\csep590b_c04\p1_clamp_sub\clamp_sub_spec.cry `
             -CryptolFn   clamp_sub_spec -Function clamp_sub

# Single Rust problem
.\verify-rust.ps1 -RustFile    tests\e2e\cases\01-tutorial\csep590b_c04\p1_clamp_sub\clamp_sub_verified.rs `
                  -CryptolSpec tests\e2e\cases\01-tutorial\csep590b_c04\p1_clamp_sub\clamp_sub_spec.cry `
                  -CryptolFn   clamp_sub_spec -Function clamp_sub

# Whole suite — all 10 cases (5 problems × disproved/verified, both languages)
pwsh tests\e2e\Run-E2ETests.ps1 -Tag csep590b_c04
```

Cases are registered in [`tests/e2e/cases.psd1`](../../../cases.psd1).

## Notes

* Problem 2's disproved Rust port differs from the disproved C++
  port: the literal `if c / a != b` round-trip ported via
  `wrapping_div` hits crucible-llvm's poison-value path on
  `sdiv i32::MIN, -1` and aborts the simulator instead of yielding a
  clean DISPROVED. The Rust port drops the overflow check entirely (a
  simpler bug that still trips the spec); the C++ port keeps the
  literal translation. See per-file headers.
* Newton's-isqrt divisor `n / x` relies on `x ≥ 1` as a loop-carried
  invariant. SAW discharges it during bounded unrolling without a
  hint.
