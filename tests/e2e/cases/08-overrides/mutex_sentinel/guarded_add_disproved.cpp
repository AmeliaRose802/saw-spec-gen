// Companion to guarded_add_verified.cpp: confirms the mutex
// success-sentinel override does NOT mask a genuine bug, again against
// a REAL `std::mutex`.
//
// Same lock-guarded shape — whichever platform mutex primitive gets
// emitted (`_Mtx_*`, `__gthread_*`, or `pthread_*`) is still pinned to
// success and `g_mtx` is still seeded from its static initializer — but
// the guarded body returns `x + 2` while the Cryptol spec
// `guarded_add_spec` is `x + 1`. The proof must therefore DISPROVE with
// a counterexample: the sentinel only removes the spurious lock-failure
// branch, it does not make every path pass.

#include <cstdint>
#include <mutex>

static std::mutex g_mtx_disproved;

extern "C" uint32_t guarded_add_disproved(uint32_t x) {
    std::lock_guard<std::mutex> guard(g_mtx_disproved);
    return x + 2;  // BUG: off by one versus the spec's x + 1.
}
