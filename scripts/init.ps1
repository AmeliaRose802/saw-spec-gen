<#
.SYNOPSIS
    One-shot installer that prepares a machine to build saw-spec-gen and
    run its verify.ps1 / verify-rust.ps1 pipelines.

.DESCRIPTION
    Idempotent. Each step probes for what's already installed and only
    downloads what's missing. Everything that this script installs goes
    under  $HOME\.saw-spec-gen\ , so the install can be removed by
    deleting that one directory.

    Steps:
      1. Verify rustc + cargo are on PATH (must be installed via rustup).
      2. cargo build --release   (builds the saw-spec-gen CLI used by all
         verify scripts).
      3. Verify clang + llvm-as are findable. If not:
           Windows : download the official LLVM binary release tarball.
           Linux   : print apt/dnf/pacman hints (won't `sudo` for you).
           MacOS   : print  brew install llvm  hint.
      4. Download SAW with-solvers bundle from the GaloisInc release page.
      5. Write  $HOME\.saw-spec-gen\env.ps1  with the discovered paths,
         which discover-tools.ps1 dot-sources on every verify run.

.PARAMETER SawVersion
    Which SAW release to install (default: 1.5). The script downloads
    "saw-<ver>-<os>-with-solvers" from the saw-script GitHub release.

.PARAMETER LlvmVersion
    Which LLVM tarball to install when clang isn't found on Windows
    (default: 20.1.6).

.PARAMETER Force
    Re-download / rebuild even if everything is already in place.
#>

[CmdletBinding()]
param(
    [string]$SawVersion = '1.5',
    [string]$LlvmVersion = '20.1.6',
    [switch]$Force
)

$ErrorActionPreference = 'Stop'

# This script lives in scripts/. Repo root is one level up.
$ScriptRoot = Split-Path -Parent $PSCommandPath
$RepoRoot   = Resolve-Path (Join-Path $ScriptRoot '..')

. (Join-Path $ScriptRoot 'discover-tools.ps1')

$platform = Get-SawSpecGenPlatform
$userHome = if ($env:HOME) { $env:HOME } else { $env:USERPROFILE }
$installRoot = Join-Path $userHome '.saw-spec-gen'
New-Item -ItemType Directory -Path $installRoot -Force | Out-Null

function Write-Step([string]$msg) {
    Write-Host ''
    Write-Host '═══════════════════════════════════════════════════════' -ForegroundColor Cyan
    Write-Host (" " + $msg) -ForegroundColor Cyan
    Write-Host '═══════════════════════════════════════════════════════' -ForegroundColor Cyan
}

function Expand-DownloadedArchive {
    [CmdletBinding()]
    param(
        [Parameter(Mandatory)][string]$Url,
        [Parameter(Mandatory)][string]$DestDir,
        [switch]$Tarball
    )
    if ((Test-Path -LiteralPath $DestDir) -and (-not $Force)) {
        Write-Host "  already present: $DestDir" -ForegroundColor DarkGreen
        return
    }
    if (Test-Path -LiteralPath $DestDir) { Remove-Item -Recurse -Force -LiteralPath $DestDir }
    New-Item -ItemType Directory -Path $DestDir -Force | Out-Null

    $leaf = [System.IO.Path]::GetFileName(([uri]$Url).AbsolutePath)
    $tmp  = Join-Path ([System.IO.Path]::GetTempPath()) $leaf
    Write-Host "  downloading: $Url"
    Invoke-WebRequest -Uri $Url -OutFile $tmp -UseBasicParsing

    Write-Host "  extracting → $DestDir"
    if ($Tarball -or $leaf -match '\.tar\.(gz|xz|bz2)$') {
        # tar.exe ships with Win10+; on Linux/macOS we use the system tar.
        # On Windows GitHub runners, `tar` on PATH resolves to git-bash's
        # MSYS tar (/usr/bin/tar) which interprets Windows paths like
        # `C:\Users\...` as a remote host ("Cannot connect to C: resolve
        # failed"). Pin to System32 bsdtar explicitly to avoid that.
        $tarExe = if ($platform -eq 'Windows') {
            Join-Path $env:SystemRoot 'System32\tar.exe'
        } else { 'tar' }
        Push-Location $DestDir
        try { & $tarExe -xf $tmp } finally { Pop-Location }
    } else {
        Expand-Archive -LiteralPath $tmp -DestinationPath $DestDir -Force
    }
    Remove-Item -LiteralPath $tmp -Force -ErrorAction SilentlyContinue
}

# ── Step 1: rustc/cargo ───────────────────────────────────────────────────
Write-Step 'Step 1: Check rustc + cargo'
$rustc = Find-OnPath ('rustc' + $(if ($platform -eq 'Windows') { '.exe' } else { '' }))
$cargo = Find-OnPath ('cargo' + $(if ($platform -eq 'Windows') { '.exe' } else { '' }))
if (-not $rustc -or -not $cargo) {
    Write-Error @'
rustc / cargo not found on PATH.

Install Rust via rustup: https://rustup.rs
After install, restart this shell and re-run scripts/init.ps1.
'@
    exit 1
}
Write-Host "  rustc: $rustc" -ForegroundColor Green
Write-Host "  cargo: $cargo" -ForegroundColor Green

# ── Step 2: build saw-spec-gen ────────────────────────────────────────────
Write-Step 'Step 2: cargo build --release'
$specGenPath = Join-Path $RepoRoot ("target/release/saw-spec-gen" + $(if ($platform -eq 'Windows') { '.exe' } else { '' }))
if ((Test-Path -LiteralPath $specGenPath) -and (-not $Force)) {
    Write-Host "  already built: $specGenPath" -ForegroundColor DarkGreen
} else {
    Push-Location $RepoRoot
    try { & cargo build --release } finally { Pop-Location }
    if ($LASTEXITCODE -ne 0) { Write-Error 'cargo build failed'; exit 1 }
    Write-Host "  built: $specGenPath" -ForegroundColor Green
}

# ── Step 3: clang / llvm tools ────────────────────────────────────────────
Write-Step 'Step 3: clang + llvm-as'
$exeExt = if ($platform -eq 'Windows') { '.exe' } else { '' }
$tools  = Find-SawSpecGenTools -RepoRoot $RepoRoot
$llvmBin = $tools.LlvmBin
if (-not $llvmBin -or $Force) {
    switch ($platform) {
        'Windows' {
            $llvmDest = Join-Path $installRoot 'llvm'
            # Galois's own LLVM redistributable from llvm.org releases.
            # Filename pattern is stable for the 20.x series.
            $url = "https://github.com/llvm/llvm-project/releases/download/llvmorg-$LlvmVersion/clang+llvm-$LlvmVersion-x86_64-pc-windows-msvc.tar.xz"
            Expand-DownloadedArchive -Url $url -DestDir $llvmDest -Tarball
            # The archive extracts into a single top-level dir; find the bin/.
            $inner = Get-ChildItem -LiteralPath $llvmDest -Directory | Select-Object -First 1
            if ($inner) { $llvmBin = Join-Path $inner.FullName 'bin' }
        }
        'MacOS' {
            Write-Host '  clang not found.' -ForegroundColor Yellow
            Write-Host '  Install with Homebrew:    brew install llvm' -ForegroundColor Yellow
            Write-Host '  Then re-run scripts/init.ps1.' -ForegroundColor Yellow
            exit 1
        }
        'Linux' {
            Write-Host '  clang not found.' -ForegroundColor Yellow
            Write-Host '  Install via your package manager, e.g.:' -ForegroundColor Yellow
            Write-Host '    sudo apt install clang llvm                 # Debian/Ubuntu' -ForegroundColor Yellow
            Write-Host '    sudo dnf install clang llvm                 # Fedora' -ForegroundColor Yellow
            Write-Host '    sudo pacman -S clang llvm                   # Arch' -ForegroundColor Yellow
            Write-Host '  Then re-run scripts/init.ps1.' -ForegroundColor Yellow
            exit 1
        }
    }
}
if (-not (Test-Path -LiteralPath (Join-Path $llvmBin ('clang' + $exeExt)))) {
    Write-Error "clang still not found in $llvmBin"
    exit 1
}
Write-Host "  llvm bin: $llvmBin" -ForegroundColor Green

# ── Step 4: SAW + solvers ─────────────────────────────────────────────────
Write-Step "Step 4: SAW $SawVersion with bundled solvers"
$sawRoot = Join-Path $installRoot 'saw'
if (-not (Test-Path -LiteralPath (Join-Path $sawRoot ('bin/saw' + $exeExt))) -or $Force) {
    $sawAsset = switch ($platform) {
        'Windows' { "saw-$SawVersion-windows-2022-X64-with-solvers.tar.gz" }
        'MacOS'   { "saw-$SawVersion-macOS-x86_64-with-solvers.tar.gz" }
        'Linux'   { "saw-$SawVersion-Linux-x86_64-with-solvers.tar.gz" }
    }
    $url = "https://github.com/GaloisInc/saw-script/releases/download/v$SawVersion/$sawAsset"
    Expand-DownloadedArchive -Url $url -DestDir $sawRoot -Tarball
    # Flatten: the tarball contains <one-top-dir>/bin/saw — hoist it up.
    $inner = Get-ChildItem -LiteralPath $sawRoot -Directory | Where-Object { $_.Name -like "saw-*" } | Select-Object -First 1
    if ($inner) {
        Get-ChildItem -LiteralPath $inner.FullName -Force | ForEach-Object {
            Move-Item -LiteralPath $_.FullName -Destination $sawRoot -Force
        }
        Remove-Item -Recurse -Force -LiteralPath $inner.FullName -ErrorAction SilentlyContinue
    }
}
$sawExe   = Join-Path $sawRoot ('bin/saw' + $exeExt)
$solverDir = Join-Path $sawRoot 'bin'
if (-not (Test-Path -LiteralPath $sawExe)) {
    Write-Error "SAW download/extract failed: $sawExe missing"
    exit 1
}
Write-Host "  saw:     $sawExe" -ForegroundColor Green
Write-Host "  solvers: $solverDir" -ForegroundColor Green

# ── Step 5: write env.ps1 ─────────────────────────────────────────────────
Write-Step 'Step 5: write env file'
$envFile = Join-Path $installRoot 'env.ps1'
$envContent = @"
# Auto-generated by scripts/init.ps1 on $(Get-Date -Format 'yyyy-MM-dd HH:mm')
# Dot-sourced by scripts/discover-tools.ps1 before every verify run.
# Delete this file (or edit the values) to point at different installs.

`$env:SAW_SPEC_GEN_LLVM_BIN    = '$llvmBin'
`$env:SAW_SPEC_GEN_SAW         = '$sawExe'
`$env:SAW_SPEC_GEN_SOLVER_BIN  = '$solverDir'
"@
Set-Content -LiteralPath $envFile -Value $envContent -Encoding utf8
Write-Host "  wrote: $envFile" -ForegroundColor Green

# ── Sanity check ──────────────────────────────────────────────────────────
Write-Step 'Verifying installation'
$check = Find-SawSpecGenTools -RepoRoot $RepoRoot
$ok = $true
foreach ($k in @('Clang', 'LlvmAs', 'Saw', 'SolverDir', 'SpecGen')) {
    if ($check[$k]) {
        Write-Host ("  {0,-10} OK   {1}" -f $k, $check[$k]) -ForegroundColor Green
    } else {
        Write-Host ("  {0,-10} MISSING" -f $k) -ForegroundColor Red
        $ok = $false
    }
}
if (-not $ok) { exit 1 }

Write-Host ''
Write-Host 'saw-spec-gen is ready. Try:' -ForegroundColor Cyan
Write-Host '    ./verify.ps1 -CppFile demo/bounded_loop/add_one.cpp `' -ForegroundColor White
Write-Host '                 -CryptolSpec demo/bounded_loop/add_one_spec.cry `' -ForegroundColor White
Write-Host '                 -CryptolFn add_one_spec -Function add_one' -ForegroundColor White
