<#
.SYNOPSIS
    Run the end-to-end test suite declared in tests/e2e/cases.psd1.

.DESCRIPTION
    For each case the runner:
      1. Resolves the underlying verify*.ps1 (or custom) script.
      2. Removes any stale output directory the script would reuse.
      3. Invokes the script, captures all output.
      4. Extracts `RESULT: <verdict>` and compares it to `Expected`.
      5. Prints one line per case (TAP-ish):
             ok   12 - cpp_havoc/pointer_aliasing/add_one_verified.cpp  (VERIFIED)
             not ok 13 - rust_havoc/...  expected=VERIFIED got=DISPROVED

    Exit code is 0 iff every executed case matched its expected verdict.

.PARAMETER Tag
    One or more tag values to filter the manifest by (case-insensitive).
    Defaults to the suite's standard tag set (everything except
    `box_allocator`, which is known UNKNOWN).

.PARAMETER All
    Run every case in the manifest, regardless of tag.

.PARAMETER List
    Print the cases that would run, then exit 0.

.PARAMETER ManifestPath
    Override the path to cases.psd1 (used by tests).

.EXAMPLE
    pwsh tests/e2e/Run-E2ETests.ps1

.EXAMPLE
    pwsh tests/e2e/Run-E2ETests.ps1 -Tag cpp_havoc,rust_havoc

.EXAMPLE
    $env:SKIP_SAW_TESTS = '1'; pwsh tests/e2e/Run-E2ETests.ps1   # no-op
#>
[CmdletBinding()]
param(
    [string[]]$Tag,
    [switch]$All,
    [switch]$List,
    [string]$ManifestPath
)

$ErrorActionPreference = 'Stop'

# ── Opt-out via env var (used by the pre-commit hook on slow machines). ─────
if ($env:SKIP_SAW_TESTS -eq '1') {
    Write-Host "SKIP_SAW_TESTS=1 set; skipping end-to-end test suite." -ForegroundColor Yellow
    exit 0
}

# ── Locate repo root (this script lives at tests/e2e/). ──────────────
$ScriptRoot = Split-Path -Parent $PSCommandPath
$RepoRoot   = Resolve-Path (Join-Path $ScriptRoot '..' '..')

if (-not $ManifestPath) {
    $ManifestPath = Join-Path $ScriptRoot 'cases.psd1'
}

# Auto-skip if SAW isn't installed (CI runners, fresh clones).
. (Join-Path $RepoRoot 'scripts/discover-tools.ps1')
$tools = Find-SawSpecGenTools -RepoRoot $RepoRoot
if (-not $tools.Saw) {
    Write-Host "SAW not found on this machine; skipping end-to-end test suite." -ForegroundColor Yellow
    Write-Host "  (Run scripts/init.ps1 / scripts/init.sh to install, or set SAW_SPEC_GEN_SAW.)" -ForegroundColor DarkGray
    exit 0
}

# ── Load + filter the manifest. ────────────────────────────────────────────
$data  = Import-PowerShellDataFile -Path $ManifestPath
$cases = @($data.Cases)
if (-not $cases -or $cases.Count -eq 0) {
    Write-Error "No cases found in $ManifestPath"
}

# Default tag set: everything except known-UNKNOWN research cases.
$defaultTags = @(
    'cpp_havoc'
    'cpp_overrides'
    'cpp_throws'
    'rust_havoc'
    'bounded_loop'
    'csep590b_c04'
    'rust_equiv'
    'rust_adversarial'
    'string_ops'
    'strings'
    'cryptol_len_bind'
    'int_ops'
        'string_content'
    'aggregate_bridge'
)
if ($All) {
    $selected = $cases
} else {
    $tagFilter = if ($Tag) { $Tag } else { $defaultTags }
    $tagSet    = New-Object System.Collections.Generic.HashSet[string] (
        ,[string[]]($tagFilter | ForEach-Object { $_.ToLowerInvariant() })
    )
    $selected  = @($cases | Where-Object { $tagSet.Contains($_.Tag.ToLowerInvariant()) })
}

if ($selected.Count -eq 0) {
    Write-Error "No cases match the requested tags: $($Tag -join ',')"
}

