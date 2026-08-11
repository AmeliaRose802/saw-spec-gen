// DISPROVED companion to add_one_verified.cpp — proves the orphan-override
// havoc spec for what() is *sound*, not vacuous.
//
// Same orphan-override setup: AppError overrides std::exception::what(),
// a virtual declared in the <exception> system header (docs/07). The fix
// synthesizes a havoc stub for AppError::what() and binds it with
// llvm_unsafe_assume_spec. Because what() returns `const char*`, the
// pointer-return havoc path fires: the stub allocates a fresh pointee and
// fills it with an unconstrained fresh byte, then returns that pointer —
// i.e. "what() may hand back a pointer to ANY bytes."
//
// Here add_one *dereferences* what()'s result and folds the byte into the
// return value. The verified sibling ignored the return, so the havoc
// couldn't reach the result (→ VERIFIED). This one lets the havoc'd byte
// through, so add_one(x) is x + 1 + <arbitrary 0..255> — which is NOT
// equal to the total spec add_one_spec(x) = x + 1. SAW finds a
// counterexample (any ret_val != 0) → DISPROVED.
//
// If the havoc spec had wrongly modeled what() as returning a fixed/null
// pointer to zeroed memory, this would falsely VERIFY. The DISPROVED
// verdict confirms the spec correctly treats what()'s return — and the
// memory it points at — as adversarial.

#include <cstdint>
#include <exception>

class AppError : public std::exception {
public:
    // Declared, not defined in this TU: an external virtual whose base
    // what() lives in <exception>, making AppError::what() an orphan
    // override. saw-spec-gen synthesizes a pointer-return havoc stub.
    const char* what() const noexcept override;
};

uint32_t add_one(uint32_t x) {
    AppError err;
    AppError* e = &err;
    const char* msg = e->what();  // havoc'd: pointer to an arbitrary byte
    // Folding the havoc'd byte into the result makes add_one escape the
    // x + 1 spec whenever what() returns a pointer to a non-zero byte.
    return x + 1 + static_cast<uint32_t>(static_cast<unsigned char>(msg[0]));
}
