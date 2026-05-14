#include "interface_example.h"

AuditResult audit_token(const IValidator *validator,
                        const SessionToken *token) noexcept {
    AuditResult result{};

    if (!validator || !token) {
        result.valid = false;
        result.error = -1;
        result.token_version = 0;
        return result;
    }

    // Call through the interface — virtual dispatch
    result.valid = validator->validate(*token);

    if (!result.valid) {
        result.error = validator->error_code();
    } else {
        result.error = 0;
    }

    result.token_version = token->version;
    return result;
}
