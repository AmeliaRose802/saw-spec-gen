<#
.SYNOPSIS
    SAW formal verification: prove a Rust function matches a hand-written
    Cryptol spec. No modifications to the user's .rs source required.

.DESCRIPTION
    Pipeline:
      1. rustc --emit=llvm-bc -C link-dead-code=yes  (preserves private fn)
      2. llvm-dis → .ll for symbol resolution
      3. saw-spec-gen gen-verify-rust → emits verify_rust.saw + meta sidecar
         (Rust binary owns symbol resolution + Cryptol/LLVM Bit-bridge
         emission so the convention stays in sync with the C++ generator.)
      4. Run SAW.
      5. On DISPROVED, evaluate the Cryptol spec at the SAW counterexample
         and compile + run a tiny Rust harness that calls the function on
         the same inputs, then print a side-by-side diagnostic.

    For Rust targets that grow globals, traits, or heap, the per-case
    .saw additions (overrides, stubs) will land in the same gen-verify-rust
    subcommand — same pattern verify.ps1 uses for the C++ side today.

.PARAMETER RustFile
    Path to the Rust source file. The target function can be private —
    it does NOT need `#[no_mangle]` or `pub extern "C"`.

.PARAMETER CryptolSpec
    Path to the Cryptol spec file (.cry).

.PARAMETER CryptolFn
    Name of the Cryptol function to check against.

.PARAMETER Function
    Name of the Rust function (as written in source, e.g. "add_one").

.PARAMETER OutputDir
    Optional output directory; default: out_<basename>/ next to the .rs file.

.EXAMPLE
    .\verify-rust.ps1 `
        -RustFile    tests\e2e\cases\02-havoc-coverage\nothing_sketchy\add_one_verified.rs `
        -CryptolSpec tests\e2e\cases\02-havoc-coverage\nothing_sketchy\add_one_spec.cry `
        -CryptolFn   add_one_spec `
        -Function    add_one
#>

param(
    [Parameter(Mandatory)][string]$RustFile,
    [Parameter(Mandatory)][string]$CryptolSpec,
    [Parameter(Mandatory)][string]$CryptolFn,
    [Parameter(Mandatory)][string]$Function,
    [string]$OutputDir
)

$ErrorActionPreference = "Stop"

# ── Resolve paths ──────────────────────────────────────────────────────────────
$RustFile    = Resolve-Path $RustFile
$CryptolSpec = Resolve-Path $CryptolSpec
$baseName    = [System.IO.Path]::GetFileNameWithoutExtension($RustFile)

if (-not $OutputDir) {
    $OutputDir = Join-Path (Split-Path $RustFile) "out_rust_${baseName}"
}
if (Test-Path $OutputDir) { Remove-Item -Recurse -Force $OutputDir }
New-Item -ItemType Directory -Path $OutputDir -Force | Out-Null
$OutputDir = Resolve-Path $OutputDir

# ── Tool discovery ────────────────────────────────────────────────────────────
# Shared helper: same search order as verify.ps1 / the e2e runner so all
# entry points agree (env vars, ~/.saw-spec-gen/env.ps1, PATH, defaults).
$ScriptRoot = Split-Path -Parent $PSCommandPath
. (Join-Path $ScriptRoot 'scripts/discover-tools.ps1')

# saw-spec-gen owns SAW spec emission for Rust targets — build it on
# demand (no-op if up to date) before any other discovery so a fresh
# checkout / rebase can't leave us calling an out-of-date CLI.
$specGen = Build-SawSpecGen -RepoRoot $ScriptRoot

$tools = Find-SawSpecGenTools -RepoRoot $ScriptRoot
Assert-SawSpecGenTools -Tools $tools -Require @('LlvmDis', 'Saw', 'Rustc')
Add-SolverDirToPath -Tools $tools

$llvmDis    = $tools.LlvmDis
$rustc      = $tools.Rustc
$saw        = $tools.Saw
$llvmTarget = $tools.LlvmTarget

# ── Step 1: rustc → LLVM bitcode (private fn preserved via link-dead-code) ────
Write-Host ""
Write-Host "═══════════════════════════════════════════════════════" -ForegroundColor Cyan
Write-Host " Step 1: rustc $baseName.rs → bitcode" -ForegroundColor Cyan
Write-Host "═══════════════════════════════════════════════════════" -ForegroundColor Cyan

