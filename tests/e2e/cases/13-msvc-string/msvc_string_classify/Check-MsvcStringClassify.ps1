<#
.SYNOPSIS
    E2E regression test: MSVC-mangled basic_string methods receive functional
    SAW overrides, not unoptimized havoc specs.

.DESCRIPTION
    Exercises three bug fixes from the MSVC STL override PR:

    (1) classify_basic_string_msvc — MSVC-decorated names (??0, ??1, ?resize@,
        ?size@) are now matched and emit functional specs.
    (2) is_basic_string_alias — MSVC full-template struct names containing
        "char_traits" are no longer excluded from layout discovery.
    (3) The combination proves the full pipeline: resize(x+1); size() == x+1.

    The test assembles the pre-written LLVM IR (which mimics clang output for
    -target x86_64-pc-windows-msvc), runs gen-verify, inspects the generated
    SAW script for the expected functional override annotations, and then runs
    SAW to confirm actual verification succeeds.

    Expected RESULT: VERIFIED
#>
param()
$ErrorActionPreference = "Stop"

$caseDir  = Split-Path -Parent $PSCommandPath
$RepoRoot = Resolve-Path (Join-Path $caseDir '../../../../..')

. (Join-Path $RepoRoot 'scripts/discover-tools.ps1')
$specGen = Build-SawSpecGen -RepoRoot $RepoRoot
$tools   = Find-SawSpecGenTools -RepoRoot $RepoRoot
Assert-SawSpecGenTools -Tools $tools -Require @('LlvmAs', 'Saw')

$llvmAs = $tools.LlvmAs
$saw    = $tools.Saw

$llFile  = Join-Path $caseDir 'msvc_resize_size.ll'
$cryFile = Join-Path $caseDir 'add_one_spec.cry'
$outDir  = Join-Path $caseDir 'out_msvc_classify'

if (Test-Path $outDir) { Remove-Item -Recurse -Force $outDir }
New-Item -ItemType Directory -Path $outDir -Force | Out-Null

# ── Assemble LLVM IR → bitcode ────────────────────────────────────────
$bcFile = Join-Path $outDir 'msvc_resize_size.bc'
& $llvmAs $llFile -o $bcFile 2>&1 | Write-Host
if ($LASTEXITCODE -ne 0) {
    Write-Error "llvm-as failed to assemble msvc_resize_size.ll"
    Write-Host "RESULT: DISPROVED"
    exit 1
}

# ── Minimal clang AST JSON for `add_one(uint32_t) -> uint32_t` ────────
$astFile = Join-Path $outDir 'ast.json'
@'
{"id":"0x1","kind":"TranslationUnitDecl","loc":{},"range":{"begin":{},"end":{}},"inner":[{"id":"0x2","kind":"FunctionDecl","loc":{"offset":0,"file":"test.cpp","line":1,"col":1,"tokLen":7},"range":{"begin":{},"end":{}},"name":"add_one","mangledName":"add_one","type":{"qualType":"unsigned int (unsigned int)"},"inner":[{"id":"0x3","kind":"ParmVarDecl","loc":{},"range":{"begin":{},"end":{}},"name":"x","type":{"qualType":"unsigned int"}}]}]}
'@ | Set-Content -Path $astFile -Encoding utf8

# ── Run gen-verify ────────────────────────────────────────────────────
& $specGen gen-verify `
    --ast          $astFile `
    --bitcode      $bcFile `
    --llvm-ir      $llFile `
    --cryptol-spec $cryFile `
    --cryptol-fn   add_one_spec `
    --function     add_one `
    --output       $outDir 2>&1 | Write-Host

if ($LASTEXITCODE -ne 0) {
    Write-Error "gen-verify failed"
    Write-Host "RESULT: DISPROVED"
    exit 1
}

# ── Inspect generated SAW script for functional MSVC overrides ────────
$sawScript = Join-Path $outDir 'verify.saw'
if (-not (Test-Path $sawScript)) {
    Write-Error "verify.saw was not generated"
    Write-Host "RESULT: DISPROVED"
    exit 1
}

$sawText = Get-Content $sawScript -Raw

$checks = @(
    @{ Pattern = '\[stl-functional: BasicStringCtorDefault\]'; Desc = 'MSVC ctor ??0 classified as BasicStringCtorDefault' }
    @{ Pattern = '\[stl-functional: BasicStringDtor\]';        Desc = 'MSVC dtor ??1 classified as BasicStringDtor' }
    @{ Pattern = '\[stl-functional: BasicStringResize\]';      Desc = 'MSVC ?resize@ classified as BasicStringResize' }
    @{ Pattern = '\[stl-functional: BasicStringSize\]';        Desc = 'MSVC ?size@ classified as BasicStringSize' }
    @{ Pattern = 'llvm_alias "class\.std::basic_string<char,struct std::char_traits'; Desc = 'MSVC struct alias (with char_traits) accepted by is_basic_string_alias' }
    @{ Pattern = 'llvm_elem s 1';                             Desc = 'size field accessed via llvm_elem s 1' }
)

$allPassed = $true
foreach ($chk in $checks) {
    if ($sawText -notmatch $chk.Pattern) {
        Write-Host "FAIL: $($chk.Desc)"
        $allPassed = $false
    } else {
        Write-Host "PASS: $($chk.Desc)"
    }
}

if (-not $allPassed) {
    Write-Host "Generated SAW script does not contain expected MSVC overrides."
    Write-Host "Script content:"
    Write-Host $sawText
    Write-Host "RESULT: DISPROVED"
    exit 1
}

Write-Host "All spec-generation checks passed; running SAW verification."

# ── Run SAW ────────────────────────────────────────────────────────────
Push-Location $outDir
try {
    $sawOut = & $saw verify.saw 2>&1 | Out-String
    Write-Host $sawOut
} finally {
    Pop-Location
}

if ($sawOut -match 'PROVED add_one' -or $sawOut -match '=== VERIFIED:') {
    Write-Host "RESULT: VERIFIED"
} else {
    Write-Host "RESULT: DISPROVED"
}
