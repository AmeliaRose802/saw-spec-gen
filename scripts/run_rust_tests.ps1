# Thin wrapper: run the Rust havoc-spec + trait_unknown_impl tests via
# the consolidated suite. See tests/e2e/cases.psd1 for the source
# of truth.
$ScriptRoot = Split-Path -Parent $PSCommandPath
$RepoRoot   = Resolve-Path (Join-Path $ScriptRoot '..')
& (Join-Path $RepoRoot 'tests' 'e2e' 'Run-E2ETests.ps1') -Tag rust_havoc,trait_unknown_impl
exit $LASTEXITCODE