# ── Helpers ────────────────────────────────────────────────────────────────
function Resolve-RepoPath([string]$rel) {
    if ([System.IO.Path]::IsPathRooted($rel)) { return $rel }
    return (Join-Path $RepoRoot $rel)
}

function Get-CaseDefaults($c) {
    $cry       = if ($c.Cry)       { $c.Cry }       else { 'add_one_spec.cry' }
    $cryptolFn = if ($c.CryptolFn) { $c.CryptolFn } else { 'add_one_spec' }
    $function  = if ($c.Function)  { $c.Function }  else { 'add_one' }
    return @{ Cry = $cry; CryptolFn = $cryptolFn; Function = $function }
}

function Remove-StaleOutputDir([string]$dir, [string]$prefix, [string]$file) {
    if (-not $dir -or -not $file) { return }
    $base = [System.IO.Path]::GetFileNameWithoutExtension($file)
    $out  = Join-Path (Resolve-RepoPath $dir) ("${prefix}${base}")
    if (Test-Path $out) {
        Remove-Item -Recurse -Force $out -ErrorAction SilentlyContinue
    }
}

function Invoke-Case($c) {
    switch ($c.Runner) {
        'cpp' {
            $d = Get-CaseDefaults $c
            Remove-StaleOutputDir $c.Dir 'out_' $c.File
            $cpp = Resolve-RepoPath (Join-Path $c.Dir $c.File)
            $cry = Resolve-RepoPath (Join-Path $c.Dir $d.Cry)
            $verifyArgs = @{
                CppFile     = $cpp
                CryptolSpec = $cry
                CryptolFn   = $d.CryptolFn
                Function    = $d.Function
            }
            if ($c.InBufferSize) { $verifyArgs.InBufferSize = $c.InBufferSize }
            if ($c.OutBufferParam) { $verifyArgs.OutBufferParam = $c.OutBufferParam }
            if ($c.CryptolFnOut) { $verifyArgs.CryptolFnOut = $c.CryptolFnOut }
            if ($c.MaxLenPrecond) { $verifyArgs.MaxLenPrecond = $c.MaxLenPrecond }
            if ($c.NoStructShapeRecognizer) { $verifyArgs.NoStructShapeRecognizer = $true }
            & (Join-Path $RepoRoot 'verify.ps1') @verifyArgs *>&1 | Out-String
        }
        'rust' {
            $d = Get-CaseDefaults $c
            Remove-StaleOutputDir $c.Dir 'out_rust_' $c.File
            $rs  = Resolve-RepoPath (Join-Path $c.Dir $c.File)
            $cry = Resolve-RepoPath (Join-Path $c.Dir $d.Cry)
            & (Join-Path $RepoRoot 'verify-rust.ps1') `
                -RustFile $rs -CryptolSpec $cry `
                -CryptolFn $d.CryptolFn -Function $d.Function *>&1 | Out-String
        }
        'equiv' {
            $d = Get-CaseDefaults $c
            $cpp  = Resolve-RepoPath (Join-Path $c.Dir $c.Cpp)
            $rs   = Resolve-RepoPath (Join-Path $c.Dir $c.Rust)
            $cry  = Resolve-RepoPath (Join-Path $c.Dir $d.Cry)
            $base = [System.IO.Path]::GetFileNameWithoutExtension($c.Cpp)
            $out  = Join-Path (Resolve-RepoPath $c.Dir) "out_equiv_${base}"
            if (Test-Path $out) { Remove-Item -Recurse -Force $out -ErrorAction SilentlyContinue }
            & (Join-Path $RepoRoot 'verify-equiv.ps1') `
                -CppFile $cpp -RustFile $rs -CryptolSpec $cry `
                -CryptolFn $d.CryptolFn -Function $d.Function -OutputDir $out *>&1 | Out-String
        }
        'custom' {
            $script = Resolve-RepoPath $c.Script
            $splat  = @{}
            if ($c.ScriptArgs) {
                foreach ($k in $c.ScriptArgs.Keys) {
                    $v = $c.ScriptArgs[$k]
                    # Resolve relative paths on string args that look like files.
                    if ($v -is [string] -and $v -match '\.(rs|cpp|cry|saw|ll|bc)$') {
                        $v = Resolve-RepoPath $v
                    }
                    $splat[$k] = $v
                }
            }
            & $script @splat *>&1 | Out-String
        }
        default { throw "Unknown Runner '$($c.Runner)' for case: $($c | ConvertTo-Json -Compress)" }
    }
}

