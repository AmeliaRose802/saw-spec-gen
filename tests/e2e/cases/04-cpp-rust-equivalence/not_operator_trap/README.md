# The `!` operator trap — C++ vs Rust

A demo of why hand-eyeballing a C++→Rust port is not safe, even when the
two source files are **character-for-character identical** in the body
of the function.

## The files

`negate.cpp`
```cpp
unsigned int negate(unsigned int x) {
    return !x + 1;
}
```

`negate.rs`
```rust
fn negate(x: u32) -> u32 {
    !x + 1
}
```

`negate_spec.cry`
```cryptol
negate_spec : [32] -> [32]
negate_spec x = -x
```

A reviewer skimming the diff sees the *same expression* `!x + 1` in
both languages, both wrapping `u32` arithmetic, both with the comment
*"two's complement negation"*. It looks like a trivial, mechanical port.

## What `!` actually means

| Language | `!x` when `x` is an integer | Result of `!x + 1` |
| -------- | -------------------------- | ------------------ |
| C++      | **logical** NOT, returns `bool` → `0` or `1` | `1` if `x != 0`, `2` if `x == 0` |
| Rust     | **bitwise** NOT (same as `~` in C), returns the original integer type | `(~x) + 1` = two's-complement negation = `-x` |

So:

* The **Rust** version is correct — it really is `-x`.
* The **C++** version is wildly wrong — it ignores `x`'s value entirely
  beyond "is it zero?" and always returns `1` or `2`.

The bug is invisible to a reader because C++ silently promotes the
`bool` result of `!` to `int`, and Rust silently uses `!` as bitwise
NOT on integers. The two compilers agree to disagree without
complaining.

## Running the demo

From the repo root:

```powershell
.\verify-equiv.ps1 `
    -CppFile     tests\e2e\cases\04-cpp-rust-equivalence\not_operator_trap\negate.cpp `
    -RustFile    tests\e2e\cases\04-cpp-rust-equivalence\not_operator_trap\negate.rs `
    -CryptolSpec tests\e2e\cases\04-cpp-rust-equivalence\not_operator_trap\negate_spec.cry `
    -CryptolFn   negate_spec `
    -Function    negate
```

## Expected verdict

```
  ── C++ side ────────────────────────────────────────
    Verdict: DISPROVED ✗
    Counterexample input:
      x = <some non-zero value, e.g. 5>
    Expected (negate_spec(5)) = 0xfffffffb
    Actual   (negate(5))      = 0x00000001

  ── Rust side ───────────────────────────────────────
    Verdict: VERIFIED ✓
    negate matches negate_spec on all inputs.

  RESULT: NOT EQUIVALENT
    C++  implementation disagrees with negate_spec.
```

## Why this matters

This is the kind of bug a code review *will* miss and a unit test
*might* miss (you'd have to feed it `x = 5` and check the exact bit
pattern of the return value — not just that it's non-zero or that it
"looks negative"). SAW reads the LLVM bitcode that each compiler
actually emitted, so language-level surprises like operator overloading
disagreements are caught the moment a Cryptol spec exists.

The lesson generalises: any time two languages share syntax but disagree
on semantics (`/` and `%` on signed integers, `<<` on overflow, integer
promotion rules in C, `as` truncation in Rust, etc.), a formal
equivalence proof against a single language-neutral spec is the
cheapest way to find out which side is lying.
