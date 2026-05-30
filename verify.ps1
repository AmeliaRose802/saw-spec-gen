<#
.SYNOPSIS
    End-to-end SAW formal verification: compile → AST → gen-verify → SAW.

.DESCRIPTION
    Single script that takes a C++ source file and a Cryptol spec, then:
      1. Compiles the C++ to LLVM bitcode (clang)
      2. Dumps the clang AST to JSON
      3. Runs saw-spec-gen gen-verify to generate all specs + verify.saw
      4. Assembles vtable stubs (llvm-as)
      5. Runs SAW to check equivalence

    All artifacts are placed under a single output directory:
      out/
        add_one.bc              ← compiled bitcode
        add_one_ast.json        ← clang AST dump
        add_one_spec.cry        ← copy of Cryptol spec
        verify.saw              ← generated verification script
        specs/                  ← override specs + vtable stubs
          vtable_stubs.ll
          vtable_stubs.bc
          interface_overrides.saw
          ILog_log_havoc_spec.saw
          ...
        specs_experimental/     ← experimental (llvm_unspecified_globals) specs
          rand_auto_spec.saw
          ...

.PARAMETER CppFile
    Path to the C++ source file.

.PARAMETER CryptolSpec
    Path to the Cryptol spec file (.cry).

.PARAMETER CryptolFn
    Name of the Cryptol function to check against (e.g. "add_one_spec").

.PARAMETER Function
    Name of the C++ function to verify (unmangled, e.g. "add_one").

.PARAMETER OutputDir
    Output directory for all generated artifacts. Defaults to "out/" next to the .cpp file.

.PARAMETER IncludeDirs
    Extra `-I` include directories to add when compiling the C++ source
    and dumping its AST. Useful when the .cpp pulls in headers from a
    sibling `include/` tree (e.g. demo projects layered on top of
    saw-spec-gen). Each entry is passed as `-I <dir>` to clang.

.PARAMETER CxxStandard
    C++ standard to pass to clang, e.g. `c++17`, `c++20`. Translated to
    `-std=<CxxStandard>` and applied to every clang invocation in the
    pipeline. Omit to use clang's default (matches historical behaviour).

.PARAMETER ClangFlags
    Additional raw flags appended verbatim after the defaults and after
    `IncludeDirs` / `CxxStandard`, on every clang invocation. Use this
    for `-fexceptions`, `-fno-inline`, `-D…`, etc. Later flags win on
    most clang options, so callers can override the built-in defaults
    when needed.

