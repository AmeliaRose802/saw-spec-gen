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
    [string[]]$ExtraSpecGenArgs = @(),
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
foreach ($flag in $ExtraSpecGenArgs) {
    # Use the `--opt=value` form so clap accepts values that themselves
    # begin with `--` (e.g. forwarding `--in-buffer-size` to gen-verify).
    # The space-separated form would make clap treat such a value as an
    # unknown flag. See tests/e2e/cases/08-overrides/bounded_copy.
    $args += @("--extra-spec-gen-arg=$flag")
}
if ($SpecOnlyOnMissing) {
    $args += '--spec-only-on-missing'
}

& $specGen @args
exit $LASTEXITCODE
