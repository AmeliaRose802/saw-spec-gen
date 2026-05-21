#pragma once
#include <cstdint>

class ILog {
public:
    virtual void log(const char* message) = 0;
};

// Opaque factory — compiler cannot see the implementation,
// so it cannot devirtualize calls on the returned pointer.
ILog* create_logger();
