/*
DEMO: Bounded-loop verification.

This implementation of `add_one` does not use the built-in `+` operator
on the wide value.  Instead it builds the result bit-by-bit using a
32-iteration ripple-carry adder.  Each iteration:

  - reads one bit of x,
  - XORs it with the running carry to get the result bit,
  - ANDs the two to propagate the carry to the next position.

The loop bound is a compile-time constant (32) so SAW will fully unroll
it.  The Cryptol spec is the mathematical statement `x + 1` over 32-bit
words, and SAW (via z3) must prove the unrolled bit-level circuit is
equivalent to that high-level definition.
*/

unsigned int add_one(unsigned int x) {
    unsigned int result = 0u;
    unsigned int carry  = 1u;   // adding the constant 1

    for (int i = 0; i < 32; i++) {
        unsigned int bit_x = (x >> i) & 1u;
        unsigned int sum   = bit_x ^ carry;     // result bit at position i
        unsigned int next  = bit_x & carry;     // carry into position i+1
        result |= (sum << i);
        carry = next;
    }

    return result;
}
