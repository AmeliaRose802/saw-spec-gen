// DEMO: multi-catch dispatch on different exception types.
//
// Two custom exception types, both thrown inline (no helper that
// could be auto-overridden). The two catches do very different
// things:
//
//   * `catch (const HarmlessTag&)` — swallows the exception, prints
//     a message, falls through to `return x + 1`. Observable side
//     effect, but spec-preserving.
//
//   * `catch (const HarmfulTag&)`  — short-circuits with `return 0`,
//     which is *not* `x + 1`. A real bug at runtime.
//
// ─────────────────────────────────────────────────────────────────
//  What SAW sees, and why exception-lower changes the verdict
// ─────────────────────────────────────────────────────────────────
//
// Without exception-lower, clang lowers `try` / `catch` to MSVC
// funclet IR (`catchswitch`/`catchpad`/`catchret`) and SAW's
// llvm-pretty-bc-parser fails to load the bitcode at
// `FUNC_CODE_CATCHSWITCH`. No verdict is reached.
//
// With the upstream `saw-tools/exception-lower` pass (built native
// on Windows) plus the `scripts/fix_exclow_i8.ps1` clean-up:
//
//   1. The `throw HarmlessTag{}` and `throw HarmfulTag{}` calls are
//      rewritten to "set in-flight error flag + return sentinel".
//   2. The `catchswitch` becomes a normal CFG fork that checks the
//      flag and matches by typeinfo to pick a catch arm.
//   3. SAW symbolically executes both catch arms.
//
// Equivalence target is the *total* Cryptol spec
// `add_one_spec x = x + 1`:
//
//   * For most `x`: no throw, returns `x + 1`. ✓
//   * For `x == 7`: throws HarmlessTag → caught → returns
//     `x + 1 = 8`. ✓
//   * For `x == 99`: throws HarmfulTag → caught → returns `0`.
//     But the spec says `100`. ✗
//
// Expected RESULT (with the lowering pipeline): **UNSAT** —
// DISPROVED with counterexample `x = 99`. The harmless catch
// causes no counterexample; only the harmful catch breaks the
// equivalence, and SAW pinpoints exactly that input.
//
// This is the canonical "exception handling matters" demo: a
// program that *looks* SAT-equivalent (catches everything!) but
// secretly has a wrong return on one input, and SAW can see it
// only because the lowering pass put the catch bodies on the
// CFG instead of inside unwinder funclets.

#include <cstdint>
#include <cstdio>

struct HarmlessTag {};
struct HarmfulTag {};

uint32_t add_one(uint32_t x) {
    try {
        if (x == 7u) {
            throw HarmlessTag{};
        }
        if (x == 99u) {
            throw HarmfulTag{};
        }
    } catch (const HarmlessTag&) {
        // Harmless: log and fall through to the spec-correct return.
        std::printf("harmless tag caught at x=7\n");
    } catch (const HarmfulTag&) {
        // Harmful: short-circuit with a wrong value. This is the
        // hole the equivalence proof should expose.
        return 0;
    }
    return x + 1;
}
