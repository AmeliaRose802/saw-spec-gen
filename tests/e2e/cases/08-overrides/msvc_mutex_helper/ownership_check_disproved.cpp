// Companion to ownership_check_verified.cpp.
//
// The _Verify_ownership_levels override is still emitted and returns true,
// but the body has an off-by-one (x + 2 vs spec x + 1): DISPROVED for the
// right reason (real arithmetic bug, not the override).

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
