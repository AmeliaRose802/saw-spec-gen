#pragma once
#include <cstdint>

// Same interface as validator.h but with a `const` pointee — the
// validator promises (via the type system) not to write through `val`.
//
// In production code, the _In_ SAL annotation serves the same purpose
// for static analysis:
//     virtual void validate(_In_ uint32_t* val) = 0;
class IValidator {
public:
    virtual void validate(const uint32_t* val) = 0;
};
