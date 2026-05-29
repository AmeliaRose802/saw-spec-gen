<#
.SYNOPSIS
    Auto-install the exception-lower LLVM pass if needed, then run it on
    the given bitcode file.

.DESCRIPTION
    Called from verify.ps1 to encapsulate:
      1. Auto-install logic (download pre-built / build from source)
      2. Step 1.25 lowering of C++ EH constructs in bitcode

    The pass rewrites both Itanium (invoke/landingpad/__cxa_throw) and
    MSVC SEH (catchswitch/catchpad/_CxxThrowException) constructs into
    explicit error-flag CFG. Without this pass:
      * Itanium catches are pruned (throw → partial-correctness only).
      * MSVC catches fail to load (FUNC_CODE_CATCHSWITCH unsupported).

.PARAMETER ExceptionLower
    Path to the exception-lower binary, or empty/null if not yet found.

.PARAMETER IsMsvc
    $true when the LLVM target triple contains 'windows-msvc'.

.PARAMETER ScriptRoot
    Root of the repository (used to locate install scripts).

.PARAMETER BcFile
    Path to the .bc bitcode file to lower (modified in-place on success).

.PARAMETER LlFile
    Path to the .ll text file to refresh after lowering (optional).

.PARAMETER LlvmDis
    Path to llvm-dis (used to refresh the .ll after lowering). Optional.

.PARAMETER OutputDir
    Directory for intermediate artifacts (lowered .bc).

.PARAMETER BaseName
    Base file name (without extension) for naming intermediates.

.OUTPUTS
    Returns the (possibly updated) path to the exception-lower binary.
#>
param(
    [string]$ExceptionLower,
    [bool]$IsMsvc,
    [Parameter(Mandatory)][string]$ScriptRoot,
    [Parameter(Mandatory)][string]$BcFile,
    [string]$LlFile,
    [string]$LlvmDis,
    [Parameter(Mandatory)][string]$OutputDir,
    [Parameter(Mandatory)][string]$BaseName,
    [hashtable]$Tools
)

# ── Auto-install on MSVC when binary is missing ───────────────────────
if (-not $ExceptionLower -and $IsMsvc) {
    Write-Host ""
    Write-Host "[*] exception-lower pass not found; attempting auto-install..." -ForegroundColor Cyan
    $installScript = Join-Path $ScriptRoot 'scripts/install-exception-lower.ps1'
    try {
        $built = & $installScript -LlvmBin $Tools.LlvmBin
        if ($LASTEXITCODE -eq 0 -and $built -and (Test-Path -LiteralPath $built)) {
            $ExceptionLower = $built
            $env:SAW_SPEC_GEN_EXCEPTION_LOWER = $built
            Write-Host "[*] exception-lower installed: $built" -ForegroundColor Green
        }
    } catch {
        Write-Host "[!] exception-lower auto-install failed: $_" -ForegroundColor Yellow
        Write-Host "    Continuing with text-only MSVC EH stripping." -ForegroundColor Yellow
    }
}

# ── Step 1.25: Lower C++ exception handling ───────────────────────────
if ($ExceptionLower) {
    Write-Host ""
    Write-Host "═══════════════════════════════════════════════════════" -ForegroundColor Cyan
    Write-Host " Step 1.25: Lower C++ exception handling" -ForegroundColor Cyan
    Write-Host "═══════════════════════════════════════════════════════" -ForegroundColor Cyan
    $loweredBc = Join-Path $OutputDir "${BaseName}_lowered.bc"
    & $ExceptionLower $BcFile -o $loweredBc 2>&1 | Write-Host
    if ($LASTEXITCODE -ne 0) {
        Write-Host "  warning: exception-lower failed; continuing with unlowered IR" -ForegroundColor Yellow
    } else {
        Copy-Item $loweredBc $BcFile -Force
        Write-Host "  → $BcFile (lowered)" -ForegroundColor Green
        if ($LlFile -and $LlvmDis) {
            & $LlvmDis $BcFile -o $LlFile 2>&1
            if ($LASTEXITCODE -ne 0) {
                Write-Host "  warning: llvm-dis failed on lowered .bc; .ll may be stale" -ForegroundColor Yellow
            } else {
                Write-Host "  → $LlFile (refreshed from lowered .bc)" -ForegroundColor Green
            }
        }
    }
} elseif ($IsMsvc) {
    Write-Host "  note: exception-lower binary not available; C++ try/catch demos will not load." -ForegroundColor DarkYellow
    Write-Host "        Run scripts/install-exception-lower.ps1 to install, or set" -ForegroundColor DarkYellow
    Write-Host "        SAW_SPEC_GEN_EXCEPTION_LOWER to point at an existing build." -ForegroundColor DarkYellow
}

return $ExceptionLower
