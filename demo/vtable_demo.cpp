#include "vtable_demo.h"

AuditResult audit_token(const ITokenValidator *validator,
                        const SessionToken *token) noexcept {
    AuditResult result{};

    if (!validator || !token) {
        result.error = -1;
        return result;
    }

    // Virtual dispatch: validator->validate(*token)
    // The tool generates an adversarial override for this.
    result.valid = validator->validate(*token) ? 1 : 0;

    if (!result.valid) {
        // Virtual dispatch: validator->error_code()
        result.error = validator->error_code();
    } else {
        result.error = 0;
    }

    // Copy version from token — should be unchanged since validate() is const
    result.token_version = token->version;

    return result;
}
