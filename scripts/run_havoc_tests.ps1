# Thin wrapper: run the C++ havoc-spec tests via the consolidated suite.
# See tests/e2e/cases.psd1 for the source of truth.
$ScriptRoot = Split-Path -Parent $PSCommandPath
$RepoRoot   = Resolve-Path (Join-Path $ScriptRoot '..')
& (Join-Path $RepoRoot 'tests' 'e2e' 'Run-E2ETests.ps1') -Tag cpp_havoc
exit $LASTEXITCODE
