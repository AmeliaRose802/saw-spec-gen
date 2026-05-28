<#
.SYNOPSIS
    Cross-platform discovery of the tools saw-spec-gen needs to build,
    verify and run demos: clang/LLVM, SAW, SMT solvers, rustc, and the
    saw-spec-gen binary itself.

.DESCRIPTION
    This file is *dot-sourceable*. Other scripts use it like:

        . "$PSScriptRoot/scripts/discover-tools.ps1"
        $tools = Find-SawSpecGenTools -RepoRoot $PSScriptRoot

    Discovery order for every tool:
      1. Environment variables (SAW_SPEC_GEN_*).
      2. User config file at  $HOME/.saw-spec-gen/env.ps1
         (dot-sourced if it exists; should set the SAW_SPEC_GEN_* vars).
      3. PATH lookup.
      4. A short list of platform-specific install locations that
         scripts/init.ps1 and scripts/init.sh tend to drop tools into.

    None of these need to succeed — the returned hashtable contains
    `$null` for anything that wasn't found, and the caller decides how
    important that is.

.OUTPUTS
    Hashtable with keys:
      Platform      Windows | Linux | MacOS
      ExeExt        ".exe" on Windows, "" elsewhere
      LlvmTarget    default rustc/clang target tuple for this OS
      LlvmBin       directory containing clang+friends
      Clang         full path to clang(.exe)
      LlvmAs        full path to llvm-as(.exe)
      LlvmDis       full path to llvm-dis(.exe)
      LlvmLink      full path to llvm-link(.exe)
      CxxFilt       demangler (undname.exe on Windows, c++filt elsewhere)
      Saw           full path to saw(.exe)
      SolverDir     directory containing z3 / yices / cvc4
      Rustc         full path to rustc(.exe) — from rustup if available
      SpecGen       full path to saw-spec-gen(.exe) under target/release
#>

function Get-SawSpecGenPlatform {
    if ($IsWindows -or ($null -eq $IsWindows -and $env:OS -eq 'Windows_NT')) { return 'Windows' }
    if ($IsMacOS)   { return 'MacOS' }
    if ($IsLinux)   { return 'Linux' }
    # PowerShell 5.1 on Windows — $IsWindows is undefined but $env:OS is set.
    return 'Windows'
}

function Get-SawSpecGenLlvmTarget([string]$Platform) {
    switch ($Platform) {
        'Windows' { 'x86_64-pc-windows-msvc' }
        'MacOS'   {
            if ((uname -m 2>$null) -eq 'arm64') { 'aarch64-apple-darwin' }
            else { 'x86_64-apple-darwin' }
        }
        'Linux'   { 'x86_64-unknown-linux-gnu' }
    }
}

function Get-SawSpecGenUserConfigPath {
    $userHome = if ($env:HOME) { $env:HOME } else { $env:USERPROFILE }
    if (-not $userHome) { return $null }
    return (Join-Path $userHome '.saw-spec-gen/env.ps1')
}

function Find-FirstExisting([string[]]$paths) {
    foreach ($p in $paths) {
        if ($p -and (Test-Path -LiteralPath $p)) { return (Resolve-Path -LiteralPath $p).Path }
    }
    return $null
}

function Find-OnPath([string]$name) {
    $cmd = Get-Command $name -ErrorAction SilentlyContinue
    if ($cmd) { return $cmd.Source }
    return $null
}

