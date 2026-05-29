<#
.SYNOPSIS
    Async-Rust verification demo: compile, generate adversarial
    specs with `saw-spec-gen`, and prove the awaited primitive
    against the Cryptol spec with SAW.

.DESCRIPTION
    This is the async counterpart to verify-rust.ps1. The vanilla
    rust-equivalence pipeline only supports `iN -> iN` signatures,
    but `async fn add_one(x: u32) -> u32` lowers to a coroutine
    state machine — `add_one` itself becomes `(sret([12 x i8]), i32)
    -> void` (it just constructs the future). SAW cannot drive an
    executor.

    The demo instead does the compositional thing:
      1. rustc emits the .bc with the coroutine + every Future
         impl as separate LLVM functions.
      2. `saw-spec-gen from-llvm-ir` walks every symbol and writes
         an adversarial havoc spec for each (overrides for the
         coroutine resume, IntoFuture, Pin::new_unchecked, etc.).
      3. A hand-written SAW script proves the leaf future
         `<ReadyU32 as Future>::poll(ReadyU32(v), _) == Ready(v)`,
         which is the only async-specific obligation in the body
         of `add_one`. Composed with the (adversarial) override of
         the coroutine, this gives `add_one(x).await == x + 1`.

.EXAMPLE
    pwsh demo/async_rust/run_async_demo.ps1
#>

[CmdletBinding()]
param()

$ErrorActionPreference = "Stop"

$here       = Split-Path -Parent $MyInvocation.MyCommand.Path
$repoRoot   = Resolve-Path (Join-Path $here "..\..")
$rustFile   = Join-Path $here "add_one_verified.rs"
$cryFile    = Join-Path $here "add_one_spec.cry"
$outDir     = Join-Path $here "out_async_demo"
$specsDir   = Join-Path $outDir "specs"
$bcFile     = Join-Path $outDir "add_one_verified.bc"
$llFile     = Join-Path $outDir "add_one_verified.ll"
$sawScript  = Join-Path $outDir "verify_async.saw"

# Tool discovery (same convention as verify-rust.ps1).
. (Join-Path $repoRoot 'scripts/discover-tools.ps1')
$tools = Find-SawSpecGenTools -RepoRoot $repoRoot
Assert-SawSpecGenTools -Tools $tools -Require @('LlvmDis', 'Saw', 'Rustc')
Add-SolverDirToPath -Tools $tools
$llvmBin    = $tools.LlvmBin
$llvmDis    = $tools.LlvmDis
$saw        = $tools.Saw
$llvmTarget = $tools.LlvmTarget

$specGen = $tools.SpecGen
if (-not $specGen) {
    Write-Host "Building saw-spec-gen (release)..." -ForegroundColor Yellow
    Push-Location $repoRoot
    cargo build --release | Out-Null
    Pop-Location
    $tools = Find-SawSpecGenTools -RepoRoot $repoRoot
    $specGen = $tools.SpecGen
}

# ── Fresh output dir ─────────────────────────────────────────────
if (Test-Path $outDir) { Remove-Item -Recurse -Force $outDir }
New-Item -ItemType Directory $outDir | Out-Null

# ── Step 1: rustc → LLVM bitcode ─────────────────────────────────
Write-Host ""
Write-Host "═══════════════════════════════════════════════════════" -ForegroundColor Cyan
Write-Host " Step 1: rustc add_one_verified.rs → bitcode (async lowering)" -ForegroundColor Cyan
Write-Host "═══════════════════════════════════════════════════════" -ForegroundColor Cyan
& rustc `
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
    -o (Join-Path $outDir "add_one_verified.out") `
    $rustFile
if (-not (Test-Path $bcFile)) { Write-Error "rustc failed"; exit 1 }
& $llvmDis $bcFile -o $llFile | Out-Null
Write-Host "  → $bcFile" -ForegroundColor Green

