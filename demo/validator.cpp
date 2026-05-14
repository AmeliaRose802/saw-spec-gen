#include "validator.h"

// ---------------------------------------------------------------------------
// RequestValidator methods
// ---------------------------------------------------------------------------

bool RequestValidator::validate(const SessionToken &token) const noexcept {
    if (token.version == 0 || token.version > max_version_)
        return false;
    if (token.expires_at < current_time_)
        return false;
    if (token.issued_at > current_time_)
        return false;
    return true;
}

int RequestValidator::error_code() const noexcept {
    return static_cast<int>(last_error_);
}

void RequestValidator::set_time(uint64_t new_time) noexcept {
    current_time_ = new_time;
}

bool RequestValidator::validate_and_record(const SessionToken &token,
                                           uint32_t *out_error) noexcept {
    bool ok = validate(token);
    if (!ok) {
        last_error_ = 1;
        if (out_error)
            *out_error = 1;
    } else {
        last_error_ = 0;
        if (out_error)
            *out_error = 0;
    }
    return ok;
}

// ---------------------------------------------------------------------------
// Free functions
// ---------------------------------------------------------------------------

uint32_t compute_hmac(const uint8_t *data, size_t len,
                      const uint8_t *key, size_t key_len) noexcept {
    // Toy HMAC — just XOR-fold
    uint32_t acc = 0;
    for (size_t i = 0; i < len; ++i)
        acc ^= static_cast<uint32_t>(data[i]) << ((i % 4) * 8);
    for (size_t i = 0; i < key_len; ++i)
        acc ^= static_cast<uint32_t>(key[i]) << ((i % 4) * 8);
    return acc;
}

bool decrypt_inplace(uint8_t *buffer, size_t len,
                     const uint8_t *key, size_t key_len) noexcept {
    if (key_len == 0)
        return false;
    for (size_t i = 0; i < len; ++i)
        buffer[i] ^= key[i % key_len];
    return true;
}

SessionToken make_token(uint32_t version, uint64_t now, uint64_t ttl) noexcept {
    SessionToken t{};
    t.version = version;
    t.issued_at = now;
    t.expires_at = now + ttl;
    // hmac left zeroed for demo
    return t;
}
