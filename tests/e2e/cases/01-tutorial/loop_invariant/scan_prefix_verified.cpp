/*
DEMO: Variable-length loop verified with a Hoare/fixpoint invariant.

CONTEXT:
   SAW's default bounded symbolic execution cannot terminate when a loop's
   exit condition depends on a symbolic input.  The `sum_first_n` test in
   the sibling `bounded_loop/` directory shows the "static-bound + guard"
   workaround: restructure the loop with a compile-time constant bound.

   This directory demonstrates the ALTERNATIVE: declare a Cryptol loop
   invariant in the spec config and let SAW use its fixpoint/CHC backend
   (llvm_verify_fixpoint_chc) to prove the function correct for ALL
   lengths without any explicit unroll bound.

FUNCTION:
   `scan_prefix` counts how many leading bytes of `buf` satisfy a simple
   predicate (here: non-zero).  The loop variable `i` advances by one on
   each iteration and the exit conditions are `i >= len` or `buf[i] == 0`.

LOOP INVARIANT (expressed in Cryptol):
   At entry to each iteration, `scan_prefix_inv` holds:
     i == (number of leading non-zero bytes seen so far)
     i <= len

   After exit, i == scan_prefix_spec(buf, len).

CONFIG:
   See `scan_prefix_spec.toml` which declares:
     [functions.scan_prefix_spec]
     loop_invariants = ["scan_prefix_inv"]

   This causes saw-spec-gen to emit `llvm_verify_fixpoint_chc` in the
   generated verify.saw and to annotate result.json with
     "proof_mode": "invariant"
*/

#include <cstdint>

// Count the number of leading non-zero bytes in buf[0..len).
// Returns: number of bytes before the first zero (or len if none).
uint32_t scan_prefix(const uint8_t* buf, uint32_t len) {
    uint32_t i = 0;
    while (i < len && buf[i] != 0) {
        i++;
    }
    return i;
}
