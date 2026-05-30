/*
DEMO: Enum-tag range constraint, automatically derived from the
`enum class AuthResult : uint8_t { Ok = 0, Pending = 1, Failed = 2 }`
declaration.

Expected verdict: VERIFIED.

The function switches over every declared variant and ends with
`__builtin_unreachable()` so any out-of-range discriminant is undefined
behaviour. SAW's crucible-llvm raises an error when the `unreachable`
LLVM instruction is reachable, which means without a precondition
constraining `r` to `{0, 1, 2}` the solver will pick `r = 0xff` and the
case DISPROVES.

`saw-spec-gen gen-verify` is expected to emit

    llvm_precond {{ r <= (2 : [8]) }};

automatically based on the EnumDecl + its three variants. The
underlying-type width (`uint8_t` here) becomes the Cryptol bit width.
*/

#include <cstdint>

enum class AuthResult : uint8_t {
    Ok      = 0,
    Pending = 1,
    Failed  = 2,
};

uint32_t classify(AuthResult r) {
    switch (r) {
        case AuthResult::Ok:      return 0;
        case AuthResult::Pending: return 1;
        case AuthResult::Failed:  return 2;
    }
    __builtin_unreachable();
}
