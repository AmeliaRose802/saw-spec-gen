// Regression for docs/07: an "orphan override" vtable slot.
//
// AppError derives from std::exception and overrides what(). The
// virtual it overrides is declared in <exception> — a *system* header —
// so saw-spec-gen's clang-AST parser never records what() as an
// originating (non-override) virtual method. Left as a bare override,
// AppError::what() still earns a vtable slot, but there is no in-module
// base stub for it to reuse. Before the fix, vtable_stubs.ll therefore
// referenced an *undefined* stub symbol and failed to assemble to
// vtable_stubs.bc, so SAW aborted at llvm_load_module — before reaching
// any proof obligation.
//
// The fix reclassifies orphan overrides as originating methods: each
// gets its own stub definition, havoc spec, and llvm_unsafe_assume_spec
// binding — modelled exactly like every other external virtual call.
// The vtable global no longer dangles, the module assembles, and the
// proof runs to completion.
//
// The havoc'd what() call cannot touch the return value, so add_one is
// equivalent to the total spec add_one_spec(x) = x + 1 → VERIFIED.

#include <cstdint>
#include <exception>

class AppError : public std::exception {
public:
    // Declared but NOT defined in this TU: its body lives in another
    // object file, so AppError's vtable slot resolves to an *external*
    // symbol. saw-spec-gen must synthesize a havoc stub for it — and
    // because the overridden what() is declared in <exception> (system
    // header) there is no originating base stub to reuse, making this an
    // orphan override (the exact docs/07 condition).
    const char* what() const noexcept override;
};

uint32_t add_one(uint32_t x) {
    AppError err;
    AppError* e = &err;
    e->what();     // resolves to AppError::what — the orphan-override slot
    return x + 1;  // havoc'd what() cannot affect the result
}
