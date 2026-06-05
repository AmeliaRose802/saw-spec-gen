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
    [string[]]$ClangFlags = @(),
    # Extra flags appended verbatim to `saw-spec-gen gen-verify`. Used to
    # thread per-case buffer-override CLI flags (--out-buffer-param,
    # --in-buffer-size, --cryptol-fn-out, --max-len-precond,
    # --cryptol-arg-order, …) without hard-coding them here. Each entry
    # is passed as a separate argv element.
    [string[]]$ExtraSpecGenArgs = @(),
    # Soft-exit (write result.json with status=not_attempted) instead
    # of erroring out when the target Cryptol function has no matching
    # C++ symbol — i.e. it's a Cryptol-only helper. Intended for batch
    # pipelines (pretty-specs/pipeline.ps1) that drive verify.ps1 over
    # every Cryptol top-level def.
    [switch]$SpecOnlyOnMissing
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
$llvmDis   = $tools.LlvmDis
$saw       = $tools.Saw
$exceptionLower = $tools.ExceptionLower
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

# ── Step 1.25: Lower C++ exception handling (optional) ────────────────────────
# Delegates to scripts/ensure-exception-lower.ps1 which handles
# auto-install (MSVC only) and the actual lowering pass.
$exceptionLower = & (Join-Path $ScriptRoot 'scripts/ensure-exception-lower.ps1') `
    -ExceptionLower $exceptionLower `
    -IsMsvc $isMsvc `
    -ScriptRoot $ScriptRoot `
    -BcFile $bcFile `
    -LlFile $llFile `
    -LlvmDis $llvmDis `
    -OutputDir $OutputDir `
    -BaseName $baseName `
    -Tools $tools

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
if ($SpecOnlyOnMissing) {
    $genVerifyArgs += @('--spec-only-on-missing')
}
if ($ExtraSpecGenArgs -and $ExtraSpecGenArgs.Count -gt 0) {
    $genVerifyArgs += $ExtraSpecGenArgs
}
& $specGen @genVerifyArgs 2>&1 | Write-Host
if ($LASTEXITCODE -ne 0) { Write-Error "saw-spec-gen failed"; exit 1 }

# Spec-only short-circuit: if --spec-only-on-missing was set and the
# target had no implementation, gen-verify wrote a result.json marking
# it as not_attempted and produced no verify.saw. Skip the SAW steps
# and exit cleanly so the parent pipeline classifies this as a soft
# success (not_attempted) rather than a hard error.
$resultFile = Join-Path $OutputDir "result.json"
if ($SpecOnlyOnMissing -and (Test-Path $resultFile)) {
    try {
        $existing = Get-Content $resultFile -Raw | ConvertFrom-Json
        if ($existing.status -eq "not_attempted") {
            Write-Host "  spec-only: no C++ implementation for '$Function' — skipping SAW." -ForegroundColor DarkYellow
            exit 0
        }
    } catch {
        # malformed result.json — fall through and let SAW try
    }
}
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
. (Join-Path $ScriptRoot 'scripts/Invoke-CounterexampleProbe.ps1')
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
                $cexVars += ("    {0} = {1}  ({2} signed)" -f $varName, $rawVal, $signed)
            } else {
                $cexVars += ("    {0} = {1}" -f $varName, $rawVal)
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
    $expectedVal = $null
    $actualVal   = $null
    if ($cexPairs.Count -gt 0) {
        $probeResult = Invoke-CounterexampleProbe `
            -CexPairs $cexPairs -CryptolFn $CryptolFn `
            -OutputDir $OutputDir -CryDest $cryDest `
            -SawExe $saw -CppFile $CppFile -ExeExt $exeExt `
            -Function $Function -ClangExe $clang `
            -LlvmTarget $llvmTarget -UserClangFlags $userClangFlags
        $expectedVal = $probeResult.ExpectedVal
        $actualVal   = $probeResult.ActualVal
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