.EXAMPLE
    .\verify.ps1 -CppFile tests\e2e\cases\01-tutorial\bounded_loop\add_one_verified.cpp -CryptolSpec tests\e2e\cases\01-tutorial\bounded_loop\add_one_spec.cry -CryptolFn add_one_spec -Function add_one
    .\verify.ps1 -CppFile tests\e2e\cases\01-tutorial\bounded_loop\add_one_verified.cpp -CryptolSpec tests\e2e\cases\01-tutorial\bounded_loop\add_one_spec.cry -CryptolFn add_one_spec -Function add_one -OutputDir my_output
    .\verify.ps1 -CppFile demo\cpp\saw\verify_targets.cpp -CryptolSpec demo\spec.cry -CryptolFn authenticate -Function authenticate `
                 -IncludeDirs demo\cpp\include -CxxStandard c++20 -ClangFlags '-fexceptions','-fno-inline'
#>

param(
    [Parameter(Mandatory)][string]$CppFile,
    [Parameter(Mandatory)][string]$CryptolSpec,
    [Parameter(Mandatory)][string]$CryptolFn,
    [Parameter(Mandatory)][string]$Function,
    [string]$OutputDir,
    [string[]]$IncludeDirs = @(),
    [string]$CxxStandard,
    [string[]]$ClangFlags = @()
)

$ErrorActionPreference = "Stop"

# ── Resolve paths ──────────────────────────────────────────────────────────────
$CppFile     = Resolve-Path $CppFile
$CryptolSpec = Resolve-Path $CryptolSpec
$ScriptRoot  = $PSScriptRoot  # directory where this .ps1 lives (repo root)
$baseName    = [System.IO.Path]::GetFileNameWithoutExtension($CppFile)

if (-not $OutputDir) {
    $OutputDir = Join-Path (Split-Path $CppFile) "out_${baseName}"
}
if (Test-Path $OutputDir) { Remove-Item -Recurse -Force $OutputDir }
New-Item -ItemType Directory -Path $OutputDir -Force | Out-Null
$OutputDir = Resolve-Path $OutputDir

# ── Find tools ─────────────────────────────────────────────────────────────────
# All tool discovery (clang, llvm-as, saw, z3, saw-spec-gen) lives in the
# shared helper so verify.ps1 / verify-rust.ps1 / end-to-end test scripts agree on
# search order and cross-platform behaviour. The helper consults env
# vars, ~/.saw-spec-gen/env.ps1, PATH, then platform-specific defaults.
# Run scripts/init.ps1 (Windows) or scripts/init.sh (Linux/macOS) once
# to populate the env file with auto-downloaded toolchain paths.
. (Join-Path $ScriptRoot 'scripts/discover-tools.ps1')

# saw-spec-gen is built from this repo, so build it on demand before the
# rest of discovery runs. Build-SawSpecGen rebuilds when the binary is
# missing OR stale (any Rust source newer than the binary), so a checkout
# or rebase can't leave us running an out-of-date CLI.
$specGen = Build-SawSpecGen -RepoRoot $ScriptRoot

$tools = Find-SawSpecGenTools -RepoRoot $ScriptRoot
Assert-SawSpecGenTools -Tools $tools -Require @('Clang', 'LlvmAs', 'Saw')
Add-SolverDirToPath -Tools $tools

$clang     = $tools.Clang
$llvmAs    = $tools.LlvmAs
$saw       = $tools.Saw
$llvmTarget= $tools.LlvmTarget   # e.g. x86_64-pc-windows-msvc / -unknown-linux-gnu
$isMsvc    = $llvmTarget -match 'windows-msvc'
# Host executable suffix for the counterexample probe (Step 5b). Without
# this, Windows produces an extensionless file that PowerShell refuses to
# run mid-pipeline ("Cannot run a document in the middle of a pipeline"),
# turning every DISPROVED case into an EXCEPTION.
$exeExt    = if ($IsWindows) { '.exe' } else { '' }

# ── User-supplied clang flag pass-through ─────────────────────────────────────
# Demo projects (e.g. pretty-specs) layer their own C++ on top of
# saw-spec-gen and need extra -I dirs / -std= / -fexceptions etc. to
# parse. Build the list once and splat it into every clang invocation
# (steps 1, 1.5/.ll, 2/ast-dump, and the cex probe in Step 5b) so the
# same source compiles the same way in all four spots. Resolve include
# paths up-front so relative dirs stay correct after we cwd into
# $OutputDir later.
$userClangFlags = @()
foreach ($d in $IncludeDirs) {
    $resolved = (Resolve-Path $d -ErrorAction SilentlyContinue)
    if (-not $resolved) { Write-Error "IncludeDirs path not found: $d"; exit 1 }
    $userClangFlags += @('-I', $resolved.Path)
}
if ($CxxStandard) { $userClangFlags += "-std=$CxxStandard" }
if ($ClangFlags)  { $userClangFlags += $ClangFlags }

# ── All artifacts go under $OutputDir ──────────────────────────────────────────
$bcFile   = Join-Path $OutputDir "$baseName.bc"
$llFile   = Join-Path $OutputDir "$baseName.ll"
$astFile  = Join-Path $OutputDir "${baseName}_ast.json"

# ── Step 1: Compile C++ → LLVM bitcode ────────────────────────────────────────
Write-Host ""
Write-Host "═══════════════════════════════════════════════════════" -ForegroundColor Cyan
Write-Host " Step 1: Compile $baseName.cpp → bitcode" -ForegroundColor Cyan
Write-Host "═══════════════════════════════════════════════════════" -ForegroundColor Cyan
& $clang -c -emit-llvm -O0 -fno-rtti -target $llvmTarget @userClangFlags $CppFile -o $bcFile 2>&1
if ($LASTEXITCODE -ne 0) { Write-Error "clang failed"; exit 1 }
Write-Host "  → $bcFile ($((Get-Item $bcFile).Length) bytes)" -ForegroundColor Green

# Also emit IR text so gen-verify can resolve fully qualified struct names.
& $clang -S -emit-llvm -O0 -fno-rtti -target $llvmTarget @userClangFlags $CppFile -o $llFile 2>&1
if ($LASTEXITCODE -ne 0) {
    Write-Host "  warning: failed to emit .ll (continuing without struct-size resolution)" -ForegroundColor Yellow
    $llFile = $null
} else {
    Write-Host "  → $llFile" -ForegroundColor Green
}

# ── Step 1.5: Patch IR for SAW/Crucible quirks ────────────────────────────────
# All passes are safe no-ops when their patterns are absent, so we run
# unconditionally: strip-msvc-eh, poison-to-undef, strip-nsw-nuw, etc.
if ($llFile) {
    $patchedLl = $llFile  # in-place rewrite
    Write-Host "  patch-llvm-ir: $specGen patch-llvm-ir --input $llFile --output $patchedLl" -ForegroundColor DarkGray
    $patchOut = & $specGen patch-llvm-ir --input $llFile --output $patchedLl 2>&1
    $patchExit = $LASTEXITCODE
    if ($patchOut) { $patchOut | ForEach-Object { Write-Host "    | $_" } }
    if ($patchExit -ne 0) {
        Write-Error ("patch-llvm-ir failed (exit=$patchExit) for $llFile`n" +
            "specGen=$specGen`n--- captured output ---`n" + (($patchOut | Out-String).Trim()))
        exit 1
    }
    # Re-assemble the .bc so SAW sees the patched module.
    $asmOut = & $llvmAs $patchedLl -o $bcFile 2>&1
    $asmExit = $LASTEXITCODE
    if ($asmOut) { $asmOut | ForEach-Object { Write-Host "    | $_" } }
    if ($asmExit -ne 0) {
        Write-Error ("llvm-as (post-patch) failed (exit=$asmExit) for $patchedLl`n" +
            "llvmAs=$llvmAs`n--- captured output ---`n" + (($asmOut | Out-String).Trim()))
        exit 1
    }
}

