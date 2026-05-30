<#
.SYNOPSIS
    Batch SAW verification driver. Consumes a JSON spec manifest (from
    `pretty-specs --emit-function-list`) and dispatches one verify run
    per Cryptol function, writing all `result.json` files under a
    single output tree that `saw-spec-gen collect-results` can scan.

.DESCRIPTION
    Pipeline:
      1. Parse `-SpecManifest` (JSON array of `{name, signature, arity}`).
      2. For each entry, search `-CppDir` and/or `-RustDir` for an
         implementation file matching the function name.
      3. Dispatch `verify.ps1` (C++) and/or `verify-rust.ps1` (Rust)
         with `-OutputDir <Output>/<lang>/<name>`.
      4. Log per-function status (RAN / SKIPPED_NO_IMPL / FAILED) and
         exit zero unless `-StrictMissing` was passed.

    Implementation discovery rules (case-sensitive):
      C++: prefer  <name>_verified.cpp  → fall back to <name>.cpp
      Rust: prefer <name>_verified.rs   → fall back to <name>.rs
    When neither candidate exists, the function is logged as
    SKIPPED_NO_IMPL but the run continues (per bd-gcd acceptance:
    "Functions with no implementation get logged but don't fail").

    The output tree is shaped for `collect-results`:
      <Output>/
        cpp/<name>/result.json   ← verify.ps1 writes here
        rust/<name>/result.json  ← verify-rust.ps1 writes here
        verify-all-summary.json  ← this script writes the per-fn log

.PARAMETER SpecManifest
    JSON file emitted by `pretty-specs --emit-function-list`. Each
    element must contain at least a `name` field. `signature` and
    `arity` are accepted but not currently consumed by this driver.

.PARAMETER CryptolSpec
    Path to the `.cry` file holding the Cryptol functions referenced
    by the manifest.

.PARAMETER CppDir
    Directory to search for C++ implementations. Omit to skip the C++
    side entirely.

.PARAMETER RustDir
    Directory to search for Rust implementations. Omit to skip the
    Rust side entirely.

.PARAMETER Output
    Root output directory. Created if missing. Subdirectories
    `cpp/<name>` and `rust/<name>` are populated per function.

.PARAMETER CryptolFnSuffix
    Suffix appended to each manifest `name` to form the Cryptol
    function name. Default `_spec` matches the demos convention
    (`add_one_spec` for impl `add_one`). Pass `''` to use the bare
    name.

.PARAMETER StrictMissing
    When set, exit non-zero if any manifest function has neither a
    C++ nor a Rust implementation. Default off — missing impls are
    logged but tolerated.

.EXAMPLE
    # Full pipeline:
    pretty-specs SDEP.cry --emit-function-list > fns.json
    ./verify-all.ps1 -SpecManifest fns.json `
                     -CryptolSpec SDEP.cry `
                     -CppDir impl/cpp -RustDir impl/rust `
                     -Output runs/
    ./target/release/saw-spec-gen collect-results `
        --root runs/ --output proof_manifest.json
#>

param(
    [Parameter(Mandatory)][string]$SpecManifest,
    [Parameter(Mandatory)][string]$CryptolSpec,
    [string]$CppDir,
    [string]$RustDir,
    [Parameter(Mandatory)][string]$Output,
    [string]$CryptolFnSuffix = '_spec',
    [switch]$StrictMissing
)

$ErrorActionPreference = 'Stop'

if (-not $CppDir -and -not $RustDir) {
    throw "verify-all: at least one of -CppDir or -RustDir is required."
}

# ── Resolve paths ─────────────────────────────────────────────────────────────
$SpecManifest = Resolve-Path $SpecManifest
$CryptolSpec  = Resolve-Path $CryptolSpec
$ScriptRoot   = $PSScriptRoot

if (-not (Test-Path $Output)) {
    New-Item -ItemType Directory -Path $Output -Force | Out-Null
}
$Output = Resolve-Path $Output

if ($CppDir)  { $CppDir  = Resolve-Path $CppDir }
if ($RustDir) { $RustDir = Resolve-Path $RustDir }

$verifyCpp  = Join-Path $ScriptRoot 'verify.ps1'
$verifyRust = Join-Path $ScriptRoot 'verify-rust.ps1'

if ($CppDir  -and -not (Test-Path $verifyCpp))  { throw "verify.ps1 not found at $verifyCpp" }
if ($RustDir -and -not (Test-Path $verifyRust)) { throw "verify-rust.ps1 not found at $verifyRust" }

# ── Parse manifest ────────────────────────────────────────────────────────────
$manifestRaw = Get-Content $SpecManifest -Raw
try {
    $manifest = $manifestRaw | ConvertFrom-Json
} catch {
    throw "Failed to parse $SpecManifest as JSON: $_"
}

# pretty-specs --emit-function-list emits a top-level array, but tolerate
# `{ "functions": [...] }` for forward compatibility.
if ($manifest -is [System.Management.Automation.PSCustomObject] -and $manifest.PSObject.Properties['functions']) {
    $manifest = $manifest.functions
}
if (-not ($manifest -is [System.Array] -or $manifest -is [System.Collections.IEnumerable])) {
    throw "Spec manifest must be a JSON array (or { functions: [...] }); got $($manifest.GetType().FullName)"
}

