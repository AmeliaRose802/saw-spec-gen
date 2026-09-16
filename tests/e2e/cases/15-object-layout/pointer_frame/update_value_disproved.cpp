// Negative control: scalar return/post-state are correct; pointer frame fails.
// No pointer-to-integer conversion or pointee access hides the lost identity.
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
    p->data = nullptr;
    return p->value;
}