/*
DEMO: Boolean disjunction widened to u32.

Returns `1` when either input bit is set, `0` otherwise. Exercises:
  * `bool` parameter — lowers to `i1 zeroext` in the LLVM signature,
    a width no other case in the suite covers as an *input* type.
  * Widening i1 → i32 in the return path (`zext i1 to i32`).

The return type is `uint32_t` (not `bool`) so the Cryptol spec can
return `[32]`, which avoids ABI churn around Windows-vs-SysV bool
return widening.

Expected verdict: VERIFIED.
*/

#include <cstdint>

uint32_t bool_or_to_u32(bool a, bool b) {
    return (a || b) ? 1u : 0u;
}