# ── Step 2: Dump clang AST → JSON ─────────────────────────────────────────────
Write-Host ""
Write-Host "═══════════════════════════════════════════════════════" -ForegroundColor Cyan
Write-Host " Step 2: Dump clang AST → JSON" -ForegroundColor Cyan
Write-Host "═══════════════════════════════════════════════════════" -ForegroundColor Cyan
& $clang -Xclang -ast-dump=json -fsyntax-only -target $llvmTarget @userClangFlags $CppFile 2>$null | Out-File -Encoding utf8 $astFile
if (-not (Test-Path $astFile) -or (Get-Item $astFile).Length -eq 0) {
    Write-Error "AST dump failed"; exit 1
}
Write-Host "  → $astFile ($([math]::Round((Get-Item $astFile).Length / 1MB, 1)) MB)" -ForegroundColor Green

# ── Step 2.5: Strip system-header decls from large ASTs ───────────────────────
# Including STL headers like <string> can balloon the AST dump past
# 100 MB (the size limit gen-verify enforces). The filter takes the
# .cpp's parent directory as the "user code" root and drops every
# top-level decl whose source file isn't underneath it. The check is
# purely path-prefix based, so no per-toolchain allowlist is required.
$astSizeMb = [math]::Round((Get-Item $astFile).Length / 1MB, 1)
if ($astSizeMb -gt 10) {
    Write-Host ""
    Write-Host "═══════════════════════════════════════════════════════" -ForegroundColor Cyan
    Write-Host " Step 2.5: Filter AST to user-code paths" -ForegroundColor Cyan
    Write-Host "═══════════════════════════════════════════════════════" -ForegroundColor Cyan
    $userRoot = Split-Path $CppFile -Parent
    & $specGen filter-ast --input $astFile --output $astFile --keep $userRoot 2>&1 | Write-Host
    if ($LASTEXITCODE -ne 0) { Write-Error "filter-ast failed"; exit 1 }
    $astSizeMbAfter = [math]::Round((Get-Item $astFile).Length / 1MB, 1)
    Write-Host "  → $astFile (${astSizeMbAfter} MB after filter, was ${astSizeMb} MB)" -ForegroundColor Green
}

