#pragma once
#include <cstdint>

struct SessionToken {
    uint32_t version;
    uint64_t issued_at;
    uint64_t expires_at;
    uint8_t  hmac[32];
};

struct AuditResult {
    uint8_t  valid;      // bool
    int32_t  error;
    uint32_t token_version;
};

// ---------------------------------------------------------------------------
// These functions are DECLARED but NOT DEFINED.
// They represent any possible implementation — SAW will model them
// with havoc specs derived from the type system.
// ---------------------------------------------------------------------------

// const token → must not modify token. Returns bool.
extern "C" bool validator_validate(const SessionToken *token);

// No pointer args → pure computation. Returns error code.
extern "C" int validator_error_code(void);

// ---------------------------------------------------------------------------
// THIS is the function we verify. It calls the undefined functions above.
// ---------------------------------------------------------------------------
extern "C" AuditResult audit_token(const SessionToken *token);
