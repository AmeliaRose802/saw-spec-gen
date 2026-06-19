/*
DEMO: std::vector::push_back + back() — the canonical "smart
container" workflow. Allocates a heap buffer, copies a value in,
then reads it back.

Now VERIFIED: saw-spec-gen-i47 /
`bitcode_overrides_functional_vector` emits a SAW ghost-state
coupling for `std::vector::{C1Ev, C2Ev, D1Ev, D2Ev,
push_back(T&&), back()}`. A single `declare_ghost_state
"vec_back_val_ghost"` binding is emitted at the top of the
overrides snippet (only when at least one vector method is
present), then:

  * `vector()`               -> initialises the ghost to a fresh
                                symbolic so back() before
                                push_back() is well-defined.
  * `push_back(T&&)`         -> loads `*arg`, stashes it in the
                                ghost as `val`.
  * `back()`                 -> reads `val` from the ghost,
                                returns a fresh allocation
                                `out` with `*out = val`.
  * `~vector()`              -> no-op.

The two-state coupling lets SAW prove `back() == push_back's
argument`, which is exactly what `add_one_spec x = x + 1`
requires.
*/

#include <cstdint>
#include <vector>

uint32_t add_one(uint32_t x) {
    std::vector<uint32_t> v;
    v.push_back(x + 1u);
    return v.back();
}
