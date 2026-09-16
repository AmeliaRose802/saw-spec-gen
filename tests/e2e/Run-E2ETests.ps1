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
    'object_layout'
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
            $config = if ($IsWindows -and $c.WindowsConfig) { $c.WindowsConfig } elseif (-not $IsWindows -and $c.LinuxConfig) { $c.LinuxConfig } else { $c.Config }
            if ($config) { $verifyArgs.Config = Resolve-RepoPath (Join-Path $c.Dir $config) }
            if ($c.CxxStandard) { $verifyArgs.CxxStandard = $c.CxxStandard }
            & (Join-Path $RepoRoot 'verify.ps1') @verifyArgs *>&1 | Out-String
        }
        'rust' {
            $d = Get-CaseDefaults $c
            Remove-StaleOutputDir $c.Dir 'out_rust_' $c.File
            $rs  = Resolve-RepoPath (Join-Path $c.Dir $c.File)
            $cry = Resolve-RepoPath (Join-Path $c.Dir $d.Cry)
            $rustArgs = @{
                RustFile    = $rs
                CryptolSpec = $cry
                CryptolFn   = $d.CryptolFn
                Function    = $d.Function
            }
            if ($c.Config) { $rustArgs.Config = Resolve-RepoPath (Join-Path $c.Dir $c.Config) }
            & (Join-Path $RepoRoot 'verify-rust.ps1') @rustArgs *>&1 | Out-String
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
    $verdictMatches = [regex]::Matches(
        $text,
        'RESULT:\s*(NOT EQUIVALENT|EQUIVALENT|VERIFIED|DISPROVED|UNKNOWN)'
    )
    if ($verdictMatches.Count -gt 0) {
        return $verdictMatches[$verdictMatches.Count - 1].Groups[1].Value.Trim()
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

function Get-ContractMetadataError($c) {
    if (-not $c.ContractClauses) { return $null }
    if ($c.Runner -ne 'cpp') { return 'ContractClauses is supported by the cpp runner only' }
    $base = [System.IO.Path]::GetFileNameWithoutExtension($c.File)
    $resultPath = Join-Path (Resolve-RepoPath $c.Dir) "out_${base}/result.json"
    if (-not (Test-Path $resultPath)) { return "missing contract result: $resultPath" }
    try {
        $result = Get-Content -Raw $resultPath | ConvertFrom-Json
    } catch {
        return "invalid contract result JSON: $_"
    }
    $defaults = Get-CaseDefaults $c
    if ($result.function -ne $defaults.Function) {
        return "contract result names function '$($result.function)', expected '$($defaults.Function)'"
    }
    $actual = @($result.contract.clauses)
    $expected = @($c.ContractClauses)
    if ($actual.Count -ne $expected.Count) {
        return "contract has $($actual.Count) clauses, expected $($expected.Count)"
    }
    for ($i = 0; $i -lt $expected.Count; $i++) {
        foreach ($field in @('name', 'assertion', 'region', 'cryptol_fn', 'projection')) {
            $want = $expected[$i][$field]
            $got = $actual[$i].$field
            if ($got -ne $want) {
                return "contract clause $i field '$field' is '$got', expected '$want'"
            }
        }
    }
    return $null
}

function Get-LayoutMetadataError($c) {
    if (-not $c.LayoutRegions) { return $null }
    $base = [System.IO.Path]::GetFileNameWithoutExtension($c.File)
    $dir = Join-Path (Resolve-RepoPath $c.Dir) "out_${base}"
    if (-not (Test-Path (Join-Path $dir 'result.json'))) { return 'missing result for layout validation' }
    $result = Get-Content -Raw (Join-Path $dir 'result.json') | ConvertFrom-Json
    $layout = $result.memory_layout
    if (-not $layout -or $layout.schema_version -ne 1) { return 'missing compiler layout metadata' }
    $abi = if ($IsWindows) { 'msvc' } else { 'itanium' }
    if ($layout.abi -ne $abi) { return "layout ABI $($layout.abi), expected $abi" }
    foreach ($name in $c.LayoutRegions) {
        $object = $layout.objects.$name
        if (-not $object -or $object.layout.size -le 0 -or $object.layout.alignment -le 0) { return "invalid compiler object $name" }
        if ($object.layout.unresolved.Count -ne 0) { return "unresolved compiler object $name" }
        if ($name -eq 'return') {
            if ($object.asserted.Count -ne $object.layout.fields.Count) { return 'return omits semantic fields' }
            if ($object.lowering -ne 'sret') { return 'return ABI metadata is not sret' }
        }
    }
    $script = Get-Content -Raw (Join-Path $dir 'verify.saw')
    if ($script -notmatch 'llvm_alloc(?:_readonly)?_aligned \d+ \(llvm_(?:alias|struct_type|packed_struct_type)') { return 'no typed compiler allocation in proof' }
    foreach ($symbol in $c.ForbiddenOverrides) {
        if ($script -match ('llvm_unsafe_assume_spec m "[^"\r\n]*' + [regex]::Escape($symbol))) { return "defined helper was overridden: $symbol" }
    }
    return $null
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
        if ($c.ExpectedError) {
            if ($got -eq 'NO-RESULT' -and $out -match $c.ExpectedError -and $out -notmatch 'BEGIN_PROOF') {
                $got = 'REJECTED'
            }
        }
        $contractError = Get-ContractMetadataError $c
        if ($contractError) {
            $out += "`ncontract metadata error: $contractError"
            $got = 'INVALID-CONTRACT-METADATA'
        }
        $layoutError = Get-LayoutMetadataError $c
        if ($layoutError) { $out += "`nlayout metadata error: $layoutError"; $got = 'INVALID-LAYOUT-METADATA' }
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