# Key flags:
#   -C link-dead-code=yes      preserves private/unused fns in the bitcode
#                              (so we don't need #[no_mangle]/pub extern "C")
#   -C symbol-mangling-version=v0
#                              predictable, parseable mangling
#                              (_RNvCs<hash>_<crate><N><name>)
#   -C overflow-checks=off / debug-assertions=off
#                              modular arithmetic, matches Cryptol's +
#                              (otherwise debug builds call core::panicking
#                              which has no body and SAW can't resolve)
#   -C panic=abort             no unwinding personality functions
#   -C codegen-units=1         single LLVM module
#   -C debuginfo=0             smaller, self-contained
#   -C lto=off / embed-bitcode=no
#                              force full bitcode (with function bodies), not
#                              the summary-only ThinLTO bitcode rustc emits
#                              by default for `--emit=llvm-bc --crate-type=lib`
#                              when `#[inline(never)]` or similar attributes
#                              are present.
$bcFile = Join-Path $OutputDir "$baseName.bc"
& $rustc `
    --emit=llvm-bc="$bcFile" `
    --crate-type=lib `
    --edition=2021 `
    --target $llvmTarget `
    -C opt-level=0 `
    -C link-dead-code=yes `
    -C symbol-mangling-version=v0 `
    -C overflow-checks=off `
    -C debug-assertions=off `
    -C panic=abort `
    -C codegen-units=1 `
    -C debuginfo=0 `
    -C lto=off `
    -C embed-bitcode=no `
    -o (Join-Path $OutputDir "$baseName.out") `
    $RustFile 2>&1 | Write-Host
if (-not (Test-Path $bcFile)) {
    Write-Error "rustc did not produce $bcFile"
    exit 1
}
Write-Host "  → $bcFile ($((Get-Item $bcFile).Length) bytes)" -ForegroundColor Green

# ── Step 2: disassemble (saw-spec-gen reads .ll for symbol resolution) ────────
Write-Host ""
Write-Host "═══════════════════════════════════════════════════════" -ForegroundColor Cyan
Write-Host " Step 2: llvm-dis → $baseName.ll" -ForegroundColor Cyan
Write-Host "═══════════════════════════════════════════════════════" -ForegroundColor Cyan
$llFile = Join-Path $OutputDir "$baseName.ll"
& $llvmDis $bcFile -o $llFile 2>&1 | Write-Host
if (-not (Test-Path $llFile)) { Write-Error "llvm-dis failed"; exit 1 }
Write-Host "  → $llFile" -ForegroundColor Green

# ── Step 3: saw-spec-gen gen-verify-rust ──────────────────────────────────────
# The Rust subcommand:
#   - resolves the mangled symbol for $Function (v0 mangling, integer-only
#     signature filter, shortest-symbol tiebreak — mirrors what the C++
#     side gets via gen-verify),
#   - scans the .ll for mutable global definitions and emits
#     llvm_alloc_global + llvm_points_to seeding for each one,
#   - emits verify_rust.saw using the SAME cryptol_arg_for Bit/`[1]`
#     bridge the C++ emitter uses, so spec authors writing
#     `f : Bit -> ...` get a spec that type-checks under both runners,
#   - writes verify_rust.meta.json (mangled name, per-arg bit width,
#     globals) which we read below for counterexample evaluation +
#     pretty-printing.
Write-Host ""
Write-Host "═══════════════════════════════════════════════════════" -ForegroundColor Cyan
Write-Host " Step 3: saw-spec-gen gen-verify-rust" -ForegroundColor Cyan
Write-Host "═══════════════════════════════════════════════════════" -ForegroundColor Cyan
& $specGen gen-verify-rust `
    --llvm-ir      $llFile `
    --bitcode      $bcFile `
    --cryptol-spec $CryptolSpec `
    --cryptol-fn   $CryptolFn `
    --function     $Function `
    --output       $OutputDir 2>&1 | Write-Host
if ($LASTEXITCODE -ne 0) {
    Write-Error "saw-spec-gen gen-verify-rust failed (exit=$LASTEXITCODE)"
    exit 1
}

$sawScript = Join-Path $OutputDir "verify_rust.saw"
$metaPath  = Join-Path $OutputDir "verify_rust.meta.json"
if (-not (Test-Path $sawScript)) { Write-Error "missing $sawScript"; exit 1 }
if (-not (Test-Path $metaPath))  { Write-Error "missing $metaPath"; exit 1 }

$meta     = Get-Content $metaPath -Raw | ConvertFrom-Json
$mangled  = $meta.mangled_name
$cryName  = [System.IO.Path]::GetFileName($CryptolSpec)
# Build a typed view of params: each entry has .Index / .Name / .Bits /
# .LlvmType so the counterexample probe below can format Cryptol literals
# and Rust harness arguments without re-parsing the .ll.
$paramView = @($meta.params | ForEach-Object {
    [PSCustomObject]@{
        Index    = [int]$_.index
        Name     = [string]$_.name
        Bits     = [int]$_.bits
        LlvmType = [string]$_.llvm_type
    }
})