# ── Step 1.5: poison → undef rewrite ────────────────────────────
# rustc emits `insertvalue { i32 0, i32 poison }, i32 %x, 1` style
# aggregates for `Poll<u32>` returns. Crucible's llvmExtensionEval
# panics with "Attempting to evaluate poison value" when it
# materialises the partial-aggregate constant. `undef` is
# semantically weaker (commutes cleanly with insertvalue) and
# Crucible accepts it. The rewrite is a no-op when the IR has no
# `poison` tokens, so it's safe to run unconditionally.
$llvmAs = Join-Path $llvmBin "llvm-as.exe"
& $specGen patch-llvm-ir `
    --input  $llFile `
    --output $llFile `
    --poison-to-undef 2>&1 | Write-Host
if ($LASTEXITCODE -ne 0) { Write-Error "patch-llvm-ir failed"; exit 1 }
& $llvmAs $llFile -o $bcFile 2>&1
if ($LASTEXITCODE -ne 0) { Write-Error "llvm-as (post-patch) failed"; exit 1 }

# Show the async-relevant symbols rustc emitted (the proof of the
# coroutine lowering: there's a separate resume fn and ReadyU32::poll).
Write-Host ""
Write-Host "  Async-relevant LLVM symbols (after rustc lowering):" -ForegroundColor Yellow
Select-String -Path $llFile -Pattern '^define ' | ForEach-Object {
    if ($_.Line -match '@([^\s(]+)') {
        $sym = $Matches[1]
        $human =
            if     ($sym -match '_RNvCs[^_]+_[0-9]+\S+?7add_one$')                { "  add_one (future constructor)" }
            elseif ($sym -match '_RNCNvCs[^_]+_[0-9]+\S+?7add_one0')              { "  add_one coroutine resume" }
            elseif ($sym -match '8ReadyU32\S+Future4poll')                        { "  <ReadyU32 as Future>::poll" }
            elseif ($sym -match 'panic_const_async_fn_resumed')                   { "  panic: async fn resumed after completion" }
            else { $null }
        if ($human) { Write-Host "    $human" -ForegroundColor Yellow }
    }
}

# ── Step 2: saw-spec-gen → havoc specs for every async symbol ───
Write-Host ""
Write-Host "═══════════════════════════════════════════════════════" -ForegroundColor Cyan
Write-Host " Step 2: saw-spec-gen from-llvm-ir → adversarial specs" -ForegroundColor Cyan
Write-Host "═══════════════════════════════════════════════════════" -ForegroundColor Cyan
& $specGen from-llvm-ir --input $llFile --output $specsDir
Write-Host "  Generated specs:" -ForegroundColor Green
Get-ChildItem $specsDir -Filter *_auto_spec.saw | ForEach-Object {
    Write-Host "    $($_.Name)" -ForegroundColor DarkGray
}

# ── Step 3: emit verify_async.saw (REAL proof — targets the
#           coroutine resume function directly, not the leaf
#           Future impl). ─────────────────────────────────────────
Write-Host ""
Write-Host "═══════════════════════════════════════════════════════" -ForegroundColor Cyan
Write-Host " Step 3: emit verify_async.saw (proves the coroutine resume)" -ForegroundColor Cyan
Write-Host "═══════════════════════════════════════════════════════" -ForegroundColor Cyan

# Resolve the mangled symbol for the coroutine resume function. This
# is the LLVM function where the actual arithmetic (`y + 1`) lives —
# rustc emits it alongside the user-visible `add_one` (which is just
# a constructor for the coroutine state). Symbol form is
# `_RNCNv<crate-hash>_<crate-name-len><crate-name>7add_one0<…>`.
$resumeSym = (Select-String -Path $llFile -Pattern '^define .* @(_RNCNvCs\S+?7add_one0\S+)\(' |
                ForEach-Object { $_.Matches[0].Groups[1].Value } |
                Select-Object -First 1)
if (-not $resumeSym) { Write-Error "Could not find async add_one coroutine resume in $llFile"; exit 1 }
Write-Host "  add_one coroutine resume = $resumeSym" -ForegroundColor Green

Copy-Item $cryFile $outDir -Force

@"
// Auto-generated by run_async_demo.ps1.
//
// REAL proof: targets the coroutine resume function — the LLVM
// function where the arithmetic actually lives after rustc lowers
// `async fn add_one`. No overrides, no skipped functions: SAW
// symbolically executes the resume, which calls into_future and
// <ReadyU32 as Future>::poll using their real bodies from this
// bitcode, then proves the result matches add_one_spec.
//
// Coroutine state layout (rustc lowering of `async fn add_one`):
//   offset 0..4 : i32 x             (captured argument)
//   offset 4..8 : i32 polled_value  (filled in by into_future)
//   offset 8    : i8  state_tag     (0 = initial entry)
//
// Modelled as `llvm_array 3 (llvm_int 32)` — 12 bytes, 4-aligned,
// matching rustc's `alloca align 4`. state_tag = 0 selects the
// initial-entry path.

m <- llvm_load_module "add_one_verified.bc";

import "add_one_spec.cry";

let add_one_resume_spec = do {
    x      <- llvm_fresh_var "x"      (llvm_int 32);
    polled <- llvm_fresh_var "polled" (llvm_int 32);

    state_ptr <- llvm_alloc (llvm_array 3 (llvm_int 32));
    llvm_points_to state_ptr
        (llvm_array_value [
            llvm_term x,
            llvm_term polled,
            llvm_term {{ 0 : [32] }}
        ]);

    // Task context — opaque, never dereferenced, just needs to be a
    // valid pointer.
    cx_ptr <- llvm_alloc (llvm_array 8 (llvm_int 8));

    llvm_execute_func [state_ptr, cx_ptr];

    // Return type: Poll<u32> as { i32 disc, i32 val }.
    // Ready(x + 1) == (0, x + 1).
    llvm_return (llvm_term {{ (0 : [32], add_one_spec x) }});
};

print "Verifying async add_one coroutine resume == add_one_spec ...";
llvm_verify m "$resumeSym" [] true add_one_resume_spec z3;

print "VERIFIED: async add_one body actually computes x + 1";
"@ | Set-Content $sawScript -Encoding ascii
Write-Host "  → $sawScript" -ForegroundColor Green

# ── Step 4: run SAW ──────────────────────────────────────────────
Write-Host ""
Write-Host "═══════════════════════════════════════════════════════" -ForegroundColor Cyan
Write-Host " Step 4: SAW verification" -ForegroundColor Cyan
Write-Host "═══════════════════════════════════════════════════════" -ForegroundColor Cyan
Push-Location $outDir
$sawOut = & $saw "verify_async.saw" 2>&1 | Out-String
Pop-Location
Write-Host $sawOut

# ── Verdict ──────────────────────────────────────────────────────
Write-Host ""
Write-Host "═══════════════════════════════════════════════════════" -ForegroundColor Cyan
Write-Host " Result" -ForegroundColor Cyan
Write-Host "═══════════════════════════════════════════════════════" -ForegroundColor Cyan
if ($sawOut -match "Proof succeeded") {
    Write-Host "  RESULT: VERIFIED" -ForegroundColor Green
    Write-Host "    async add_one coroutine resume  ≡  add_one_spec" -ForegroundColor Green
    Write-Host "    => async add_one(x).await        ≡  x + 1" -ForegroundColor Green
    exit 0
} elseif ($sawOut -match "Counterexample") {
    Write-Host "  RESULT: DISPROVED" -ForegroundColor Red
    if ($sawOut -match 'Actual term:\s*\r?\n\s*(\([^)]+\))') {
        Write-Host "    Actual:   $($Matches[1])" -ForegroundColor Red
    }
    if ($sawOut -match 'x:\s*(\d+)') {
        Write-Host "    counterexample x = $($Matches[1])" -ForegroundColor Red
    }
    exit 1
} else {
    Write-Host "  RESULT: UNKNOWN" -ForegroundColor Yellow
    exit 2
}
