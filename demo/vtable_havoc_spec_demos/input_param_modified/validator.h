#pragma once
#include <cstdint>

// Interface — implementations live in another translation unit.
// The header is all SAW sees; concrete subclasses are opaque.
class IValidator {
public:
    virtual void validate(uint32_t* val) = 0;
};
