// key_store (disproved) — the buggy counterpart that proves the
// whole-object post-state assertion is not vacuous.
//
// This `key_store_activate` *toggles* the flag instead of latching it,
// so an already-Active key (pre-state isActive = 1) is reverted to 0.
// That violates the monotone invariant (post-state must be 1 for every
// pre-state, per `--cryptol-fn-out ks=key_store_activate_post`), and SAW
// finds the counterexample pre-state isActive = 1 → post-state 0.
//
// The return value (1) still matches the Cryptol return model, so the
// *only* discriminating check is the stateful post-condition — exactly
// what this fixture is meant to exercise.

#include <cstdint>

struct KeyStore {
    std::uint8_t isActive;
};

extern "C"
std::uint8_t key_store_activate(KeyStore* ks) noexcept {
    // BUG: toggles rather than latching — reverts an active key.
    ks->isActive = ks->isActive ? 0 : 1;
    return 1;
}
