#pragma once
#include <cstdint>

struct SessionToken {
    uint32_t version;
    uint64_t issued_at;
    uint64_t expires_at;
    uint8_t  hmac[32];
};

struct AuditResult {
    uint8_t  valid;
    int32_t  error;
    uint32_t token_version;
};

// Abstract interface — implementation unknown at verification time.
// The tool will generate adversarial havoc overrides for these methods.
class ITokenValidator {
public:
    virtual ~ITokenValidator() = default;

    // const method, const ref param: must not modify this or token
    virtual bool validate(const SessionToken &token) const noexcept = 0;

    // const method, no pointer params: pure query
    virtual int error_code() const noexcept = 0;
};

// THIS is the function we verify. It calls through the interface.
// We prove properties about it that hold for ALL possible ITokenValidator
// implementations, using auto-generated adversarial overrides.
AuditResult audit_token(const ITokenValidator *validator,
                        const SessionToken *token) noexcept;
