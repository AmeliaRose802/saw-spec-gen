#!/usr/bin/env bash
# One-shot installer for saw-spec-gen on Linux / macOS.
#
# Idempotent. Each step probes for what's already installed and only
# downloads what's missing. Anything this script installs goes under
# $HOME/.saw-spec-gen/ so removal is a single  rm -rf .
#
# Usage:
#   bash scripts/init.sh                       # default versions (prebuilt SAW)
#   SAW_VERSION=1.5 bash scripts/init.sh       # pin SAW release version
#   LLVM_VERSION=20.1.6 bash scripts/init.sh   # pin LLVM version (Linux)
#   FORCE=1 bash scripts/init.sh               # redownload everything
#   SKIP_RUST_INSTALL=1 bash scripts/init.sh   # don't auto-install Rust
#                                              # via rustup if missing
#   SKIP_LLVM_INSTALL=1 bash scripts/init.sh   # don't auto-download LLVM
#                                              # (Linux); use system clang
#   SKIP_PWSH_INSTALL=1 bash scripts/init.sh   # don't auto-download
#                                              # PowerShell; use $PATH pwsh
#   PWSH_VERSION=7.6.2 bash scripts/init.sh    # pin pwsh version
#
#   SAW_SOURCE=binary  bash scripts/init.sh    # (default) GaloisInc prebuilt
#                                              # v1.5. Works because
#                                              # saw-spec-gen pre-links vtable
#                                              # stubs with llvm-link instead
#                                              # of relying on the post-v1.5
#                                              # llvm_combine_modules primitive.
#   SAW_SOURCE=fork    bash scripts/init.sh    # build AmeliaRose802/saw-script
#                                              # from source. Needed only when
#                                              # saw-spec-gen is invoked with
#                                              # --use-llvm-combine-modules or
#                                              # starts emitting other
#                                              # fork-only primitives
#                                              # (llvm_bind_method, …).
#                                              # Auto-installs the GHC toolchain
#                                              # via ghcup (~/.ghcup, no sudo)
#                                              # and will offer to sudo-install
#                                              # the system C dev libs cabal
#                                              # needs (libgmp-dev, …) unless
#                                              # SKIP_SUDO_INSTALL=1 is set.
#   SAW_SOURCE=upstream bash scripts/init.sh   # same, but GaloisInc/saw-script
#                                              # master.
#   SAW_FORK_REPO=...   bash scripts/init.sh   # custom git repo for SAW_SOURCE=fork
#   SAW_FORK_REF=master bash scripts/init.sh   # branch/tag/sha to check out
#   GHC_VERSION=9.6.7   bash scripts/init.sh   # ghcup target ghc version
#   CABAL_VERSION=3.10.3.0  bash scripts/init.sh
#   SAW_BUILD_JOBS=$(nproc) bash scripts/init.sh
#   SKIP_SUDO_INSTALL=1 bash scripts/init.sh   # never invoke sudo for libgmp-dev
#                                              # etc. Fail with manual hint.
#
# After this script finishes, scripts/discover-tools.ps1 will pick up
# the install via the env file at ~/.saw-spec-gen/env.sh — your verify
# scripts will find clang, llvm-as, saw and z3 automatically.
set -euo pipefail

SAW_VERSION="${SAW_VERSION:-1.5}"
# Default to the prebuilt v$SAW_VERSION tarball. saw-spec-gen now
# pre-links vtable_stubs.bc with main.bc via `llvm-link` at gen time
# (see --use-llvm-combine-modules), so the emitted verify.saw doesn't
# need any post-v1.5 primitive. SAW_SOURCE=fork|upstream are still
# available for future work that depends on real fork-only primitives.
SAW_SOURCE="${SAW_SOURCE:-binary}"
SAW_FORK_REPO="${SAW_FORK_REPO:-https://github.com/AmeliaRose802/saw-script.git}"
SAW_UPSTREAM_REPO="https://github.com/GaloisInc/saw-script.git"
SAW_FORK_REF="${SAW_FORK_REF:-master}"
GHC_VERSION="${GHC_VERSION:-9.6.7}"
CABAL_VERSION="${CABAL_VERSION:-3.10.3.0}"
SAW_BUILD_JOBS="${SAW_BUILD_JOBS:-}"
# Set SKIP_SUDO_INSTALL=1 to suppress the interactive sudo prompt that
# auto-installs missing system C dev libraries (libgmp-dev, …) when
# SAW_SOURCE=fork or SAW_SOURCE=upstream. Useful in CI / unattended runs.
SKIP_SUDO_INSTALL="${SKIP_SUDO_INSTALL:-0}"
FORCE="${FORCE:-0}"

# Resolve repo root regardless of where the script is invoked from.
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
INSTALL_ROOT="${HOME}/.saw-spec-gen"
mkdir -p "$INSTALL_ROOT"

