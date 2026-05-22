/*
DEMO: count_digits over a `const std::string&` parameter --
HEAP-MODE PROOF.

This is the same operation as count_digits_cstr.cpp but takes a
real C++ std::string by reference. The proof handles MSVC's
heap-allocated representation (NOT the small-string optimisation
inline buffer), so it covers strings of arbitrary length up to a
domain-specific cap.

How the proof works
-------------------
* count_digits_string_spec.cry declares the domain bound as a
  single Cryptol type alias:

      type MAX_LEN = 32

  Edit that number to change the cap. No other files need to change.

* verify_count_digits_string.saw is a hand-rolled SAW driver that:
    1. Allocates the 32-byte `std::basic_string` struct using
       `llvm_alloc (llvm_alias "class.std::basic_string")` -- a
       typed allocation that preserves the nested layout
       (`_Compressed_pair` -> `_String_val` -> `_Bxty` union +
       `_Mysize` + `_Myres`).
    2. Allocates a *separate* heap content buffer of MAX_LEN
       bytes and wires it into the basic_string's `_Bxty._Ptr`
       slot via `llvm_struct_value [...]`.
    3. Picks fresh symbolic `_Mysize` and `_Myres`.
    4. Asserts the Cryptol-defined `valid_string content mysize`
       predicate via `llvm_precond` -- this is the SINGLE place
       to control the input bounds.
    5. Forces heap mode (`_Myres > 15`) so MSVC's `_Myptr` returns
       the heap pointer rather than `&_Buf[0]`.
    6. Hands `s_ptr` to count_digits and checks the return value
       against `count_digits_string_spec content mysize`.

  SAW symbolically executes through MSVC's inline `size()`,
  `operator[]`, `_Myptr`, and `_Large_mode_engaged` definitions
  (all `linkonce_odr` in the same bitcode), unrolling the loop
  exactly MAX_LEN times, and discharges the resulting verification
  conditions to z3. The proof completes in a couple of seconds.

* The companion count_digits_string_unsat.cpp omits the upper
  digit bound; SAW finds a counterexample (e.g. byte = ':') within
  the MAX_LEN unroll.

To raise the cap
----------------
1. Edit `type MAX_LEN = 32` in count_digits_string_spec.cry.
2. Edit `let max_len = 32` in verify_count_digits_string.saw to match.
3. Re-run. (Step 2 is only required because SAW-script can't yet
   evaluate the Cryptol type-level numeric directly; we duplicate
   the constant.)

What's NOT solved by this demo
------------------------------
* gen-verify can't synthesise the basic_string layout / heap
  pointer setup on its own -- the driver is hand-rolled. The
  "valid_<TypeName>" convention (auto-emit `llvm_precond
  {{ valid_X arg }}` whenever the imported Cryptol defines such a
  predicate) is also planned but not yet implemented in
  src/saw_emit/verify_script.rs. Until then, std::string proofs
  ship as standalone .saw drivers.

Registered in tests/saw_demos/cases.psd1 with `Expected = VERIFIED`
under the `strings` tag.
*/

#include <cstdint>
#include <string>

uint32_t count_digits(const std::string& s) {
    uint32_t n = 0;
    for (size_t i = 0; i < s.size(); i++) {
        uint8_t b = static_cast<uint8_t>(s[i]);
        if (b >= 0x30 && b <= 0x39) {
            n += 1;
        }
    }
    return n;
}

