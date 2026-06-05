/*
DEMO: Gapped (non-contiguous) enum tag constraint.

Expected verdict: VERIFIED.

Counterpart to `auth_enum/auth_enum_verified.cpp`, but with explicit
non-zero / non-contiguous discriminant values:

    enum class Status : uint8_t { Ok = 0, NotFound = 2, Denied = 100 };

Before bd issue `saw_spec_gen-iyh`, saw-spec-gen emitted a contiguous
range precondition `r <= (variants-1 : [N])` regardless of the source
spelling. For this enum that produced `r <= (2 : [8])` — wrongly
admitting `r = 1` (undefined) and wrongly excluding `r = 100` (valid).

The fix in iyh teaches the parser to capture each `EnumConstantDecl`'s
explicit initializer and emit a membership disjunction instead:

    llvm_precond {{ (r == (0 : [8])) \/ (r == (2 : [8])) \/ (r == (100 : [8])) }};

This case verifies once that precondition is generated correctly; the
implementation hits `__builtin_unreachable()` for any other tag, so
SAW would DISPROVE otherwise.
*/

#include <cstdint>

enum class Status : uint8_t {
    Ok       = 0,
    NotFound = 2,
    Denied   = 100,
};

uint32_t classify(Status r) {
    switch (r) {
        case Status::Ok:       return 0;
        case Status::NotFound: return 2;
        case Status::Denied:   return 100;
    }
    __builtin_unreachable();
}
