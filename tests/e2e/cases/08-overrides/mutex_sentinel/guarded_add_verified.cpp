// E2E coverage for the mutex success-sentinel override (doc item 5 of
// docs/03-stateful-method-specs.md).
//
// `_Mtx_lock` / `_Mtx_unlock` are declare-only MSVC/UCRT threading
// status primitives returning `_Thrd_result`, whose success value
// `_Thrd_success` is 0. saw-spec-gen emits an assumed override for each
// (an `_auto_spec.saw` for the declared prototype here) and, via
// `status_primitives::success_sentinel`, pins the return to that
// sentinel instead of a fresh symbolic value.
//
// This function's VERIFIED verdict DEPENDS on the pin: on a nonzero
// (failure) status code it takes an error path returning a value that
// does not equal the Cryptol spec `x + 1`. With a fresh symbolic
// return the failure branch is reachable and the proof would DISPROVE;
// with the success sentinel it is dead and the proof VERIFIES. Remove
// the sentinel rule and this case flips to DISPROVED — that is the
// regression it guards.

#include <cstdint>

// Declare-only status primitives (no body in this TU). `extern "C"`
// keeps the symbol names exactly `_Mtx_lock` / `_Mtx_unlock`, which is
// what the curated sentinel set matches. The mutex handle is a typed
// pointer (as the real `_Mtx_t` is), so the override's pre-state binds
// it as a pointer rather than a scalar.
extern "C" int _Mtx_lock(int* m);
extern "C" int _Mtx_unlock(int* m);

uint32_t guarded_add(uint32_t x) {
    // Stand-in for the object's mutex member.
    int mtx_state = 0;

    if (_Mtx_lock(&mtx_state) != 0) {
        // Error path — dead only because the override pins success.
        return 0xDEADBEEFu;
    }
    uint32_t r = x + 1;
    _Mtx_unlock(&mtx_state);
    return r;
}
