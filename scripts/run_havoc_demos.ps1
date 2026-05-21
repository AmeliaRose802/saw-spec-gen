# Thin wrapper: run the C++ havoc-spec demos via the consolidated suite.
# See tests/saw_demos/cases.psd1 for the source of truth.
$ScriptRoot = Split-Path -Parent $PSCommandPath
$RepoRoot   = Resolve-Path (Join-Path $ScriptRoot '..')
& (Join-Path $RepoRoot 'tests' 'saw_demos' 'Run-SawDemos.ps1') -Tag cpp_havoc
exit $LASTEXITCODE
