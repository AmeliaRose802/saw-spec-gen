# Compiler-derived C++ object layouts

**Status:** Implemented (issue #98). The `object_layout` E2E group has been
validated on Windows/MSVC and Linux/Itanium, including real `KeyStore::provision`
source using `std::mutex`, `std::scoped_lock` and `std::optional`.

`verify-cpp` derives receiver, indirect aggregate argument and hidden return
storage from compiler evidence. It does not require hand-maintained `this`,
argument or sret sizes, field offsets, or a return prefix. Cryptol describes
semantic values; the generated SAW setup uses the actual typed object storage.

This is a sequential implementation proof under the generated preconditions
and explicit callee assumptions, not a proof of concurrency, general C++ object
lifetimes, or compiler correctness.

## 1. Source, compiler and bitcode evidence

The [compilation pipeline](../src/verify_cpp/compile.rs#L53-L159) captures Clang
AST/IRgen record-layout dumps in the same invocation that produces LLVM text,
using the source/ABI flags shared with bitcode compilation. The evidence includes
source record trees, bases, sizes, alignments, member/bit offsets, exact named
LLVM definitions, target triple, `DataLayout`, compiler identity and command.

The [layout planner](../src/object_layout/plan.rs#L1) then:

1. Resolves the source declaration and a unique compiler storage type. Equal
   sizes alone are not a type mapping.
2. Checks Clang `sizeof` against LLVM allocation size using the target's
   `DataLayout`; retains source alignment, including stronger `alignas`
   requirements, rather than assuming host or eight-byte alignment.
3. Matches semantic leaves to LLVM storage, including nested records, arrays,
   bases, supported bitfields and selected union/variant storage.
4. Reads the actual function ABI: pointer, `byval`, indirect-by-value, and the
   exact zero-based hidden sret argument position. A method does not imply that
   sret is always argument one.
5. Validates configuration and [disassembles the bitcode SAW will load](../src/object_layout/bitcode.rs#L1).
   Target properties, the target's ABI signature, allocated aliases and required
   LLVM type definitions must agree with the evidence. Missing `llvm-dis`, a
   placeholder module or a mismatch is an error, not a fallback to supplied text.

Opaque-pointer code generation can omit an otherwise needed named record from
the module. Its exact Clang IRgen type can supply an inline `llvm_struct_type`
or `llvm_packed_struct_type` allocation instead. This enriches layout analysis;
it does not invent an alias or add replacement code to the executable bitcode.

### Cache provenance

The [C++ cache fingerprint](../src/verify_cpp/cache_fingerprint.rs#L1) uses
SHA-256 over the full preprocessed translation unit, ordered flags, target,
source/working-directory identities and compiler identity. Preprocessing covers
transitive and system headers and environment-selected macros/includes. Compiler
identity includes version output and the resolved executable's path, size and
modification time; this is not a hash of the compiler executable itself.

Reloading the [compiler sidecar](../src/object_layout/capture.rs#L1) checks its
schema, triple, data layout and recorded LLVM definitions against current IR.
The subsequent bitcode check binds layout/ABI evidence to the loaded module.
These are consistency/provenance checks, not mathematical proofs of compiler
correctness, instruction-body equality or semantics-preserving transformations.

## 2. Using the existing verification command

No new CLI flag is needed. Use `verify-cpp` with the existing source, Cryptol,
function, language-standard and `--config` options. `preconditions` and layout
settings are configuration-only.

From the repository root, with the verification tools available:

```powershell
$case = 'tests/e2e/cases/15-object-layout/key_store'
$config = if ($IsWindows) { "$case/key_store_windows.toml" } else { "$case/key_store_linux.toml" }
saw-spec-gen verify-cpp `
    --cpp-file "$case/key_store_verified.cpp" `
    --cryptol-spec "$case/key_store_spec.cry" `
    --function provision --cryptol-fn provision_contract `
    --cxx-standard c++17 --config $config
```

The [Windows configuration](../tests/e2e/cases/15-object-layout/key_store/key_store_windows.toml#L1)
uses global contract bindings, before the layout table:

```toml
contract_return = "ret"
contract_ensures = ["this=thisPost"]
# Windows/MSVC only: explicit sequential unlocked-mutex scope.
preconditions = ["this.mu_._Count == 0"]

[layout]
projection = "fields"
modifies = ["this.key_"]
```

The [Linux configuration](../tests/e2e/cases/15-object-layout/key_store/key_store_linux.toml#L1)
has the same bindings and layout settings, but no user `preconditions` entry.
Neither configuration supplies buffer sizes, offsets, alignments or
`sret_assert_bytes`. The compiler-derived mutable receiver makes a separate
`out_buffer_param` extent unnecessary for this contract.

Global `[layout]` defaults can be replaced by
`[functions.<cryptol_fn>.layout]`. A per-function layout table replaces the
global table as a unit; its nested maps are not merged with global maps.

### KeyStore proof subject and ABI

The [verified method](../tests/e2e/cases/15-object-layout/key_store/key_store_verified.cpp#L1)
executes the real scoped-lock and optional code. An occupied store returns
`nullopt`; otherwise it clears the by-value key's `isActive`, stores the key and
returns it. The [Cryptol contract](../tests/e2e/cases/15-object-layout/key_store/key_store_spec.cry#L1)
has type `State -> Key -> { ret : OptionalKey, thisPost : State }`. Both effects
belong to one implementation-function proof, not separate return/state proofs.

The [disproved companion](../tests/e2e/cases/15-object-layout/key_store/key_store_disproved.cpp#L1)
keeps the return correct but increments the stored key's version. Its genuine
postcondition failure demonstrates that `thisPost` is checked.

The inspected Windows artifact (`clang version 20.1.6`,
`x86_64-pc-windows-msvc19.44.35228`) records:

| Region | Size in bytes | Source alignment | LLVM argument index | Lowering |
|---|---:|---:|---:|---|
| `this` | 120 | 8 | 0 | `pointer` |
| `newKey` | 32 | 8 | 2 | `indirect_by_value` |
| `return` | 40 | 8 | 1 | `sret` |

These are observations, not configuration constants. The Linux/Itanium run has
a different receiver layout and sret at index **0**. Read that run's root
`objects.this.layout` for its extent; do not infer it by adding presumed STL
member sizes or copy the Windows layout. By-value argument storage is mutable
callee-owned copy storage, not a readonly buffer imposed on the implementation.

## 3. Contract projections and names

| Projection | Contract view and restrictions |
|---|---|
| `bytes` (default) | Existing byte-oriented contracts remain usable for supported objects. The view has the exact compiler extent, but semantic fields, not padding, supply the postconditions. Pointer-containing objects cannot use byte projection. |
| `fields` | A flat Cryptol record of non-pointer, non-runtime semantic scalar leaves. Normal integers retain LLVM widths; a stored bool is typically `[8]`, not `Bit`. Bitfields use their declared bit widths. |
| `llvm` | Metadata for a validated legacy integer/tuple shape, selected from an explicit shape or supported scalar inference. It is not an unchecked allocation escape hatch; pointer fields, guarded optionals and bitfields cannot use legacy non-byte shapes. |

For fields projection, record keys are **relative to the region**. `field_key`
replaces `.` with `__`, `[` with `_`, and removes `]`:

| Named semantic path | Key in that region's Cryptol record |
|---|---|
| `this.key_.has_value` | `key___has_value` |
| `this.key_.value.id` | `key___value__id` |
| `return.value.isActive` | `value__isActive` |
| `p.arr[1]` | `arr_1` |

The mapping is checked for collisions: for example, `a.b` and `a__b` cannot
silently become the same key. Configuration uses canonical source paths, not
these flattened record keys.

Supported `std::optional` layouts expose `.has_value` and `.value` (with nested
members such as `.value.id`). The engagement flag and payload must be uniquely
identified in the compiler's member tree; offsets are not hard-coded for an STL.
Pointer fields are omitted from the Cryptol record and retained as real LLVM
pointer values with unchanged frames. Runtime mutex storage is likewise absent
from the application record, but remains in the typed allocation and metadata.

## 4. Preconditions and checked configuration

### Representation validity versus user restrictions

Bool storage receives automatic canonical `0`/`1` preconditions. For unscoped
enums **without a fixed underlying type**, compiler-evaluated enumerators determine
the C++ representation range, not just a set of named enumerators. Fixed-underlying
and scoped enums admit their full underlying domain. User restrictions remain
separate; unresolved required enum facts do not justify a guessed restriction.

Named preconditions support `valid(p.inner.enabled)` or
`p.inner.enabled is valid` for compiler-proven **bool** storage. `valid(...)` is
not an enum-membership shorthand. Simple named comparisons, such as
`p.inner.x <= 100` or `this.key_.value.version <= 100`, lower to the appropriate
contract view. A comparison on optional payload is guarded by its engagement
condition, including enclosing optional guards. Complex inactive-payload
expressions that cannot be guarded safely are rejected.

Literal numeric `@` indexing is retained only in byte projection and only when
the selected byte belongs to one semantic field. It emits a warning identifying
the **actual** field. A wrong-but-semantic offset cannot reveal which field the
author intended; prefer names. Padding, out-of-bounds or ambiguous storage,
dynamic indexing, unvalidated slicing/transformation and unknown named leaves
are rejected. Raw indexing does not provide the named optional-guard lowering.

### Assertions do not define layouts

- `[layout.offsets]` maps exact leaves, such as `"p.inner.x"`, to asserted byte
  offsets. Each must equal the compiler offset; aggregate/padding paths are not
  leaf assertions.
- `[layout.alignments]` maps region names, such as `p`, to asserted alignments.
  Each must equal that region's source/compiler alignment.
- Existing buffer shapes and known `alias_size` entries are checked against
  compiler facts. Both undersized **and oversized** object extents are errors.
  An equal total size is insufficient for a legacy typed shape: its semantic
  scalar types, offsets and coverage must also match.

### Selected unions and variants

An ordinary union requires a real named member; it is not selected by size.
For example, the [selected-union configuration](../tests/e2e/cases/15-object-layout/selected_union/increment_left_spec.toml#L1)
uses this entry inside `[layout]`:

```toml
active_members = { "p.value" = "left" }
```

A supported `std::variant` uses a numeric alternative **string**, as in the
[variant configuration](../tests/e2e/cases/15-object-layout/variant/variant_spec.toml#L1):

```toml
active_members = { "p.choice" = "0" }
```

The index must match the compiler's ordered alternative/storage chain. The
variant's canonical `.index` is constrained to the selected alternative in
pre- and post-state; `.value` denotes that alternative. Missing, unknown, unused
or ambiguous selections fail closed. This is explicit active-member scope, not
automatic validity inference. Switching union/variant alternatives is unsupported;
the model does not prove arbitrary lifetime transitions or type-punning legality.

## 5. One typed allocation, guarded payloads and frames

The [emitter](../src/object_layout/emit.rs#L1) allocates the original compiler
type at the compiler alignment. In byte projection, typed pre-state scalars are
derived from the **same byte variable passed to the contract** and installed
with `llvm_points_to_at_type` into that allocation. There is no independent
symbolic byte heap whose values can disagree with execution. Fields projection
similarly connects the record's scalars to the original typed storage.

Optional inputs use `llvm_conditional_points_to_at_type` for payloads: an
inactive payload is not asserted to be a live initialized value. Raw sret
storage is seeded before construction, without imposing input-object validity
on it; returned payload postconditions are conditional on the expected
engagement flag. This supports the checked sequential cases, **not** a general
C++ object-lifetime proof.

`modifies` selects semantic fields or subtrees in caller-owned mutable objects.
For example, `modifies = ["this.key_"]` frames the receiver's other fields.
Framed fields must equal their pre-state values in the final state; if a field
also appears in the model postcondition, that model value must agree with the
frame. By-value copies are not treated as caller-owned mutation regions.

**A frame is a post-state condition, not a write trace.** A transient write
restored before return can satisfy the frame. There is no claim that a framed
location was never written, or that concurrent observers cannot see changes.
Without `modifies`, ordinary application fields are not all implicitly framed;
inspect the emitted `asserted`/`framed` sets. A mutable pointer object without a
semantic post-state contract produces a proof-scope warning.

Named bitfields sharing a backing integer use one typed LLVM word. Masks
constrain the named asserted/framed bits; compiler bit padding may change.
Ordinary compiler padding is also excluded from semantic postconditions.
A real array member, even if named `tail` or `_Pad`, is not automatically padding.

### Explicit abstraction boundaries

- **Pointers:** actual LLVM pointer identity/provenance is preserved, not encoded
  as Cryptol integer bits. Selecting pointer-field mutation or returning a
  pointer-containing object through this layout path requires a provenance-aware
  contract that is not supported; generation fails closed. Preserving a pointer
  does not establish a layout or functional contract for its pointee.
- **Mutexes:** the exact `std::mutex` runtime subobject retains all compiler-typed
  storage, including runtime union/array storage and pointers, with unchanged
  frames in the KeyStore contract. Defined lock/optional wrapper bodies execute;
  low-level runtime calls remain explicit assumptions. The inspected Windows
  boundaries include `_Mtx_lock`, `_Mtx_unlock` and the declare-only throw helper.
  The named `_Count == 0` restriction is a user assumption, not inferred C++
  validity. No OS synchronization or concurrency proof is claimed.
- **Raw buffers:** when a parameter has no complete compiler-derived aggregate
  extent, its manual/inferred storage shape remains a caller-provided allocation
  assumption with a warning, not validated C++ object coverage.
- **Abstract interfaces:** eligible abstract-interface parameters with only
  vptr fields remain on the explicit vtable assumed-contract backend. Their
  `abstract_objects` entries do not claim the complete layout of an unknown
  derived implementation; a complete-object buffer override is not permitted.

## 6. Fail-closed behavior and migration

Generation rejects missing or ambiguous compiler aggregate types, unresolved
semantic storage, conflicting source/LLVM layouts, invalid configuration paths,
unsupported expanded/register-coerced aggregate **arguments**, and incompatible
sidecar/bitcode evidence. Big-endian projections, vector semantic fields and
floating-point semantic fields are unsupported. Opaque mutex alignment storage
is not a floating-point computation contract.

For aggregate/method generation without a compiler layout sidecar, use
`verify-cpp` to capture the required facts. An AST-only or placeholder-bitcode
`gen-verify` run is not an alternative proof path. Generation with valid evidence
can produce a checked plan, but generation alone is still not verification.

`sret_assert_bytes` is now only a checked coverage assertion: it cannot exceed
the compiler extent and must cover **every semantic return field**, including
each field's complete backing range. It can exclude trailing padding, not a
real array tail. Named-field returns need no prefix setting.

In particular, the historical
[partial-sret fixture](../tests/e2e/cases/12-aggregate-bridge/partial_sret/partial_sret_verified.cpp#L1)
has a real `tail[5]` member. Its four-byte prefix omits semantic fields. Both
legacy prefix cases now intentionally expect **REJECTED**, with
`omits semantic field return.tail`, before any `BEGIN_PROOF` marker. Older
comments/recipes describing that tail as safely ignored are not current behavior.

## 7. Artifacts and metadata

The output directory contains:

```text
<module>.layout.json   compiler facts captured beside the LLVM text
layout-plan.json       checked per-target allocation/projection plan
result.json            verification record, with top-level memory_layout
```

The [serialized data structures](../src/object_layout/model.rs#L1) distinguish
the compiler sidecar (`CompilerLayouts`) from the checked `LayoutPlan`:

- The sidecar retains full `records`, `llvm_types` **and** `irgen_types`, plus
  numeric `schema_version = 1`, `target_triple`, `data_layout`, `compiler` and
  `command`.
- The plan adds `abi` (`msvc` or `itanium`), `objects`, `abstract_objects`,
  `warnings`, `validity_constraints`, original `semantic_preconditions` and
  `abstraction_boundaries`. It retains the full record trees and `llvm_types`,
  but does **not** duplicate the sidecar's `irgen_types` map.
- `objects` is a region-keyed map (`BTreeMap`), not an array. It includes
  `return` for sret. Each entry records `projection`, `mutable`, zero-based
  `argument_index`, `lowering`, `configured_shape`, `inferred_shape`, `asserted`,
  `framed` and legacy-shape `selectors`.
- Each object's `layout` records source/LLVM type names, `allocation_type`,
  `size`, `alignment`, `llvm_alignment`, `fields`, `padding`, `bases`,
  `validation` notes and `unresolved` facts. `allocation_type` explains either
  the module alias or the exact inline allocation generated from IRgen evidence.
- Fields carry relative `path`, source/LLVM types, extent, optional bit/array
  metadata, `guard`, `validity`, `is_pointer` and `runtime`. Padding ranges
  include reasons, distinguishing compiler padding from selected inactive
  storage. `abstract_objects` values are layouts, not full `ObjectPlan` entries.

The native C++ result writer embeds the checked plan as `memory_layout` when
available. Its numeric nested schema version is distinct from the outer result
schema string `"1"`. Read [result-json.md](result-json.md) for the result contract.
Layout metadata records proof scope; it is not an additional verdict or evidence
that every runtime dependency was proved.

## 8. Regression coverage

The 19-case `object_layout` group uses the built-in `cpp` runner on both platforms:
nested POD/sret, multiple bases, pointer frames, selected unions, variants, enums,
shared-word bitfields and KeyStore. VERIFIED/DISPROVED pairs exercise real return
and/or state obligations; two selected-union cases exercise pre-proof rejection.
The legacy partial-sret rejection cases remain under `aggregate_bridge`.

```powershell
pwsh tests/e2e/Run-E2ETests.ps1 -Tag object_layout
```

The [E2E documentation](../tests/e2e/README.md#L1) describes `ExpectedError`,
`CxxStandard`, platform configurations, `LayoutRegions` and `ForbiddenOverrides`.
The [compiler integration tests](../tests/object_layout_integration.rs#L1)
separately check compiler facts and generation for both target ABIs; they do not
invoke SAW or replace the manifest's implementation proofs.