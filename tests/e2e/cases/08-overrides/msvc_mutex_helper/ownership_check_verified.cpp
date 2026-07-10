// E2E regression for the mutex-helper override path (bd issue #65),
// exercised against a REAL std::recursive_mutex on BOTH toolchains.
//
// The contract is platform-independent: a recursive_mutex-guarded
// increment must verify against the Cryptol spec `x + 1`. Each STL
// lowers the guard differently, and the override machinery makes the
// same contract close on both:
//
//   * MSVC/UCRT: std::recursive_mutex inherits std::_Mutex_base and
//     calls _Verify_ownership_levels (compiled linkonce_odr, doing
//     typed field reads on the mutex struct). Those reads abort SAW
//     with "Error during memory load" when the struct is flat symbolic
//     bytes, so they are classified MsvcMutexHelper and overridden as
//     no-ops (return true). The declare-only _Mtx_lock/_Mtx_unlock
//     status primitives are pinned to _Thrd_success (0).
//
//   * libstdc++/glibc: std::recursive_mutex::lock() -> __gthread_*
//     wrappers -> the declare-only POSIX primitives pthread_mutex_lock
//     / pthread_mutex_unlock (i32, success == 0). A fresh-symbolic
//     return would let lock() take its __throw_system_error path, so
//     the primitives are pinned to the POSIX success sentinel (0).
//     There is no _Verify_ownership_levels analog here -- libstdc++
//     delegates recursion tracking to the kernel -- so only the
//     sentinel pin is exercised.
//
// On BOTH platforms the verdict DEPENDS on the override: remove the
// pin and the spurious lock-failure/throw branch becomes reachable and
// the proof stops closing. Platform-independent unit coverage lives in
// src/emit/saw_emit/status_primitives.rs (sentinel + helper matching),
// src/transform/extern_override_scan_tests.rs (classification), and
// src/emit/saw_emit/bitcode_overrides_tests.rs (the emitted pins).

#include <cstdint>
#include <mutex>

static std::recursive_mutex g_mtx;

extern "C" uint32_t ownership_check(uint32_t x) {
    std::lock_guard<std::recursive_mutex> guard(g_mtx);
    return x + 1;
}