# Platform detection drives the SAW asset name and (later) the default
# LLVM target tuple the verify scripts pass to clang/rustc.
case "$(uname -s)" in
    Linux*)   PLATFORM=linux  ;;
    Darwin*)  PLATFORM=macos  ;;
    *) echo "Unsupported OS: $(uname -s). Use scripts/init.ps1 on Windows." >&2; exit 1 ;;
esac

step() {
    echo
    echo '═══════════════════════════════════════════════════════'
    echo " $*"
    echo '═══════════════════════════════════════════════════════'
}

have() { command -v "$1" >/dev/null 2>&1; }

download_extract_tarball() {
    # $1 = URL, $2 = destination directory
    local url="$1" dest="$2"
    if [[ -d "$dest" && "$FORCE" != "1" ]]; then
        echo "  already present: $dest"
        return 0
    fi
    rm -rf "$dest"
    mkdir -p "$dest"
    local tmp
    tmp="$(mktemp -t sawspecgen.XXXXXX.tar)"
    echo "  downloading: $url"
    if have curl; then
        curl -fL --retry 3 -o "$tmp" "$url"
    elif have wget; then
        wget -q -O "$tmp" "$url"
    else
        echo "Need curl or wget to download $url" >&2
        return 1
    fi
    echo "  extracting → $dest"
    tar -xf "$tmp" -C "$dest"
    rm -f "$tmp"
}

# ── Step 1: rustc / cargo ──────────────────────────────────────────────
step 'Step 1: Check rustc + cargo'

# If a previous run installed rustup but the current shell hasn't sourced
# its env file yet, pick it up so subsequent `have` checks succeed.
if ! have cargo && [[ -f "${HOME}/.cargo/env" ]]; then
    # shellcheck disable=SC1091
    . "${HOME}/.cargo/env"
fi

if ! have rustc || ! have cargo; then
    if [[ "${SKIP_RUST_INSTALL:-0}" == "1" ]]; then
        cat >&2 <<'EOF'
rustc / cargo not found on PATH and SKIP_RUST_INSTALL=1.

Install Rust via rustup: https://rustup.rs
    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh

After install, source ~/.cargo/env (or restart your shell) and re-run
scripts/init.sh.
EOF
        exit 1
    fi

    echo "  rustc / cargo not found — installing Rust via rustup (user-scope, no sudo)."
    echo "  set SKIP_RUST_INSTALL=1 to skip this and install Rust yourself."
    if ! have curl && ! have wget; then
        echo "Need curl or wget to download rustup-init." >&2
        exit 1
    fi
    rustup_tmp="$(mktemp -t rustup-init.XXXXXX.sh)"
    if have curl; then
        curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs -o "$rustup_tmp"
    else
        wget -q -O "$rustup_tmp" https://sh.rustup.rs
    fi
    # -y: non-interactive, accept defaults (stable toolchain, ~/.cargo, ~/.rustup).
    sh "$rustup_tmp" -y --default-toolchain stable --profile minimal
    rm -f "$rustup_tmp"
    # Pick up the newly-installed cargo/rustc in *this* shell.
    if [[ -f "${HOME}/.cargo/env" ]]; then
        # shellcheck disable=SC1091
        . "${HOME}/.cargo/env"
    fi
    if ! have rustc || ! have cargo; then
        echo "rustup install completed but rustc/cargo still not on PATH." >&2
        echo "Try opening a new shell and re-running scripts/init.sh." >&2
        exit 1
    fi
fi
echo "  rustc: $(command -v rustc)"
echo "  cargo: $(command -v cargo)"

# ── Step 2: build saw-spec-gen ─────────────────────────────────────────
step 'Step 2: cargo build --release'
SPEC_GEN="${REPO_ROOT}/target/release/saw-spec-gen"
if [[ -x "$SPEC_GEN" && "$FORCE" != "1" ]]; then
    echo "  already built: $SPEC_GEN"
else
    ( cd "$REPO_ROOT" && cargo build --release )
    echo "  built: $SPEC_GEN"
fi

# ── Step 3: clang / llvm tools ─────────────────────────────────────────
step 'Step 3: clang + llvm-as'
LLVM_VERSION="${LLVM_VERSION:-20.1.6}"
LLVM_BIN=""

# 3a. Prefer a previously-downloaded copy under ~/.saw-spec-gen/llvm/.
LLVM_DEST="${INSTALL_ROOT}/llvm"
if [[ -z "$LLVM_BIN" && "$FORCE" != "1" ]]; then
    candidate="$(find "$LLVM_DEST" -mindepth 2 -maxdepth 3 -type f -name 'clang' 2>/dev/null | head -n 1 || true)"
    if [[ -n "$candidate" ]]; then
        LLVM_BIN="$(dirname "$candidate")"
    fi
fi

# 3b. Otherwise pick up clang+llvm-as from PATH.
if [[ -z "$LLVM_BIN" ]] && have clang && have llvm-as; then
    LLVM_BIN="$(dirname "$(command -v clang)")"
