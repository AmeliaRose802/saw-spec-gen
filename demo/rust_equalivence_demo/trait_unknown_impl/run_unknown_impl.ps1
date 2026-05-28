<#
.SYNOPSIS
    Verify a Rust function that takes `&dyn Trait` from an opaque
    caller — i.e. the concrete impl of the trait method is NOT visible
    to SAW.  Uses an assumed spec ("havoc spec") for the trait method,
    exactly like the C++ side's `vtable_stubs.bc` flow.

.DESCRIPTION
    Pipeline:
      1. rustc  add_step_*.rs                       → add_step.bc
      2. saw-spec-gen gen-rust-trait-stubs          → trait_stubs.ll
                                                      + interface_overrides.saw
      3. llvm-as trait_stubs.ll                     → trait_stubs.bc
      4. llvm-link the two                          → linked.bc
      5. Resolve mangled `add_step` symbol from the disassembly.
      6. Emit a thin SAW script that:
           * loads linked.bc
           * `include`s interface_overrides.saw (registers stub overrides
             and exposes the shared `Stepper_step_ret` symbolic Term)
           * verifies `add_step` against `add_step_spec x Stepper_step_ret`
      7. Run SAW.

.PARAMETER RustFile
    Source file: must define `pub trait Stepper { fn step(&self) -> u32; }`
    and `pub fn add_step(x: u32, s: &dyn Stepper) -> u32`.

.PARAMETER ExpectedResult
    "VERIFIED" (default) or "DISPROVED" — used by the harness to set
    its exit code so callers can script SAT vs UNSAT cases.
#>
param(
    [Parameter(Mandatory)][string]$RustFile,
    [string]$ExpectedResult = "VERIFIED"
)

$ErrorActionPreference = "Stop"

$here       = Split-Path -Parent $PSCommandPath
$rsAbs      = Resolve-Path $RustFile
$baseName   = [System.IO.Path]::GetFileNameWithoutExtension($rsAbs)
$outDir     = Join-Path $here "out_$baseName"
if (Test-Path $outDir) { Remove-Item -Recurse -Force $outDir }
New-Item -ItemType Directory -Path $outDir | Out-Null

# ── Tool discovery (mirrors verify-rust.ps1) ──────────────────────────
$repoRoot = Resolve-Path (Join-Path $here "..\..\..")
. (Join-Path $repoRoot 'scripts/discover-tools.ps1')
$tools = Find-SawSpecGenTools -RepoRoot $repoRoot
Assert-SawSpecGenTools -Tools $tools -Require @('LlvmDis', 'LlvmAs', 'LlvmLink', 'Saw', 'Rustc')
Add-SolverDirToPath -Tools $tools
$llvmDis    = $tools.LlvmDis
$llvmAs     = $tools.LlvmAs
$llvmLink   = $tools.LlvmLink
$rustc      = $tools.Rustc
$saw        = $tools.Saw
$llvmTarget = $tools.LlvmTarget

