<#
.SYNOPSIS
    E2E sanity check for scripts/Parse-PropertyLog.ps1.

.DESCRIPTION
    Builds a synthetic SAW log containing BEGIN_PROOF / PROVED marker
    pairs (and one missing PROVED to simulate a failure), runs the
    parser, and asserts the resulting per-property result.json files
    match the schema-1 contract.

    Emits a single `RESULT: VERIFIED` line on success (consumed by
    tests/e2e/Run-E2ETests.ps1 via its `RESULT:\s*(...)` regex).
    Emits `RESULT: DISPROVED` and writes a diagnostic on any failed
    assertion so the harness reports a real failure.

    Has no toolchain dependencies — no SAW, no clang, no rustc — so
    it runs everywhere the e2e harness does.
#>
[CmdletBinding()]
param()

$ErrorActionPreference = 'Stop'

$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot '..' '..' '..' '..')).Path
$parser   = Join-Path $repoRoot 'scripts' 'Parse-PropertyLog.ps1'

if (-not (Test-Path -LiteralPath $parser)) {
    Write-Host "FAIL: parser not found: $parser"
    Write-Host 'RESULT: DISPROVED'
    exit 1
}

$tmp = Join-Path ([System.IO.Path]::GetTempPath()) ("proof_markers_" + [System.Guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Force -Path $tmp | Out-Null

try {
    # Synthetic SAW log mimicking the output of a multi-property
    # verification script: two proofs succeed (add_one, double_input),
    # one fails (broken — BEGIN_PROOF with no matching PROVED). The
    # interleaved noise lines simulate normal SAW chatter.
    $log = @'
[06:21:33.123] Loading file "verify.saw"
[06:21:33.456] Loaded LLVM module
BEGIN_PROOF add_one
[06:21:34.000] Verifying add_one ...
PROVED add_one
BEGIN_PROOF double_input
[06:21:35.000] Verifying double_input ...
PROVED double_input
BEGIN_PROOF broken
[06:21:36.000] Verifying broken ...
Counterexample:
  x = 7
[06:21:36.500] subgoal failed
'@

    $logFile = Join-Path $tmp 'saw.log'
    Set-Content -LiteralPath $logFile -Value $log -Encoding utf8

    $outDir = Join-Path $tmp 'out'
    New-Item -ItemType Directory -Force -Path $outDir | Out-Null

    # Run the parser.  Suppress warnings so they don't end up in the
    # captured stdout the harness scans for RESULT: lines.
    & $parser -LogPath $logFile -OutputDir $outDir -Side 'cpp' -Solver 'z3' 3>$null

    $failures = @()

    function Assert-Property {
        param(
            [string]$Name,
            [string]$ExpectedVerdict
        )
        $path = Join-Path $outDir (Join-Path 'properties' (Join-Path $Name 'result.json'))
        if (-not (Test-Path -LiteralPath $path)) {
            return "missing result.json for '$Name' (expected $path)"
        }
        try {
            $obj = Get-Content -LiteralPath $path -Raw | ConvertFrom-Json
        } catch {
            return "result.json for '$Name' is not valid JSON: $_"
        }
        if ($obj.schema_version -ne '1') {
            return "result.json for '$Name' has wrong schema_version: $($obj.schema_version)"
        }
        if ($obj.function -ne $Name) {
            return "result.json for '$Name' has wrong function: $($obj.function)"
        }
        if ($obj.verdict -ne $ExpectedVerdict) {
            return "result.json for '$Name' has verdict $($obj.verdict), expected $ExpectedVerdict"
        }
        if ($obj.side -ne 'cpp') {
            return "result.json for '$Name' has wrong side: $($obj.side)"
        }
        return $null
    }

    foreach ($pair in @(
            @{ Name = 'add_one';      Verdict = 'VERIFIED'  },
            @{ Name = 'double_input'; Verdict = 'VERIFIED'  },
            @{ Name = 'broken';       Verdict = 'DISPROVED' }
        )) {
        $err = Assert-Property -Name $pair.Name -ExpectedVerdict $pair.Verdict
        if ($err) { $failures += $err }
    }

    # The failure case must preserve the SAW log slice between
    # BEGIN_PROOF broken and EOF as evidence so collect-results sees a
    # populated counterexample.
    $brokenPath = Join-Path $outDir (Join-Path 'properties' (Join-Path 'broken' 'result.json'))
    if (Test-Path -LiteralPath $brokenPath) {
        $brokenObj = Get-Content -LiteralPath $brokenPath -Raw | ConvertFrom-Json
        if (-not $brokenObj.counterexample -or $brokenObj.counterexample.Count -eq 0) {
            $failures += "broken's counterexample is empty; expected captured evidence"
        } elseif ($brokenObj.counterexample[0].value -notmatch 'Counterexample') {
            $failures += "broken's evidence missing 'Counterexample' line"
        }
    }

    if ($failures.Count -gt 0) {
        foreach ($f in $failures) { Write-Host "FAIL: $f" }
        Write-Host 'RESULT: DISPROVED'
        exit 1
    }

    Write-Host "OK: parsed 3 properties; result.json contents match contract."
    Write-Host 'RESULT: VERIFIED'
}
finally {
    if (Test-Path -LiteralPath $tmp) {
        Remove-Item -Recurse -Force -LiteralPath $tmp -ErrorAction SilentlyContinue
    }
}