fi

# 3c. Still nothing — auto-download a prebuilt LLVM (Linux only, x86_64
# only; macOS doesn't have an official x86_64 LLVM tarball for 20.x so
# we still fall through to the brew hint).
if [[ -z "$LLVM_BIN" && "$PLATFORM" == "linux" && "${SKIP_LLVM_INSTALL:-0}" != "1" ]]; then
    arch="$(uname -m)"
    case "$arch" in
        x86_64|amd64) LLVM_ARCH=X64 ;;
        aarch64|arm64) LLVM_ARCH=ARM64 ;;
        *) LLVM_ARCH="" ;;
    esac
    if [[ -n "$LLVM_ARCH" ]]; then
        echo "  clang / llvm-as not found — downloading LLVM ${LLVM_VERSION} (~1 GB)"
        echo "  set SKIP_LLVM_INSTALL=1 to skip this and install clang/llvm yourself."
        ASSET="LLVM-${LLVM_VERSION}-Linux-${LLVM_ARCH}.tar.xz"
        URL="https://github.com/llvm/llvm-project/releases/download/llvmorg-${LLVM_VERSION}/${ASSET}"
        download_extract_tarball "$URL" "$LLVM_DEST"
        # Tarball extracts to a top-level <LLVM-…>/ directory containing
        # bin/, lib/, etc. Locate it.
        inner="$(find "$LLVM_DEST" -mindepth 1 -maxdepth 1 -type d | head -n 1 || true)"
        if [[ -n "$inner" && -x "$inner/bin/clang" ]]; then
            LLVM_BIN="$inner/bin"
        fi
    fi
fi

if [[ -z "$LLVM_BIN" ]]; then
    cat >&2 <<EOF
clang / llvm-as not found on PATH and auto-install did not run
(SKIP_LLVM_INSTALL=${SKIP_LLVM_INSTALL:-0}, arch=$(uname -m)).

Install via your package manager:
EOF
    if [[ "$PLATFORM" == "macos" ]]; then
        cat >&2 <<'EOF'
    brew install llvm

Then add the keg-only LLVM bin to your PATH (the brew install command
prints the exact line to add), and re-run scripts/init.sh.
EOF
    else
        cat >&2 <<'EOF'
    sudo apt install clang llvm                 # Debian/Ubuntu
    sudo dnf install clang llvm                 # Fedora
    sudo pacman -S clang llvm                   # Arch
EOF
    fi
    exit 1
fi
echo "  llvm bin: $LLVM_BIN"

# ── Step 4: SAW + bundled solvers ──────────────────────────────────────
step "Step 4: SAW ${SAW_VERSION} with bundled solvers"
SAW_ROOT="${INSTALL_ROOT}/saw"
SAW_EXE="${SAW_ROOT}/bin/saw"

# Pick the right release asset. The GaloisInc/saw-script naming scheme
# (as of SAW 1.5) is OS-version-specific: ubuntu-22.04 / ubuntu-24.04 on
# Linux and macos-15-{intel-X64,ARM64} on macOS. Falling back to the older
# saw-<ver>-{Linux,macOS}-x86_64 names produces a 404 on current releases.
saw_asset_for_platform() {
    case "$PLATFORM" in
        linux)
            local arch_tag=X64
            case "$(uname -m)" in
                x86_64|amd64) arch_tag=X64 ;;
                aarch64|arm64)
                    echo "SAW ${SAW_VERSION} ships no Linux ARM64 build." >&2
                    return 1 ;;
            esac
            # Prefer the Ubuntu build matching /etc/os-release; otherwise
            # default to 22.04 (older glibc → broader compatibility).
            local ubuntu_tag=22.04
            if [[ -r /etc/os-release ]]; then
                # shellcheck disable=SC1091
                local _id _vid
                _id="$(. /etc/os-release; echo "${ID:-}")"
                _vid="$(. /etc/os-release; echo "${VERSION_ID:-}")"
                if [[ "$_id" == "ubuntu" && "$_vid" == "24.04" ]]; then
                    ubuntu_tag=24.04
                fi
            fi
            echo "saw-${SAW_VERSION}-ubuntu-${ubuntu_tag}-${arch_tag}-with-solvers.tar.gz"
            ;;
        macos)
            case "$(uname -m)" in
                arm64|aarch64) echo "saw-${SAW_VERSION}-macos-15-ARM64-with-solvers.tar.gz" ;;
                x86_64|amd64)  echo "saw-${SAW_VERSION}-macos-15-intel-X64-with-solvers.tar.gz" ;;
                *) echo "Unknown macOS arch: $(uname -m)" >&2; return 1 ;;
            esac
            ;;
    esac
}

