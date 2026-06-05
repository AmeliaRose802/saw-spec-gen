<#
.SYNOPSIS
    Evaluate Cryptol spec and C++ impl at counterexample inputs.

.DESCRIPTION
    Given counterexample variable-value pairs from a SAW DISPROVED run,
    this function:
      1. Evaluates the Cryptol spec at those inputs via SAW eval_int.
      2. Compiles and runs the C++ source at those inputs.
      3. Displays expected-vs-actual comparison.
      4. Detects the poison / UB heuristic (same value → LLVM UB flag).

    Returns a hashtable with ExpectedVal and ActualVal (each $null when
    evaluation fails or is skipped).

.NOTES
    Dot-sourced by verify.ps1.
#>
function Invoke-CounterexampleProbe {
    [CmdletBinding()]
    param(
        [Parameter(Mandatory)][array]$CexPairs,
        [Parameter(Mandatory)][string]$CryptolFn,
        [Parameter(Mandatory)][string]$OutputDir,
        [Parameter(Mandatory)][string]$CryDest,
        [Parameter(Mandatory)][string]$SawExe,
        [Parameter(Mandatory)][string]$CppFile,
        [Parameter(Mandatory)][AllowEmptyString()][string]$ExeExt,
        [Parameter(Mandatory)][string]$Function,
        [Parameter(Mandatory)][string]$ClangExe,
        [Parameter(Mandatory)][string]$LlvmTarget,
        [Parameter()][string[]]$UserClangFlags = @()
    )

    $displayArgs = ($CexPairs | ForEach-Object { "$($_.Value)" }) -join ", "

    # Evaluate Cryptol spec at counterexample values
    $cryptolArgs = ($CexPairs | ForEach-Object { "($($_.Value) : [32])" }) -join " "
    $cryptolExpr = "$CryptolFn $cryptolArgs"
    $evalScript  = Join-Path $OutputDir "_eval_cex.saw"
    $cryFileName = [System.IO.Path]::GetFileName($CryDest)
    @"
import "$cryFileName";
let r = eval_int {{ $cryptolExpr }};
print (str_concat "CRYPTOL_RESULT=" (show r));
"@ | Set-Content $evalScript -Encoding utf8

    Push-Location $OutputDir
    $evalOut = & $SawExe "_eval_cex.saw" 2>&1 | Out-String
    Pop-Location

    $expectedVal = $null
    if ($evalOut -match "CRYPTOL_RESULT=(\d+)") {
        $expectedVal = $Matches[1]
    }

    # Compile + run C++ function at counterexample values
    $testCpp = Join-Path $OutputDir "_test_cex.cpp"
    $testExe = Join-Path $OutputDir ("_test_cex" + $ExeExt)
    $cppArgs = ($CexPairs | ForEach-Object { "$($_.Value)u" }) -join ", "
    $origSrc = Get-Content $CppFile -Raw
    @"
$origSrc

#include <cstdio>
#include <cstring>
int main() {
    auto result = ${Function}($cppArgs);
    // memcpy zero-fills any padding so signed return types don't get
    // sign-extended into the upper bits of the printed u64. Matches the
    // bit pattern SAW sees, so the poison-detection heuristic below
    // can compare it apples-to-apples against the Cryptol spec value.
    unsigned long long _bits = 0;
    size_t _n = sizeof(result) < sizeof(_bits) ? sizeof(result) : sizeof(_bits);
    std::memcpy(&_bits, &result, _n);
    printf("CPP_RESULT=%llu\n", _bits);
    return 0;
}
"@ | Set-Content $testCpp -Encoding utf8

    & $ClangExe -O0 -target $LlvmTarget @UserClangFlags $testCpp -o $testExe 2>$null
    $actualVal = $null
    if (Test-Path $testExe) {
        $cppOut = & $testExe 2>&1 | Out-String
        if ($cppOut -match "CPP_RESULT=(\d+)") {
            $actualVal = $Matches[1]
        }
    }

    if ($expectedVal -or $actualVal) {
        Write-Host "  Expected vs Actual at ($displayArgs):" -ForegroundColor White
        if ($expectedVal) {
            Write-Host "    Cryptol $CryptolFn($displayArgs) = $expectedVal" -ForegroundColor Green
        }
        if ($actualVal) {
            Write-Host "    C++     $Function($displayArgs)  = $actualVal" -ForegroundColor Red
        }
        Write-Host ""
    }

    # ── Poison / UB heuristic ─────────────────────────────────
    # If the Cryptol spec and a concrete recompile-and-run of the C++
    # produce the *same* value at the counterexample inputs, the proof
    # almost certainly failed not because of a logic disagreement but
    # because the LLVM IR carries an `nsw` / `nuw` / `inbounds` flag,
    # or an `sdiv` / `udiv` whose UB-on-overflow case is reachable,
    # which turns the operation into *poison* at those inputs. SAW
    # compares LLVM semantics (poison ≠ any concrete spec value), so
    # the obligation fails even though both sides agree on the value.
    if ($expectedVal -and $actualVal -and $expectedVal -eq $actualVal) {
        Write-Host "  NOTE: Expected and Actual agree at the counterexample." -ForegroundColor Yellow
        Write-Host "        This is the signature of an LLVM UB / poison failure," -ForegroundColor Yellow
        Write-Host "        not a logic disagreement. Common causes in C++:" -ForegroundColor Yellow
        Write-Host "          - signed arithmetic with nsw       (signed overflow -> poison)" -ForegroundColor DarkYellow
        Write-Host "          - unsigned arithmetic with nuw     (unsigned overflow -> poison)" -ForegroundColor DarkYellow
        Write-Host "          - sdiv / udiv on a path where the divisor or overflow" -ForegroundColor DarkYellow
        Write-Host "            corner is reachable (sdiv INT_MIN,-1 / udiv x,0 -> poison)" -ForegroundColor DarkYellow
        Write-Host "          - getelementptr with inbounds      (out-of-bounds -> poison)" -ForegroundColor DarkYellow
        Write-Host "        Inspect the emitted .ll for the relevant flag, and either" -ForegroundColor DarkYellow
        Write-Host "        recompile with -fwrapv / cast through unsigned, or fix" -ForegroundColor DarkYellow
        Write-Host "        the underlying bug the flag is warning about." -ForegroundColor DarkYellow
        Write-Host ""
    }

    return @{ ExpectedVal = $expectedVal; ActualVal = $actualVal }
}
