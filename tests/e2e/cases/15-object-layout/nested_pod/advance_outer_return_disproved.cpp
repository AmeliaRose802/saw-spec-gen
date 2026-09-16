// Negative control: caller state is correct, but a returned array cell is not.
using uint8_t = unsigned char;
using uint32_t = unsigned int;
using uint64_t = unsigned long long;

struct Inner {
    uint32_t x;
    bool enabled;
};

struct alignas(16) Outer {
    uint8_t tag;
    Inner inner;
    uint64_t arr[2];
};

Outer advance_outer(Outer &p, uint32_t delta) noexcept {
    Outer next = p;
    if (next.inner.enabled) {
        next.inner.x += delta;
        next.arr[1] += next.arr[0];
    }
    p.inner.x = next.inner.x;
    next.arr[1] ^= 1;
    return next;
}