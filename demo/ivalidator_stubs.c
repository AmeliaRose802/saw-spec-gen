// Vtable resolution stubs for IValidator
//
// MSVC x64 vtable layout for IValidator:
//   slot 0: destructor  (not called in audit_token, but must exist)
//   slot 1: validate(const SessionToken&) -> bool
//   slot 2: error_code() -> int
//
// We define concrete functions and a global vtable array pointing to them.
// SAW follows: this->vptr -> vtable[slot] -> stub function -> havoc spec

#include <stdbool.h>
#include <stdint.h>

struct SessionToken {
    uint32_t version;
    uint64_t issued_at;
    uint64_t expires_at;
    uint8_t  hmac[32];
};

// Stub functions — bodies are irrelevant, SAW will override them
void ivalidator_dtor_stub(void *self) {
    (void)self;
}

bool ivalidator_validate_stub(void *self, const struct SessionToken *token) {
    (void)self;
    (void)token;
    return false;
}

int ivalidator_error_code_stub(void *self) {
    (void)self;
    return 0;
}

// Concrete vtable — a global array of function pointers matching MSVC layout.
// SAW can resolve indirect calls through this.
typedef void (*vfn)(void);
vfn ivalidator_vtable[3] = {
    (vfn)ivalidator_dtor_stub,
    (vfn)ivalidator_validate_stub,
    (vfn)ivalidator_error_code_stub,
};
