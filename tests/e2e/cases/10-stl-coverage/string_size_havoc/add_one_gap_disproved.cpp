/*
DEMO: std::string::resize + size() — the canonical "smart
container" workflow for the std::string family. Allocates a
heap buffer of length (x + 1) and returns that length back to
the caller.

This is intentionally on the gap side: the `jpp` STL override
registry (`src/emit/saw_emit/stl_overrides.rs`) matches both
`St12basic_string` and `St7__cxx1112basic_string`, so every
member function on `std::string` — the (n, char) ctor, resize,
size(), and the destructor — gets a fresh havoc override. The
auto-generated specs cannot couple the length we wrote in via
`resize(x + 1)` with the length we read back via `size()`, so
the equivalence is refuted.

Expected RESULT: DISPROVED (string compositional gap). When the
tool ships functional std::string overrides (or the container-
layout catalog from saw_spec_gen-26d learns to lower
`{ data_ptr, size, capacity }` triples into coupled `points_to`
edits), rename to add_one_verified.cpp and flip to VERIFIED.
*/

#include <cstdint>
#include <string>

uint32_t add_one(uint32_t x) {
    std::string s;
    s.resize(x + 1u);
    return static_cast<uint32_t>(s.size());
}
