<#
.SYNOPSIS
    E2E regression test for issue #68: gen-verify must include the hidden
    sret return-pointer in llvm_execute_func for external sub-callees that
    return a struct by value.

.DESCRIPTION
    Compiles sret_sub_callee_verified.cpp with clang (bitcode + AST JSON),
    runs `saw-spec-gen gen-verify --lang cpp`, then inspects the generated
    specs_experimental/ sub-callee spec for `canonicalize` and asserts:

      1. result_ptr is allocated in the pre-state.
      2. result_ptr appears as the first argument in llvm_execute_func.
      3. The post-state uses llvm_points_to result_ptr (not llvm_return).

    This test is toolchain-light: it needs clang + llvm-as but NOT SAW.
    It runs on both Linux and Windows CI runners.
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

$cppFile = Join-Path $caseDir 'sret_sub_callee_verified.cpp'
$cryFile = Join-Path $caseDir 'sret_sub_callee_spec.cry'
$outDir  = Join-Path $caseDir 'out_sret_sub_callee_havoc'

if (Test-Path $outDir) { Remove-Item -Recurse -Force $outDir }
New-Item -ItemType Directory -Path $outDir -Force | Out-Null

# ── Compile C++ → bitcode ────────────────────────────────────────────────────
$bcFile = Join-Path $outDir 'sret_sub_callee.bc'
& $clang -c -emit-llvm -O0 -fno-rtti -target $llvmTarget `
    $cppFile -o $bcFile 2>&1 | Write-Host

if ($LASTEXITCODE -ne 0) {
    Write-Error "clang bitcode compilation failed"
    Write-Host "RESULT: DISPROVED"
    exit 1
}

# ── Dump clang AST to JSON ────────────────────────────────────────────────────
$astFile = Join-Path $outDir 'sret_sub_callee_ast.json'
$astOut  = & $clang -Xclang -ast-dump=json -fsyntax-only -target $llvmTarget `
    $cppFile 2>&1
[System.IO.File]::WriteAllText($astFile, ($astOut | Out-String))

if (-not (Test-Path $astFile) -or (Get-Item $astFile).Length -eq 0) {
    Write-Error "clang AST dump produced empty output"
    Write-Host "RESULT: DISPROVED"
    exit 1
}

# ── Run gen-verify (C++ path, no SAW) ────────────────────────────────────────
& $specGen gen-verify `
    --lang      cpp `
    --ast       $astFile `
    --bitcode   $bcFile `
    --cryptol-spec $cryFile `
    --cryptol-fn   wrap_canonicalize_spec `
    --function     wrap_canonicalize `
    --output       $outDir 2>&1 | Write-Host

if ($LASTEXITCODE -ne 0) {
    Write-Error "gen-verify failed"
    Write-Host "RESULT: DISPROVED"
    exit 1
}

# ── Find the generated sub-callee spec for canonicalize ──────────────────────
$expDir = Join-Path $outDir 'specs_experimental'
if (-not (Test-Path $expDir)) {
    Write-Error "specs_experimental/ directory not created"
    Write-Host "RESULT: DISPROVED"
    exit 1
}

$specFile = Get-ChildItem $expDir -Filter '*canonicalize*_auto_spec.saw' |
            Select-Object -First 1
if (-not $specFile) {
    Write-Error "No canonicalize auto-spec found in specs_experimental/"
    Write-Host "Existing specs:"
    Get-ChildItem $expDir | ForEach-Object { Write-Host "  $($_.Name)" }
    Write-Host "RESULT: DISPROVED"
    exit 1
}

$specText = Get-Content $specFile.FullName -Raw
Write-Host "Generated spec: $($specFile.Name)"
Write-Host $specText

# ── Check 1: result_ptr is allocated in the pre-state ────────────────────────
if ($specText -notmatch 'result_ptr\s*<-\s*llvm_alloc') {
    Write-Error "FAIL: result_ptr not allocated for sret sub-callee spec"
    Write-Host "RESULT: DISPROVED"
    exit 1
}

# ── Check 2: result_ptr is the first arg in llvm_execute_func ─────────────────
$execLine = ($specText -split "`n") | Where-Object { $_ -match 'llvm_execute_func' } |
            Select-Object -First 1
if (-not $execLine) {
    Write-Error "FAIL: llvm_execute_func line not found in spec"
    Write-Host "RESULT: DISPROVED"
    exit 1
}
$resultIdx  = $execLine.IndexOf('result_ptr')
$canonInput = 'x_ptr', 'canonicalize_ptr', '_ptr' |
              ForEach-Object { $execLine.IndexOf($_) } |
              Where-Object { $_ -gt 0 } |
              Select-Object -First 1
if ($resultIdx -lt 0) {
    Write-Error "FAIL: result_ptr not in llvm_execute_func args: $execLine"
    Write-Host "RESULT: DISPROVED"
    exit 1
}
if ($canonInput -and $resultIdx -gt $canonInput) {
    Write-Error "FAIL: result_ptr must be first in llvm_execute_func, got: $execLine"
    Write-Host "RESULT: DISPROVED"
    exit 1
}

# ── Check 3: post-state uses llvm_points_to result_ptr, not llvm_return ───────
if ($specText -notmatch 'llvm_points_to result_ptr') {
    Write-Error "FAIL: sret spec must use llvm_points_to result_ptr in post-state"
    Write-Host "RESULT: DISPROVED"
    exit 1
}
if ($specText -match 'llvm_return\s*\(llvm_term ret\)') {
    Write-Error "FAIL: sret spec must not emit llvm_return for struct return"
    Write-Host "RESULT: DISPROVED"
    exit 1
}

Write-Host "sret sub-callee havoc spec is well-formed"
Write-Host "RESULT: VERIFIED"
