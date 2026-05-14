#pragma once
#include <cstdint>

struct SessionToken {
    uint32_t version;
    uint64_t issued_at;
    uint64_t expires_at;
    uint8_t  hmac[32];
};

// Abstract interface — implementation unknown at verification time
class IValidator {
public:
    virtual ~IValidator() = default;

    // const method: must not modify *this or *token
    // returns bool: only valid values are 0 or 1
    virtual bool validate(const SessionToken &token) const noexcept = 0;

    // const method: must not modify *this
    // returns int error code
    virtual int error_code() const noexcept = 0;
};

// ---------------------------------------------------------------------------
// THIS is the function we want to verify.
// It calls through the interface — we don't know the implementation.
// ---------------------------------------------------------------------------
struct AuditResult {
    bool     valid;
    int      error;
    uint32_t token_version;
};

// Calls IValidator::validate() and IValidator::error_code() through vtable.
// We will verify this function using assumed specs for the interface methods.
AuditResult audit_token(const IValidator *validator,
                        const SessionToken *token) noexcept;