# Parse Rust source for the function signature so we can:
#   (a) show parameter *source names* in the counterexample (instead of x0/x1/...)
#   (b) compile a harness that calls $Function at cex inputs using the
#       right Rust types (u8/u16/u32/u64/i*…).
# Pure cex-pretty-printing concern; SAW spec generation no longer cares.
$rustSrc        = Get-Content $RustFile -Raw
$rustParamNames = @()
$rustParamTypes = @()
$sigPattern     = "(?ms)fn\s+$([regex]::Escape($Function))\s*\(([^)]*)\)"
if ($rustSrc -match $sigPattern) {
    $paramList = $Matches[1].Trim()
    if ($paramList -ne "") {
        foreach ($p in ($paramList -split ',')) {
            if ($p.Trim() -match '^\s*(\w+)\s*:\s*(\S+)\s*$') {
                $rustParamNames += $Matches[1]
                $rustParamTypes += $Matches[2]
            }
        }
    }
}

# ── Step 4: run SAW ───────────────────────────────────────────────────────────
Write-Host ""
Write-Host "═══════════════════════════════════════════════════════" -ForegroundColor Cyan
Write-Host " Step 4: SAW verification" -ForegroundColor Cyan
Write-Host "═══════════════════════════════════════════════════════" -ForegroundColor Cyan
Push-Location $OutputDir
$sawOut = & $saw ([System.IO.Path]::GetFileName($sawScript)) 2>&1 | Out-String
Pop-Location
Write-Host $sawOut

# ── Report ────────────────────────────────────────────────────────────────────
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
        Side           = 'rust'
        Function       = $Function
        CryptolFn      = $CryptolFn
        Verdict        = $verdict
        Counterexample = @($cex)
        Solver         = 'z3'
        ImplFile       = (Split-Path -Leaf $RustFile)
    }
    if ($expected) { $payloadArgs['Expected'] = [string]$expected }
    if ($actual)   { $payloadArgs['Actual']   = [string]$actual   }
    Write-VerifyResult @payloadArgs
}

