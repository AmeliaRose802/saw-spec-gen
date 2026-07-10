// Companion to ownership_check_verified.cpp.
//
// Same std::recursive_mutex shape, verified on both toolchains -- the
// mutex primitives are still overridden (MSVC _Mutex_base helpers as
// no-ops; libstdc++ pthread_mutex_lock/unlock pinned to the success
// sentinel) -- but the guarded body returns x + 2 while the Cryptol
// spec says x + 1. The proof must DISPROVE on every platform: the
// override transparently models the lock without masking a genuine
// arithmetic bug.

#include <cstdint>
#include <mutex>

static std::recursive_mutex g_mtx_disproved;

extern "C" uint32_t ownership_check_disproved(uint32_t x) {
    std::lock_guard<std::recursive_mutex> guard(g_mtx_disproved);
    return x + 2;  // BUG: off by one vs spec x + 1.
}