# ── Step 3: Generate specs + verify.saw ────────────────────────────────────────
Write-Host ""
Write-Host "═══════════════════════════════════════════════════════" -ForegroundColor Cyan
Write-Host " Step 3: saw-spec-gen gen-verify" -ForegroundColor Cyan
Write-Host "═══════════════════════════════════════════════════════" -ForegroundColor Cyan
# Copy Cryptol spec into output dir so verify.saw can reference it locally
$cryDest = Join-Path $OutputDir ([System.IO.Path]::GetFileName($CryptolSpec))
Copy-Item $CryptolSpec $cryDest -Force

$genVerifyArgs = @(
    'gen-verify',
    '--ast', $astFile,
    '--bitcode', $bcFile,
    '--cryptol-spec', $cryDest,
    '--function', $Function,
    '--cryptol-fn', $CryptolFn,
    '--output', $OutputDir
)
if ($llFile) {
    $genVerifyArgs += @('--llvm-ir', $llFile)
}
& $specGen @genVerifyArgs 2>&1 | Write-Host
if ($LASTEXITCODE -ne 0) { Write-Error "saw-spec-gen failed"; exit 1 }
$verifySaw = Join-Path $OutputDir "verify.saw"
Write-Host "  → $verifySaw" -ForegroundColor Green

# ── Step 4: Assemble vtable stubs (.ll → .bc) ─────────────────────────────────
$stubsLl = Join-Path $OutputDir "vtable_stubs.ll"
$stubsBc = Join-Path $OutputDir "vtable_stubs.bc"
if (Test-Path $stubsLl) {
    Write-Host ""
    Write-Host "═══════════════════════════════════════════════════════" -ForegroundColor Cyan
    Write-Host " Step 4: Assemble vtable stubs → bitcode" -ForegroundColor Cyan
    Write-Host "═══════════════════════════════════════════════════════" -ForegroundColor Cyan
    & $llvmAs $stubsLl -o $stubsBc 2>&1
    if ($LASTEXITCODE -ne 0) { Write-Error "llvm-as failed"; exit 1 }
    Write-Host "  → $stubsBc ($((Get-Item $stubsBc).Length) bytes)" -ForegroundColor Green

    # Patch verify script to use .bc instead of .ll
    (Get-Content $verifySaw -Raw) -replace 'vtable_stubs\.ll', 'vtable_stubs.bc' |
        Set-Content $verifySaw -NoNewline
}

# ── Step 5: Run SAW ───────────────────────────────────────────────────────────
Write-Host ""
Write-Host "═══════════════════════════════════════════════════════" -ForegroundColor Cyan
Write-Host " Step 5: SAW verification" -ForegroundColor Cyan
Write-Host "═══════════════════════════════════════════════════════" -ForegroundColor Cyan

Push-Location $OutputDir
$sawOutput = & $saw ([System.IO.Path]::GetFileName($verifySaw)) 2>&1 | Out-String
Pop-Location

# ── Demangle helper ───────────────────────────────────────────────────────────
# undname.exe (MSVC ABI) and c++filt / llvm-cxxfilt (Itanium ABI) differ
# both in output framing and supported manglings, so we branch once and
# treat them uniformly afterwards.
$undname = $tools.CxxFilt

