// Negative control: corrupt the selected member, not an inactive alternative.
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
    p->value.left ^= 1u;
    return p->value.left;
}