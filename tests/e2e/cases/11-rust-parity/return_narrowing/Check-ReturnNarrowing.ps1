<#
.SYNOPSIS
    Custom e2e test: verify that a config `variant_map = ["return=..."]`
    emits a
    two-variant if/then/else narrowing adapter in the generated SAW
    script. When a Cryptol spec returns Bit (True/False) but the Rust
    impl returns u8 (discriminant 0/1), the adapter bridges them.
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

$rsFile  = Join-Path $caseDir 'check_positive_verified.rs'
$cryFile = Join-Path $caseDir 'check_positive_spec.cry'
$outDir  = Join-Path $caseDir 'out_return_narrowing'

if (Test-Path $outDir) { Remove-Item -Recurse -Force $outDir }
New-Item -ItemType Directory -Path $outDir -Force | Out-Null

# ── Compile Rust → bitcode ────────────────────────────────────────────────────
$bcFile = Join-Path $outDir 'check_positive_verified.bc'
& $rustc --emit=llvm-bc="$bcFile" --crate-type=lib --edition=2021 `
    --target $llvmTarget `
    -C opt-level=0 -C link-dead-code=yes -C symbol-mangling-version=v0 `
    -C overflow-checks=off -C debug-assertions=off -C panic=unwind `
    -C codegen-units=1 -C debuginfo=0 -C lto=off -C embed-bitcode=no `
    -o (Join-Path $outDir 'check_positive.out') $rsFile 2>&1 | Write-Host

$llFile = Join-Path $outDir 'check_positive_verified.ll'
& $llvmDis $bcFile -o $llFile 2>&1 | Write-Host

# ── Call gen-verify-rust; return variant map comes from config ────────────────
& $specGen gen-verify-rust `
    --llvm-ir      $llFile `
    --bitcode      $bcFile `
    --cryptol-spec $cryFile `
    --cryptol-fn   check_positive_spec `
    --function     check_positive `
    --output       $outDir 2>&1 | Write-Host

if ($LASTEXITCODE -ne 0) {
    Write-Error "gen-verify-rust with return variant map failed"
    Write-Host "RESULT: DISPROVED"
    exit 1
}

# ── Check the generated SAW script contains if/then/else adapter ─────────────
$sawScript = Join-Path $outDir 'verify_rust.saw'
if (-not (Test-Path $sawScript)) {
    Write-Error "verify_rust.saw not generated"
    Write-Host "RESULT: DISPROVED"
    exit 1
}

$sawText = Get-Content $sawScript -Raw
if ($sawText -notmatch 'if.*check_positive_spec') {
    Write-Error "Missing VariantRemap return adapter in SAW script"
    Write-Host "SAW script content:"
    Write-Host $sawText
    Write-Host "RESULT: DISPROVED"
    exit 1
}

if ($sawText -notmatch 'then \(0 : \[8\]\)') {
    Write-Error "Missing 'then (0 : [8])' in return adapter"
    Write-Host "RESULT: DISPROVED"
    exit 1
}

if ($sawText -notmatch 'else \(1 : \[8\]\)') {
    Write-Error "Missing 'else (1 : [8])' in return adapter"
    Write-Host "RESULT: DISPROVED"
    exit 1
}

Write-Host "Return narrowing adapter found in SAW script"
Write-Host "RESULT: VERIFIED"
