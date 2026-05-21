#include "audit.h"

AuditResult audit_token(const SessionToken *token) {
    AuditResult result;
    result.valid = 0;
    result.error = 0;
    result.token_version = 0;

    if (!token) {
        result.error = -1;
        return result;
    }

    // Non-const call: normalizes the token before validation.
    // Since it takes a non-const pointer, ANY implementation could
    // overwrite token->version with an arbitrary value.
    validator_normalize(const_cast<SessionToken*>(token));

    // Call the undefined validator — any implementation is possible
    result.valid = validator_validate(token) ? 1 : 0;

    if (!result.valid) {
        result.error = validator_error_code();
    }

    // BUG: reads version AFTER the non-const normalize call.
    // The spec says token_version == original input version,
    // but normalize() could have changed it to anything.
    result.token_version = token->version;

    return result;
}
