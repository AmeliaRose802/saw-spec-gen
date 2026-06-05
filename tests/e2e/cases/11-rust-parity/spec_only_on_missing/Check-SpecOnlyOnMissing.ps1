<#
.SYNOPSIS
    Custom e2e test: verify that gen-verify-rust --spec-only-on-missing
    soft-exits with a result.json status=not_attempted when the target
    function has no matching symbol in the LLVM IR.
#>
param()
$ErrorActionPreference = "Stop"

$ScriptRoot = Split-Path -Parent $PSCommandPath
$RepoRoot   = Resolve-Path (Join-Path $ScriptRoot '../../../../..')
$caseDir    = Split-Path -Parent $PSCommandPath

# ── Build saw-spec-gen ────────────────────────────────────────────────────────
. (Join-Path $RepoRoot 'scripts/discover-tools.ps1')
$specGen = Build-SawSpecGen -RepoRoot $RepoRoot
$tools   = Find-SawSpecGenTools -RepoRoot $RepoRoot
Assert-SawSpecGenTools -Tools $tools -Require @('LlvmDis', 'Rustc')

$rustc     = $tools.Rustc
$llvmDis   = $tools.LlvmDis
$llvmTarget = $tools.LlvmTarget

$rsFile  = Join-Path $caseDir 'dummy.rs'
$cryFile = Join-Path $caseDir 'add_one_spec.cry'
$outDir  = Join-Path $caseDir 'out_spec_only_on_missing'

if (Test-Path $outDir) { Remove-Item -Recurse -Force $outDir }
New-Item -ItemType Directory -Path $outDir -Force | Out-Null

# ── Compile Rust → bitcode ────────────────────────────────────────────────────
$bcFile = Join-Path $outDir 'dummy.bc'
& $rustc --emit=llvm-bc="$bcFile" --crate-type=lib --edition=2021 `
    --target $llvmTarget `
    -C opt-level=0 -C link-dead-code=yes -C symbol-mangling-version=v0 `
    -C overflow-checks=off -C debug-assertions=off -C panic=abort `
    -C codegen-units=1 -C debuginfo=0 -C lto=off -C embed-bitcode=no `
    -o (Join-Path $outDir 'dummy.out') $rsFile 2>&1 | Write-Host

$llFile = Join-Path $outDir 'dummy.ll'
& $llvmDis $bcFile -o $llFile 2>&1 | Write-Host

# ── Call gen-verify-rust with --spec-only-on-missing ──────────────────────────
& $specGen gen-verify-rust `
    --llvm-ir      $llFile `
    --bitcode      $bcFile `
    --cryptol-spec $cryFile `
    --cryptol-fn   nonexistent_fn `
    --function     nonexistent_fn `
    --output       $outDir `
    --spec-only-on-missing 2>&1 | Write-Host

if ($LASTEXITCODE -ne 0) {
    Write-Error "gen-verify-rust failed unexpectedly"
    exit 1
}

# ── Check result.json ─────────────────────────────────────────────────────────
$rjPath = Join-Path $outDir 'result.json'
if (-not (Test-Path $rjPath)) {
    Write-Error "result.json not written"
    Write-Host "RESULT: DISPROVED"
    exit 1
}

$rj = Get-Content $rjPath -Raw | ConvertFrom-Json
if ($rj.status -ne 'not_attempted') {
    Write-Error "Expected status=not_attempted, got: $($rj.status)"
    Write-Host "RESULT: DISPROVED"
    exit 1
}

Write-Host "result.json status=not_attempted as expected"
Write-Host "RESULT: VERIFIED"