if [[ ! -x "$SAW_EXE" || "$FORCE" == "1" ]]; then
    ASSET="$(saw_asset_for_platform)"
    URL="https://github.com/GaloisInc/saw-script/releases/download/v${SAW_VERSION}/${ASSET}"
    download_extract_tarball "$URL" "$SAW_ROOT"
    # Tarball contains a single top-level <saw-…> directory; hoist its
    # contents up to $SAW_ROOT so $SAW_ROOT/bin/saw is the install layout
    # discover-tools.ps1 expects.
    inner="$(find "$SAW_ROOT" -mindepth 1 -maxdepth 1 -type d -name 'saw-*' | head -n 1 || true)"
    if [[ -n "$inner" ]]; then
        ( cd "$inner" && tar -cf - . ) | ( cd "$SAW_ROOT" && tar -xf - )
        rm -rf "$inner"
    fi
fi
if [[ ! -x "$SAW_EXE" ]]; then
    echo "SAW download/extract failed: $SAW_EXE missing" >&2
    exit 1
fi
SOLVER_DIR="${SAW_ROOT}/bin"
echo "  saw:     $SAW_EXE"
echo "  solvers: $SOLVER_DIR"

# ── Step 4b (optional): build SAW from source and replace bin/saw ──────
# The prebuilt v1.5 tarball gives us z3/yices/cvc4 (we keep those), but
# some saw-spec-gen-generated scripts use primitives that only exist on
# the fork or in post-v1.5 upstream master (e.g. `llvm_combine_modules`,
# `llvm_bind_method`, `llvm_subclasses`). When SAW_SOURCE != "binary" we
# clone+build the requested source tree and overwrite SAW_ROOT/bin/saw.
SAW_SOURCE_ROOT="${INSTALL_ROOT}/saw-src"
SAW_SOURCE_STAMP="${SAW_SOURCE_ROOT}/.built-from"

