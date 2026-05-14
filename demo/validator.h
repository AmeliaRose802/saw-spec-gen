#pragma once
#include <cstdint>
#include <cstddef>

// ---------------------------------------------------------------------------
// A session token with versioned format
// ---------------------------------------------------------------------------
struct SessionToken {
    uint32_t version;
    uint64_t issued_at;
    uint64_t expires_at;
    uint8_t  hmac[32];
};

// ---------------------------------------------------------------------------
// Abstract base class for request validation
// ---------------------------------------------------------------------------
class IValidator {
public:
    virtual ~IValidator() = default;
    virtual bool validate(const SessionToken &token) const noexcept = 0;
    virtual int  error_code() const noexcept = 0;
};

// ---------------------------------------------------------------------------
// Concrete validator with internal state
// ---------------------------------------------------------------------------
class RequestValidator : public IValidator {
    uint64_t current_time_;
    uint32_t max_version_;
    uint32_t last_error_;

public:
    RequestValidator(uint64_t now, uint32_t max_ver) noexcept
        : current_time_(now), max_version_(max_ver), last_error_(0) {}

    // const method — reads token and internal state, modifies nothing
    bool validate(const SessionToken &token) const noexcept override;

    // const method — just returns a field
    int error_code() const noexcept override;

    // non-const method — mutates internal state
    void set_time(uint64_t new_time) noexcept;

    // non-const method — takes mutable pointer, writes result into it
    bool validate_and_record(const SessionToken &token, uint32_t *out_error) noexcept;
};

// ---------------------------------------------------------------------------
// Free functions operating on raw buffers
// ---------------------------------------------------------------------------

// Read-only: compute HMAC over a buffer
uint32_t compute_hmac(const uint8_t *data, size_t len,
                      const uint8_t *key, size_t key_len) noexcept;

// Mixed: decrypt in-place, key is read-only
bool decrypt_inplace(uint8_t *buffer, size_t len,
                     const uint8_t *key, size_t key_len) noexcept;

// Struct return: build a token (will use sret on large struct)
SessionToken make_token(uint32_t version, uint64_t now, uint64_t ttl) noexcept;
