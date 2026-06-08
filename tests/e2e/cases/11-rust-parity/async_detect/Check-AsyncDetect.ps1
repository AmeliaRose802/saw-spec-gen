<#
.SYNOPSIS
    E2E test: verify that gen-verify-rust auto-detects an async fn and emits
    a mir_verify script targeting the _RNC coroutine resume symbol.
    No extra flag is required — detection is automatic.

.DESCRIPTION
    Pipeline:
      1. Compile add_one_verified.rs (contains `pub async fn add_one`) to
         LLVM bitcode with rustc, then disassemble to .ll.
      2. Run `saw-spec-gen gen-verify-rust` (no --async flag).
      3. Assert the generated verify_rust.saw:
           - uses `mir_verify` (not `llvm_verify`)
           - targets a `_RNC`-prefixed resume symbol
           - contains `BEGIN_PROOF add_one` and `VERIFIED`
      4. Assert verify_rust.meta.json records `"async": true` and
         `"resume_symbol"` pointing to the _RNC symbol.

    Toolchain requirements: rustc + llvm-dis (SAW not required — spec
    generation only).  Same category as the other rust_parity cases.
#>
param()
$ErrorActionPreference = "Stop"

$ScriptRoot = Split-Path -Parent $PSCommandPath
$RepoRoot   = Resolve-Path (Join-Path $ScriptRoot '../../../../..')
$caseDir    = Split-Path -Parent $PSCommandPath

. (Join-Path $RepoRoot 'scripts/discover-tools.ps1')
$specGen = Build-SawSpecGen -RepoRoot $RepoRoot
$tools   = Find-SawSpecGenTools -RepoRoot $RepoRoot
Assert-SawSpecGenTools -Tools $tools -Require @('LlvmDis', 'Rustc')

$rustc      = $tools.Rustc
$llvmDis    = $tools.LlvmDis
$llvmTarget = $tools.LlvmTarget

$rsFile  = Join-Path $caseDir 'add_one_verified.rs'
$cryFile = Join-Path $caseDir 'add_one_spec.cry'
$outDir  = Join-Path $caseDir 'out_async_detect'

if (Test-Path $outDir) { Remove-Item -Recurse -Force $outDir }
New-Item -ItemType Directory -Path $outDir -Force | Out-Null

# ── Step 1: Compile async Rust → LLVM bitcode ────────────────────────────────
Write-Host "Step 1: rustc → bitcode"
$bcFile = Join-Path $outDir 'add_one_verified.bc'
& $rustc `
    --emit=llvm-bc="$bcFile" `
    --crate-type=lib `
    --edition=2021 `
    --target $llvmTarget `
    -C opt-level=0 `
    -C link-dead-code=yes `
    -C symbol-mangling-version=v0 `
    -C overflow-checks=off `
    -C debug-assertions=off `
    -C panic=unwind `
    -C codegen-units=1 `
    -C debuginfo=0 `
    -C lto=off `
    -C embed-bitcode=no `
    -o (Join-Path $outDir 'add_one.out') `
    $rsFile 2>&1 | Write-Host

if (-not (Test-Path $bcFile)) {
    Write-Error "rustc did not produce $bcFile"
    Write-Host "RESULT: DISPROVED"
    exit 1
}

# ── Step 2: Disassemble bitcode → LLVM IR ────────────────────────────────────
Write-Host "Step 2: llvm-dis → .ll"
$llFile = Join-Path $outDir 'add_one_verified.ll'
& $llvmDis $bcFile -o $llFile 2>&1 | Write-Host

if (-not (Test-Path $llFile)) {
    Write-Error "llvm-dis failed to produce $llFile"
    Write-Host "RESULT: DISPROVED"
    exit 1
}

# Verify the IR actually contains a _RNC resume symbol (sanity check on the
# compiler output — if this fails the rustc flags need revisiting).
$irText = Get-Content $llFile -Raw
if ($irText -notmatch '_RNC') {
    Write-Error "LLVM IR does not contain a _RNC coroutine resume symbol.`nCheck that rustc emits async coroutine lowering with these flags."
    Write-Host "RESULT: DISPROVED"
    exit 1
}
Write-Host "  IR contains _RNC resume symbol — async fn lowered correctly."

# ── Step 3: gen-verify-rust (no --async flag) ────────────────────────────────
Write-Host "Step 3: saw-spec-gen gen-verify-rust (auto-detection)"
& $specGen gen-verify-rust `
    --llvm-ir      $llFile `
    --bitcode      $bcFile `
    --cryptol-spec $cryFile `
    --cryptol-fn   add_one_spec `
    --function     add_one `
    --output       $outDir 2>&1 | Write-Host

if ($LASTEXITCODE -ne 0) {
    Write-Error "gen-verify-rust failed (exit=$LASTEXITCODE)"
    Write-Host "RESULT: DISPROVED"
    exit 1
}

# ── Step 4: Assert verify_rust.saw targets the resume symbol ─────────────────
$sawScript = Join-Path $outDir 'verify_rust.saw'
if (-not (Test-Path $sawScript)) {
    Write-Error "verify_rust.saw was not generated"
    Write-Host "RESULT: DISPROVED"
    exit 1
}

$sawText = Get-Content $sawScript -Raw

# Must use mir_verify (async path), not llvm_verify (sync path).
if ($sawText -notmatch 'mir_verify') {
    Write-Error "Expected mir_verify in async SAW script, but got:`n$sawText"
    Write-Host "RESULT: DISPROVED"
    exit 1
}

if ($sawText -match 'llvm_verify') {
    Write-Error "Async SAW script must not contain llvm_verify:`n$sawText"
    Write-Host "RESULT: DISPROVED"
    exit 1
}

# Must target a _RNC resume symbol.
if ($sawText -notmatch '_RNC') {
    Write-Error "mir_verify call does not target a _RNC resume symbol:`n$sawText"
    Write-Host "RESULT: DISPROVED"
    exit 1
}

# Must contain the standard BEGIN_PROOF / VERIFIED proof tokens.
if ($sawText -notmatch 'BEGIN_PROOF add_one') {
    Write-Error "Missing BEGIN_PROOF add_one in generated script:`n$sawText"
    Write-Host "RESULT: DISPROVED"
    exit 1
}

if ($sawText -notmatch 'VERIFIED') {
    Write-Error "Missing VERIFIED token in generated script:`n$sawText"
    Write-Host "RESULT: DISPROVED"
    exit 1
}

Write-Host "  verify_rust.saw uses mir_verify with _RNC resume symbol — correct."

# ── Step 5: Assert meta.json records async=true and resume_symbol ────────────
$metaPath = Join-Path $outDir 'verify_rust.meta.json'
if (-not (Test-Path $metaPath)) {
    Write-Error "verify_rust.meta.json was not generated"
    Write-Host "RESULT: DISPROVED"
    exit 1
}

$meta = Get-Content $metaPath -Raw | ConvertFrom-Json

if ($meta.async -ne $true) {
    Write-Error "Expected async=true in meta.json, got: $($meta.async)"
    Write-Host "RESULT: DISPROVED"
    exit 1
}

if (-not $meta.resume_symbol -or $meta.resume_symbol -notmatch '_RNC') {
    Write-Error "Expected _RNC resume_symbol in meta.json, got: $($meta.resume_symbol)"
    Write-Host "RESULT: DISPROVED"
    exit 1
}

Write-Host "  meta.json: async=true, resume_symbol=$($meta.resume_symbol)"
Write-Host "Auto-detection of async fn verified — no --async flag required."
Write-Host "RESULT: VERIFIED"