Write-Host "verify-all: loaded $($manifest.Count) function(s) from manifest"
Write-Host "  spec      : $CryptolSpec"
if ($CppDir)  { Write-Host "  cpp dir   : $CppDir" }
if ($RustDir) { Write-Host "  rust dir  : $RustDir" }
Write-Host "  output    : $Output"

# ── Helpers ───────────────────────────────────────────────────────────────────
function Find-Impl {
    param([string]$Dir, [string]$Name, [string]$Ext)
    # Prefer the explicit `<name>_verified.<ext>` convention used by
    # the demos under tests/e2e/cases. Fall back to plain `<name>.<ext>`.
    foreach ($candidate in @("${Name}_verified.${Ext}", "${Name}.${Ext}")) {
        $p = Join-Path $Dir $candidate
        if (Test-Path $p) { return (Resolve-Path $p).Path }
    }
    # Last-ditch: any file containing the function name in its stem.
    $matches = Get-ChildItem -Path $Dir -Filter "*$Name*.$Ext" -ErrorAction SilentlyContinue
    if ($matches) { return $matches[0].FullName }
    return $null
}

function Invoke-VerifyOne {
    param(
        [string]$Script,
        [hashtable]$ParamMap,
        [string]$Lang,
        [string]$Name
    )
    Write-Host ""
    Write-Host "─── $Lang : $Name ───────────────────────────────────────────────────────"
    try {
        & $Script @ParamMap
        return [pscustomobject]@{ status = 'RAN'; reason = $null }
    } catch {
        $msg = $_.Exception.Message
        Write-Warning "$Lang verify of $Name failed: $msg"
        return [pscustomobject]@{ status = 'FAILED'; reason = $msg }
    }
}

# ── Main loop ─────────────────────────────────────────────────────────────────
$summary = [System.Collections.ArrayList]::new()
$missingCount = 0

foreach ($entry in $manifest) {
    if (-not $entry.name) {
        Write-Warning "verify-all: skipping manifest entry with no 'name' field"
        continue
    }
    $name      = [string]$entry.name
    $cryptolFn = "${name}${CryptolFnSuffix}"

    $row = [ordered]@{
        name       = $name
        cryptol_fn = $cryptolFn
        cpp        = $null
        rust       = $null
    }

    $sawImpl = $false

    if ($CppDir) {
        $cppFile = Find-Impl -Dir $CppDir -Name $name -Ext 'cpp'
        if ($cppFile) {
            $sawImpl = $true
            $outDir = Join-Path $Output (Join-Path 'cpp' $name)
            $res = Invoke-VerifyOne -Script $verifyCpp -Lang 'cpp' -Name $name -ParamMap @{
                CppFile     = $cppFile
                CryptolSpec = $CryptolSpec
                CryptolFn   = $cryptolFn
                Function    = $name
                OutputDir   = $outDir
            }
            $row.cpp = @{
                impl   = $cppFile
                status = $res.status
                reason = $res.reason
            }
        } else {
            $row.cpp = @{ status = 'SKIPPED_NO_IMPL'; reason = "no <name>_verified.cpp or <name>.cpp under $CppDir" }
        }
    }

    if ($RustDir) {
        $rustFile = Find-Impl -Dir $RustDir -Name $name -Ext 'rs'
        if ($rustFile) {
            $sawImpl = $true
            $outDir = Join-Path $Output (Join-Path 'rust' $name)
            $res = Invoke-VerifyOne -Script $verifyRust -Lang 'rust' -Name $name -ParamMap @{
                RustFile    = $rustFile
                CryptolSpec = $CryptolSpec
                CryptolFn   = $cryptolFn
                Function    = $name
                OutputDir   = $outDir
            }
            $row.rust = @{
                impl   = $rustFile
                status = $res.status
                reason = $res.reason
            }
        } else {
            $row.rust = @{ status = 'SKIPPED_NO_IMPL'; reason = "no <name>_verified.rs or <name>.rs under $RustDir" }
        }
    }

    if (-not $sawImpl) {
        Write-Warning "verify-all: no implementation found for '$name' in any provided dir"
        $missingCount++
    }

    [void]$summary.Add($row)
}

# ── Summary ───────────────────────────────────────────────────────────────────
$summaryPath = Join-Path $Output 'verify-all-summary.json'
$summaryDoc = [ordered]@{
    schema_version = '1'
    generator      = 'saw-spec-gen verify-all'
    spec_manifest  = $SpecManifest.ToString()
    cryptol_spec   = $CryptolSpec.ToString()
    entries        = $summary
}
$summaryDoc | ConvertTo-Json -Depth 8 | Set-Content -Path $summaryPath -Encoding UTF8

Write-Host ""
Write-Host "═══════════════════════════════════════════════════════════════"
Write-Host "verify-all: wrote per-function summary to $summaryPath"
Write-Host "  functions: $($summary.Count)"
Write-Host "  missing  : $missingCount"

if ($StrictMissing -and $missingCount -gt 0) {
    Write-Error "verify-all: $missingCount function(s) had no implementation (-StrictMissing was set)"
    exit 2
}

exit 0
