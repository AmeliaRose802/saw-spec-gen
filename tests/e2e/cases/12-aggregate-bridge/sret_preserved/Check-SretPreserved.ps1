<#
.SYNOPSIS
    E2E test: verify that gen-verify-rust handles sret struct returns
    correctly, emitting llvm_points_to result_ptr for the sret buffer.
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

$rsFile  = Join-Path $caseDir 'get_status_verified.rs'
$cryFile = Join-Path $caseDir 'get_status_spec.cry'
$outDir  = Join-Path $caseDir 'out_sret_preserved'

if (Test-Path $outDir) { Remove-Item -Recurse -Force $outDir }
New-Item -ItemType Directory -Path $outDir -Force | Out-Null

# ── Compile Rust → bitcode ───────────────────────────────────────────
$bcFile = Join-Path $outDir 'get_status_verified.bc'
& $rustc --emit=llvm-bc="$bcFile" --crate-type=lib --edition=2021 `
    --target $llvmTarget `
    -C opt-level=0 -C link-dead-code=yes -C symbol-mangling-version=v0 `
    -C overflow-checks=off -C debug-assertions=off -C panic=unwind `
    -C codegen-units=1 -C debuginfo=0 -C lto=off -C embed-bitcode=no `
    -o (Join-Path $outDir 'get_status.out') $rsFile 2>&1 | Write-Host

$llFile = Join-Path $outDir 'get_status_verified.ll'
& $llvmDis $bcFile -o $llFile 2>&1 | Write-Host

# ── Call gen-verify-rust ─────────────────────────────────────────────
& $specGen gen-verify-rust `
    --llvm-ir      $llFile `
    --bitcode      $bcFile `
    --cryptol-spec $cryFile `
    --cryptol-fn   get_status_spec `
    --function     get_status `
    --output       $outDir 2>&1 | Write-Host

if ($LASTEXITCODE -ne 0) {
    Write-Error "gen-verify-rust failed"
    Write-Host "RESULT: DISPROVED"
    exit 1
}

# ── Verify generated SAW script uses sret pattern ────────────────────
$sawScript = Join-Path $outDir 'verify_rust.saw'
if (-not (Test-Path $sawScript)) {
    Write-Error "verify_rust.saw not generated"
    Write-Host "RESULT: DISPROVED"
    exit 1
}

$sawText = Get-Content $sawScript -Raw

# sret: must allocate result_ptr
if ($sawText -notmatch 'result_ptr.*llvm_alloc') {
    Write-Error "Missing result_ptr allocation for sret"
    Write-Host "SAW script content:"
    Write-Host $sawText
    Write-Host "RESULT: DISPROVED"
    exit 1
}

# sret: postcondition must use llvm_points_to result_ptr
if ($sawText -notmatch 'llvm_points_to result_ptr') {
    Write-Error "Missing llvm_points_to result_ptr for sret postcondition"
    Write-Host "SAW script content:"
    Write-Host $sawText
    Write-Host "RESULT: DISPROVED"
    exit 1
}

Write-Host "sret struct bridge found in generated SAW script"
Write-Host "RESULT: VERIFIED"
