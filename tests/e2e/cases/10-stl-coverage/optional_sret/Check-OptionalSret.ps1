<#
.SYNOPSIS
    E2E test: verify that gen-verify handles std::optional<T> return types
    by emitting llvm_array N (llvm_int 8) instead of llvm_alias.

.DESCRIPTION
    Compiles optional_sret_verified.cpp to LLVM bitcode and AST JSON with
    clang, then runs `saw-spec-gen gen-verify` to produce verify.saw.
    Asserts that the generated script represents the std::optional<proto::Key>
    sret return as a byte array rather than an unresolvable type alias —
    the pattern that caused SAW to abort with "unsupported type" before
    the three-part fix landed in alias_fallbacks_ir / spec_rewrite /
    type_resolve.
    SAW is NOT invoked; this test validates spec generation only.
#>
param()
$ErrorActionPreference = "Stop"

$ScriptRoot = Split-Path -Parent $PSCommandPath
$RepoRoot   = Resolve-Path (Join-Path $ScriptRoot '../../../../..')
$caseDir    = Split-Path -Parent $PSCommandPath

. (Join-Path $RepoRoot 'scripts/discover-tools.ps1')
$specGen = Build-SawSpecGen -RepoRoot $RepoRoot
$tools   = Find-SawSpecGenTools -RepoRoot $RepoRoot
Assert-SawSpecGenTools -Tools $tools -Require @('Clang', 'LlvmAs')

$clang      = $tools.Clang
$llvmTarget = $tools.LlvmTarget

$cppFile = Join-Path $caseDir 'optional_sret_verified.cpp'
$cryFile = Join-Path $caseDir 'optional_sret_spec.cry'
$outDir  = Join-Path $caseDir 'out_optional_sret'

if (Test-Path $outDir) { Remove-Item -Recurse -Force $outDir }
New-Item -ItemType Directory -Path $outDir -Force | Out-Null

# ── Compile C++ → AST JSON (stdout of clang -ast-dump=json) ─────────
$astFile = Join-Path $outDir 'optional_sret.ast.json'
& $clang -Xclang '-ast-dump=json' -fsyntax-only -fno-rtti `
    -target $llvmTarget -std=c++17 $cppFile > $astFile 2>&1
if ($LASTEXITCODE -ne 0) {
    Write-Error "clang AST dump failed (exit $LASTEXITCODE)"
    Write-Host "RESULT: DISPROVED"
    exit 1
}

# ── Compile C++ → bitcode ────────────────────────────────────────────
$bcFile = Join-Path $outDir 'optional_sret.bc'
& $clang -c -emit-llvm -O0 -fno-rtti `
    -target $llvmTarget -std=c++17 `
    $cppFile -o $bcFile 2>&1 | Write-Host
if ($LASTEXITCODE -ne 0) {
    Write-Error "clang bitcode compilation failed (exit $LASTEXITCODE)"
    Write-Host "RESULT: DISPROVED"
    exit 1
}

# ── Compile C++ → LLVM IR text (best-effort; enables alias size lookup) ─
$llFile = Join-Path $outDir 'optional_sret.ll'
& $clang -S -emit-llvm -O0 -fno-rtti `
    -target $llvmTarget -std=c++17 `
    $cppFile -o $llFile 2>&1 | Write-Host

# ── Call gen-verify to generate the SAW spec ────────────────────────
& $specGen gen-verify `
    --ast          $astFile `
    --bitcode      $bcFile `
    --llvm-ir      $llFile `
    --cryptol-spec $cryFile `
    --cryptol-fn   get_key_spec `
    --function     get_key `
    --output       $outDir 2>&1 | Write-Host

if ($LASTEXITCODE -ne 0) {
    Write-Error "gen-verify failed (exit $LASTEXITCODE)"
    Write-Host "RESULT: DISPROVED"
    exit 1
}

# ── Verify generated SAW script uses byte-array not alias ────────────
$sawScript = Join-Path $outDir 'verify.saw'
if (-not (Test-Path $sawScript)) {
    Write-Error "verify.saw not generated"
    Write-Host "RESULT: DISPROVED"
    exit 1
}

$sawText = Get-Content $sawScript -Raw

# Fix 2 / Fix 3: the optional return type must be a byte array.
# llvm_alias "std::optional<..." would cause SAW to abort with
# "unsupported type: std::optional<...>" before reaching proof obligations.
if ($sawText -match 'llvm_alias "std::optional<') {
    Write-Error "Generated SAW script still contains llvm_alias for std::optional"
    Write-Host "SAW script content:"
    Write-Host $sawText
    Write-Host "RESULT: DISPROVED"
    exit 1
}

# Fix 1: the sret return buffer must be allocated as a byte array of
# some non-zero size.
if ($sawText -notmatch 'llvm_array \d+ \(llvm_int 8\)') {
    Write-Error "Generated SAW script does not use llvm_array for optional sret return"
    Write-Host "SAW script content:"
    Write-Host $sawText
    Write-Host "RESULT: DISPROVED"
    exit 1
}

Write-Host "std::optional sret return correctly represented as llvm_array in SAW script"
Write-Host "RESULT: VERIFIED"
