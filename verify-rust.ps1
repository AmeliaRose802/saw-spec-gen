<#
.SYNOPSIS
    Compatibility shim for the native `saw-spec-gen verify-rust`
    subcommand.

.DESCRIPTION
    Keeps the historical script entry-point stable for callers while
    delegating all Rust verification orchestration to the Rust CLI.
#>

param(
    [Parameter(Mandatory)][string]$RustFile,
    [Parameter(Mandatory)][string]$CryptolSpec,
    [Parameter(Mandatory)][string]$CryptolFn,
    [Parameter(Mandatory)][string]$Function,
    [string]$OutputDir,
    [switch]$SpecOnlyOnMissing
)

$ErrorActionPreference = "Stop"
$ScriptRoot = Split-Path -Parent $PSCommandPath
. (Join-Path $ScriptRoot 'scripts/discover-tools.ps1')

$specGen = Build-SawSpecGen -RepoRoot $ScriptRoot

$nativeArgs = @(
    'verify-rust'
    '--rust-file', $RustFile
    '--cryptol-spec', $CryptolSpec
    '--cryptol-fn', $CryptolFn
    '--function', $Function
)
if ($OutputDir) { $nativeArgs += @('--output', $OutputDir) }
if ($SpecOnlyOnMissing) { $nativeArgs += '--spec-only-on-missing' }

& $specGen @nativeArgs
exit $LASTEXITCODE
