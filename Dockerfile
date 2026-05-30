# syntax=docker/dockerfile:1.7
#
# Pre-built CI image for saw-spec-gen's Linux SAW-demo job.
#
# Layers the same toolchain that scripts/init.sh installs locally:
#   - LLVM 20.1.6 (clang, llvm-as, opt, llvm-link)
#   - SAW 1.5 with bundled solvers (z3, yices, cvc4/5, abc, ...)
#   - PowerShell 7.6.2 (verify*.ps1 and discover-tools.ps1 are pwsh)
#   - Rust stable (rustup, minimal profile)
#
# Tools are dropped at the exact paths scripts/discover-tools.ps1 looks
# for ($HOME/.saw-spec-gen/{llvm,saw}/bin), so no env.ps1 is required —
# discovery just works. Built and published by
# .github/workflows/publish-ci-image.yml to:
#     ghcr.io/ameliarose802/saw-spec-gen-ci:latest
#
# Bump SAW_VERSION / LLVM_VERSION / PWSH_VERSION here when scripts/init.sh
# pins are bumped, then re-run the publish workflow.

FROM ubuntu:22.04

ARG SAW_VERSION=1.5
ARG LLVM_VERSION=20.1.6
ARG PWSH_VERSION=7.6.2
ARG RUST_TOOLCHAIN=stable

ENV DEBIAN_FRONTEND=noninteractive \
    LANG=C.UTF-8 \
    LC_ALL=C.UTF-8

# System deps: curl/tar/xz for tarball installs, build-essential for
# native cargo deps, libssl-dev for openssl-sys, libtinfo5/libncurses5
# for the LLVM 20 binaries (clang dynamically links libtinfo.so.5).
RUN apt-get update \
 && apt-get install -y --no-install-recommends \
      ca-certificates curl wget tar gzip xz-utils \
      git pkg-config build-essential libssl-dev \
      zlib1g libtinfo5 libncurses5 \
      libicu70 libssl3 \
 && rm -rf /var/lib/apt/lists/*

# ── LLVM 20.1.6 (uses the same asset scripts/init.sh downloads) ───────
RUN mkdir -p /root/.saw-spec-gen/llvm \
 && curl -fsSL -o /tmp/llvm.tar.xz \
      "https://github.com/llvm/llvm-project/releases/download/llvmorg-${LLVM_VERSION}/LLVM-${LLVM_VERSION}-Linux-X64.tar.xz" \
 && tar -xJf /tmp/llvm.tar.xz -C /root/.saw-spec-gen/llvm --strip-components=1 \
 && rm /tmp/llvm.tar.xz \
 && /root/.saw-spec-gen/llvm/bin/clang --version

# ── SAW 1.5 with bundled solvers (ubuntu-22.04 build) ─────────────────
RUN mkdir -p /root/.saw-spec-gen/saw \
 && curl -fsSL -o /tmp/saw.tar.gz \
      "https://github.com/GaloisInc/saw-script/releases/download/v${SAW_VERSION}/saw-${SAW_VERSION}-ubuntu-22.04-X64-with-solvers.tar.gz" \
 && tar -xzf /tmp/saw.tar.gz -C /root/.saw-spec-gen/saw --strip-components=1 \
 && rm /tmp/saw.tar.gz \
 && /root/.saw-spec-gen/saw/bin/saw --version

# ── PowerShell 7 (self-contained tarball; no Microsoft apt repo dance) ─
RUN mkdir -p /opt/pwsh \
 && curl -fsSL -o /tmp/pwsh.tar.gz \
      "https://github.com/PowerShell/PowerShell/releases/download/v${PWSH_VERSION}/powershell-${PWSH_VERSION}-linux-x64.tar.gz" \
 && tar -xzf /tmp/pwsh.tar.gz -C /opt/pwsh \
 && chmod +x /opt/pwsh/pwsh \
 && ln -s /opt/pwsh/pwsh /usr/local/bin/pwsh \
 && rm /tmp/pwsh.tar.gz \
 && pwsh -NoProfile -Command '$PSVersionTable.PSVersion'

# ── Rust toolchain (minimal profile; rustfmt/clippy live in their own
#     CI jobs and pull their own toolchain via dtolnay/rust-toolchain) ──
ENV RUSTUP_HOME=/usr/local/rustup \
    CARGO_HOME=/usr/local/cargo
RUN curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs \
      | sh -s -- -y --default-toolchain ${RUST_TOOLCHAIN} --profile minimal --no-modify-path \
 && /usr/local/cargo/bin/rustc --version

# Tools on PATH for any shell (bash, pwsh, ...). discover-tools.ps1 also
# probes $HOME/.saw-spec-gen/{llvm,saw}/bin explicitly so this is belt-
# and-braces.
ENV PATH=/root/.saw-spec-gen/llvm/bin:/root/.saw-spec-gen/saw/bin:/root/.saw-spec-gen/exception-lower/bin:/usr/local/cargo/bin:${PATH}

# ── llvm-exception-lower (C++ throw/catch lowering for SAW) ───────────
# The install script downloads a prebuilt binary from GitHub Releases
# or falls back to a cmake source build. discover-tools.ps1 probes
# $HOME/.saw-spec-gen/exception-lower/bin/ automatically.
COPY scripts/install-exception-lower.sh /tmp/install-exception-lower.sh
RUN chmod +x /tmp/install-exception-lower.sh \
 && LLVM_BIN=/root/.saw-spec-gen/llvm/bin /tmp/install-exception-lower.sh \
 && rm -f /tmp/install-exception-lower.sh \
 && /root/.saw-spec-gen/exception-lower/bin/exception-lower --help 2>&1 | head -1

LABEL org.opencontainers.image.source="https://github.com/AmeliaRose802/saw-spec-gen" \
      org.opencontainers.image.description="saw-spec-gen CI toolchain: SAW ${SAW_VERSION}, LLVM ${LLVM_VERSION}, PowerShell ${PWSH_VERSION}, Rust ${RUST_TOOLCHAIN}" \
      org.opencontainers.image.licenses="MIT"

WORKDIR /work
CMD ["/bin/bash"]