if ($sawOut -match "Counterexample") {
    # ── Parse counterexample bindings (x0, x1, ...) from SAW output ─────────
    $cexPairs = @()
    $sawOut -split "`n" | ForEach-Object {
        if ($_ -match '^\s+(x\d+):\s+(\d+)\s*$') {
            $idx  = [int]($Matches[1].Substring(1))
            $bits = if ($idx -lt $paramView.Count) { $paramView[$idx].Bits } else { 32 }
            $nm   = if ($idx -lt $rustParamNames.Count) { $rustParamNames[$idx] } else { $Matches[1] }
            $cexPairs += [PSCustomObject]@{
                Index = $idx
                Name  = $nm
                Value = [uint64]$Matches[2]
                Bits  = $bits
            }
        }
    }
    $cexPairs = $cexPairs | Sort-Object Index

    # ── Evaluate Cryptol spec at counterexample inputs ──────────────────────
    $expectedVal = $null
    if ($cexPairs.Count -gt 0) {
        # For i1 (boolean) inputs, bridge to `Bit` via `((v : [1]) ! 0)`
        # so the call matches the spec's declared `Bit` parameter type —
        # mirrors the (x0 ! 0) wrapping in verify_rust.saw (which lives
        # in saw-spec-gen / cryptol_bridge.rs now). Both bridges must
        # agree or the cex eval prints a misleading "expected" value.
        $cryptolArgs = ($cexPairs | ForEach-Object {
            if ($_.Bits -eq 1) {
                "(($($_.Value) : [1]) ! 0)"
            } else {
                "($($_.Value) : [$($_.Bits)])"
            }
        }) -join " "
        $evalScript = Join-Path $OutputDir "_eval_cex.saw"
        @"
import "$cryName";
let r = eval_int {{ $CryptolFn $cryptolArgs }};
print (str_concat "CRYPTOL_RESULT=" (show r));
"@ | Set-Content $evalScript -Encoding utf8
        Push-Location $OutputDir
        $evalOut = & $saw "_eval_cex.saw" 2>&1 | Out-String
        Pop-Location
        if ($evalOut -match "CRYPTOL_RESULT=(\d+)") { $expectedVal = $Matches[1] }
    }

    # ── Compile + run a tiny Rust harness that calls $Function on cex ───────
    # Bool needs a real bool literal; Rust rejects `1u64 as bool`.
    $actualVal = $null
    if ($cexPairs.Count -gt 0 -and $cexPairs.Count -eq $rustParamTypes.Count) {
        $callArgs = for ($i = 0; $i -lt $cexPairs.Count; $i++) {
            $rustType = $rustParamTypes[$i]
            if ($rustType -eq 'bool') {
                if ($cexPairs[$i].Value -eq 0) { 'false' } else { 'true' }
            } else {
                "($($cexPairs[$i].Value)u64 as $rustType)"
            }
        }
        $harness    = Join-Path $OutputDir "_harness.rs"
        $harnessExe = Join-Path $OutputDir "_harness.exe"
        @"
// Auto-generated: calls ${Function} at SAW counterexample inputs.
include!(r"$RustFile");
#[allow(dead_code)]
fn main() {
    let r = ${Function}($($callArgs -join ', '));
    // Print as unsigned bit pattern so i32::MIN doesn't break RUST_RESULT=(\d+).
    let mut bits: u64 = 0;
    let n = std::cmp::min(std::mem::size_of_val(&r), std::mem::size_of::<u64>());
    unsafe { std::ptr::copy_nonoverlapping(
        &r as *const _ as *const u8, &mut bits as *mut _ as *mut u8, n); }
    println!("RUST_RESULT={}", bits);
}
"@ | Set-Content $harness -Encoding utf8
        $harnessBuild = & $rustc --crate-type=bin --edition=2021 --target $llvmTarget `
            -C opt-level=0 -C overflow-checks=off -C debug-assertions=off `
            -C panic=abort -C codegen-units=1 -C debuginfo=0 `
            -A dead_code -A unused `
            -o $harnessExe $harness 2>&1 | Out-String
        if (Test-Path $harnessExe) {
            $hOut = & $harnessExe 2>&1 | Out-String
            if ($hOut -match "RUST_RESULT=(\d+)") { $actualVal = $Matches[1] }
        } elseif ($harnessBuild.Trim()) {
            Write-Host "  Rust counterexample harness failed to compile:" -ForegroundColor Yellow
            Write-Host $harnessBuild.Trim() -ForegroundColor DarkYellow
        }
    }

    # ── Pretty-print ────────────────────────────────────────────────────────
    $displayArgs = ($cexPairs | ForEach-Object { "$($_.Value)" }) -join ", "
    Write-Host ""
    Write-Host "  RESULT: DISPROVED" -ForegroundColor Red
    Write-Host "    Rust $Function  ≢  $CryptolFn" -ForegroundColor Red
    Write-Host ""
    if ($cexPairs.Count -gt 0) {
        Write-Host "  Counterexample input:" -ForegroundColor Yellow
        foreach ($p in $cexPairs) {
            Write-Host ("    {0,-8} = {1}" -f $p.Name, $p.Value) -ForegroundColor Yellow
        }
        Write-Host ""
    }
    if ($expectedVal -or $actualVal) {
        Write-Host "  At this input:" -ForegroundColor White
        if ($expectedVal) {
            Write-Host ("    Cryptol  {0}({1}) = {2}" -f $CryptolFn, $displayArgs, $expectedVal) -ForegroundColor Green
        }
        if ($actualVal) {
            $marker = if ($actualVal -eq $expectedVal) { "✓" } else { "✗" }
            Write-Host ("    Rust     {0}({1}) = {2}  {3}" -f $Function, $displayArgs, $actualVal, $marker) -ForegroundColor Red
        }
        Write-Host ""
    }
    Write-Host "  Output dir: $OutputDir" -ForegroundColor Gray
    Write-Host ""
    Write-Host "RESULT: DISPROVED"
    Write-ResultJson 'DISPROVED' $cexPairs $expectedVal $actualVal
    exit 1
}
elseif ($sawOut -match "VERIFIED") {
    Write-Host "  RESULT: VERIFIED" -ForegroundColor Green
    Write-Host "    Rust $Function  ≡  $CryptolFn (for all u32 inputs)" -ForegroundColor Green
    Write-Host "  Output dir: $OutputDir" -ForegroundColor Gray
    Write-Host ""
    Write-Host "RESULT: VERIFIED"
    Write-ResultJson 'VERIFIED' @() $null $null
    exit 0
}
else {
    # SAW emitted neither a "Counterexample" nor "VERIFIED" banner —
    # usually a Cryptol type error in the spec or a SAW load failure.
    # Use 'UNKNOWN' (not 'ERROR') so:
    #   (a) the verdict matches verify.ps1's fall-through branch and
    #       the shared Write-VerifyResult ValidateSet, and
    #   (b) the runner picks up the real RESULT line instead of
    #       catching a Stop-mode PowerShell exception and reporting
    #       the case as EXCEPTION (which hides the SAW output that
    #       would let us diagnose it).
    Write-Host "  RESULT: UNKNOWN — could not classify SAW output" -ForegroundColor Magenta
    Write-Host "  Inspect $OutputDir for the .saw script + raw SAW output." -ForegroundColor Magenta
    Write-Host ""
    Write-Host "RESULT: UNKNOWN"
    Write-ResultJson 'UNKNOWN' @() $null $null
    exit 2
}
