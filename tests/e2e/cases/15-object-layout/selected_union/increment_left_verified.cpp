// The contract explicitly selects left for the caller's active union member.
// This is a scalar union test, not a model of std::variant's representation.
using uint32_t = unsigned int;
using uint64_t = unsigned long long;

union Choice {
    uint32_t left;
    uint32_t right;
};

struct Tagged {
    bool valid;
    Choice value;
    uint64_t tail;
};

uint32_t increment_left(Tagged *p) noexcept {
    if (p->valid) {
        ++p->value.left;
    }
    return p->value.left;
}