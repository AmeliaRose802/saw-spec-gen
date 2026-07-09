// E2E regression for BrokenReason::MsvcMutexHelper (bd issue #65).
//
// On MSVC, std::_Mutex_base methods are compiled with linkonce_odr linkage
// and perform typed field reads on the mutex struct.  Before the fix,
// extern_override_scan skipped defined non-vararg bodies, so SAW tried to
// inline the function.  With a symbolically-allocated struct the typed reads
// cause "Error during memory load".
//
// The scanner now matches `_Mutex_base@std` as a substring of the
// MSVC-mangled name (e.g. `?_Verify_ownership_levels@_Mutex_base@std@@...`).
// This broad pattern catches all sibling helpers of std::_Mutex_base without
// a per-method allowlist, and is exercised by the integration test in
// tests/verify_subcommand_test.rs with real MSVC-style IR.
//
// This e2e case uses a simulation struct `_Mutex_base_sim` compiled with
// GCC/Clang.  The GCC-mangled name does NOT contain `_Mutex_base@std`, so
// the scanner does not classify the method as MsvcMutexHelper here.  SAW
// can execute the simple int-comparison body inline, so VERIFIED/DISPROVED
// outcomes still reflect the arithmetic correctness of `ownership_check`.
//
// To run the full MSVC-style regression without a real MSVC toolchain,
// see the `msvc_mutex_helper_emits_noop_override_in_verify_script`
// integration test.

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
