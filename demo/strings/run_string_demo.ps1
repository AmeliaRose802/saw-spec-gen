<#
.SYNOPSIS
    Run the hand-rolled count_digits(const std::string&) proof.

.DESCRIPTION
    Builds bitcode for one of the std::string demo .cpp files,
    stages the spec + verify.saw alongside it, runs SAW, and prints
    a TAP-style `RESULT:` line for the demo harness to parse.

    This demo doesn't go through gen-verify because the proof
    requires a hand-rolled SAW driver that:
      * allocates a heap-mode std::string struct with a real
        pointer field pointing at a separately-allocated content
        buffer (gen-verify can't synthesise this layout yet);
      * asserts the Cryptol-defined `valid_string` predicate so
        SAW unrolls the symbolic loop to a finite depth.

    The same verify_count_digits_string.saw driver works for both
    the SAT (correct) and UNSAT (broken) variants.

.PARAMETER CppFile
    Path to the .cpp to verify (sat or unsat variant).
.PARAMETER ExpectedResult
    "VERIFIED" or "DISPROVED" -- passed through into the RESULT
    line emitted at the end.
#>
[CmdletBinding()]
param(
    [Parameter(Mandatory)][string]$CppFile,
    [Parameter(Mandatory)][ValidateSet('VERIFIED', 'DISPROVED')][string]$ExpectedResult
)

$ErrorActionPreference = 'Stop'

$ScriptRoot = Split-Path -Parent $PSCommandPath
$RepoRoot   = Resolve-Path (Join-Path $ScriptRoot '..' '..')
$CppFile    = Resolve-Path $CppFile

$base    = [System.IO.Path]::GetFileNameWithoutExtension($CppFile)
$outDir  = Join-Path $ScriptRoot "out_${base}"
if (Test-Path $outDir) { Remove-Item -Recurse -Force $outDir }
New-Item -ItemType Directory -Path $outDir -Force | Out-Null

# ── Tool discovery (mirrors verify.ps1). ─────────────────────────────────
$clangCandidates = @(
    'C:\Users\ameliapayne\clang+llvm-20.1.6-x86_64-pc-windows-msvc\bin'
    'C:\Program Files\LLVM\bin'
)
$llvmBin = $null
foreach ($dir in $clangCandidates) {
    if (Test-Path "$dir\clang.exe") { $llvmBin = $dir; break }
}
if (-not $llvmBin) { Write-Error 'clang.exe not found'; exit 1 }
$clang = Join-Path $llvmBin 'clang.exe'

$sawCandidates = @(
    'C:\Users\ameliapayne\saw-script\dist-newstyle\build\x86_64-windows\ghc-9.6.7\saw-1.5.0.99\x\saw\build\saw\saw.exe'
    (Get-Command saw -ErrorAction SilentlyContinue).Source
)
$saw = $null
foreach ($s in $sawCandidates) {
    if ($s -and (Test-Path $s)) { $saw = $s; break }
}
if (-not $saw) { Write-Error 'saw.exe not found'; exit 1 }

# Make sure SAW can find z3 / yices on PATH.
$env:PATH = "C:\Users\ameliapayne\saw-1.5-windows-2022-X64-with-solvers\bin;$env:PATH"

# ── Stage workspace. ─────────────────────────────────────────────────────
$bcFile = Join-Path $outDir 'count_digits_string.bc'
& $clang -c -emit-llvm -target x86_64-pc-windows-msvc $CppFile -o $bcFile 2>&1 | Out-Null
if (-not (Test-Path $bcFile)) { Write-Error 'clang failed'; exit 1 }

Copy-Item (Join-Path $ScriptRoot 'count_digits_string_spec.cry') (Join-Path $outDir 'count_digits_string_spec.cry') -Force
Copy-Item (Join-Path $ScriptRoot 'verify_count_digits_string.saw') (Join-Path $outDir 'verify.saw') -Force

# ── Run SAW. ─────────────────────────────────────────────────────────────
Push-Location $outDir
$sawOutput = & $saw 'verify.saw' 2>&1 | Out-String
$sawExit = $LASTEXITCODE
Pop-Location

Write-Host $sawOutput
Write-Host ''

# ── Verdict. ─────────────────────────────────────────────────────────────
# SAW exits 0 on success, non-zero on a failed proof.  Map that to the
# VERIFIED/DISPROVED vocabulary the demo harness understands.
$verdict = if ($sawExit -eq 0) { 'VERIFIED' } else { 'DISPROVED' }
Write-Host ('RESULT: {0}' -f $verdict)
if ($verdict -ne $ExpectedResult) {
    Write-Host "expected $ExpectedResult, got $verdict" -ForegroundColor Red
    exit 1
}
exit 0
