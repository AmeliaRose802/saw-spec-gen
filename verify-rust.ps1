<#
.SYNOPSIS
    SAW formal verification: prove a Rust function matches a hand-written
    Cryptol spec. No modifications to the user's .rs source required.

.DESCRIPTION
    Pipeline:
      1. rustc --emit=llvm-bc -C link-dead-code=yes  (preserves private fn)
      2. llvm-dis → scan symbols → resolve the mangled name for $Function
      3. Emit a tiny SAW script that llvm_verify's the mangled symbol
         against the Cryptol spec.
      4. Run SAW.

    The script auto-detects the function's u32→u32-style signature from the
    LLVM IR. For functions that touch globals, traits, or heap, this is the
    place we'll later invoke `saw-spec-gen from-mir-json` to generate
    overrides + stubs (same pattern verify.ps1 uses for C++ side).

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
# All tool discovery (rustc, llvm-dis, saw, z3) goes through the shared
# helper so verify.ps1, verify-rust.ps1 and the end-to-end test scripts agree on
# search order and cross-platform behaviour. Env vars or
# ~/.saw-spec-gen/env.ps1 override the defaults; run scripts/init.ps1
# (Windows) or scripts/init.sh (Linux/macOS) to populate that file.
$ScriptRoot = Split-Path -Parent $PSCommandPath
. (Join-Path $ScriptRoot 'scripts/discover-tools.ps1')
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

# ── Step 2: disassemble + locate mangled symbol for $Function ─────────────────
Write-Host ""
Write-Host "═══════════════════════════════════════════════════════" -ForegroundColor Cyan
Write-Host " Step 2: resolve mangled symbol for '$Function'" -ForegroundColor Cyan
Write-Host "═══════════════════════════════════════════════════════" -ForegroundColor Cyan
$llFile = Join-Path $OutputDir "$baseName.ll"
& $llvmDis $bcFile -o $llFile 2>&1 | Write-Host
if (-not (Test-Path $llFile)) { Write-Error "llvm-dis failed"; exit 1 }

# Pull every `define ... @<symbol>(<args>)` from the IR.
# Rust v0 mangling for a free function `fn name` at crate root looks like:
#   _RNvCs<hash>_<crate_name_len><crate_name><name_len><name>
# More generally, the function name appears as the FINAL length-prefixed
# segment of the symbol (before LLVM args). So we look for `<digits><name>`
# at the end of the mangled identifier.
$funcLen   = $Function.Length
$nameRegex = "${funcLen}${Function}`$"   # anchored at end of symbol

$defines = Select-String -Path $llFile -Pattern '^define\s.*?@([^\s(]+)\s*\(([^)]*)\)' -AllMatches |
    ForEach-Object { $_.Matches } |
    ForEach-Object {
        [PSCustomObject]@{
            Symbol = $_.Groups[1].Value
            Args   = $_.Groups[2].Value
            Line   = $_.Value
        }
    }

$candidates = @($defines | Where-Object { $_.Symbol -match $nameRegex })
if (-not $candidates) {
    Write-Error @"
Could not find a defined function whose mangled name ends with '${funcLen}${Function}'.
Defined symbols in $llFile :
$($defines | ForEach-Object { '  ' + $_.Symbol } | Out-String)
"@
    exit 1
}

# v0 mangling caveat — `nameRegex` is anchored at end-of-string, but v0 appends
# the *instantiating crate* (`Cs<hash>_<lenCrate><crate>`) to monomorphized
# generics. When the user's crate is named e.g. `add_one`, that suffix happens
# to BE `7add_one`, so `core::ptr::drop_in_place::<Box<u32>>` instantiated by
# the user's crate ALSO matches. We need to drop those.
#
# We do it indirectly via a signature filter: keep only candidates whose
# LLVM args+return type look like a plain integer function. Generic shims
# (drop glue, alloc thunks, vtable functions, Cell::set, …) almost always
# have `ptr` / `void` somewhere in their signature and get filtered out.
function Test-IntegerSignature($cand) {
    if ($cand.Args.Trim() -ne "") {
        foreach ($a in ($cand.Args -split ',')) {
            $t = ($a.Trim() -split '\s+')[0]
            if ($t -notmatch '^i\d+$') { return $false }
        }
    }
    $rl = Select-String -Path $llFile -Pattern "^define\s+(.+?)\s+@$([regex]::Escape($cand.Symbol))\s*\(" |
        ForEach-Object { $_.Matches[0].Groups[1].Value }
    if (-not $rl) { return $false }
    $rt = ($rl -split '\s+')[-1]
    return ($rt -match '^i\d+$')
}

