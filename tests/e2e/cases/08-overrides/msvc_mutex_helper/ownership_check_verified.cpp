// E2E regression for BrokenReason::MsvcMutexHelper (bd issue #65).
//
// On MSVC, std::_Mutex_base::_Verify_ownership_levels is compiled with
// linkonce_odr linkage and performs typed field reads on the mutex struct.
// Before the fix, extern_override_scan skipped defined non-vararg bodies,
// so SAW tried to inline the function. With a symbolically-allocated struct
// the typed reads cause "Error during memory load".
//
// With the fix, the scanner detects _Verify_ownership_levels as a substring
// in both MSVC mangled names (?_Verify_ownership_levels@_Mutex_base@std@@...)
// and GCC/Clang mangled names (_ZN..._Mutex_base_sim..._Verify_...) and
// emits an assumed no-op override returning {{ 1 : [1] }}.
//
// Regression contract: remove MsvcMutexHelper classification and this
// case fails when SAW tries to execute the typed-read body.

#include <cstdint>

// Simulates std::_Mutex_base with typed int fields.
struct _Mutex_base_sim {
    int _owner;
    int _level;

    // Simulates std::_Mutex_base::_Verify_ownership_levels.
    // The GCC-mangled name embeds "_Verify_ownership_levels" as a substring
    // so the pattern match fires on Linux/GCC builds too.
    bool _Verify_ownership_levels() {
        return _owner <= _level;
    }
};

static _Mutex_base_sim g_sim;

extern "C" uint32_t ownership_check(uint32_t x) {
    g_sim._Verify_ownership_levels();
    return x + 1;
}
