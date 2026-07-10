<#
.SYNOPSIS
    E2E regression test: extern calls with [N x i8] aggregate return types
    emit `llvm_fresh_var "rv" (llvm_array N (llvm_int 8))` overrides.

.DESCRIPTION
    Exercises two bug fixes from the MSVC STL override PR:

    (1) split_bracket_tokens — `[16 x i8]` is now kept as a single token
        instead of being split into three fragments by split_whitespace.
    (2) ReturnSetup::ByteArray — ir_return_setup now matches `[N x i8]`
        patterns and emits `llvm_array N (llvm_int 8)` fresh vars.

    Without these fixes, the generated spec had no llvm_return for the
    extern call, which caused SAW to report
    "Incompatible types for return value".

    The test assembles a pre-written LLVM IR file containing a function
    that calls a no-argument extern returning [16 x i8], runs gen-verify,
    checks that the generated override uses llvm_array (not ptr or missing),
    and verifies with SAW.

    Expected RESULT: VERIFIED
#>
param()
$ErrorActionPreference = "Stop"

$caseDir  = Split-Path -Parent $PSCommandPath
$RepoRoot = Resolve-Path (Join-Path $caseDir '../../../../..')

. (Join-Path $RepoRoot 'scripts/discover-tools.ps1')
$specGen = Build-SawSpecGen -RepoRoot $RepoRoot
$tools   = Find-SawSpecGenTools -RepoRoot $RepoRoot
Assert-SawSpecGenTools -Tools $tools -Require @('LlvmAs', 'Saw')

$llvmAs = $tools.LlvmAs
$saw    = $tools.Saw

$llFile  = Join-Path $caseDir 'byte_array_return.ll'
$cryFile = Join-Path $caseDir 'add_one_spec.cry'
$outDir  = Join-Path $caseDir 'out_byte_array'

if (Test-Path $outDir) { Remove-Item -Recurse -Force $outDir }
New-Item -ItemType Directory -Path $outDir -Force | Out-Null

# ── Assemble LLVM IR → bitcode ────────────────────────────────────────
$bcFile = Join-Path $outDir 'byte_array_return.bc'
& $llvmAs $llFile -o $bcFile 2>&1 | Write-Host
if ($LASTEXITCODE -ne 0) {
    Write-Error "llvm-as failed to assemble byte_array_return.ll"
    Write-Host "RESULT: DISPROVED"
    exit 1
}

# ── Minimal clang AST JSON for `add_one(uint32_t) -> uint32_t` ────────
$astFile = Join-Path $outDir 'ast.json'
@'
{"id":"0x1","kind":"TranslationUnitDecl","loc":{},"range":{"begin":{},"end":{}},"inner":[{"id":"0x2","kind":"FunctionDecl","loc":{"offset":0,"file":"test.cpp","line":1,"col":1,"tokLen":7},"range":{"begin":{},"end":{}},"name":"add_one","mangledName":"add_one","type":{"qualType":"unsigned int (unsigned int)"},"inner":[{"id":"0x3","kind":"ParmVarDecl","loc":{},"range":{"begin":{},"end":{}},"name":"x","type":{"qualType":"unsigned int"}}]}]}
'@ | Set-Content -Path $astFile -Encoding utf8

# ── Run gen-verify ────────────────────────────────────────────────────
& $specGen gen-verify `
    --ast          $astFile `
    --bitcode      $bcFile `
    --llvm-ir      $llFile `
    --cryptol-spec $cryFile `
    --cryptol-fn   add_one_spec `
    --function     add_one `
    --output       $outDir 2>&1 | Write-Host

if ($LASTEXITCODE -ne 0) {
    Write-Error "gen-verify failed"
    Write-Host "RESULT: DISPROVED"
    exit 1
}

# ── Inspect generated SAW script for [16 x i8] → llvm_array override ─
$sawScript = Join-Path $outDir 'verify.saw'
if (-not (Test-Path $sawScript)) {
    Write-Error "verify.saw was not generated"
    Write-Host "RESULT: DISPROVED"
    exit 1
}

$sawText = Get-Content $sawScript -Raw

# The override for ?get_token@... must use llvm_array 16 (llvm_int 8),
# not a pointer or missing return — that was the pre-fix behaviour.
if ($sawText -notmatch 'llvm_array 16 \(llvm_int 8\)') {
    Write-Host "FAIL: override for [16 x i8] return does not contain 'llvm_array 16 (llvm_int 8)'"
    Write-Host "Generated script:"
    Write-Host $sawText
    Write-Host "RESULT: DISPROVED"
    exit 1
}

if ($sawText -notmatch 'llvm_fresh_var "rv"') {
    Write-Host "FAIL: override missing 'llvm_fresh_var ""rv""' for aggregate return"
    Write-Host "RESULT: DISPROVED"
    exit 1
}

Write-Host "PASS: [16 x i8] return emits llvm_array 16 (llvm_int 8) fresh var"
Write-Host "Spec-generation check passed; running SAW verification."

# ── Run SAW ────────────────────────────────────────────────────────────
Push-Location $outDir
try {
    $sawOut = & $saw verify.saw 2>&1 | Out-String
    Write-Host $sawOut
} finally {
    Pop-Location
}

if ($sawOut -match 'PROVED add_one' -or $sawOut -match '=== VERIFIED:') {
    Write-Host "RESULT: VERIFIED"
} else {
    Write-Host "RESULT: DISPROVED"
}
