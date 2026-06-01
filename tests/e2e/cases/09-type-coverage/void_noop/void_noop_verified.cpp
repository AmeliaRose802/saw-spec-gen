/*
DEMO: simplest possible void-returning function — no parameters that
hold output state, no observable side-effects.

Coverage: confirms that a void return-type at the TOP of the verified
target lowers cleanly. Before the fix in `emit_postcondition_and_close`,
the auto-generated SAW script emitted
    llvm_return (llvm_term {{ void_noop_spec x }});
against a function that returns nothing — causing SAW to reject the
spec with a Cryptol type-mismatch error.

After the fix, no `llvm_return` is emitted and SAW verifies the
no-UB property trivially.
*/

#include <cstdint>

void void_noop(uint32_t x) {
    (void)x;
}