build_saw_from_source() {
    local repo="$1" ref="$2" label="$3"
    step "Step 4b: build SAW from source (${label} @ ${ref})"

    # Skip if we already built this exact (repo,ref) combo.
    local want_stamp="${repo}#${ref}"
    if [[ -x "$SAW_EXE" && -f "$SAW_SOURCE_STAMP" \
          && "$(cat "$SAW_SOURCE_STAMP" 2>/dev/null)" == "$want_stamp" \
          && "$FORCE" != "1" ]]; then
        echo "  already built from ${want_stamp}; reusing $SAW_EXE"
        return 0
    fi

    # Sanity-check C build deps that ghcup doesn't install for us.
    # Run this BEFORE the multi-GB clone + ghcup install so users get
    # immediate feedback when something's missing.
    #
    # We use `gcc -lFOO` link tests rather than `ldconfig -p` because the
    # runtime library (libgmp.so.10) is normally present but the linker
    # needs the unsuffixed symlink (libgmp.so) that only ships in -dev
    # packages. A bad check here means cabal happily downloads 200+
    # packages and fails 20 minutes in.
    have_dev_lib() {
        local lib="$1"
        echo 'int main(void){return 0;}' \
            | "${CC:-cc}" -x c - -o /dev/null "-l$lib" >/dev/null 2>&1
    }
    # Returns a space-separated list of *logical* missing deps.
    detect_missing_build_deps() {
        local missing=()
        have cc                || missing+=(cc)
        have make              || missing+=(make)
        have pkg-config        || missing+=(pkgconfig)
        if have cc; then
            have_dev_lib gmp   || missing+=(libgmp)
            have_dev_lib z     || missing+=(libz)
            have_dev_lib ffi   || missing+=(libffi)
        fi
        printf '%s\n' "${missing[@]}"
    }
    # Map (logical dep, distro) → concrete package name.
    pkg_for() {
        local logical="$1" distro="$2"
        case "$distro:$logical" in
            apt:cc|apt:make)        echo build-essential ;;
            apt:pkgconfig)          echo pkg-config ;;
            apt:libgmp)             echo libgmp-dev ;;
            apt:libz)               echo zlib1g-dev ;;
            apt:libffi)             echo libffi-dev ;;
            dnf:cc)                 echo gcc ;;
            dnf:make)               echo make ;;
            dnf:pkgconfig)          echo pkgconf-pkg-config ;;
            dnf:libgmp)             echo gmp-devel ;;
            dnf:libz)               echo zlib-devel ;;
            dnf:libffi)             echo libffi-devel ;;
            pacman:cc|pacman:make|pacman:pkgconfig) echo base-devel ;;
            pacman:libgmp)          echo gmp ;;
            pacman:libz)            echo zlib ;;
            pacman:libffi)          echo libffi ;;
            brew:libgmp)            echo gmp ;;
            brew:libffi)            echo libffi ;;
            brew:pkgconfig)         echo pkg-config ;;
            *)                      echo "" ;;
        esac
    }
    # Detect which package manager + install command to use. Echoes
    # "<distro>|<install-cmd-prefix>" or "" if nothing usable is found.
    detect_pkg_manager() {
        if   have apt-get; then echo "apt|sudo apt-get install -y"
        elif have apt;     then echo "apt|sudo apt install -y"
        elif have dnf;     then echo "dnf|sudo dnf install -y"
        elif have pacman;  then echo "pacman|sudo pacman -S --noconfirm --needed"
        elif have brew;    then echo "brew|brew install"          # brew refuses to run under sudo
        else                    echo ""
        fi
    }
    install_missing_via_sudo() {
        local missing=("$@")
        local pm_info distro install_cmd
        pm_info="$(detect_pkg_manager)"
        if [[ -z "$pm_info" ]]; then
            return 1   # no supported package manager
        fi
        distro="${pm_info%%|*}"
        install_cmd="${pm_info#*|}"

        # Translate logical → distro-specific package names, deduped.
        local pkgs=() seen=()
        for dep in "${missing[@]}"; do
            local p
            p="$(pkg_for "$dep" "$distro")"
            if [[ -n "$p" ]]; then
                local already=0
                for s in "${seen[@]:-}"; do
                    [[ "$s" == "$p" ]] && { already=1; break; }
                done
                if (( already == 0 )); then
                    pkgs+=("$p"); seen+=("$p")
                fi
            fi
        done
        if (( ${#pkgs[@]} == 0 )); then
            return 1
        fi

        # On macOS Homebrew doesn't need sudo, but everywhere else we do.
        # If sudo isn't available we can't proceed.
        if [[ "$distro" != "brew" ]] && ! have sudo; then
            echo "  sudo is not installed; can't auto-install ${pkgs[*]}" >&2
            return 1
        fi

        # Prime apt's package index once so install doesn't fail on stale
        # cache. dnf/pacman/brew handle this transparently.
        if [[ "$distro" == "apt" ]]; then
            echo "  → sudo apt-get update"
            sudo apt-get update -qq || true
        fi

        echo "  → $install_cmd ${pkgs[*]}"
        # shellcheck disable=SC2086
        if $install_cmd "${pkgs[@]}"; then
            return 0
        fi
        return 1
    }

    mapfile -t missing_logical < <(detect_missing_build_deps)
    if (( ${#missing_logical[@]} > 0 )); then
        echo "  missing build deps: ${missing_logical[*]}"

        if [[ "$SKIP_SUDO_INSTALL" == "1" ]]; then
            cat >&2 <<EOF

SKIP_SUDO_INSTALL=1 set; not invoking sudo.

Install the missing packages manually, e.g. on Debian/Ubuntu:
    sudo apt install build-essential pkg-config libgmp-dev zlib1g-dev libffi-dev
On Fedora:
    sudo dnf install gcc make pkgconf-pkg-config gmp-devel zlib-devel libffi-devel
On Arch:
    sudo pacman -S base-devel gmp zlib libffi
On macOS:
    brew install gmp libffi pkg-config

then re-run scripts/init.sh.
(Or set SAW_SOURCE=binary to skip the source build entirely.)
EOF
            exit 1
        fi

        # Offer to install via the system package manager. sudo itself
        # is the interactive elevation prompt — we just need a yes/no
        # from the user (defaulting to Yes since they asked for fork
        # builds, which can't proceed without these).
        echo
        echo "saw-spec-gen can install these via your system package manager."
        echo "This is the only step that needs root; sudo will prompt for"
        echo "your password. Set SKIP_SUDO_INSTALL=1 to skip and install manually."
        printf "Proceed with auto-install? [Y/n] "
        local reply=""
        if [[ -t 0 ]]; then
            read -r reply || reply=""
        else
            # No TTY (piped install, CI without env override) — refuse
            # to silently sudo.
            echo
            echo "stdin is not a terminal; refusing to prompt for sudo." >&2
            echo "Either run scripts/init.sh from an interactive shell, or" >&2
            echo "set SKIP_SUDO_INSTALL=1 and install the listed packages first." >&2
            exit 1
        fi
        case "${reply,,}" in
            ""|y|yes) ;;
            *) echo "Declined. Install the packages manually and re-run." >&2; exit 1 ;;
        esac

        if ! install_missing_via_sudo "${missing_logical[@]}"; then
            cat >&2 <<EOF
Auto-install failed (no supported package manager, or sudo refused).

Install manually (Debian/Ubuntu):
    sudo apt install build-essential pkg-config libgmp-dev zlib1g-dev libffi-dev

then re-run scripts/init.sh.
EOF
            exit 1
        fi

        # Re-test; bail if something's still missing.
        mapfile -t still_missing < <(detect_missing_build_deps)
        if (( ${#still_missing[@]} > 0 )); then
            echo "Still missing after install: ${still_missing[*]}" >&2
            exit 1
        fi
        echo "  build deps OK after auto-install"
    fi

    # Now (after the cheap prereq check) install GHC/cabal via ghcup.
    ensure_haskell_toolchain

    # Fresh clone (or pull) into ~/.saw-spec-gen/saw-src.
    # NOTE: we deliberately do a full clone (no --depth 1) because SAW's
    # nested submodule pointers reference arbitrary historical commits
    # that shallow clones often can't satisfy.
    if [[ -d "$SAW_SOURCE_ROOT/.git" && "$FORCE" != "1" ]]; then
        echo "  updating existing clone at $SAW_SOURCE_ROOT"
        ( cd "$SAW_SOURCE_ROOT" \
          && git remote set-url origin "$repo" \
          && git fetch --tags origin "$ref" \
          && git checkout --detach FETCH_HEAD )
    else
        rm -rf "$SAW_SOURCE_ROOT"
        echo "  git clone $repo → $SAW_SOURCE_ROOT (full history; ~200 MB)"
        git clone "$repo" "$SAW_SOURCE_ROOT"
        ( cd "$SAW_SOURCE_ROOT" && git checkout "$ref" )
    fi

    echo "  git submodule update --init --recursive (this is slow)"
    # SAW's submodule tree contains many nested .gitmodules files that pin
    # URLs of the form  git@github.com:GaloisInc/...  which require an SSH
    # key the user may not have. A `git config url.<https>.insteadOf` set
    # on the parent clone does NOT propagate to nested submodule clones,
    # which look up their own (initially empty) local config.
    #
    # The reliable cross-version fix is to point git at a temporary
    # GIT_CONFIG_GLOBAL that defines the rewrite — every git clone
    # invoked by `submodule update --init --recursive` will inherit it.
    # We also point at /dev/null for GIT_CONFIG_SYSTEM so the user's
    # system config can't interfere.
    local tmp_gitconfig
    tmp_gitconfig="$(mktemp -t saw-spec-gen-gitconfig.XXXXXX)"
    cat > "$tmp_gitconfig" <<'GITCONF'
[url "https://github.com/"]
    insteadOf = git@github.com:
    insteadOf = ssh://git@github.com/
GITCONF
    # Note: we still let git find the user's identity etc. by *including*
    # their real global config from our temp one if it exists.
    if [[ -f "${HOME}/.gitconfig" ]]; then
        printf '[include]\n    path = %s\n' "${HOME}/.gitconfig" >> "$tmp_gitconfig"
    fi
    (
        cd "$SAW_SOURCE_ROOT"
        GIT_CONFIG_GLOBAL="$tmp_gitconfig" \
        GIT_SSH_COMMAND="${GIT_SSH_COMMAND:-ssh -o BatchMode=yes -o StrictHostKeyChecking=accept-new}" \
            git submodule update --init --recursive
    )
    rm -f "$tmp_gitconfig"

    # Build. SAW pins GHC via its build.sh / cabal.project; we trust that.
    local jobs="${SAW_BUILD_JOBS:-$(nproc 2>/dev/null || sysctl -n hw.ncpu 2>/dev/null || echo 2)}"
    echo "  cabal build (using GHC ${GHC_VERSION}, -j${jobs}); this can take 30-60+ min"
    (
        cd "$SAW_SOURCE_ROOT"
        # Make sure ghcup's shims are in PATH for the subshell.
        export PATH="${HOME}/.ghcup/bin:${PATH}"
        ./build.sh -j"$jobs"
    )

    local built_saw="$SAW_SOURCE_ROOT/bin/saw"
    if [[ ! -x "$built_saw" ]]; then
        echo "Source build finished but $built_saw missing." >&2
        exit 1
    fi
    cp "$built_saw" "$SAW_EXE"
    echo "$want_stamp" > "$SAW_SOURCE_STAMP"
    echo "  installed: $SAW_EXE  (from ${label} @ ${ref})"
}

ensure_haskell_toolchain() {
    # ghcup installs GHC + cabal under ~/.ghcup with no sudo. We pin the
    # version SAW's CI uses (GHC_VERSION) so behaviour matches the fork
    # maintainers' tested combo.
    export GHCUP_INSTALL_BASE_PREFIX="${GHCUP_INSTALL_BASE_PREFIX:-$HOME}"
    local ghcup_bin="${HOME}/.ghcup/bin/ghcup"
    if [[ ! -x "$ghcup_bin" ]]; then
        echo "  installing ghcup (non-interactive) → ${HOME}/.ghcup"
        if ! have curl && ! have wget; then
            echo "Need curl or wget to bootstrap ghcup." >&2
            exit 1
        fi
        # ghcup's official bootstrap script honours these vars to skip
        # all interactive prompts.
        export BOOTSTRAP_HASKELL_NONINTERACTIVE=1
        export BOOTSTRAP_HASKELL_MINIMAL=1
        export BOOTSTRAP_HASKELL_ADJUST_BASHRC=0
        export BOOTSTRAP_HASKELL_INSTALL_NO_STACK=1
        if have curl; then
            curl --proto '=https' --tlsv1.2 -sSf https://get-ghcup.haskell.org | sh
        else
            wget -qO- https://get-ghcup.haskell.org | sh
        fi
    fi
    export PATH="${HOME}/.ghcup/bin:${PATH}"

    # Install (no-op if already present) and select the pinned versions.
    # We stream progress directly to the user; these downloads are big.
    if [[ ! -x "${HOME}/.ghcup/ghc/${GHC_VERSION}/bin/ghc" ]]; then
        echo "  ghcup install ghc ${GHC_VERSION} (large download; may take 5-15 min)"
        ghcup install ghc "$GHC_VERSION" --no-set --force
    else
        echo "  ghc ${GHC_VERSION} already installed"
    fi
    ghcup set ghc "$GHC_VERSION" >/dev/null
    if [[ ! -x "${HOME}/.ghcup/cabal/${CABAL_VERSION}/cabal" \
          && ! -x "${HOME}/.ghcup/bin/cabal-${CABAL_VERSION}" ]]; then
        echo "  ghcup install cabal ${CABAL_VERSION}"
        ghcup install cabal "$CABAL_VERSION" --no-set --force
    else
        echo "  cabal ${CABAL_VERSION} already installed"
    fi
    ghcup set cabal "$CABAL_VERSION" >/dev/null

    echo "  ghc:   $(ghc --version)"
    echo "  cabal: $(cabal --version | head -n1)"
    cabal update 2>&1 | tail -3
}

case "$SAW_SOURCE" in
    binary)
        : # nothing to do; prebuilt v$SAW_VERSION already installed.
        ;;
    fork)
        build_saw_from_source "$SAW_FORK_REPO" "$SAW_FORK_REF" "fork"
        ;;
    upstream)
        build_saw_from_source "$SAW_UPSTREAM_REPO" "$SAW_FORK_REF" "upstream"
        ;;
    *)
        echo "Unknown SAW_SOURCE='$SAW_SOURCE' (expected binary|fork|upstream)" >&2
        exit 1
        ;;
esac

# ── Step 5: PowerShell ─────────────────────────────────────────────────
# The verify scripts (verify.ps1, verify-rust.ps1, verify-equiv.ps1) and
# the discover-tools layer are written in PowerShell so they can share
# code with the Windows install. Drop a self-contained pwsh tarball
# under ~/.saw-spec-gen/pwsh/ so we don't need root / a package manager.
step "Step 5: PowerShell ${PWSH_VERSION:-7.6.2}"
PWSH_VERSION="${PWSH_VERSION:-7.6.2}"
PWSH_ROOT="${INSTALL_ROOT}/pwsh"
PWSH_EXE="${PWSH_ROOT}/pwsh"

pwsh_asset_for_platform() {
    local arch
    arch="$(uname -m)"
    case "$PLATFORM" in
        linux)
            case "$arch" in
                x86_64|amd64)  echo "powershell-${PWSH_VERSION}-linux-x64.tar.gz" ;;
                aarch64|arm64) echo "powershell-${PWSH_VERSION}-linux-arm64.tar.gz" ;;
                *) echo "Unsupported Linux arch for pwsh: $arch" >&2; return 1 ;;
            esac
            ;;
        macos)
            case "$arch" in
                arm64|aarch64) echo "powershell-${PWSH_VERSION}-osx-arm64.tar.gz" ;;
                x86_64|amd64)  echo "powershell-${PWSH_VERSION}-osx-x64.tar.gz" ;;
                *) echo "Unsupported macOS arch for pwsh: $arch" >&2; return 1 ;;
            esac
            ;;
    esac
}

