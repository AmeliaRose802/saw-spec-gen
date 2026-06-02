# The `!` operator trap — C++ vs Rust

A demo of why hand-eyeballing a C++→Rust port is unsafe even when the
two source bodies are **character-for-character identical**.

## The files

```cpp
// negate.cpp
unsigned int negate(unsigned int x) { return !x + 1; }
```

```rust
// negate.rs
fn negate(x: u32) -> u32 { !x + 1 }
```

```cryptol
// negate_spec.cry
negate_spec : [32] -> [32]
negate_spec x = -x
```

A reviewer skimming the diff sees the same expression in both
languages with the same "two's complement negation" comment. It looks
trivial.

## What `!` actually means

| Language | `!x` on integer | `!x + 1` |
| -------- | --------------- | -------- |
| C++  | **logical** NOT → `bool` → `0` or `1` | `1` if `x != 0`, `2` if `x == 0` |
| Rust | **bitwise** NOT (`~` in C) — same width | `(~x) + 1` = two's-complement negation = `-x` |

So Rust is correct (`-x`); C++ is wildly wrong (always `1` or `2`,
ignoring `x` beyond is-it-zero). The bug is invisible because C++
silently promotes `bool` to `int`, and Rust silently uses `!` as
bitwise NOT on integers.

## Running

```powershell
.\verify-equiv.ps1 -CppFile  tests\e2e\cases\04-cpp-rust-equivalence\not_operator_trap\negate.cpp `
                   -RustFile tests\e2e\cases\04-cpp-rust-equivalence\not_operator_trap\negate.rs `
                   -CryptolSpec tests\e2e\cases\04-cpp-rust-equivalence\not_operator_trap\negate_spec.cry `
                   -CryptolFn negate_spec -Function negate
```

Expected verdict:

```
  ── C++ side ──  Verdict: DISPROVED ✗
    Counterexample: x = 5
    Expected = 0xfffffffb   Actual = 0x00000001
  ── Rust side ── Verdict: VERIFIED ✓
  RESULT: NOT EQUIVALENT
```

## Why this matters

Any time two languages share syntax but disagree on semantics — `/`
and `%` on signed integers, `<<` on overflow, C integer promotion,
Rust `as` truncation — a formal equivalence proof against one
language-neutral spec is the cheapest way to find out which side is
lying.