$intCandidates = @($candidates | Where-Object { Test-IntegerSignature $_ })
if (-not $intCandidates) {
    Write-Error @"
Found symbol(s) ending in '${funcLen}${Function}' but none have an integer-only
(iN, ...) -> iN signature compatible with this verifier. Candidates were:
$($candidates | ForEach-Object { '  ' + $_.Symbol + '  (' + $_.Args + ')' } | Out-String)
"@
    exit 1
}

# Among matching-signature candidates, prefer the shortest. The user's own
# function is structurally shorter than any generic monomorphization their
# crate happens to instantiate (the latter carries the full instantiated
# path in the symbol).
$intCandidates = @($intCandidates | Sort-Object -Property @{ Expression = { $_.Symbol.Length } })
if ($intCandidates.Count -gt 1) {
    # If multiple distinct symbols remain after both filters, we're genuinely
    # ambiguous — refuse to pick rather than silently verify the wrong one.
    $shortestLen = $intCandidates[0].Symbol.Length
    $tied = @($intCandidates | Where-Object { $_.Symbol.Length -eq $shortestLen })
    if ($tied.Count -gt 1) {
        Write-Error @"
Ambiguous '${Function}': multiple matching symbols of equal length. Qualify
the function name (e.g. `mod::add_one`) or rename one. Tied candidates:
$($tied | ForEach-Object { '  ' + $_.Symbol } | Out-String)
"@
        exit 1
    }
    Write-Host "  NOTE: multiple matching-signature candidates; picked the shortest." -ForegroundColor Yellow
    $intCandidates | ForEach-Object { Write-Host "    candidate: $($_.Symbol)" -ForegroundColor Yellow }
}
$mangled = $intCandidates[0].Symbol
$rawArgs = $intCandidates[0].Args
Write-Host "  → mangled symbol: $mangled" -ForegroundColor Green
Write-Host "  → LLVM signature: ($rawArgs)" -ForegroundColor Green

# Read the return type by also pulling the bit between `define` and `@<symbol>`.
$retLine = Select-String -Path $llFile -Pattern "^define\s+(.+?)\s+@$([regex]::Escape($mangled))\s*\(" |
    ForEach-Object { $_.Matches[0].Groups[1].Value }
# Strip linkage / attribute modifiers — keep only the final type token
# (e.g. "hidden i32" → "i32", "internal noundef i32" → "i32").
$retType = ($retLine -split '\s+')[-1]
Write-Host "  → LLVM return type: $retType" -ForegroundColor Green

# Parse LLVM args (handles "i32 %x", "i32 noundef %x, i64 %y", "" for nullary).
# `@(...)` forces an array even when the pipeline emits a single element —
# without it PowerShell unwraps to a scalar string and indexing yields chars.
$argTokens = @()
if ($rawArgs.Trim() -ne "") {
    $argTokens = @($rawArgs -split ',' | ForEach-Object {
        $parts = $_.Trim() -split '\s+'
        # First token is the type (e.g. "i32"); subsequent tokens may be
        # parameter attributes (noundef, signext, …) or the name (%x).
        $parts[0]
    })
}

# Parse Rust source for the function signature so we can:
#   (a) show parameter names in the result block (instead of x0/x1/...)
#   (b) compile a harness that calls the function at the counterexample
#       inputs with the right Rust types (u8/u16/u32/u64/i*…).
$rustSrc       = Get-Content $RustFile -Raw
$rustParamNames = @()
$rustParamTypes = @()
$sigPattern = "(?ms)fn\s+$([regex]::Escape($Function))\s*\(([^)]*)\)"
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

