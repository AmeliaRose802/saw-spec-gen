#include "ilog.h"



/*
DEMO: Unsat due to global memory clobbering

create_logger() is an opaque factory — the compiler cannot see its
implementation, so it cannot devirtualize logger->log().  The call
goes through the vtable and SAW's havoc model applies.

The havoc model says logger->log() could write ANY value to any
mutable global.  Since super_important is mutable, the solver can
set it to -1, triggering the bailout path and breaking the
postcondition (returns 12 instead of x+1).
*/

int super_important = 7;

// Takes an unsigned integer, returns it plus 1.
uint32_t add_one(uint32_t x) {
    ILog* logger = create_logger();

    logger->log("Adding one to x");

    // If a virtual method clobbered super_important, bail out
    if (super_important == -1) {
        return 12;
    }

    return x + 1;
}