// Updating bookkeeping must preserve the data pointer's allocation identity.
// The pointee is deliberately neither read nor written by this operation.
using uint32_t = unsigned int;
using uint64_t = unsigned long long;

struct Node {
    uint32_t *data;
    uint64_t value;
    bool enabled;
};

uint64_t update_value(Node *p, uint64_t delta) noexcept {
    if (p->enabled) {
        p->value += delta;
    }
    return p->value;
}