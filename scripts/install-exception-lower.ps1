<#
.SYNOPSIS
    Installs the llvm-exception-lower pass into
    ~/.saw-spec-gen/exception-lower/bin/.

.DESCRIPTION
    Two-stage installer:
      1. Try to download a prebuilt binary from the matching GitHub
         release of AmeliaRose802/llvm-exception-lower. Fast (no build
         deps), but only available for platforms we publish.
      2. Fall back to clone + cmake build from source. Slower; requires
         cmake + git + a C++ host compiler. Works on every platform LLVM
         itself builds on.

    Reusable installer used by both scripts/init.ps1 (one-shot machine
    setup) and verify.ps1 (auto-install on first need for an MSVC demo
    that requires C++ exception handling). Idempotent — exits 0 with
    the existing binary path on stdout when the pass is already
    installed, unless -Force.

    Returns (writes to stdout) the absolute path to the binary on
    success. Exits non-zero with a diagnostic on stderr on failure so
    callers can fall back to text-only EH stripping.

.PARAMETER InstallRoot
    Directory to install under. Defaults to ~/.saw-spec-gen.

.PARAMETER ReleaseTag
    Which release of llvm-exception-lower to download prebuilt binaries
    from (default: v0.2.0). The asset name is derived from the host
    platform: exception-lower-{platform}-{arch}.{ext} where {ext} is
    .zip on Windows and .tar.gz elsewhere.

.PARAMETER Ref
    Git ref to fall back to when no prebuilt is available
    (default: main).

.PARAMETER LlvmBin
    LLVM bin directory passed to the source-build fallback so cmake
    picks up the same LLVM verify.ps1 is using. Optional.

.PARAMETER Quiet
    Suppress informational output. Errors still surface on stderr.

.PARAMETER Force
    Re-install even if the binary already exists.

.PARAMETER NoDownload
    Skip the prebuilt download and go straight to the source build.

.PARAMETER NoBuild
    Skip the source-build fallback when no prebuilt is available
    (exit non-zero instead).
#>
[CmdletBinding()]
param(
    [string]$InstallRoot,
    [string]$ReleaseTag = 'v0.3.0',
    [string]$Ref = 'main',
    [string]$LlvmBin,
    [switch]$Quiet,
    [switch]$Force,
    [switch]$NoDownload,
    [switch]$NoBuild
)

$ErrorActionPreference = 'Stop'

if (-not $InstallRoot) {
    $userHome = if ($env:HOME) { $env:HOME } else { $env:USERPROFILE }
    $InstallRoot = Join-Path $userHome '.saw-spec-gen'
}

$isWindowsHost = $IsWindows -or ($null -eq $IsWindows -and $env:OS -eq 'Windows_NT')
$exeExt = if ($isWindowsHost) { '.exe' } else { '' }
$elRoot = Join-Path $InstallRoot 'exception-lower'
$elBinDir = Join-Path $elRoot 'bin'
# The downloaded binary lands in bin/; the source build produces
# build/exception-lower. Both layouts work — discover-tools.ps1 looks
# in both. Canonical install path is bin/.
$elBin = Join-Path $elBinDir ('exception-lower' + $exeExt)

function _Log([string]$msg, [string]$color = 'White') {
    if (-not $Quiet) { Write-Host $msg -ForegroundColor $color }
}

# Fast path: already installed.
if ((Test-Path -LiteralPath $elBin) -and (-not $Force)) {
    _Log "  exception-lower already installed: $elBin" 'DarkGreen'
    Write-Output $elBin
    exit 0
}

# Detect platform + arch for the prebuilt asset name. We keep the
# label space tiny on purpose (windows-x64 / linux-x64 / macos-arm64)
# — the llvm-exception-lower release page uses the same names.
function _PlatformLabel {
    if ($isWindowsHost) { return 'windows-x64' }
    if ($IsMacOS) {
        $arch = (uname -m 2>$null)
        if ($arch -eq 'arm64' -or $arch -eq 'aarch64') { return 'macos-arm64' }
        return 'macos-x64'
    }
    if ($IsLinux) {
        $arch = (uname -m 2>$null)
        if ($arch -eq 'aarch64') { return 'linux-arm64' }
        return 'linux-x64'
    }
    return $null
}

