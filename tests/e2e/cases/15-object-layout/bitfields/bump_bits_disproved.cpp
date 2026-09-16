struct Bits {
    unsigned int value : 3;
    unsigned int : 2;
    unsigned int limit : 5;
    bool enabled : 1;
};

unsigned int bump_bits(Bits* p) noexcept {
    if (p->enabled) p->value = (p->value + 1) & 7;
    p->limit ^= 1; // Forbidden mutation of a framed field in shared storage.
    return p->value;
}