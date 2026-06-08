<#
.SYNOPSIS
    Custom e2e test: verify that `gen-verify --lang rust` produces the
    same SAW script as `gen-verify-rust`, proving the unified command
    dispatches correctly to the Rust backend.
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

# ── Compile Rust → bitcode ────────────────────────────────────────────────────
$outDir1 = Join-Path $caseDir 'out_unified'
$outDir2 = Join-Path $caseDir 'out_legacy'
foreach ($d in @($outDir1, $outDir2)) {
    if (Test-Path $d) { Remove-Item -Recurse -Force $d }
    New-Item -ItemType Directory -Path $d -Force | Out-Null
}

$bcFile = Join-Path $outDir1 'add_one_verified.bc'
& $rustc --emit=llvm-bc="$bcFile" --crate-type=lib --edition=2021 `
    --target $llvmTarget `
    -C opt-level=0 -C link-dead-code=yes -C symbol-mangling-version=v0 `
    -C overflow-checks=off -C debug-assertions=off -C panic=unwind `
    -C codegen-units=1 -C debuginfo=0 -C lto=off -C embed-bitcode=no `
    -o (Join-Path $outDir1 'add_one.out') $rsFile 2>&1 | Write-Host

$llFile = Join-Path $outDir1 'add_one_verified.ll'
& $llvmDis $bcFile -o $llFile 2>&1 | Write-Host

# Copy bc/ll to the legacy dir so both runs use the same inputs
Copy-Item -Force $bcFile (Join-Path $outDir2 'add_one_verified.bc')
Copy-Item -Force $llFile (Join-Path $outDir2 'add_one_verified.ll')

# ── gen-verify --lang rust (unified path) ─────────────────────────────────────
Write-Host "Testing: gen-verify --lang rust"
& $specGen gen-verify `
    --lang         rust `
    --llvm-ir      $llFile `
    --bitcode      $bcFile `
    --cryptol-spec $cryFile `
    --cryptol-fn   add_one_spec `
    --function     add_one `
    --output       $outDir1 2>&1 | Write-Host
if ($LASTEXITCODE -ne 0) {
    Write-Error "gen-verify --lang rust failed"
    Write-Host "RESULT: DISPROVED"
    exit 1
}

# ── gen-verify-rust (legacy path) ─────────────────────────────────────────────
Write-Host "Testing: gen-verify-rust (legacy)"
$bcFile2 = Join-Path $outDir2 'add_one_verified.bc'
$llFile2 = Join-Path $outDir2 'add_one_verified.ll'
& $specGen gen-verify-rust `
    --llvm-ir      $llFile2 `
    --bitcode      $bcFile2 `
    --cryptol-spec $cryFile `
    --cryptol-fn   add_one_spec `
    --function     add_one `
    --output       $outDir2 2>&1 | Write-Host
if ($LASTEXITCODE -ne 0) {
    Write-Error "gen-verify-rust failed"
    Write-Host "RESULT: DISPROVED"
    exit 1
}

# ── Compare outputs ──────────────────────────────────────────────────────────
$saw1 = Get-Content (Join-Path $outDir1 'verify_rust.saw') -Raw
$saw2 = Get-Content (Join-Path $outDir2 'verify_rust.saw') -Raw

if ($saw1 -ne $saw2) {
    Write-Error "gen-verify --lang rust and gen-verify-rust produced different SAW scripts"
    Write-Host "=== Unified ==="
    Write-Host $saw1
    Write-Host "=== Legacy ==="
    Write-Host $saw2
    Write-Host "RESULT: DISPROVED"
    exit 1
}

# ── Aggregate inventory sidecar + snapshot check ───────────────────────────────
$inventoryPath = Join-Path $outDir1 'implementation_inventory.json'
& $specGen aggregate-inventory $outDir1 --output $inventoryPath 2>&1 | Write-Host
if ($LASTEXITCODE -ne 0) {
    Write-Error "aggregate-inventory failed"
    Write-Host "RESULT: DISPROVED"
    exit 1
}

if (-not (Test-Path $inventoryPath)) {
    Write-Error "aggregate-inventory did not emit $inventoryPath"
    Write-Host "RESULT: DISPROVED"
    exit 1
}

$inventory = Get-Content $inventoryPath -Raw | ConvertFrom-Json
if (-not $inventory.functions -or $inventory.functions.Count -ne 1) {
    Write-Error "expected 1 inventory function entry; got $($inventory.functions.Count)"
    Write-Host "RESULT: DISPROVED"
    exit 1
}
if (-not $inventory.functions[0].symbol) {
    Write-Error "inventory symbol is missing/empty"
    Write-Host "RESULT: DISPROVED"
    exit 1
}

$normalized = [ordered]@{
    functions = @(
        [ordered]@{
            name   = [string]$inventory.functions[0].name
            lang   = [string]$inventory.functions[0].lang
            symbol = '<resolved>'
            models = [string]$inventory.functions[0].models
        }
    )
}
$snapshotPath = Join-Path $caseDir 'implementation_inventory.snapshot.json'
$expectedJson = ((Get-Content $snapshotPath -Raw | ConvertFrom-Json) | ConvertTo-Json -Depth 8 -Compress)
$actualJson   = ($normalized | ConvertTo-Json -Depth 8 -Compress)
if ($actualJson -ne $expectedJson) {
    Write-Error "implementation_inventory snapshot mismatch.`nexpected: $expectedJson`nactual:   $actualJson"
    Write-Host "RESULT: DISPROVED"
    exit 1
}

Write-Host "Unified and legacy gen-verify produce identical SAW scripts."
Write-Host "RESULT: VERIFIED"
