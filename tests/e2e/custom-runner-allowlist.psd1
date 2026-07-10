# Temporary allowlist for custom-script E2E runner exceptions.
#
# Policy: Runner='custom' and Script= entries are banned in cases.psd1.
# Use built-in runners (cpp, rust, equiv) instead. If a capability is
# missing, add runner support rather than wrapping a script here.
#
# This allowlist provides a time-bounded grace period for cases that
# pre-date the ban and require migration effort.
#
# Fields per entry:
#   Script   — the exact Script= value from cases.psd1 (required, used as key).
#   Owner    — GitHub handle responsible for migrating this case.
#   Expires  — ISO date (yyyy-MM-dd) after which CI treats this as a violation.
#   Reason   — Why migration is non-trivial / what is blocked.
#
# When a case is migrated to a built-in runner, remove it from this file.
# To renew an expiry, update Expires and file an issue tracking the new date.
# Never add new entries unless you also file a migration issue.
@{
    # All five entries below share a single migration blocker:
    # the built-in rust runner has no output-inspection mode, so tests that
    # need to assert specific SAW script content (preconditions, return
    # adapters, struct bridges) cannot use it yet.  Once the rust runner
    # gains an AssertOutput / snapshot capability, all five can be migrated
    # and this file can be emptied.
    Exceptions = @(
        @{
            Script  = 'tests/e2e/cases/11-rust-parity/variant_map/Check-VariantMap.ps1'
            Owner   = 'AmeliaRose802'
            Expires = '2027-01-31'
            Reason  = 'Asserts SAW script contains variant membership precondition (x0 == (0:[8]) \/ x0 == (1:[8])); blocked on rust runner output-inspection support.'
        }
        @{
            Script  = 'tests/e2e/cases/11-rust-parity/return_narrowing/Check-ReturnNarrowing.ps1'
            Owner   = 'AmeliaRose802'
            Expires = '2027-01-31'
            Reason  = 'Asserts SAW script contains if/then/else return-discriminant adapter; blocked on rust runner output-inspection support.'
        }
        @{
            Script  = 'tests/e2e/cases/12-aggregate-bridge/packed_tuple_return/Check-PackedTupleReturn.ps1'
            Owner   = 'AmeliaRose802'
            Expires = '2027-01-31'
            Reason  = 'Asserts SAW script uses llvm_struct_value and field accessors .0/.1 for aggregate { i1, i1 } return; blocked on rust runner output-inspection support.'
        }
        @{
            Script  = 'tests/e2e/cases/12-aggregate-bridge/sret_preserved/Check-SretPreserved.ps1'
            Owner   = 'AmeliaRose802'
            Expires = '2027-01-31'
            Reason  = 'Asserts SAW script allocates result_ptr and uses llvm_points_to for sret return; blocked on rust runner output-inspection support.'
        }
        @{
            Script  = 'tests/e2e/cases/12-aggregate-bridge/niche_enum_remap/Check-NicheEnumRemap.ps1'
            Owner   = 'AmeliaRose802'
            Expires = '2027-01-31'
            Reason  = 'Asserts SAW script contains VariantRemap bridge and correct discriminant values for niche-packed enum return; blocked on rust runner output-inspection support.'
        }
    )
}
