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

    // Call the undefined validator — any implementation is possible
    result.valid = validator_validate(token) ? 1 : 0;

    if (!result.valid) {
        result.error = validator_error_code();
    }

    // Copy the version from the token
    result.token_version = token->version;

    return result;
}
