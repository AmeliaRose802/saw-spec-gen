enum Mode { Idle = 0, Busy = 3 };
enum class Flags : unsigned char { None = 0, One = 1 };
struct State { Mode mode; Flags flags; bool active; };

unsigned int read_code(const State* p) noexcept {
    // Flags with a fixed underlying type may legally contain any byte, not
    // only enumerator values. Incorrectly clamping them must be disproved.
    auto flags = static_cast<unsigned int>(p->flags);
    if (flags > 1) flags = 0;
    return static_cast<unsigned int>(p->mode) + flags + (p->active ? 1u : 0u);
}