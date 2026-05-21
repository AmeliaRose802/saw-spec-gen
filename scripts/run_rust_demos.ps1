# Thin wrapper: run the Rust havoc-spec + trait_unknown_impl demos via
# the consolidated suite. See tests/saw_demos/cases.psd1 for the source
# of truth.
$ScriptRoot = Split-Path -Parent $PSCommandPath
$RepoRoot   = Resolve-Path (Join-Path $ScriptRoot '..')
& (Join-Path $RepoRoot 'tests' 'saw_demos' 'Run-SawDemos.ps1') -Tag rust_havoc,trait_unknown_impl
exit $LASTEXITCODE
