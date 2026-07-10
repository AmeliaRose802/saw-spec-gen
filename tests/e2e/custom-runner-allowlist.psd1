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
    Exceptions = @(
        @{
            Script  = 'tests/e2e/cases/10-proof-markers/Check-ProofMarkers.ps1'
            Owner   = 'AmeliaRose802'
            Expires = '2027-01-31'
            Reason  = 'Toolchain-free log-parser sanity check; needs a dedicated test harness mode before it can use a built-in runner.'
        }
        @{
            Script  = 'tests/e2e/cases/11-rust-parity/spec_only_on_missing/Check-SpecOnlyOnMissing.ps1'
            Owner   = 'AmeliaRose802'
            Expires = '2027-01-31'
            Reason  = 'Validates gen-verify-rust soft-exit on missing symbol; needs rust runner support for non-SAW CLI surface tests.'
        }
        @{
            Script  = 'tests/e2e/cases/11-rust-parity/variant_map/Check-VariantMap.ps1'
            Owner   = 'AmeliaRose802'
            Expires = '2027-01-31'
            Reason  = 'Checks generated SAW script content for variant-map preconditions; needs output-inspection support in the rust runner.'
        }
        @{
            Script  = 'tests/e2e/cases/11-rust-parity/return_narrowing/Check-ReturnNarrowing.ps1'
            Owner   = 'AmeliaRose802'
            Expires = '2027-01-31'
            Reason  = 'Checks generated SAW script for if/then/else return adapter; needs output-inspection support in the rust runner.'
        }
        @{
            Script  = 'tests/e2e/cases/11-rust-parity/unified_gen_verify/Check-UnifiedGenVerify.ps1'
            Owner   = 'AmeliaRose802'
            Expires = '2027-01-31'
            Reason  = 'Validates gen-verify --lang rust output parity with gen-verify-rust; needs rust runner CLI-surface testing mode.'
        }
        @{
            Script  = 'tests/e2e/cases/12-aggregate-bridge/packed_tuple_return/Check-PackedTupleReturn.ps1'
            Owner   = 'AmeliaRose802'
            Expires = '2027-01-31'
            Reason  = 'Inspects generated SAW script for StructValue bridge; needs output-inspection support in the cpp runner.'
        }
        @{
            Script  = 'tests/e2e/cases/12-aggregate-bridge/sret_preserved/Check-SretPreserved.ps1'
            Owner   = 'AmeliaRose802'
            Expires = '2027-01-31'
            Reason  = 'Checks generated SAW script for sret byte-buffer allocation; needs output-inspection support in the cpp runner.'
        }
        @{
            Script  = 'tests/e2e/cases/12-aggregate-bridge/niche_enum_remap/Check-NicheEnumRemap.ps1'
            Owner   = 'AmeliaRose802'
            Expires = '2027-01-31'
            Reason  = 'Checks generated SAW script for VariantRemap bridge; needs output-inspection support in the cpp runner.'
        }
    )
}
