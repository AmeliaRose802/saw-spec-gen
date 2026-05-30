#!/usr/bin/env bash
# Installs the llvm-exception-lower pass into
# $INSTALL_ROOT/exception-lower/bin/.
#
# Two-stage installer:
#   1. Try to download a prebuilt binary from the matching GitHub
#      release of AmeliaRose802/llvm-exception-lower. Fast (no build
#      deps), but only available for platforms we publish.
#   2. Fall back to clone + cmake build from source. Slower; requires
#      cmake + git + a C++ host compiler. Works on every platform LLVM
#      itself builds on.
#
# Reusable installer used by both scripts/init.sh (one-shot machine
# setup) and verify.ps1 (auto-install on first need for an MSVC demo
# that requires C++ exception handling). Idempotent — exits 0 with
# the existing binary path on stdout when already installed, unless
# FORCE=1.
#
# Environment overrides:
#   INSTALL_ROOT          install root           (default: ~/.saw-spec-gen)
#   EXCEPTION_LOWER_TAG   release tag to fetch   (default: v0.3.1)
#   EXCEPTION_LOWER_REF   git ref for fallback   (default: main)
#   LLVM_BIN              llvm bin dir for cmake (default: empty)
#   QUIET                 1 to suppress info     (default: 0)
#   FORCE                 1 to reinstall         (default: 0)
#   NO_DOWNLOAD           1 to skip prebuilt     (default: 0)
#   NO_BUILD              1 to skip source build (default: 0)
set -euo pipefail

INSTALL_ROOT="${INSTALL_ROOT:-${HOME}/.saw-spec-gen}"
EXCEPTION_LOWER_TAG="${EXCEPTION_LOWER_TAG:-v0.3.1}"
EXCEPTION_LOWER_REF="${EXCEPTION_LOWER_REF:-main}"
LLVM_BIN="${LLVM_BIN:-}"
QUIET="${QUIET:-0}"
FORCE="${FORCE:-0}"
NO_DOWNLOAD="${NO_DOWNLOAD:-0}"
NO_BUILD="${NO_BUILD:-0}"

EL_ROOT="${INSTALL_ROOT}/exception-lower"
EL_BIN_DIR="${EL_ROOT}/bin"
EL_BIN="${EL_BIN_DIR}/exception-lower"

log() {
    [[ "$QUIET" == "1" ]] && return 0
    echo "$*" >&2
}

# Fast path: already installed.
if [[ -x "$EL_BIN" && "$FORCE" != "1" ]]; then
    log "  exception-lower already installed: $EL_BIN"
    echo "$EL_BIN"
    exit 0
fi

# Pick the prebuilt asset name for this platform. Keep the label space
# small on purpose; the llvm-exception-lower release page uses the same
# names.
platform_label() {
    case "$(uname -s)" in
        Linux*)
            case "$(uname -m)" in
                aarch64) echo 'linux-arm64' ;;
                *)       echo 'linux-x64'   ;;
            esac
            ;;
        Darwin*)
            case "$(uname -m)" in
                arm64|aarch64) echo 'macos-arm64' ;;
                *)             echo 'macos-x64'   ;;
            esac
            ;;
        *) echo '' ;;
    esac
}

have() { command -v "$1" >/dev/null 2>&1; }

