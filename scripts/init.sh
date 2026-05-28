#!/usr/bin/env bash
# One-shot installer for saw-spec-gen on Linux / macOS.
#
# Idempotent. Each step probes for what's already installed and only
# downloads what's missing. Anything this script installs goes under
# $HOME/.saw-spec-gen/ so removal is a single  rm -rf .
#
# Usage:
#   bash scripts/init.sh                       # default versions
#   SAW_VERSION=1.5 bash scripts/init.sh       # pin SAW version
#   FORCE=1 bash scripts/init.sh               # redownload everything
#
# After this script finishes, scripts/discover-tools.ps1 will pick up
# the install via the env file at ~/.saw-spec-gen/env.sh — your verify
# scripts will find clang, llvm-as, saw and z3 automatically.
set -euo pipefail

SAW_VERSION="${SAW_VERSION:-1.5}"
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
if ! have rustc || ! have cargo; then
    cat >&2 <<'EOF'
rustc / cargo not found on PATH.

Install Rust via rustup: https://rustup.rs
    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh

After install, source ~/.cargo/env (or restart your shell) and re-run
scripts/init.sh.
EOF
    exit 1
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
LLVM_BIN=""
if have clang && have llvm-as; then
    LLVM_BIN="$(dirname "$(command -v clang)")"
fi
if [[ -z "$LLVM_BIN" ]]; then
    cat >&2 <<EOF
clang / llvm-as not found on PATH.

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

if [[ ! -x "$SAW_EXE" || "$FORCE" == "1" ]]; then
    case "$PLATFORM" in
        linux) ASSET="saw-${SAW_VERSION}-Linux-x86_64-with-solvers.tar.gz" ;;
        macos) ASSET="saw-${SAW_VERSION}-macOS-x86_64-with-solvers.tar.gz" ;;
    esac
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

# ── Step 5: write env files (bash + pwsh) ──────────────────────────────
step 'Step 5: write env file'
ENV_SH="${INSTALL_ROOT}/env.sh"
ENV_PS1="${INSTALL_ROOT}/env.ps1"

cat > "$ENV_SH" <<EOF
# Auto-generated by scripts/init.sh on $(date '+%Y-%m-%d %H:%M')
# Source this to put the saw-spec-gen tools on PATH:
#     . "$ENV_SH"
export SAW_SPEC_GEN_LLVM_BIN="$LLVM_BIN"
export SAW_SPEC_GEN_SAW="$SAW_EXE"
export SAW_SPEC_GEN_SOLVER_BIN="$SOLVER_DIR"
case ":\${PATH}:" in
    *":\${SAW_SPEC_GEN_SOLVER_BIN}:"*) ;;
    *) export PATH="\${SAW_SPEC_GEN_SOLVER_BIN}:\${PATH}" ;;
esac
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
EOF
echo "  wrote: $ENV_PS1"

# ── Sanity check ──────────────────────────────────────────────────────
step 'Verifying installation'
ok=1
for tool in "$LLVM_BIN/clang" "$LLVM_BIN/llvm-as" "$SAW_EXE" "$SOLVER_DIR/z3" "$SPEC_GEN"; do
    if [[ -x "$tool" ]]; then
        printf '  %-45s OK\n' "$tool"
    else
        printf '  %-45s MISSING\n' "$tool"
        ok=0
    fi
done
[[ "$ok" == "1" ]] || exit 1

cat <<EOF

saw-spec-gen is ready. Try (with pwsh installed):

    pwsh ./verify.ps1 \\
        -CppFile     demo/bounded_loop/add_one.cpp \\
        -CryptolSpec demo/bounded_loop/add_one_spec.cry \\
        -CryptolFn   add_one_spec \\
        -Function    add_one

If you don't have pwsh: install PowerShell 7+ from
https://learn.microsoft.com/powershell/scripting/install/installing-powershell
(the verify scripts are written in pwsh so they can share code with
the Windows install).

To put the tools on PATH in a plain bash shell:

    . "$ENV_SH"
EOF
