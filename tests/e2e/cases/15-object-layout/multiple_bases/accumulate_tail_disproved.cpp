// Negative control: base-field sum is correct, but the stored tail is wrong.
using uint32_t = unsigned int;
using uint64_t = unsigned long long;

struct A {
    uint32_t a;
};

struct B {
    uint64_t b;
};

struct Derived : A, B {
    bool ok;
    uint64_t tail;
};

uint64_t accumulate_tail(Derived *p) noexcept {
    const uint64_t sum = static_cast<uint64_t>(p->a) + p->b;
    if (p->ok) {
        p->tail += sum;
    }
    p->tail ^= 1;
    return sum;
}