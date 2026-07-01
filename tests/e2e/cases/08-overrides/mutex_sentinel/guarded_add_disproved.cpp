// Companion to guarded_add_verified.cpp: confirms the mutex
// success-sentinel override does NOT mask a genuine bug.
//
// Same lock-guarded shape, and the lock/unlock returns are still pinned
// to `_Thrd_success`, but the success path returns `x + 2` while the
// Cryptol spec `guarded_add_spec` is `x + 1`. The proof must therefore
// still DISPROVE with a counterexample — the sentinel only removes the
// spurious lock-failure branch, it does not make every path pass.

#include <cstdint>

extern "C" int _Mtx_lock(int* m);
extern "C" int _Mtx_unlock(int* m);

uint32_t guarded_add_disproved(uint32_t x) {
    int mtx_state = 0;

    if (_Mtx_lock(&mtx_state) != 0) {
        return 0xDEADBEEFu;
    }
    uint32_t r = x + 2;  // BUG: off by one versus the spec's x + 1.
    _Mtx_unlock(&mtx_state);
    return r;
}
