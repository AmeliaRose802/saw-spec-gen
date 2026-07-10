// Companion to ownership_check_verified.cpp.
//
// Same std::recursive_mutex shape — any std::_Mutex_base helpers are still
// overridden as no-ops via the MsvcMutexHelper classification — but the
// guarded body returns x + 2 while the Cryptol spec says x + 1.  The proof
// must DISPROVE: the override does not mask genuine arithmetic bugs.

#include <cstdint>
#include <mutex>

static std::recursive_mutex g_mtx_disproved;

extern "C" uint32_t ownership_check_disproved(uint32_t x) {
    std::lock_guard<std::recursive_mutex> guard(g_mtx_disproved);
    return x + 2;  // BUG: off by one vs spec x + 1.
}