# Build the SAW fresh-var + execute_func argument lists.
# Also remember each arg's bit width — needed later for Cryptol typed literals
# when evaluating the spec at counterexample inputs.
$freshDecls = @()
$execArgs   = @()
$cryArgs    = @()
$argBits    = @()
for ($i = 0; $i -lt $argTokens.Count; $i++) {
    $t = $argTokens[$i]
    if ($t -notmatch '^i(\d+)$') {
        Write-Error "Unsupported LLVM argument type '$t' for $Function. Only iN integers are supported in this script."
        exit 1
    }
    $bits = [int]$Matches[1]
    $argBits += $bits
    $vname = "x$i"
    $freshDecls += "    $vname <- llvm_fresh_var `"$vname`" (llvm_int $bits);"
    $execArgs   += "llvm_term $vname"
    $cryArgs    += $vname
}
if ($retType -notmatch '^i(\d+)$') {
    Write-Error "Unsupported LLVM return type '$retType' for $Function. Only iN integers are supported in this script."
    exit 1
}
# (We don't actually need the return bit width for the SAW spec; Cryptol
#  infers it from the spec function. We just guard against non-integers.)

$execArgsExpr = '[' + ($execArgs -join ', ') + ']'
$cryCall = if ($cryArgs.Count -eq 0) { $CryptolFn } else { "$CryptolFn " + ($cryArgs -join ' ') }

# Scan the .ll for mutable global *definitions* (not external decls and not
# the MSVC `__imp_*` import thunks). SAW needs `llvm_alloc_global` for each
# one before symbolic execution may read or write through it; without this
# we get "Global symbol ... has no associated allocation". `llvm_alloc_global`
# only *allocates* the cell — it does NOT seed it with the LLVM initializer,
# so a subsequent `load` (especially from a callee on the other side of a
# function-call boundary, where SAW has no chance to first observe a write)
# fails with "Error during memory load". We therefore also emit
# `llvm_points_to (llvm_global G) (llvm_global_initializer G)` so SAW sees
# `static FOO: i32 = 7;` as 7 unless the function rewrites it.
$globalAllocs = @()
$llLines = Get-Content $llFile
foreach ($ln in $llLines) {
    # Match: @<name> = <linkage-and-attr-words>* global <rest>
    # (deliberately not matching `constant` — Rust `const`s are inlined and
    # immutable constants don't need explicit allocation.)
    if ($ln -match '^@(?<name>"[^"]*"|\S+?)\s*=\s*(?:[A-Za-z_][\w]*\s+)*global\s') {
        $gname = $Matches['name']
        # Strip surrounding quotes if any (LLVM quotes names containing
        # special chars like the leading \01 on MSVC import thunks).
        if ($gname.StartsWith('"') -and $gname.EndsWith('"')) {
            $gname = $gname.Substring(1, $gname.Length - 2)
        }
        # Skip MSVC DLL import thunks — they're indirection metadata, not
        # data SAW needs to model.
        if ($gname -match '__imp_') { continue }
        $globalAllocs += $gname
    }
}
if ($globalAllocs.Count -gt 0) {
    Write-Host "  → allocating $($globalAllocs.Count) mutable global(s):" -ForegroundColor Green
    foreach ($g in $globalAllocs) { Write-Host "      $g" -ForegroundColor Green }
}

# ── Step 3: emit verify_rust.saw ──────────────────────────────────────────────
Write-Host ""
Write-Host "═══════════════════════════════════════════════════════" -ForegroundColor Cyan
Write-Host " Step 3: emit verify_rust.saw" -ForegroundColor Cyan
Write-Host "═══════════════════════════════════════════════════════" -ForegroundColor Cyan
$cryDest = Join-Path $OutputDir ([System.IO.Path]::GetFileName($CryptolSpec))
Copy-Item $CryptolSpec $cryDest -Force
$cryName    = [System.IO.Path]::GetFileName($cryDest)
$bcName     = [System.IO.Path]::GetFileName($bcFile)
$sawScript  = Join-Path $OutputDir "verify_rust.saw"

# NOTE: When the Rust target grows globals / dyn-trait / heap allocations,
# this is the section where we'll invoke `saw-spec-gen from-mir-json` to
# generate overrides + stubs and `include` them here — exactly the way
# verify.ps1 calls `saw-spec-gen gen-verify` for the C++ side today.
$freshBlock = ($freshDecls -join "`n")
$globalAllocBlock = ""
if ($globalAllocs.Count -gt 0) {
    $allocLines = $globalAllocs | ForEach-Object {
        "    llvm_alloc_global `"$_`";`n    llvm_points_to (llvm_global `"$_`") (llvm_global_initializer `"$_`");"
    }
    $globalAllocBlock = ($allocLines -join "`n") + "`n"
}
@"
// Auto-generated by verify-rust.ps1
// Prove that the Rust ${Function} (compiled to LLVM bitcode by rustc) is
// extensionally equal to the Cryptol spec ${CryptolFn}.
//
// Mangled symbol resolved from the .ll: ${mangled}

m <- llvm_load_module "${bcName}";

import "${cryName}";

let ${Function}_equiv_spec = do {
${globalAllocBlock}${freshBlock}
    llvm_execute_func ${execArgsExpr};
    llvm_return (llvm_term {{ ${cryCall} }});
};

llvm_verify m "${mangled}" [] true ${Function}_equiv_spec z3;
print "VERIFIED";
"@ | Set-Content $sawScript -Encoding utf8
Write-Host "  → $sawScript" -ForegroundColor Green

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
            $idx = [int]($Matches[1].Substring(1))
            $cexPairs += [PSCustomObject]@{
                Index = $idx
                Name  = if ($idx -lt $rustParamNames.Count) { $rustParamNames[$idx] } else { $Matches[1] }
                Value = [uint64]$Matches[2]
                Bits  = if ($idx -lt $argBits.Count) { $argBits[$idx] } else { 32 }
            }
        }
    }
    $cexPairs = $cexPairs | Sort-Object Index

    # ── Evaluate Cryptol spec at counterexample inputs ──────────────────────
    $expectedVal = $null
    if ($cexPairs.Count -gt 0) {
        $cryptolArgs = ($cexPairs | ForEach-Object { "($($_.Value) : [$($_.Bits)])" }) -join " "
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
        $harness  = Join-Path $OutputDir "_harness.rs"
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

    # ── Poison / UB heuristic ─────────────────────────────────────────────
    # If Cryptol and a concrete Rust run agree at the counterexample, the
    # failure is almost certainly an LLVM `nsw`/`nuw`/`inbounds`/`unchecked_*`
    # poison, not a logic mismatch (SAW: poison ≠ any concrete value).
    if ($expectedVal -and $actualVal -and $expectedVal -eq $actualVal) {
        Write-Host @'
  NOTE: Cryptol and Rust agree at the counterexample. This is the
        signature of an LLVM UB / poison failure (nsw, nuw, inbounds,
        unchecked_*, get_unchecked). Inspect the .ll for the flag and
        switch to wrapping_* / checked variants or fix the UB.

'@ -ForegroundColor Yellow
    }

    Write-ResultJson "DISPROVED" @($cexPairs) $expectedVal $actualVal
    exit 1
} elseif ($sawOut -match "VERIFIED" -and $sawOut -match "Proof succeeded") {
    Write-Host ""
    Write-Host "  RESULT: VERIFIED" -ForegroundColor Green
    Write-Host "    Rust $Function  ≡  $CryptolFn   (proved by z3 on all inputs)" -ForegroundColor Green
    Write-Host ""
    Write-ResultJson "VERIFIED" @() $null $null
    exit 0
} else {
    Write-Host ""
    Write-Host "  RESULT: UNKNOWN" -ForegroundColor Yellow
    Write-Host "    SAW did not produce a clear sat/unsat result." -ForegroundColor Yellow
    Write-Host ""
    Write-ResultJson "UNKNOWN" @() $null $null
    exit 2
}
