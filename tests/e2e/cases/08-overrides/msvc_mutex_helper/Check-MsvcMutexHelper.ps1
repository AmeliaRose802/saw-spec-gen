<#
.SYNOPSIS
    E2E test: verify that gen-verify emits an [msvc-mutex-helper] override
    when the LLVM IR contains a defined linkonce_odr std::_Mutex_base method
    reachable from the target function.

.DESCRIPTION
    On real Windows/MSVC builds, <mutex> headers emit helpers such as
    ?_Verify_ownership_levels@_Mutex_base@std@@IEAA_NXZ as defined
    linkonce_odr bodies. When the containing object is modelled as a flat
    byte buffer, SAW aborts on a typed load of unconstrained ownership fields
    ("Error during memory load"). saw-spec-gen must detect these via
    BrokenReason::MsvcMutexHelper and emit an llvm_unsafe_assume_spec override
    tagged [msvc-mutex-helper].

    This test injects a synthetic LLVM IR file with MSVC-mangled _Mutex_base
    names so the scanner path is exercised cross-platform (no MSVC required).
    It then asserts that the generated verify.saw contains the expected tag.
#>
param()
$ErrorActionPreference = 'Stop'

$ScriptRoot = Split-Path -Parent $PSCommandPath
$RepoRoot   = Resolve-Path (Join-Path $ScriptRoot '../../../../..')
$caseDir    = $ScriptRoot

. (Join-Path $RepoRoot 'scripts/discover-tools.ps1')
$specGen = Build-SawSpecGen -RepoRoot $RepoRoot
$tools   = Find-SawSpecGenTools -RepoRoot $RepoRoot
Assert-SawSpecGenTools -Tools $tools -Require @('Clang', 'LlvmAs')

$clang  = $tools.Clang
$llvmAs = $tools.LlvmAs
$outDir = Join-Path $caseDir 'out_msvc_mutex_helper'

if (Test-Path $outDir) { Remove-Item -Recurse -Force $outDir }
New-Item -ItemType Directory -Path $outDir -Force | Out-Null

# ── 1. Synthetic LLVM IR mimicking MSVC STL _Mutex_base output ──────────────
# add_one calls ?_Verify_ownership_levels@_Mutex_base@std@@IEAA_NXZ, which is
# defined linkonce_odr. The body loads from unconstrained mutex-ownership
# fields, causing SAW to abort when this is modelled as a byte buffer.
$llFile = Join-Path $outDir 'msvc_mutex_sim.ll'
Set-Content -Path $llFile -Encoding UTF8 -Value @'
; Synthetic IR: MSVC STL _Mutex_base linkonce_odr helper pattern (issue #65).
; add_one calls a defined linkonce_odr _Mutex_base@std member whose body loads
; from uninitialized ownership fields — the exact trigger for SAW's
; "Error during memory load" abort.

target datalayout = "e-m:e-p270:32:32-p271:32:32-p272:64:64-i64:64-f80:128-n8:16:32:64-S128"
target triple = "x86_64-unknown-linux-gnu"

%struct.MutexBase = type { i32, i32 }

define dso_local i32 @add_one(i32 %x) {
entry:
  %mtx = alloca %struct.MutexBase, align 4
  %chk = call i1 @"?_Verify_ownership_levels@_Mutex_base@std@@IEAA_NXZ"(ptr %mtx)
  %1 = add i32 %x, 1
  ret i32 %1
}

; MSVC std::_Mutex_base member — defined linkonce_odr in STL headers.
define linkonce_odr i1 @"?_Verify_ownership_levels@_Mutex_base@std@@IEAA_NXZ"(ptr %this) {
entry:
  %level = load i32, ptr %this, align 4
  %cmp = icmp eq i32 %level, 0
  ret i1 %cmp
}
'@

# ── 2. Compile synthetic IR → bitcode ────────────────────────────────────────
$bcFile = Join-Path $outDir 'msvc_mutex_sim.bc'
& $llvmAs $llFile -o $bcFile 2>&1 | Write-Host
if ($LASTEXITCODE -ne 0) {
    Write-Error "llvm-as failed to compile synthetic IR"
    Write-Host 'RESULT: DISPROVED'
    exit 1
}

# ── 3. Minimal C++ stub → clang AST (describes add_one's signature) ──────────
$cppStub = Join-Path $outDir 'add_one_stub.cpp'
Set-Content -Path $cppStub -Encoding UTF8 -Value @'
#include <cstdint>
extern "C" uint32_t add_one(uint32_t x) { return x + 1; }
'@

$astFile    = Join-Path $outDir 'ast.json'
$clangErrF  = Join-Path $outDir 'clang_ast_err.txt'
# Capture stdout (AST JSON) only; stderr (warnings) goes to a temp file so we
# can show it if the dump fails rather than swallowing genuine errors.
$astLines = & $clang -Xclang '-ast-dump=json' -fsyntax-only $cppStub 2>$clangErrF
$astLines | Set-Content -Path $astFile -Encoding UTF8
if (-not (Test-Path $astFile) -or (Get-Item $astFile).Length -eq 0) {
    Write-Error 'clang AST dump produced no output'
    if (Test-Path $clangErrF) { Get-Content $clangErrF | Write-Host }
    Write-Host 'RESULT: DISPROVED'
    exit 1
}

# ── 4. Minimal Cryptol spec ───────────────────────────────────────────────────
$cryFile = Join-Path $outDir 'add_one_spec.cry'
Set-Content -Path $cryFile -Encoding UTF8 -Value @'
module add_one_spec where
add_one_spec : [32] -> [32]
add_one_spec x = x + 1
'@

# ── 5. Run gen-verify ─────────────────────────────────────────────────────────
& $specGen gen-verify `
    --ast     $astFile `
    --bitcode $bcFile `
    --llvm-ir $llFile `
    --cryptol-spec $cryFile `
    --cryptol-fn   add_one_spec `
    --function     add_one `
    --output   $outDir 2>&1 | Write-Host

if ($LASTEXITCODE -ne 0) {
    Write-Error 'gen-verify failed'
    Write-Host 'RESULT: DISPROVED'
    exit 1
}

# ── 6. Assert [msvc-mutex-helper] appears in the generated SAW script ─────────
$sawFile = Join-Path $outDir 'verify.saw'
if (-not (Test-Path $sawFile)) {
    Write-Error 'verify.saw was not generated'
    Write-Host 'RESULT: DISPROVED'
    exit 1
}

$sawText = Get-Content $sawFile -Raw
if ($sawText -notmatch '\[msvc-mutex-helper\]') {
    Write-Error 'Missing [msvc-mutex-helper] override in generated verify.saw'
    Write-Host '--- verify.saw ---'
    Write-Host $sawText
    Write-Host 'RESULT: DISPROVED'
    exit 1
}

if ($sawText -notmatch '_Mutex_base@std') {
    Write-Error 'Missing _Mutex_base@std symbol in generated verify.saw'
    Write-Host 'RESULT: DISPROVED'
    exit 1
}

Write-Host '[msvc-mutex-helper] override confirmed in verify.saw'
Write-Host 'RESULT: VERIFIED'
