/// Demo: Rust code that saw-spec-gen generates SAW specs for.
///
/// This file is the SOURCE. The matching MIR JSON (token_service.mir.json)
/// is what saw-spec-gen actually reads — it would be produced by `cargo saw-build`
/// or `mir-json` from this source.
///
/// Types showcase: enums, structs with named fields, Option, Result,
/// references (&T, &mut T), and async functions.

/// Token validity state — SAW constrains the discriminant to {0, 1, 2}
#[derive(Debug, Clone, Copy)]
pub enum TokenState {
    Valid,
    Expired,
    Revoked,
}

/// Error type — SAW constrains discriminant to {0, 1, 2, 3}
#[derive(Debug, Clone, Copy)]
pub enum AuthError {
    InvalidToken,
    Expired,
    InsufficientPermissions,
    RateLimited,
}

/// A session token with typed fields
#[derive(Debug, Clone)]
pub struct SessionToken {
    pub user_id: u64,
    pub issued_at: u64,
    pub expires_at: u64,
    pub permissions: u32,
}

/// Response from validation — returned as Result
#[derive(Debug, Clone)]
pub struct ValidationResult {
    pub state: TokenState,
    pub remaining_ttl: u64,
}

// ============================================================================
// Functions that saw-spec-gen generates specs for
// ============================================================================

/// Pure validation: reads token, returns state.
/// &SessionToken → readonly, non-null
/// Returns TokenState → discriminant ∈ {0, 1, 2}
pub fn check_token_state(token: &SessionToken, now: u64) -> TokenState {
    if now > token.expires_at {
        TokenState::Expired
    } else {
        TokenState::Valid
    }
}

/// Takes mutable reference: can modify the token.
/// &mut SessionToken → mutable, non-null
/// Returns bool
pub fn refresh_token(token: &mut SessionToken, new_expiry: u64) -> bool {
    if new_expiry <= token.issued_at {
        return false;
    }
    token.expires_at = new_expiry;
    true
}

/// Returns Result — SAW models both Ok and Err paths.
/// &SessionToken → readonly
/// Returns Result<ValidationResult, AuthError>
pub fn validate_token(
    token: &SessionToken,
    now: u64,
) -> Result<ValidationResult, AuthError> {
    if now > token.expires_at {
        return Err(AuthError::Expired);
    }
    if token.permissions == 0 {
        return Err(AuthError::InsufficientPermissions);
    }
    Ok(ValidationResult {
        state: TokenState::Valid,
        remaining_ttl: token.expires_at - now,
    })
}

/// Returns Option — None or Some(u64)
pub fn get_remaining_ttl(token: &SessionToken, now: u64) -> Option<u64> {
    if now >= token.expires_at {
        None
    } else {
        Some(token.expires_at - now)
    }
}

/// Async function — saw-spec-gen detects the coroutine return type
pub async fn validate_remote(token: &SessionToken) -> Result<bool, AuthError> {
    // In real code this would make a network call
    if token.permissions == 0 {
        Err(AuthError::InsufficientPermissions)
    } else {
        Ok(true)
    }
}