function Demangle([string]$mangled) {
    if (-not $mangled -or -not $undname) { return $mangled }
    if ($isMsvc) {
        # undname outputs: 'is :- "demangled name"'
        $raw = & $undname $mangled 2>$null | Out-String
        if ($raw -match 'is :- "(.+)"') {
            $result = $Matches[1].Trim()
            # Clean up MSVC noise: __cdecl, __ptr64, public:, etc.
            $result = $result -replace '\s*__cdecl\s*', ' '
            $result = $result -replace '\s*__ptr64\s*', ''
            $result = $result -replace '^\s*(public|private|protected):\s*', ''
            $result = $result -replace '\s+', ' '
            return $result.Trim()
        }
    } else {
        # c++filt / llvm-cxxfilt: echo-style, demangled name on stdout.
        $raw = (& $undname $mangled 2>$null | Out-String).Trim()
        if ($raw -and $raw -ne $mangled) { return $raw }
    }
    return $mangled
}

function FormatOverride([string]$name) {
    # Skip already-readable stub names
    if ($name -notmatch '^\?') { return $name }
    $demangled = Demangle $name
    if ($demangled -ne $name) {
        return $demangled
    }
    return $name
}

# ── Report results ─────────────────────────────────────────────────────────────
Write-Host ""
Write-Host "═══════════════════════════════════════════════════════" -ForegroundColor Cyan
Write-Host " Result" -ForegroundColor Cyan
Write-Host "═══════════════════════════════════════════════════════" -ForegroundColor Cyan

# Shared writer — emits a versioned result.json shape consumed by
# verify-equiv.ps1, the e2e runner, and the `saw-spec-gen
# collect-results` adapter.  See docs/result-json.md for the schema.
. (Join-Path $ScriptRoot 'scripts/Write-ResultJson.ps1')
function Write-ResultJson($verdict, $cex, $expected, $actual) {
    $payloadArgs = @{
        OutputDir      = $OutputDir
        Side           = 'cpp'
        Function       = $Function
        CryptolFn      = $CryptolFn
        Verdict        = $verdict
        Counterexample = @($cex)
        Solver         = 'z3'
        ImplFile       = (Split-Path -Leaf $CppFile)
    }
    if ($expected) { $payloadArgs['Expected'] = [string]$expected }
    if ($actual)   { $payloadArgs['Actual']   = [string]$actual   }
    Write-VerifyResult @payloadArgs
}