function Find-SawSpecGenTools {
    [CmdletBinding()]
    param(
        [Parameter(Mandatory)][string]$RepoRoot
    )

    # ── 0. Pick up user env file (sets $env:SAW_SPEC_GEN_* if present). ───
    $userEnv = Get-SawSpecGenUserConfigPath
    if ($userEnv -and (Test-Path -LiteralPath $userEnv)) {
        . $userEnv
    }

    $platform   = Get-SawSpecGenPlatform
    $exeExt     = if ($platform -eq 'Windows') { '.exe' } else { '' }
    $llvmTarget = if ($env:SAW_SPEC_GEN_LLVM_TARGET) {
        $env:SAW_SPEC_GEN_LLVM_TARGET
    } else {
        Get-SawSpecGenLlvmTarget $platform
    }

    # ── 1. LLVM bin directory. ────────────────────────────────────────────
    $llvmBinCandidates = @()
    if ($env:SAW_SPEC_GEN_LLVM_BIN) { $llvmBinCandidates += $env:SAW_SPEC_GEN_LLVM_BIN }
    $pathClang = Find-OnPath ("clang" + $exeExt)
    if ($pathClang) { $llvmBinCandidates += (Split-Path -Parent $pathClang) }
    switch ($platform) {
        'Windows' {
            $llvmBinCandidates += @(
                'C:\Program Files\LLVM\bin'
                "$env:LOCALAPPDATA\Programs\LLVM\bin"
                "$env:USERPROFILE\.saw-spec-gen\llvm\bin"
            )
        }
        'MacOS' {
            $llvmBinCandidates += @(
                '/opt/homebrew/opt/llvm/bin'
                '/usr/local/opt/llvm/bin'
                "$HOME/.saw-spec-gen/llvm/bin"
            )
        }
        'Linux' {
            $llvmBinCandidates += @(
                '/usr/lib/llvm-20/bin'
                '/usr/lib/llvm-19/bin'
                '/usr/lib/llvm-18/bin'
                '/usr/local/lib/llvm/bin'
                "$HOME/.saw-spec-gen/llvm/bin"
            )
        }
    }
    $llvmBin = $null
    foreach ($dir in $llvmBinCandidates) {
        if ($dir -and (Test-Path -LiteralPath (Join-Path $dir ("clang" + $exeExt)))) {
            $llvmBin = (Resolve-Path -LiteralPath $dir).Path
            break
        }
    }

    function _JoinTool([string]$dir, [string]$name) {
        if (-not $dir) { return $null }
        $p = Join-Path $dir ($name + $exeExt)
        if (Test-Path -LiteralPath $p) { return $p }
        return $null
    }

    $clang   = _JoinTool $llvmBin 'clang'
    $llvmAs  = _JoinTool $llvmBin 'llvm-as'
    $llvmDis = _JoinTool $llvmBin 'llvm-dis'
    $llvmLink= _JoinTool $llvmBin 'llvm-link'

    # ── 2. SAW. ───────────────────────────────────────────────────────────
    $sawCandidates = @()
    if ($env:SAW_SPEC_GEN_SAW) { $sawCandidates += $env:SAW_SPEC_GEN_SAW }
    $pathSaw = Find-OnPath ("saw" + $exeExt)
    if ($pathSaw) { $sawCandidates += $pathSaw }
    $sawCandidates += @(
        "$env:USERPROFILE\.saw-spec-gen\saw\bin\saw$exeExt"
        "$HOME/.saw-spec-gen/saw/bin/saw$exeExt"
    )
    $saw = Find-FirstExisting $sawCandidates

    # ── 3. SMT solver bin directory (z3, yices, cvc4). ────────────────────
    $solverCandidates = @()
    if ($env:SAW_SPEC_GEN_SOLVER_BIN) { $solverCandidates += $env:SAW_SPEC_GEN_SOLVER_BIN }
    # SAW's "with-solvers" bundle ships z3 next to saw.exe.
    if ($saw) { $solverCandidates += (Split-Path -Parent $saw) }
    $pathZ3 = Find-OnPath ("z3" + $exeExt)
    if ($pathZ3) { $solverCandidates += (Split-Path -Parent $pathZ3) }
    $solverCandidates += @(
        "$env:USERPROFILE\.saw-spec-gen\saw\bin"
        "$HOME/.saw-spec-gen/saw/bin"
    )
    $solverDir = $null
    foreach ($dir in $solverCandidates) {
        if ($dir -and (Test-Path -LiteralPath (Join-Path $dir ("z3" + $exeExt)))) {
            $solverDir = (Resolve-Path -LiteralPath $dir).Path
            break
        }
    }

    # ── 4. C++ demangler. ────────────────────────────────────────────────
    $cxxFilt = $null
    if ($platform -eq 'Windows') {
        # Microsoft's undname.exe ships inside Visual Studio. We probe the
        # standard install layout; if it isn't there we just leave it $null
        # and the caller falls back to the mangled name.
        $vsRoot = 'C:\Program Files (x86)\Microsoft Visual Studio'
        if (Test-Path -LiteralPath $vsRoot) {
            $cxxFilt = (Get-ChildItem -LiteralPath $vsRoot -Recurse -Filter 'undname.exe' -ErrorAction SilentlyContinue |
                Where-Object { $_.FullName -match 'Hostx64\\x64' } |
                Select-Object -First 1).FullName
        }
    } else {
        $cxxFilt = Find-OnPath 'c++filt'
        if (-not $cxxFilt) { $cxxFilt = Find-OnPath 'llvm-cxxfilt' }
    }

    # ── 5. rustc. ────────────────────────────────────────────────────────
    $rustc = $null
    if ($env:SAW_SPEC_GEN_RUSTC) { $rustc = $env:SAW_SPEC_GEN_RUSTC }
    if (-not $rustc) { $rustc = Find-OnPath ("rustc" + $exeExt) }

    # ── 6. saw-spec-gen itself (built from $RepoRoot). ───────────────────
    $specGen = Join-Path $RepoRoot ("target/release/saw-spec-gen" + $exeExt)
    if (-not (Test-Path -LiteralPath $specGen)) {
        $alt = Join-Path $RepoRoot ("target/debug/saw-spec-gen" + $exeExt)
        if (Test-Path -LiteralPath $alt) { $specGen = $alt } else { $specGen = $null }
    }

    return @{
        Platform   = $platform
        ExeExt     = $exeExt
        LlvmTarget = $llvmTarget
        LlvmBin    = $llvmBin
        Clang      = $clang
        LlvmAs     = $llvmAs
        LlvmDis    = $llvmDis
        LlvmLink   = $llvmLink
        CxxFilt    = $cxxFilt
        Saw        = $saw
        SolverDir  = $solverDir
        Rustc      = $rustc
        SpecGen    = $specGen
    }
}