# Pick up a system-installed pwsh first; only download if none found.
PWSH_FOUND=""
if [[ "${SKIP_PWSH_INSTALL:-0}" != "1" ]] && have pwsh; then
    PWSH_FOUND="$(command -v pwsh)"
elif [[ -x "$PWSH_EXE" && "$FORCE" != "1" ]]; then
    PWSH_FOUND="$PWSH_EXE"
fi

if [[ -z "$PWSH_FOUND" && "${SKIP_PWSH_INSTALL:-0}" != "1" ]]; then
    echo "  pwsh not found — downloading PowerShell ${PWSH_VERSION}"
    echo "  set SKIP_PWSH_INSTALL=1 to skip this and install pwsh yourself."
    ASSET="$(pwsh_asset_for_platform)"
    URL="https://github.com/PowerShell/PowerShell/releases/download/v${PWSH_VERSION}/${ASSET}"
    download_extract_tarball "$URL" "$PWSH_ROOT"
    chmod +x "$PWSH_EXE" 2>/dev/null || true
    if [[ -x "$PWSH_EXE" ]]; then
        PWSH_FOUND="$PWSH_EXE"
    fi
fi

if [[ -z "$PWSH_FOUND" ]]; then
    cat >&2 <<'EOF'
PowerShell (pwsh) was not found and auto-install was skipped.