function Get-Verdict([string]$text) {
    # Pick the LAST `RESULT:` line in the output.  Equivalence tests emit
    # three: one per side (C++/Rust) plus the final equivalence verdict.
    # Taking the last one consistently lands on the verdict the script
    # treats as authoritative.  Single-result runs (cpp, rust) are
    # unaffected since they only emit one match.
    $matches = [regex]::Matches(
        $text,
        'RESULT:\s*(NOT EQUIVALENT|EQUIVALENT|VERIFIED|DISPROVED|UNKNOWN)'
    )
    if ($matches.Count -gt 0) {
        return $matches[$matches.Count - 1].Groups[1].Value.Trim()
    }
    return 'NO-RESULT'
}

function Format-CaseLabel($c) {
    if ($c.Runner -eq 'custom') {
        $base = Split-Path -Leaf $c.Script
        if ($c.ScriptArgs.RustFile) {
            $base += " ($(Split-Path -Leaf $c.ScriptArgs.RustFile))"
        }
        return "$($c.Tag)/$base"
    }
    if ($c.Runner -eq 'equiv') {
        return "$($c.Tag)/$(Split-Path -Leaf $c.Dir)/$($c.Rust)"
    }
    return "$($c.Tag)/$(Split-Path -Leaf $c.Dir)/$($c.File)"
}

if ($List) {
    Write-Host ("Would run {0} case(s):" -f $selected.Count) -ForegroundColor Cyan
    for ($i = 0; $i -lt $selected.Count; $i++) {
        $c = $selected[$i]
        Write-Host ("  {0,3}. [{1,-18}] {2} (expect {3})" -f ($i + 1), $c.Tag, (Format-CaseLabel $c), $c.Expected)
    }
    exit 0
}

# ── Run the suite ──────────────────────────────────────────────────────────
$total   = $selected.Count
$passed  = 0
$failed  = New-Object System.Collections.Generic.List[string]
$started = Get-Date

Write-Host ""
Write-Host "end-to-end test suite: $total case(s)" -ForegroundColor Cyan
Write-Host ('-' * 60)

for ($i = 0; $i -lt $total; $i++) {
    $c   = $selected[$i]
    $idx = $i + 1
    $lbl = Format-CaseLabel $c
    $sw  = [System.Diagnostics.Stopwatch]::StartNew()
    try {
        $out = Invoke-Case $c
        $got = Get-Verdict $out
    } catch {
        $got = 'EXCEPTION'
        $out = $_ | Out-String
    }
    $sw.Stop()
    $secs = '{0,5:N1}s' -f $sw.Elapsed.TotalSeconds

    if ($got -eq $c.Expected) {
        $passed++
        Write-Host ("ok     {0,3} - {1}  ({2}, {3})" -f $idx, $lbl, $got, $secs) -ForegroundColor Green
    } else {
        $failed.Add("$lbl  expected=$($c.Expected) got=$got")
        Write-Host ("not ok {0,3} - {1}  expected={2} got={3}  ({4})" -f $idx, $lbl, $c.Expected, $got, $secs) -ForegroundColor Red
        $log = Join-Path $ScriptRoot ("last-fail-{0}.log" -f $idx)
        Set-Content -Path $log -Value $out -Encoding utf8
        Write-Host "       log: $log" -ForegroundColor DarkGray
    }
}

$elapsed = (Get-Date) - $started
$summaryColor = if ($passed -eq $total) { 'Green' } else { 'Red' }
Write-Host ('-' * 60)
Write-Host ("Suite: {0}/{1} passed in {2:N1}s" -f $passed, $total, $elapsed.TotalSeconds) -ForegroundColor $summaryColor

if ($failed.Count -gt 0) {
    Write-Host ""
    Write-Host "Failures:" -ForegroundColor Red
    foreach ($f in $failed) { Write-Host "  - $f" -ForegroundColor Red }
    exit 1
}
exit 0
