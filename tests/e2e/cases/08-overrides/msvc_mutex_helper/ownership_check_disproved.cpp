// Companion to ownership_check_verified.cpp.
//
// On GCC/Linux, `_Mutex_base_sim::_Verify_ownership_levels` is NOT
// classified as MsvcMutexHelper (the GCC-mangled name lacks `_Mutex_base@std`),
// so SAW executes the simple int-comparison body inline.  The body returns
// the comparison result which is discarded by the caller, so verification
// outcome is determined solely by the arithmetic in `ownership_check_disproved`.
// The off-by-one (x + 2 vs spec x + 1) produces DISPROVED for the right reason.

#include <cstdint>

struct _Mutex_base_sim {
    int _owner;
    int _level;

    bool _Verify_ownership_levels() {
        return _owner <= _level;
    }
};

static _Mutex_base_sim g_sim_disproved;

extern "C" uint32_t ownership_check_disproved(uint32_t x) {
    g_sim_disproved._Verify_ownership_levels();
    return x + 2;  // BUG: off by one vs spec x + 1.
}
