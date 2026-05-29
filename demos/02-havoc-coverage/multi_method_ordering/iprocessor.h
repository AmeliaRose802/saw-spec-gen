#pragma once
#include <cstdint>
#include <cstdio>
#include <cstdlib>

/*
Shared interface for the "multi-method vtable ordering" demo.

IProcessor has FOUR virtual methods.  Three of them are `const`, so
under the havoc model their auto-generated specs preserve `this` (and
in particular the `bias_` member).  Exactly ONE method — `prepare()` —
is non-const, so its havoc spec is permitted to clobber `bias_`.

This is a vtable-ordering stress test: each test .cpp calls a single
method through an `IProcessor*`.  If the spec generator wires vtable
slots in the WRONG order (e.g. attaches the "prepare" havoc spec to
the slot that the compiler emits for "validate"), then calling
`validate()` would mysteriously fail and calling `prepare()` would
mysteriously succeed — i.e. the VERIFIED/DISPROVED pattern would invert.

BadProcessor adds a FIFTH virtual method `extra()` that is NOT on the
parent interface.  This exercises the "new method on a derived class"
path through the spec generator: the tool must add a stub for `extra`
to BadProcessor's vtable at slot 4 without disturbing slots 0..3.
*/

class IProcessor {
public:
    uint32_t bias_ = 0;

    // Declaration order = vtable slot order (MSVC + Itanium ABI).
    virtual void validate(uint32_t x) const = 0;  // slot 0 — const, SAFE
    virtual void audit(uint32_t x) const    = 0;  // slot 1 — const, SAFE
    virtual void prepare(uint32_t x)        = 0;  // slot 2 — NON-CONST, BAD
    virtual void report(uint32_t x) const   = 0;  // slot 3 — const, SAFE
};

// "Well-behaved" concrete impl — none of the methods touch bias_.
class GoodProcessor : public IProcessor {
public:
    void validate(uint32_t x) const override { printf("good.validate %u\n", x); }
    void audit(uint32_t x) const    override { printf("good.audit %u\n", x); }
    void prepare(uint32_t x)        override { printf("good.prepare %u\n", x); }
    void report(uint32_t x) const   override { printf("good.report %u\n", x); }
};

// "Misbehaving" concrete impl — prepare() clobbers bias_, and
// the subclass adds a brand-new virtual method `extra()` (also
// clobbers bias_) that is NOT on the IProcessor parent.
class BadProcessor : public IProcessor {
public:
    void validate(uint32_t x) const override { printf("bad.validate %u\n", x); }
    void audit(uint32_t x) const    override { printf("bad.audit %u\n", x); }
    void prepare(uint32_t x)        override { bias_ = 42; }
    void report(uint32_t x) const   override { printf("bad.report %u\n", x); }

    // NEW vtable slot 4 — not present on the IProcessor parent.
    virtual void extra(uint32_t x)           { bias_ = 99; }
};
