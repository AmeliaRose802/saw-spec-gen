/*
ADVERSARIAL (mostly): `Cell<u32>` mutated through `&self`.

The borrow checker says `prepare(&self, _)` cannot mutate `self`, but
`Cell::set` reaches in through `UnsafeCell` and writes anyway. This is
the Rust analog of C++'s `mutable` member (see
`tests/e2e/cases/02-havoc-coverage/ctor_stub_false_verdicts/add_one_disproved.cpp`).

A naive analysis that trusts the type system and skips bodies of
`&self` methods would report VERIFIED. A faithful symbolic executor
that inlines Cell::set into LLVM IR should see the store through the
UnsafeCell and report DISPROVED.

Outcome here documents which side of that line our verifier sits on.
*/

use std::cell::Cell;

struct Counter {
    bias: Cell<u32>,
}

impl Counter {
    fn prepare(&self, _x: u32) {
        // `&self`, but Cell::set is interior mutability.
        self.bias.set(42);
    }
}

fn add_one(x: u32) -> u32 {
    let counter = Counter { bias: Cell::new(0) };
    counter.prepare(x);
    x + 1 + counter.bias.get()
}
