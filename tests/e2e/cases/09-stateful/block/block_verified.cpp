// block — a *wide, byte-granular* stateful post-state. block_mask
// transforms all four bytes of an opaque buffer by XOR-ing each with a
// fixed key. Unlike a uint32 field (which forces a typed i32 load the
// byte-array object model can't satisfy), a `uint8_t[4]` member is
// accessed one byte at a time, so it models cleanly as a 4-byte buffer
// and the post-state is a relational function of every input byte:
//
//   --out-buffer-param b=4  --cryptol-fn-out b=block_mask_post
//
// verified : each byte is XOR-ed with 0xAB. The Cryptol model maps the
//            same transform over the pre-state bytes; SAW proves equality.

#include <cstdint>

struct Block {
    std::uint8_t data[4];
};

extern "C"
std::uint8_t block_mask(Block* b) noexcept {
    for (int i = 0; i < 4; ++i) {
        b->data[i] = static_cast<std::uint8_t>(b->data[i] ^ 0xAB);
    }
    return 1;
}
