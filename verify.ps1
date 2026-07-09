<#
.SYNOPSIS
    Thin PowerShell shim for the native `saw-spec-gen verify-cpp` subcommand.

.DESCRIPTION
    Keeps the historical `verify.ps1` entry point for callers and the e2e
    runner, but delegates all verification logic to the Rust CLI so the
    pipeline is cross-platform and bundleable without PowerShell.
#>

param(
    [Parameter(Mandatory)][string]$CppFile,
    [Parameter(Mandatory)][string]$CryptolSpec,
    [Parameter(Mandatory)][string]$CryptolFn,
    [Parameter(Mandatory)][string]$Function,
    [string]$OutputDir,
    [string[]]$IncludeDirs = @(),
    [string]$CxxStandard,
    [string[]]$ClangFlags = @(),
    [string]$Config,
    [string[]]$InBufferSize = @(),
    [string[]]$OutBufferParam = @(),
    [string[]]$CryptolFnOut = @(),
    [string[]]$MaxLenPrecond = @(),
    [switch]$NoStructShapeRecognizer,
    [switch]$SpecOnlyOnMissing
)

$ErrorActionPreference = 'Stop'
$ScriptRoot = $PSScriptRoot

. (Join-Path $ScriptRoot 'scripts/discover-tools.ps1')
$specGen = Build-SawSpecGen -RepoRoot $ScriptRoot
$tools = Find-SawSpecGenTools -RepoRoot $ScriptRoot
Add-SolverDirToPath -Tools $tools

$args = @(
    'verify-cpp',
    '--cpp-file', $CppFile,
    '--cryptol-spec', $CryptolSpec,
    '--cryptol-fn', $CryptolFn,
    '--function', $Function
)
if ($OutputDir) {
    $args += @('--output', $OutputDir)
}
foreach ($d in $IncludeDirs) {
    $args += @('--include-dir', $d)
}
if ($CxxStandard) {
    $args += @('--cxx-standard', $CxxStandard)
}
foreach ($flag in $ClangFlags) {
    $args += @('--clang-flag', $flag)
}
if ($Config) {
    $args += @('--config', $Config)
}
foreach ($arg in $InBufferSize) {
    $args += @('--in-buffer-size', $arg)
}
foreach ($arg in $OutBufferParam) {
    $args += @('--out-buffer-param', $arg)
}
foreach ($arg in $CryptolFnOut) {
    $args += @('--cryptol-fn-out', $arg)
}
foreach ($arg in $MaxLenPrecond) {
    $args += @('--max-len-precond', $arg)
}
if ($NoStructShapeRecognizer) {
    $args += '--no-struct-shape-recognizer'
}
if ($SpecOnlyOnMissing) {
    $args += '--spec-only-on-missing'
}

& $specGen @args
exit $LASTEXITCODE
