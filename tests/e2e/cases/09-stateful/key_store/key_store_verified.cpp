// key_store — exercises saw-spec-gen's `--state-field` stateful-method
// branch (the R3 gap; see docs/03-stateful-method-specs.md).
//
// `key_store_activate` is a *stateful* method: its headline safety
// property is relational over the pre/post heap, not a pure function of
// the inputs. The KeyStore object carries a single-byte `isActive` flag
// at offset 0. Activation must be **monotone** — once a key is Active it
// stays Active (P1: "Active is irreversible").
//
// A pure functional-equivalence spec (`f(x) == model(x)`) can't express
// that, because it treats `this`/`ks` as an opaque pointer with no heap
// post-conditions. `--state-field ks.isActive@0:1=*->1` closes the gap:
// it models the object as a byte buffer and asserts the post-state of
// the `isActive` byte directly.
//
// verified  : always latches isActive = 1, so the post-state is 1 for
//             every pre-state. The monotone invariant holds.

#include <cstdint>

// Plain-struct key store. `isActive` is a single byte at offset 0.
struct KeyStore {
    std::uint8_t isActive;
};

// Activate the key. Returns 1 (success). Monotone: regardless of the
// prior state, the key is Active afterwards.
extern "C"
std::uint8_t key_store_activate(KeyStore* ks) noexcept {
    ks->isActive = 1;
    return 1;
}