function _TryDownloadPrebuilt {
    $platform = _PlatformLabel
    if (-not $platform) {
        _Log '  no prebuilt label for this platform; falling back to source build' 'DarkYellow'
        return $false
    }
    # Use .zip on Windows (Expand-Archive handles it natively) and
    # .tar.gz everywhere else (matches the convention upstream uses for
    # stripped ELF / Mach-O builds and lets us preserve the executable
    # bit through extraction with tar).
    $ext = if ($isWindowsHost) { 'zip' } else { 'tar.gz' }
    $assetName = "exception-lower-$platform.$ext"
    $url = "https://github.com/AmeliaRose802/llvm-exception-lower/releases/download/$ReleaseTag/$assetName"
    $tmp = Join-Path ([System.IO.Path]::GetTempPath()) "el-$([guid]::NewGuid().ToString('N')).$ext"
    _Log "  downloading $url" 'Cyan'
    try {
        Invoke-WebRequest -Uri $url -OutFile $tmp -UseBasicParsing -ErrorAction Stop
    } catch {
        _Log "  prebuilt download failed: $($_.Exception.Message)" 'DarkYellow'
        if (Test-Path $tmp) { Remove-Item $tmp -Force -ErrorAction SilentlyContinue }
        return $false
    }
    if (-not (Test-Path -LiteralPath $tmp) -or (Get-Item $tmp).Length -eq 0) {
        _Log '  prebuilt download produced an empty file' 'DarkYellow'
        return $false
    }
    # Extract directly into bin/. The release archive contains the
    # single executable plus a couple of doc files at its root.
    New-Item -ItemType Directory -Path $elBinDir -Force | Out-Null
    try {
        if ($ext -eq 'zip') {
            Expand-Archive -LiteralPath $tmp -DestinationPath $elBinDir -Force
        } else {
            # tar.exe ships with Windows 10+ and is universally
            # available on Linux/macOS. -p preserves permissions so the
            # executable bit survives.
            Push-Location $elBinDir
            try { & tar -xpzf $tmp } finally { Pop-Location }
            if ($LASTEXITCODE -ne 0) { throw "tar exited $LASTEXITCODE" }
        }
    } catch {
        _Log "  extract failed: $($_.Exception.Message)" 'DarkYellow'
        Remove-Item $tmp -Force -ErrorAction SilentlyContinue
        return $false
    }
    Remove-Item $tmp -Force -ErrorAction SilentlyContinue
    if (-not (Test-Path -LiteralPath $elBin)) {
        # Some packagers put the binary under a top-level directory; do
        # a shallow search for it.
        $found = Get-ChildItem -LiteralPath $elBinDir -Recurse -Filter ('exception-lower' + $exeExt) -ErrorAction SilentlyContinue | Select-Object -First 1
        if ($found) { Move-Item -LiteralPath $found.FullName -Destination $elBin -Force }
    }
    if (-not (Test-Path -LiteralPath $elBin)) {
        _Log "  archive did not contain exception-lower$exeExt" 'DarkYellow'
        return $false
    }
    if (-not $isWindowsHost) {
        try { & chmod +x $elBin } catch { }
    }
    return $true
}

function _TryBuildFromSource {
    $cmake = (Get-Command ('cmake' + $exeExt) -ErrorAction SilentlyContinue).Source
    $git   = (Get-Command ('git'   + $exeExt) -ErrorAction SilentlyContinue).Source
    if (-not $cmake -or -not $git) {
        $missing = if (-not $git) { 'git' } else { 'cmake' }
        Write-Error @"
Cannot auto-build the exception-lower pass: $missing not on PATH and no
prebuilt binary is available for this platform.

Either install $missing and re-run, or set SAW_SPEC_GEN_EXCEPTION_LOWER
to point at an existing build.

Source: https://github.com/AmeliaRose802/llvm-exception-lower

Without the pass, verify.ps1 will still work for everything except C++
try/catch demos.
"@
        return $false
    }
    $srcDir = Join-Path $elRoot 'src'
    if ((-not (Test-Path -LiteralPath (Join-Path $srcDir '.git'))) -or $Force) {
        if (Test-Path -LiteralPath $srcDir) { Remove-Item -Recurse -Force -LiteralPath $srcDir }
        New-Item -ItemType Directory -Path $elRoot -Force | Out-Null
        _Log "  cloning https://github.com/AmeliaRose802/llvm-exception-lower@$Ref" 'Cyan'
        & $git clone --depth 1 --branch $Ref 'https://github.com/AmeliaRose802/llvm-exception-lower' $srcDir 2>&1 | ForEach-Object { _Log "    $_" }
        if ($LASTEXITCODE -ne 0) {
            Write-Error 'git clone failed'
            return $false
        }
    } else {
        _Log "  source already cloned: $srcDir" 'DarkGreen'
    }
    $buildDir = Join-Path $elRoot 'build'
    if ($Force -and (Test-Path -LiteralPath $buildDir)) { Remove-Item -Recurse -Force -LiteralPath $buildDir }
    New-Item -ItemType Directory -Path $buildDir -Force | Out-Null
    $cmakeArgs = @($srcDir, '-DCMAKE_BUILD_TYPE=Release')
    if ($LlvmBin) {
        $candidate = Resolve-Path (Join-Path $LlvmBin '../lib/cmake/llvm') -ErrorAction SilentlyContinue
        if ($candidate) { $cmakeArgs += "-DLLVM_DIR=$($candidate.Path)" }
    }
    Push-Location $buildDir
    try {
        _Log "  cmake $($cmakeArgs -join ' ')" 'Cyan'
        & $cmake @cmakeArgs 2>&1 | ForEach-Object { _Log "    $_" }
        if ($LASTEXITCODE -ne 0) { Write-Error 'cmake configure failed'; return $false }
        _Log '  cmake --build . --config Release' 'Cyan'
        & $cmake --build . --config Release 2>&1 | ForEach-Object { _Log "    $_" }
        if ($LASTEXITCODE -ne 0) { Write-Error 'cmake build failed'; return $false }
    } finally { Pop-Location }
    $built = Join-Path $buildDir ('exception-lower' + $exeExt)
    if (-not (Test-Path -LiteralPath $built)) {
        Write-Error "build completed but $built is missing"
        return $false
    }
    # Hoist the built binary into bin/ so the canonical path matches the
    # download flow.
    New-Item -ItemType Directory -Path $elBinDir -Force | Out-Null
    Copy-Item -LiteralPath $built -Destination $elBin -Force
    return $true
}

# Try download first, then source build, unless the caller disabled
# either path.
$ok = $false
if (-not $NoDownload) {
    $ok = _TryDownloadPrebuilt
}
if (-not $ok -and -not $NoBuild) {
    $ok = _TryBuildFromSource
}

if (-not $ok -or -not (Test-Path -LiteralPath $elBin)) {
    Write-Error 'exception-lower install failed (download + build both unsuccessful)'
    exit 1
}

_Log "  installed: $elBin" 'Green'
Write-Output $elBin