if ($sawOutput -match "Counterexample") {
    Write-Host ""
    Write-Host "  RESULT: DISPROVED" -ForegroundColor Red
    Write-Host ""
    Write-Host "  C++ function  : $Function" -ForegroundColor White
    Write-Host "  Cryptol spec  : $CryptolFn" -ForegroundColor White
    Write-Host "  Equivalence   : DISPROVED — counterexample found" -ForegroundColor Red
    Write-Host ""

    # Parse counterexample values from SAW output
    $cexVars = @()
    $cexPairs = @()
    $sawOutput -split "`n" | ForEach-Object {
        if ($_ -match '^\s+(\S+):\s+(\d+)\s*$') {
            $varName = $Matches[1]
            $rawVal  = [uint64]$Matches[2]
            $cexPairs += @{ Name = $varName; Value = $rawVal }
            # Show as signed if top bit set (32-bit)
            if ($rawVal -gt 2147483647 -and $rawVal -le 4294967295) {
                $signed = [int]($rawVal - 4294967296)
                $cexVars += "    $varName = $rawVal  ($signed as signed)"
            } else {
                $cexVars += "    $varName = $rawVal"
            }
        }
    }

    if ($cexVars.Count -gt 0) {
        Write-Host "  Counterexample:" -ForegroundColor Yellow
        foreach ($v in $cexVars) {
            Write-Host $v -ForegroundColor Yellow
        }
        Write-Host ""
    }

    # ── Evaluate expected vs actual at counterexample inputs ────────────────
    if ($cexPairs.Count -gt 0) {
        $displayArgs = ($cexPairs | ForEach-Object { "$($_.Value)" }) -join ", "

        # Evaluate Cryptol spec at counterexample values
        $cryptolArgs = ($cexPairs | ForEach-Object { "($($_.Value) : [32])" }) -join " "
        $cryptolExpr = "$CryptolFn $cryptolArgs"
        $evalScript  = Join-Path $OutputDir "_eval_cex.saw"
        $cryFileName = [System.IO.Path]::GetFileName($cryDest)
        @"
import "$cryFileName";
let r = eval_int {{ $cryptolExpr }};
print (str_concat "CRYPTOL_RESULT=" (show r));
"@ | Set-Content $evalScript -Encoding utf8

        Push-Location $OutputDir
        $evalOut = & $saw "_eval_cex.saw" 2>&1 | Out-String
        Pop-Location

        $expectedVal = $null
        if ($evalOut -match "CRYPTOL_RESULT=(\d+)") {
            $expectedVal = $Matches[1]
        }

        # Compile + run C++ function at counterexample values
        $testCpp = Join-Path $OutputDir "_test_cex.cpp"
        $testExe = Join-Path $OutputDir ("_test_cex" + $exeExt)
        $cppArgs = ($cexPairs | ForEach-Object { "$($_.Value)u" }) -join ", "
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

        & $clang -O0 -target $llvmTarget @userClangFlags $testCpp -o $testExe 2>$null
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
    }

    # Show which overrides fired (with demangled names)
    $overrides = @()
    $sawOutput -split "`n" | ForEach-Object {
        if ($_ -match 'Applied override!\s+(.+)$') {
            $overrides += $Matches[1].Trim()
        }
    }
    if ($overrides.Count -gt 0) {
        Write-Host "  Overrides applied during symbolic execution:" -ForegroundColor DarkGray
        foreach ($ov in $overrides) {
            Write-Host "    → $(FormatOverride $ov)" -ForegroundColor DarkGray
        }
        Write-Host ""
    }

    # Show the reason (subgoal that failed, demangled)
    $sawOutput -split "`n" | ForEach-Object {
        if ($_ -match 'Subgoal failed:\s+(\S+)') {
            $failedSym = $Matches[1]
            $friendly  = Demangle $failedSym
            Write-Host "  Failed proof obligation: $friendly" -ForegroundColor DarkGray
            if ($friendly -ne $failedSym) {
                Write-Host "    ($failedSym)" -ForegroundColor DarkGray
            }
        }
    }

    Write-Host ""
    # Persist a structured record for verify-equiv.ps1 to pick up.
    $cexForJson = @($cexPairs | ForEach-Object {
        [PSCustomObject]@{ Name = $_.Name; Value = [string]$_.Value }
    })
    Write-ResultJson "DISPROVED" $cexForJson $expectedVal $actualVal
    exit 1
} elseif ($sawOutput -match "VERIFIED") {
    Write-Host ""
    Write-Host "  RESULT: VERIFIED" -ForegroundColor Green
    Write-Host ""
    Write-Host "  C++ function  : $Function" -ForegroundColor White
    Write-Host "  Cryptol spec  : $CryptolFn" -ForegroundColor White
    Write-Host "  Equivalence   : VERIFIED by z3" -ForegroundColor Green
    Write-Host ""
    Write-ResultJson "VERIFIED" @() $null $null
    exit 0
} else {
    Write-Host ""
    Write-Host "  RESULT: UNKNOWN" -ForegroundColor Yellow
    Write-Host ""
    Write-Host "  SAW did not produce a clear verified/disproved result." -ForegroundColor Yellow
    Write-Host "  Full output:" -ForegroundColor Yellow
    Write-Host ""
    Write-Host $sawOutput
    Write-ResultJson "UNKNOWN" @() $null $null
    exit 2
}
