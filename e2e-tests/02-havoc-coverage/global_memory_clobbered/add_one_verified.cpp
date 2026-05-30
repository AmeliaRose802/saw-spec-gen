#include "ilog.h"

const int super_important = 7;

/*
DEMO: Verified — super_important is const so havoc can't clobber it

Same structure as the disproved version, but super_important is const.
The havoc model only clobbers MUTABLE globals, so the bailout
path is never reachable and add_one always returns x+1.
*/

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