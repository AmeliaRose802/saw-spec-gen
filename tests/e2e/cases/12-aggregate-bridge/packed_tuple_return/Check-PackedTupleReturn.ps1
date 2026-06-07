<#
.SYNOPSIS
    E2E test: verify that gen-verify-rust emits llvm_struct_value for
    aggregate { i1, i1 } returns, bridging a Cryptol (Bit, Bit) tuple.
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

$rsFile  = Join-Path $caseDir 'enforce_access_verified.rs'
$cryFile = Join-Path $caseDir 'enforce_access_spec.cry'
$outDir  = Join-Path $caseDir 'out_packed_tuple'

if (Test-Path $outDir) { Remove-Item -Recurse -Force $outDir }
New-Item -ItemType Directory -Path $outDir -Force | Out-Null

# ── Compile Rust → bitcode ───────────────────────────────────────────
$bcFile = Join-Path $outDir 'enforce_access_verified.bc'
& $rustc --emit=llvm-bc="$bcFile" --crate-type=lib --edition=2021 `
    --target $llvmTarget `
    -C opt-level=0 -C link-dead-code=yes -C symbol-mangling-version=v0 `
    -C overflow-checks=off -C debug-assertions=off -C panic=unwind `
    -C codegen-units=1 -C debuginfo=0 -C lto=off -C embed-bitcode=no `
    -o (Join-Path $outDir 'enforce_access.out') $rsFile 2>&1 | Write-Host

$llFile = Join-Path $outDir 'enforce_access_verified.ll'
& $llvmDis $bcFile -o $llFile 2>&1 | Write-Host

# ── Call gen-verify-rust ─────────────────────────────────────────────
& $specGen gen-verify-rust `
    --llvm-ir      $llFile `
    --bitcode      $bcFile `
    --cryptol-spec $cryFile `
    --cryptol-fn   enforce_access_spec `
    --function     enforce_access `
    --output       $outDir 2>&1 | Write-Host

if ($LASTEXITCODE -ne 0) {
    Write-Error "gen-verify-rust failed"
    Write-Host "RESULT: DISPROVED"
    exit 1
}

# ── Verify generated SAW script uses llvm_struct_value ───────────────
$sawScript = Join-Path $outDir 'verify_rust.saw'
if (-not (Test-Path $sawScript)) {
    Write-Error "verify_rust.saw not generated"
    Write-Host "RESULT: DISPROVED"
    exit 1
}

$sawText = Get-Content $sawScript -Raw

if ($sawText -notmatch 'llvm_struct_value') {
    Write-Error "Missing llvm_struct_value for aggregate return"
    Write-Host "SAW script content:"
    Write-Host $sawText
    Write-Host "RESULT: DISPROVED"
    exit 1
}

# Check that field accessors .0 and .1 are present
if ($sawText -notmatch '\.0' -or $sawText -notmatch '\.1') {
    Write-Error "Missing field accessor .0 or .1 in aggregate return"
    Write-Host "RESULT: DISPROVED"
    exit 1
}

Write-Host "StructValue bridge found in generated SAW script"
Write-Host "RESULT: VERIFIED"
