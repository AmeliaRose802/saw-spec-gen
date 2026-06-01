/*
DEMO: std::unique_ptr<uint32_t> + dereference. The smart-pointer
analog of vector_back_havoc — `std::make_unique` calls `operator
new` (havoced), constructs a uint32_t in place (writes go to a
heap allocation the override returns as a fresh pointer), then
the dereference reads from that pointer.

The compositional havoc model can't track "the value you wrote
into the heap cell is the value you read back", so the
equivalence is refuted.

Expected RESULT: DISPROVED (smart-pointer compositional gap).
*/

#include <cstdint>
#include <memory>

uint32_t add_one(uint32_t x) {
    auto p = std::make_unique<uint32_t>(x + 1u);
    return *p;
}
