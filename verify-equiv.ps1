<#
.SYNOPSIS
    SAW formal equivalence: prove BOTH a C++ function and a Rust function
    match the same hand-written Cryptol spec, and therefore each other.

.DESCRIPTION
    Pipeline:
      C++ side  : delegates to verify.ps1       (clang → bc → AST →
                                                  saw-spec-gen → SAW)
      Rust side : delegates to verify-rust.ps1  (rustc → bc → resolve
                                                  mangled symbol → SAW)

    Both proofs target the same Cryptol function (e.g. add_one_spec).
    Verdict:
        EQUIVALENT      — both sides match the Cryptol spec
        NOT EQUIVALENT  — at least one side disagrees

    Neither the C++ nor the Rust source needs modification for SAW.
    The C++ side uses saw-spec-gen to generate overrides for vtables,
    globals, etc. The Rust side uses `-C link-dead-code=yes` to preserve
    private fns and resolves mangled symbols automatically.

.PARAMETER CppFile
    Path to the C++ source file.

.PARAMETER RustFile
    Path to the Rust source file (no #[no_mangle] or pub extern "C" needed).

.PARAMETER CryptolSpec
    Path to the shared Cryptol spec file (.cry).

.PARAMETER CryptolFn
    Name of the Cryptol function to check both sides against.

.PARAMETER Function
    Shared function name used for both sides (back-compat shortcut).

.PARAMETER CppFunction
    Name of the C++ function. Defaults to -Function when omitted.

.PARAMETER RustFunction
    Name of the Rust function. Defaults to -Function when omitted.

.PARAMETER OutputDir
    Optional output directory; default: out_equiv_<cpp_basename>/ next to .cpp.

.EXAMPLE
    .\verify-equiv.ps1 `
        -CppFile     tests\e2e\cases\02-havoc-coverage\nothing_sketchy\add_one_verified.cpp `
        -RustFile    tests\e2e\cases\02-havoc-coverage\nothing_sketchy\add_one_verified.rs `
        -CryptolSpec tests\e2e\cases\02-havoc-coverage\nothing_sketchy\add_one_spec.cry `
        -CryptolFn   add_one_spec `
        -Function    add_one
#>

param(
    [Parameter(Mandatory)][string]$CppFile,
    [Parameter(Mandatory)][string]$RustFile,
    [Parameter(Mandatory)][string]$CryptolSpec,
    [Parameter(Mandatory)][string]$CryptolFn,
    [string]$Function,
    [string]$CppFunction,
    [string]$RustFunction,
    [string]$OutputDir
)

$ErrorActionPreference = "Stop"

$CppFile     = Resolve-Path $CppFile
$RustFile    = Resolve-Path $RustFile
$CryptolSpec = Resolve-Path $CryptolSpec
$ScriptRoot  = $PSScriptRoot
$cppBase     = [System.IO.Path]::GetFileNameWithoutExtension($CppFile)

if ($PSBoundParameters.ContainsKey('Function') -and
    $PSBoundParameters.ContainsKey('CppFunction') -and
    $CppFunction -ne $Function) {
    throw "-Function ('$Function') and -CppFunction ('$CppFunction') conflict. Use one shared name, or omit -Function."
}
if ($PSBoundParameters.ContainsKey('Function') -and
    $PSBoundParameters.ContainsKey('RustFunction') -and
    $RustFunction -ne $Function) {
    throw "-Function ('$Function') and -RustFunction ('$RustFunction') conflict. Use one shared name, or omit -Function."
}
if (-not $Function -and ((-not $CppFunction) -or (-not $RustFunction))) {
    throw "Provide -Function, or provide both -CppFunction and -RustFunction."
}

if (-not $CppFunction)  { $CppFunction  = $Function }
if (-not $RustFunction) { $RustFunction = $Function }

if (-not $OutputDir) {
    $OutputDir = Join-Path (Split-Path $CppFile) "out_equiv_${cppBase}"
}
if (Test-Path $OutputDir) { Remove-Item -Recurse -Force $OutputDir }
New-Item -ItemType Directory -Path $OutputDir -Force | Out-Null
$OutputDir = Resolve-Path $OutputDir

$cppOutDir  = Join-Path $OutputDir "cpp"
$rustOutDir = Join-Path $OutputDir "rust"

# ════════════════════════════════════════════════════════════════════════════════
# C++ side — verify.ps1
# ════════════════════════════════════════════════════════════════════════════════
Write-Host ""
Write-Host "████████████████████████████████████████████████████████" -ForegroundColor Magenta
Write-Host "  C++ side: prove $CppFunction (C++)  ≡  $CryptolFn (Cryptol)" -ForegroundColor Magenta
Write-Host "████████████████████████████████████████████████████████" -ForegroundColor Magenta

& (Join-Path $ScriptRoot "verify.ps1") `
    -CppFile     $CppFile `
    -CryptolSpec $CryptolSpec `
    -CryptolFn   $CryptolFn `
    -Function    $CppFunction `
    -OutputDir   $cppOutDir
$cppExit = $LASTEXITCODE

# ════════════════════════════════════════════════════════════════════════════════
# Rust side — verify-rust.ps1
# ════════════════════════════════════════════════════════════════════════════════
Write-Host ""
Write-Host "████████████████████████████████████████████████████████" -ForegroundColor Magenta
Write-Host "  Rust side: prove $RustFunction (Rust) ≡  $CryptolFn (Cryptol)" -ForegroundColor Magenta
Write-Host "████████████████████████████████████████████████████████" -ForegroundColor Magenta

& (Join-Path $ScriptRoot "verify-rust.ps1") `
    -RustFile    $RustFile `
    -CryptolSpec $CryptolSpec `
    -CryptolFn   $CryptolFn `
    -Function    $RustFunction `
    -OutputDir   $rustOutDir
$rustExit = $LASTEXITCODE

# ════════════════════════════════════════════════════════════════════════════════
# Combined verdict — read both sides' result.json and render a unified summary
# ════════════════════════════════════════════════════════════════════════════════
Write-Host ""
Write-Host "████████████████████████████████████████████████████████" -ForegroundColor Cyan
Write-Host "  Equivalence verdict" -ForegroundColor Cyan
Write-Host "████████████████████████████████████████████████████████" -ForegroundColor Cyan

function Read-ResultJson($dir) {
    $p = Join-Path $dir "result.json"
    if (Test-Path $p) {
        try { return Get-Content $p -Raw | ConvertFrom-Json } catch { return $null }
    }
    return $null
}

function Format-Verdict($v) {
    switch ($v) {
        "VERIFIED"  { return @{ Text = "VERIFIED  ✓"; Color = "Green"  } }
        "DISPROVED" { return @{ Text = "DISPROVED ✗"; Color = "Red"    } }
        default     { return @{ Text = "UNKNOWN   ?"; Color = "Yellow" } }
    }
}

function Format-CexArgs($cex) {
    if (-not $cex) { return "" }
    return (@($cex) | ForEach-Object { "$($_.Value)" }) -join ", "
}

$cppResult  = Read-ResultJson $cppOutDir
$rustResult = Read-ResultJson $rustOutDir

$cppVerdict  = if ($cppResult)  { $cppResult.verdict  } else { "UNKNOWN" }
$rustVerdict = if ($rustResult) { $rustResult.verdict } else { "UNKNOWN" }
$cppOk  = ($cppVerdict  -eq "VERIFIED")
$rustOk = ($rustVerdict -eq "VERIFIED")

Write-Host ""
Write-Host "  C++ function:  $CppFunction" -ForegroundColor White
Write-Host "  Rust function: $RustFunction" -ForegroundColor White
Write-Host "  Cryptol spec:  $CryptolFn" -ForegroundColor White
Write-Host ""

# ── Per-side blocks (with counterexample detail when disproved) ───────────────
function Show-Side($label, $r) {
    Write-Host "  ── $label side ────────────────────────────────────────" -ForegroundColor Cyan
    if (-not $r) {
        Write-Host "    Verdict: UNKNOWN (no result.json found)" -ForegroundColor Yellow
        Write-Host ""
        return
    }
    $v = Format-Verdict $r.verdict
    Write-Host ("    Verdict: " + $v.Text) -ForegroundColor $v.Color
    if ($r.verdict -eq "VERIFIED") {
        Write-Host "    $($r.function) matches $($r.cryptol_fn) on all inputs." -ForegroundColor Green
    } elseif ($r.verdict -eq "DISPROVED") {
        $cex = @($r.counterexample)
        if ($cex.Count -gt 0) {
            Write-Host "    Counterexample input:" -ForegroundColor Yellow
            foreach ($p in $cex) {
                Write-Host ("      {0,-8} = {1}" -f $p.Name, $p.Value) -ForegroundColor Yellow
            }
            $argList = Format-CexArgs $cex
            if ($r.expected) {
                Write-Host ("    Expected ({0}({1})) = {2}" -f $r.cryptol_fn, $argList, $r.expected) -ForegroundColor Green
            }
            if ($r.actual) {
                Write-Host ("    Actual   ({0}({1})) = {2}" -f $r.function,   $argList, $r.actual)   -ForegroundColor Red
            }
        }
    }
    Write-Host ""
}

Show-Side "C++"  $cppResult
Show-Side "Rust" $rustResult

# ── Bottom-line verdict ───────────────────────────────────────────────────────

# Also emit a combined result.json (side='equiv') so the collect-results
# adapter sees one entry per case rather than two per-side files only.
# Schema kept in sync with verify.ps1 / verify-rust.ps1 via the shared
# helper.  See docs/result-json.md.
. (Join-Path $ScriptRoot 'scripts/Write-ResultJson.ps1')
function Write-EquivResultJson([string]$verdict) {
    $cex = @()
    if ($cppResult -and $cppResult.verdict -eq 'DISPROVED' -and $cppResult.counterexample) {
        $cex = @($cppResult.counterexample)
    } elseif ($rustResult -and $rustResult.verdict -eq 'DISPROVED' -and $rustResult.counterexample) {
        $cex = @($rustResult.counterexample)
    }
    $functionDisplayValue = if ($CppFunction -eq $RustFunction) { $CppFunction } else { "$CppFunction vs $RustFunction" }
    Write-VerifyResult `
        -OutputDir      $OutputDir `
        -Side           'equiv' `
        -Function       $functionDisplayValue `
        -CryptolFn      $CryptolFn `
        -Verdict        $verdict `
        -Counterexample $cex `
        -Solver         'z3' `
        -ImplFile       ((Split-Path -Leaf $CppFile) + ' | ' + (Split-Path -Leaf $RustFile))

    $equivResultPath = Join-Path $OutputDir 'result.json'
    if (Test-Path $equivResultPath) {
        $equivPayload = Get-Content $equivResultPath -Raw | ConvertFrom-Json
        $equivPayload | Add-Member -NotePropertyName 'cpp_function'  -NotePropertyValue $CppFunction  -Force
        $equivPayload | Add-Member -NotePropertyName 'rust_function' -NotePropertyValue $RustFunction -Force
        $equivPayload | ConvertTo-Json -Depth 6 | Set-Content -Path $equivResultPath -Encoding utf8
    }
}

if ($cppOk -and $rustOk) {
    Write-Host "  RESULT: EQUIVALENT" -ForegroundColor Green
    Write-Host "    Both implementations agree with $CryptolFn," -ForegroundColor Green
    Write-Host "    therefore they agree with each other on every input." -ForegroundColor Green
    Write-Host ""
    Write-EquivResultJson 'EQUIVALENT'
    exit 0
} else {
    Write-Host "  RESULT: NOT EQUIVALENT" -ForegroundColor Red
    if (-not $cppOk)  { Write-Host "    C++  implementation disagrees with $CryptolFn." -ForegroundColor Red }
    if (-not $rustOk) { Write-Host "    Rust implementation disagrees with $CryptolFn." -ForegroundColor Red }
    Write-Host ""
    Write-EquivResultJson 'NOT EQUIVALENT'
    exit 1
}