Install PowerShell 7+ manually, e.g.:
    https://learn.microsoft.com/powershell/scripting/install/installing-powershell

The verify.ps1 / verify-rust.ps1 / verify-equiv.ps1 scripts require it.
EOF
    exit 1
fi
echo "  pwsh:    $PWSH_FOUND"

# ── Step 6: write env files (bash + pwsh) ──────────────────────────────
step 'Step 6: write env file'
ENV_SH="${INSTALL_ROOT}/env.sh"
ENV_PS1="${INSTALL_ROOT}/env.ps1"

cat > "$ENV_SH" <<EOF
# Auto-generated by scripts/init.sh on $(date '+%Y-%m-%d %H:%M')
# Source this to put the saw-spec-gen tools on PATH:
#     . "$ENV_SH"
export SAW_SPEC_GEN_LLVM_BIN="$LLVM_BIN"
export SAW_SPEC_GEN_SAW="$SAW_EXE"
export SAW_SPEC_GEN_SOLVER_BIN="$SOLVER_DIR"
export SAW_SPEC_GEN_PWSH="$PWSH_FOUND"
export SAW_SPEC_GEN_RUSTC="$(command -v rustc)"
case ":\${PATH}:" in
    *":\${SAW_SPEC_GEN_SOLVER_BIN}:"*) ;;
    *) export PATH="\${SAW_SPEC_GEN_SOLVER_BIN}:\${PATH}" ;;
