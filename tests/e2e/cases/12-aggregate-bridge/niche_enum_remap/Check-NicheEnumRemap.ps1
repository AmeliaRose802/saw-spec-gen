<#
.SYNOPSIS
    E2E test: verify that gen-verify-rust composes --variant-map with
    the VariantRemap bridge for niche-packed enum returns.
    The Cryptol spec returns [8] with 3 variants; Rust returns u8
    with only 2 reachable variants. The VariantRemap bridge emits
    the discriminant remap in the SAW postcondition.
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

$rsFile  = Join-Path $caseDir 'activate_verified.rs'
$cryFile = Join-Path $caseDir 'activate_spec.cry'
$outDir  = Join-Path $caseDir 'out_niche_enum'

if (Test-Path $outDir) { Remove-Item -Recurse -Force $outDir }
New-Item -ItemType Directory -Path $outDir -Force | Out-Null

# ── Compile Rust → bitcode ───────────────────────────────────────────
$bcFile = Join-Path $outDir 'activate_verified.bc'
& $rustc --emit=llvm-bc="$bcFile" --crate-type=lib --edition=2021 `
    --target $llvmTarget `
    -C opt-level=0 -C link-dead-code=yes -C symbol-mangling-version=v0 `
    -C overflow-checks=off -C debug-assertions=off -C panic=unwind `
    -C codegen-units=1 -C debuginfo=0 -C lto=off -C embed-bitcode=no `
    -o (Join-Path $outDir 'activate.out') $rsFile 2>&1 | Write-Host

$llFile = Join-Path $outDir 'activate_verified.ll'
& $llvmDis $bcFile -o $llFile 2>&1 | Write-Host

# ── Call gen-verify-rust with --variant-map on both param and return ──
& $specGen gen-verify-rust `
    --llvm-ir      $llFile `
    --bitcode      $bcFile `
    --cryptol-spec $cryFile `
    --cryptol-fn   activate_spec `
    --function     activate `
    --output       $outDir `
    --variant-map  'return=Success:0,AlreadyActive:1' `
    --variant-map  'x0=Success:1,AlreadyActive:2' 2>&1 | Write-Host

if ($LASTEXITCODE -ne 0) {
    Write-Error "gen-verify-rust with variant-map failed"
    Write-Host "RESULT: DISPROVED"
    exit 1
}

# ── Verify generated SAW script contains VariantRemap adapter ────────
$sawScript = Join-Path $outDir 'verify_rust.saw'
if (-not (Test-Path $sawScript)) {
    Write-Error "verify_rust.saw not generated"
    Write-Host "RESULT: DISPROVED"
    exit 1
}

$sawText = Get-Content $sawScript -Raw

# Check for the VariantRemap bridge in the return assertion
if ($sawText -notmatch 'if.*activate_spec') {
    Write-Error "Missing VariantRemap return adapter in SAW script"
    Write-Host "SAW script content:"
    Write-Host $sawText
    Write-Host "RESULT: DISPROVED"
    exit 1
}

# Check for param membership precondition
if ($sawText -notmatch 'x0 == \(1 : \[32\]\).*\\\/.*x0 == \(2 : \[32\]\)') {
    Write-Error "Missing variant-map membership precondition for x0"
    Write-Host "SAW script content:"
    Write-Host $sawText
    Write-Host "RESULT: DISPROVED"
    exit 1
}

# Check for return discriminant values
if ($sawText -notmatch 'then \(0 : \[8\]\)') {
    Write-Error "Missing return discriminant 0 (Success)"
    Write-Host "RESULT: DISPROVED"
    exit 1
}

if ($sawText -notmatch 'else \(1 : \[8\]\)') {
    Write-Error "Missing return discriminant 1 (AlreadyActive)"
    Write-Host "RESULT: DISPROVED"
    exit 1
}

Write-Host "VariantRemap + variant-map composition found in SAW script"
Write-Host "RESULT: VERIFIED"
