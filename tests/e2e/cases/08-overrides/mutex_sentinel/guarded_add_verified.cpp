// E2E coverage for the mutex success-sentinel override (doc item 5 of
// docs/03-stateful-method-specs.md), exercised against a REAL
// `std::mutex` — not a hand-declared stand-in.
//
// `std::lock_guard<std::mutex>` lowers to declare-only threading
// primitives that return 0 on success (MSVC: `_Mtx_lock` /
// `_Mtx_unlock`; Linux/libstdc++: `__gthread_mutex_lock` /
// `__gthread_mutex_unlock`, often forwarding to `pthread_mutex_*`).
// saw-spec-gen emits an assumed override for each and, via
// `status_primitives::success_sentinel`, pins the return to that
// sentinel instead of a fresh symbolic value. It also seeds the
// file-scope `g_mtx` global from its own compile-time static
// initializer (`llvm_global_initializer`) so the mutex's internal
// recursion-count field reads back its real freshly-constructed value.
//
// This VERIFIED verdict DEPENDS on the sentinel pin: with a fresh
// symbolic status the lock-failure path (which `_Throw_Cpp_error`s and
// diverges) would be reachable and the proof would fail to close;
// pinned to success it is dead and the proof VERIFIES against the
// Cryptol spec `guarded_add_spec x = x + 1`. Remove the sentinel rule
// and this case stops verifying — that is the regression it guards.

#include <cstdint>
#include <mutex>

// A real std::mutex member, guarded by std::lock_guard — the exact
// production shape the sentinel + global-initializer support exists for.
static std::mutex g_mtx;

extern "C" uint32_t guarded_add(uint32_t x) {
    std::lock_guard<std::mutex> guard(g_mtx);
    return x + 1;
}