esac
# Put the LLVM bin dir (clang, llvm-as, llvm-link, llvm-dis) on PATH so
# saw-spec-gen — which discovers these via Command::new("llvm-link")
# etc. — can find them. Required for the pre-link step that lets the
# emitted verify.saw work with stock SAW v1.5.
case ":\${PATH}:" in
    *":\${SAW_SPEC_GEN_LLVM_BIN}:"*) ;;
    *) export PATH="\${SAW_SPEC_GEN_LLVM_BIN}:\${PATH}" ;;
esac
# Put bundled pwsh on PATH only when it's our own download (not a
# system pwsh) so users who run \`pwsh\` directly pick it up.
_pwsh_dir="\$(dirname "\$SAW_SPEC_GEN_PWSH")"
case ":\${PATH}:" in
    *":\${_pwsh_dir}:"*) ;;
    *) export PATH="\${_pwsh_dir}:\${PATH}" ;;
esac
unset _pwsh_dir
# Ensure cargo's bin dir is on PATH so verify-rust.ps1 (which spawns
# pwsh that inherits this env) can find rustc / cargo.
_cargo_dir="\$(dirname "\$SAW_SPEC_GEN_RUSTC")"
case ":\${PATH}:" in
    *":\${_cargo_dir}:"*) ;;
    *) export PATH="\${_cargo_dir}:\${PATH}" ;;
esac
unset _cargo_dir
EOF
echo "  wrote: $ENV_SH"

# Also drop a pwsh env file so people running verify.ps1 under pwsh on
# Linux/macOS get the same auto-discovery as Windows users.
cat > "$ENV_PS1" <<EOF
# Auto-generated by scripts/init.sh on $(date '+%Y-%m-%d %H:%M')
# Dot-sourced by scripts/discover-tools.ps1 on every verify run.
\$env:SAW_SPEC_GEN_LLVM_BIN    = '$LLVM_BIN'
\$env:SAW_SPEC_GEN_SAW         = '$SAW_EXE'
\$env:SAW_SPEC_GEN_SOLVER_BIN  = '$SOLVER_DIR'
\$env:SAW_SPEC_GEN_PWSH        = '$PWSH_FOUND'
\$env:SAW_SPEC_GEN_RUSTC       = '$(command -v rustc)'
EOF
echo "  wrote: $ENV_PS1"

# ── Sanity check ──────────────────────────────────────────────────────
step 'Verifying installation'
ok=1
for tool in "$LLVM_BIN/clang" "$LLVM_BIN/llvm-as" "$SAW_EXE" "$SOLVER_DIR/z3" "$SPEC_GEN" "$PWSH_FOUND"; do
    if [[ -x "$tool" ]]; then
        printf '  %-45s OK\n' "$tool"
    else
        printf '  %-45s MISSING\n' "$tool"
        ok=0
    fi
done
[[ "$ok" == "1" ]] || exit 1

cat <<EOF

saw-spec-gen is ready. Source the env file once per shell:

    . "$ENV_SH"

then try a demo:

    pwsh ./verify.ps1 \\
        -CppFile     demo/bounded_loop/add_one.cpp \\
        -CryptolSpec demo/bounded_loop/add_one_spec.cry \\
        -CryptolFn   add_one_spec \\
        -Function    add_one

The bundled pwsh is at:
    $PWSH_FOUND
EOF
