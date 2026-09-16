enum Mode { Idle = 0, Busy = 3 };
enum class Flags : unsigned char { None = 0, One = 1 };
struct State { Mode mode; Flags flags; bool active; };

unsigned int read_code(const State* p) noexcept {
    return static_cast<unsigned int>(p->mode)
        + static_cast<unsigned int>(p->flags) + (p->active ? 1u : 0u);
}