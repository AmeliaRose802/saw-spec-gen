<#
.SYNOPSIS
    Custom e2e test: verify that --variant-map emits the correct
    membership precondition in the generated SAW script.

    The Rust fn is_success(u8) -> u8 works for any u8 input, but
    we restrict verification to status ∈ {0, 1} via --variant-map.
    The generated script must contain the membership precondition.
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

$rsFile  = Join-Path $caseDir 'is_success_verified.rs'
$cryFile = Join-Path $caseDir 'is_success_spec.cry'
$outDir  = Join-Path $caseDir 'out_variant_map'

if (Test-Path $outDir) { Remove-Item -Recurse -Force $outDir }
New-Item -ItemType Directory -Path $outDir -Force | Out-Null

# ── Compile Rust → bitcode ────────────────────────────────────────────────────
$bcFile = Join-Path $outDir 'is_success_verified.bc'
& $rustc --emit=llvm-bc="$bcFile" --crate-type=lib --edition=2021 `
    --target $llvmTarget `
    -C opt-level=0 -C link-dead-code=yes -C symbol-mangling-version=v0 `
    -C overflow-checks=off -C debug-assertions=off -C panic=abort `
    -C codegen-units=1 -C debuginfo=0 -C lto=off -C embed-bitcode=no `
    -o (Join-Path $outDir 'is_success.out') $rsFile 2>&1 | Write-Host

$llFile = Join-Path $outDir 'is_success_verified.ll'
& $llvmDis $bcFile -o $llFile 2>&1 | Write-Host

# ── Call gen-verify-rust with --variant-map ────────────────────────────────────
& $specGen gen-verify-rust `
    --llvm-ir      $llFile `
    --bitcode      $bcFile `
    --cryptol-spec $cryFile `
    --cryptol-fn   is_success_spec `
    --function     is_success `
    --output       $outDir `
    --variant-map  'x0=Success:0,Failure:1' 2>&1 | Write-Host

if ($LASTEXITCODE -ne 0) {
    Write-Error "gen-verify-rust with --variant-map failed"
    Write-Host "RESULT: DISPROVED"
    exit 1
}

# ── Check the generated SAW script contains membership precondition ──────────
$sawScript = Join-Path $outDir 'verify_rust.saw'
if (-not (Test-Path $sawScript)) {
    Write-Error "verify_rust.saw not generated"
    Write-Host "RESULT: DISPROVED"
    exit 1
}

$sawText = Get-Content $sawScript -Raw
if ($sawText -notmatch 'x0 == \(0 : \[8\]\) \\\/ x0 == \(1 : \[8\]\)') {
    Write-Error "Missing variant membership precondition in SAW script"
    Write-Host "SAW script content:"
    Write-Host $sawText
    Write-Host "RESULT: DISPROVED"
    exit 1
}

Write-Host "Variant membership precondition found in SAW script"
Write-Host "RESULT: VERIFIED"