function Assert-SawSpecGenTools {
    [CmdletBinding()]
    param(
        [Parameter(Mandatory)][hashtable]$Tools,
        [string[]]$Require = @('Clang', 'LlvmAs', 'Saw', 'SpecGen')
    )
    $missing = @()
    foreach ($k in $Require) {
        if (-not $Tools[$k]) { $missing += $k }
    }
    if ($missing.Count -gt 0) {
        $userHome = if ($env:HOME) { $env:HOME } else { $env:USERPROFILE }
        $init = if ($Tools.Platform -eq 'Windows') { 'scripts\init.ps1' } else { 'scripts/init.sh' }
        Write-Error @"
saw-spec-gen could not find the following tool(s): $($missing -join ', ')

Run the installer once to set them up:
    $init

Or set the matching environment variables manually (see
$userHome/.saw-spec-gen/env.ps1 for the format the installer writes).
"@
        exit 1
    }
}

function Add-SolverDirToPath {
    [CmdletBinding()]
    param([Parameter(Mandatory)][hashtable]$Tools)
    if ($Tools.SolverDir -and (Test-Path -LiteralPath $Tools.SolverDir)) {
        $sep = if ($Tools.Platform -eq 'Windows') { ';' } else { ':' }
        if ($env:PATH -notlike "*$($Tools.SolverDir)*") {
            $env:PATH = "$($Tools.SolverDir)$sep$($env:PATH)"
        }
    }
}
