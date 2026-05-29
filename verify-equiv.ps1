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
    Name of the C++/Rust function (must be the same identifier on both sides).

.PARAMETER OutputDir
    Optional output directory; default: out_equiv_<cpp_basename>/ next to .cpp.

.EXAMPLE
    .\verify-equiv.ps1 `
        -CppFile     demos\02-havoc-coverage\nothing_sketchy\add_one_verified.cpp `
        -RustFile    demos\02-havoc-coverage\nothing_sketchy\add_one_verified.rs `
        -CryptolSpec demos\02-havoc-coverage\nothing_sketchy\add_one_spec.cry `
        -CryptolFn   add_one_spec `
        -Function    add_one
#>

param(
    [Parameter(Mandatory)][string]$CppFile,
    [Parameter(Mandatory)][string]$RustFile,
    [Parameter(Mandatory)][string]$CryptolSpec,
    [Parameter(Mandatory)][string]$CryptolFn,
    [Parameter(Mandatory)][string]$Function,
    [string]$OutputDir
)

$ErrorActionPreference = "Stop"

$CppFile     = Resolve-Path $CppFile
$RustFile    = Resolve-Path $RustFile
$CryptolSpec = Resolve-Path $CryptolSpec
$ScriptRoot  = $PSScriptRoot
$cppBase     = [System.IO.Path]::GetFileNameWithoutExtension($CppFile)

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
Write-Host "  C++ side: prove $Function (C++)  ≡  $CryptolFn (Cryptol)" -ForegroundColor Magenta
Write-Host "████████████████████████████████████████████████████████" -ForegroundColor Magenta

& (Join-Path $ScriptRoot "verify.ps1") `
    -CppFile     $CppFile `
    -CryptolSpec $CryptolSpec `
    -CryptolFn   $CryptolFn `
    -Function    $Function `
    -OutputDir   $cppOutDir
$cppExit = $LASTEXITCODE

# ════════════════════════════════════════════════════════════════════════════════
# Rust side — verify-rust.ps1
# ════════════════════════════════════════════════════════════════════════════════
Write-Host ""
Write-Host "████████████████████████████████████████████████████████" -ForegroundColor Magenta
Write-Host "  Rust side: prove $Function (Rust) ≡  $CryptolFn (Cryptol)" -ForegroundColor Magenta
Write-Host "████████████████████████████████████████████████████████" -ForegroundColor Magenta

& (Join-Path $ScriptRoot "verify-rust.ps1") `
    -RustFile    $RustFile `
    -CryptolSpec $CryptolSpec `
    -CryptolFn   $CryptolFn `
    -Function    $Function `
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
Write-Host "  Function:      $Function" -ForegroundColor White
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
if ($cppOk -and $rustOk) {
    Write-Host "  RESULT: EQUIVALENT" -ForegroundColor Green
    Write-Host "    Both implementations agree with $CryptolFn," -ForegroundColor Green
    Write-Host "    therefore they agree with each other on every input." -ForegroundColor Green
    Write-Host ""
    exit 0
} else {
    Write-Host "  RESULT: NOT EQUIVALENT" -ForegroundColor Red
    if (-not $cppOk)  { Write-Host "    C++  implementation disagrees with $CryptolFn." -ForegroundColor Red }
    if (-not $rustOk) { Write-Host "    Rust implementation disagrees with $CryptolFn." -ForegroundColor Red }
    Write-Host ""
    exit 1
}
