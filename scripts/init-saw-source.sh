# shellcheck shell=bash
# Sourced by scripts/init.sh. Implements the optional Step 4b
# "build SAW from source" path (SAW_SOURCE=fork|upstream).
#
# Expects these to already be set / defined by the caller:
#   - functions: step, have, download_extract_tarball
#   - vars:      INSTALL_ROOT, SAW_EXE, SAW_SOURCE, SAW_FORK_REPO,
#                SAW_UPSTREAM_REPO, SAW_FORK_REF, GHC_VERSION,
#                CABAL_VERSION, SAW_BUILD_JOBS, SKIP_SUDO_INSTALL, FORCE

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
        # Important: printf with no args on an empty array would still emit
        # one empty line, which mapfile reads as an element of size 1 ->
        # caller thinks something is still missing. Emit nothing instead.
        if (( ${#missing[@]} > 0 )); then
            printf '%s\n' "${missing[@]}"
        fi
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