try_download_prebuilt() {
    local platform asset url tmp ext
    platform="$(platform_label)"
    if [[ -z "$platform" ]]; then
        log '  no prebuilt label for this platform; falling back to source build'
        return 1
    fi
    # Linux/macOS get .tar.gz (preserves the executable bit through
    # extraction); Windows gets .zip. tar(1) handles both formats but we
    # match what upstream publishes so we don't have to repackage.
    ext='tar.gz'
    asset="exception-lower-${platform}.${ext}"
    url="https://github.com/AmeliaRose802/llvm-exception-lower/releases/download/${EXCEPTION_LOWER_TAG}/${asset}"
    tmp="$(mktemp -t el.XXXXXX.${ext})"
    log "  downloading $url"
    if have curl; then
        if ! curl -fL --retry 3 -o "$tmp" "$url" 2>/dev/null; then
            log '  prebuilt download failed (curl)'
            rm -f "$tmp"
            return 1
        fi
    elif have wget; then
        if ! wget -q -O "$tmp" "$url"; then
            log '  prebuilt download failed (wget)'
            rm -f "$tmp"
            return 1
        fi
    else
        log '  neither curl nor wget on PATH; cannot download prebuilt'
        rm -f "$tmp"
        return 1
    fi
    if [[ ! -s "$tmp" ]]; then
        log '  prebuilt download produced an empty file'
        rm -f "$tmp"
        return 1
    fi
    mkdir -p "$EL_BIN_DIR"
    # tar is universal on Linux/macOS; -p preserves the +x bit.
    if ! tar -xpzf "$tmp" -C "$EL_BIN_DIR"; then
        log '  tar extract failed'
        rm -f "$tmp"
        return 1
    fi
    rm -f "$tmp"
    if [[ ! -x "$EL_BIN" ]]; then
        # Some packagers put the binary under a top-level directory.
        found="$(find "$EL_BIN_DIR" -maxdepth 3 -type f -name 'exception-lower' | head -n 1 || true)"
        [[ -n "$found" ]] && mv "$found" "$EL_BIN"
    fi
    if [[ ! -x "$EL_BIN" ]]; then
        log '  archive did not contain exception-lower'
        return 1
    fi
    chmod +x "$EL_BIN"
    return 0
}

try_build_from_source() {
    if ! have cmake || ! have git; then
        local missing
        if have cmake; then missing='git'; else missing='cmake'; fi
        cat >&2 <<EOF
Cannot auto-build the exception-lower pass: $missing not on PATH and no
prebuilt binary is available for this platform.

Either install $missing and re-run, or set SAW_SPEC_GEN_EXCEPTION_LOWER
to point at an existing build.

Source: https://github.com/AmeliaRose802/llvm-exception-lower

Without the pass, verify.ps1 will still work for everything except C++
try/catch demos.
EOF
        return 1
    fi
    local el_src="${EL_ROOT}/src"
    if [[ ! -d "${el_src}/.git" || "$FORCE" == "1" ]]; then
        rm -rf "$el_src"
        mkdir -p "$EL_ROOT"
        log "  cloning https://github.com/AmeliaRose802/llvm-exception-lower@${EXCEPTION_LOWER_REF}"
        git clone --depth 1 --branch "$EXCEPTION_LOWER_REF" \
            'https://github.com/AmeliaRose802/llvm-exception-lower' "$el_src" >&2
    else
        log "  source already cloned: $el_src"
    fi
    local el_build="${EL_ROOT}/build"
    if [[ "$FORCE" == "1" && -d "$el_build" ]]; then
        rm -rf "$el_build"
    fi
    mkdir -p "$el_build"
    local cmake_args=("$el_src" -DCMAKE_BUILD_TYPE=Release)
    if [[ -n "$LLVM_BIN" && -d "${LLVM_BIN}/../lib/cmake/llvm" ]]; then
        local llvm_cmake_dir
        llvm_cmake_dir="$(cd "${LLVM_BIN}/../lib/cmake/llvm" && pwd)"
        cmake_args+=(-DLLVM_DIR="$llvm_cmake_dir")
    fi
    (
        cd "$el_build"
        log "  cmake ${cmake_args[*]}"
        cmake "${cmake_args[@]}" >&2
        log "  cmake --build . --config Release"
        cmake --build . --config Release >&2
    )
    local built="${el_build}/exception-lower"
    if [[ ! -x "$built" ]]; then
        echo "build completed but $built is missing" >&2
        return 1
    fi
    mkdir -p "$EL_BIN_DIR"
    cp -f "$built" "$EL_BIN"
    chmod +x "$EL_BIN"
    return 0
}

ok=0
if [[ "$NO_DOWNLOAD" != "1" ]]; then
    if try_download_prebuilt; then ok=1; fi
fi
if [[ "$ok" != "1" && "$NO_BUILD" != "1" ]]; then
    if try_build_from_source; then ok=1; fi
fi

if [[ "$ok" != "1" || ! -x "$EL_BIN" ]]; then
    echo 'exception-lower install failed (download + build both unsuccessful)' >&2
    exit 1
fi

log "  installed: $EL_BIN"
echo "$EL_BIN"