# ── Step 1: rustc ─────────────────────────────────────────────────────
Write-Host ""
Write-Host "═══ Step 1: rustc → bitcode ═══" -ForegroundColor Cyan
$rsBc = Join-Path $outDir "add_step.bc"
& $rustc `
    --emit=llvm-bc="$rsBc" `
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
    -o (Join-Path $outDir "add_step.out") `
    $rsAbs 2>&1 | Write-Host
if (-not (Test-Path $rsBc)) { Write-Error "rustc failed"; exit 1 }

# ── Step 2: saw-spec-gen → trait_stubs.ll + interface_overrides.saw ──
Write-Host ""
Write-Host "═══ Step 2: saw-spec-gen gen-rust-trait-stubs ═══" -ForegroundColor Cyan
$specGen = $tools.SpecGen
if (-not $specGen) {
    Push-Location $repoRoot
    & cargo build --release 2>&1 | Write-Host
    Pop-Location
    $specGen = (Find-SawSpecGenTools -RepoRoot $repoRoot).SpecGen
}
$schema = Join-Path $here "trait_schema.json"
& $specGen gen-rust-trait-stubs --schema $schema --output $outDir 2>&1 | Write-Host
$stubLl = Join-Path $outDir "trait_stubs.ll"
if (-not (Test-Path $stubLl)) { Write-Error "saw-spec-gen did not produce trait_stubs.ll"; exit 1 }

# ── Step 3: assemble trait_stubs.ll ───────────────────────────────────
Write-Host ""
Write-Host "═══ Step 3: llvm-as trait_stubs.ll ═══" -ForegroundColor Cyan
$stubBc = Join-Path $outDir "trait_stubs.bc"
& $llvmAs $stubLl -o $stubBc 2>&1 | Write-Host
if (-not (Test-Path $stubBc)) { Write-Error "llvm-as failed"; exit 1 }

# ── Step 4: llvm-link ─────────────────────────────────────────────────
Write-Host ""
Write-Host "═══ Step 4: llvm-link → linked.bc ═══" -ForegroundColor Cyan
$linkedBc = Join-Path $outDir "linked.bc"
& $llvmLink $rsBc $stubBc -o $linkedBc 2>&1 | Write-Host
if (-not (Test-Path $linkedBc)) { Write-Error "llvm-link failed"; exit 1 }
$linkedLl = Join-Path $outDir "linked.ll"
& $llvmDis $linkedBc -o $linkedLl 2>&1 | Write-Host

# ── Step 5: resolve mangled add_step ──────────────────────────────────
Write-Host ""
Write-Host "═══ Step 5: resolve mangled symbol ═══" -ForegroundColor Cyan
$defines = Select-String -Path $linkedLl -Pattern '^define\s.*?@([^\s(]+)\s*\(' -AllMatches |
    ForEach-Object { $_.Matches } |
    ForEach-Object { $_.Groups[1].Value }
$mangled = $defines | Where-Object { $_ -match '8add_step$' }
if (-not $mangled) {
    Write-Error "Could not find mangled add_step in $linkedLl"
    exit 1
}
$mangled = @($mangled)[0]
Write-Host "  → $mangled" -ForegroundColor Green

# ── Step 5: emit SAW script ───────────────────────────────────────────
Write-Host ""
Write-Host "═══ Step 6: emit verify_unknown_impl.saw ═══" -ForegroundColor Cyan
$cryDest = Join-Path $outDir "add_step_spec.cry"
Copy-Item (Join-Path $here "add_step_spec.cry") $cryDest -Force
$sawScript = Join-Path $outDir "verify_unknown_impl.saw"

@"
// Auto-generated by run_unknown_impl.ps1
// Proves: for any impl of `Stepper::step`, the function
//   add_step(x, s) == x + step(s)
//
// The havoc spec for Stepper::step (and the shared symbolic Term
// `Stepper_step_ret`) come from interface_overrides.saw — generated
// by `saw-spec-gen gen-rust-trait-stubs` from trait_schema.json.

m <- llvm_load_module "linked.bc";

import "add_step_spec.cry";
include "interface_overrides.saw";

let add_step_equiv_spec = do {
    x <- llvm_fresh_var "x" (llvm_int 32);
    data_ptr <- llvm_alloc (llvm_int 8);
    let vtable_ptr = llvm_global "__stubvtable_Stepper";

    llvm_execute_func [llvm_term x, data_ptr, vtable_ptr];

    llvm_return (llvm_term {{ add_step_spec x Stepper_step_ret }});
};

llvm_verify m "${mangled}" trait_overrides true add_step_equiv_spec z3;
print "VERIFIED";
"@ | Set-Content $sawScript -Encoding utf8
Write-Host "  → $sawScript" -ForegroundColor Green

# ── Step 7: run SAW ───────────────────────────────────────────────────
Write-Host ""
Write-Host "═══ Step 7: SAW ═══" -ForegroundColor Cyan
Push-Location $outDir
$sawOut = & $saw "verify_unknown_impl.saw" 2>&1 | Out-String
Pop-Location
Write-Host $sawOut

# ── Report ────────────────────────────────────────────────────────────
Write-Host ""
Write-Host "═══ Result ═══" -ForegroundColor Cyan
if ($sawOut -match "Counterexample" -or $sawOut -match "Proof failed") {
    $verdict = "DISPROVED"
    Write-Host "  RESULT: DISPROVED" -ForegroundColor Yellow
    Write-Host '    add_step is NOT extensionally equal to add_step_spec' -ForegroundColor Yellow
    Write-Host '    (i.e. the function does something other than just x + step(s))'
} elseif ($sawOut -match "Proof succeeded") {
    $verdict = "VERIFIED"
    Write-Host "  RESULT: VERIFIED" -ForegroundColor Green
    Write-Host '    add_step(x, &dyn Stepper) == x + step(s) for ANY impl of Stepper::step' -ForegroundColor Green
} else {
    $verdict = "UNKNOWN"
    Write-Host "  RESULT: UNKNOWN" -ForegroundColor Red
    Write-Host "    See SAW output above."
}

if ($verdict -ne $ExpectedResult) { exit 1 }
exit 0
