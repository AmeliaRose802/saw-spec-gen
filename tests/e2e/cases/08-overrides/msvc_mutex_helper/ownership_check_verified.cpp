// E2E regression for BrokenReason::MsvcMutexHelper (bd issue #65).
//
// On MSVC, std::_Mutex_base methods are compiled with linkonce_odr linkage
// and perform typed field reads on the mutex struct.  Before the fix,
// extern_override_scan skipped defined non-vararg bodies, so SAW tried to
// inline those reads through a symbolically-allocated struct and aborted with
// "Error during memory load".
//
// std::recursive_mutex inherits from std::_Mutex_base on MSVC and calls
// _Verify_ownership_levels internally to track recursive-lock ownership — the
// exact function that triggered the original crash.  The scanner now matches
// the `_Mutex_base@std` substring present in every MSVC-mangled method of
// std::_Mutex_base, classifies each as MsvcMutexHelper, and emits a pinned
// no-op override (`{{ 1 : [1] }}`).
//
// On GCC/Linux std::recursive_mutex uses pthreads; the MSVC-specific
// scanner path is exercised instead by the integration test
// `msvc_mutex_helper_emits_noop_override_in_verify_script`.

#include <cstdint>
#include <mutex>

static std::recursive_mutex g_mtx;

extern "C" uint32_t ownership_check(uint32_t x) {
    std::lock_guard<std::recursive_mutex> guard(g_mtx);
    return x + 1;
}
