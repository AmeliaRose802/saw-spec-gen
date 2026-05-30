# Copilot / AI agent instructions

Rules for any AI agent (Copilot, Claude, etc.) working in this repo.
Human contributors should follow them too.

## Hard rules

### 500 non-whitespace lines per file

No source file may exceed **500 non-whitespace lines** (blank lines
don't count, comments do). Enforced by `scripts/check-line-count.sh`
(or `.ps1`), the `.githooks/pre-commit` hook
(`git config core.hooksPath .githooks`), and the `line-count` CI job.

When editing:

1. **Don't create new files over the limit.** Split along clear seams
   (parsing vs emission, public API vs internal helpers, per-target
   backends).
2. **Refactor when an edit pushes over the limit.** Extract cohesive
   pieces before appending more code to a near-threshold file.
3. **Never add entries to `.linecount-allow`.** That file lists
   pre-existing legacy files scheduled for refactor. If you think an
   exception is required, raise it with a human reviewer — don't
   silently add the path.
4. **Prefer shrinking files in the allow-list.** When touching a
   legacy oversize file, opportunistically extract self-contained
   sections. Once a file drops below 500, remove it from
   `.linecount-allow`.

Extensions checked: `.rs`, `.py`, `.sh`, `.ps1`, `.psm1`, `.js`,
`.ts`, `.tsx`, `.jsx`, `.c`, `.cc`, `.cpp`, `.cxx`, `.h`, `.hh`,
`.hpp`, `.hxx`, `.saw`, `.cry`, `.java`, `.go`, `.rb`. Generated data
files (JSON, LLVM IR, bitcode) are excluded.

## Running the checks

```bash
bash scripts/check-line-count.sh                  # all tracked source files
bash scripts/check-line-count.sh src/main.rs ...  # specific files
pwsh ./scripts/check-line-count.ps1               # PowerShell equivalent
```

## Pre-commit hook

```bash
git config core.hooksPath .githooks   # one-time, per clone
```

Runs the line-count check, `cargo fmt -- --check` (`SKIP_FMT=1` to
skip), `cargo clippy -- -D warnings` (`SKIP_CLIPPY=1` to skip), and
the e2e SAW suite (auto-skips when SAW isn't installed; set
`SKIP_SAW_TESTS=1` to skip explicitly).

## End-to-end suite

Lives under `tests/e2e/` — see
[tests/e2e/README.md](../tests/e2e/README.md) for the manifest schema
and the full tag list. Common invocations:

```powershell
pwsh tests/e2e/Run-E2ETests.ps1                   # full suite
pwsh tests/e2e/Run-E2ETests.ps1 -Tag cpp_havoc
pwsh tests/e2e/Run-E2ETests.ps1 -List             # dry run
```

When adding a new test, append an entry to
[`tests/e2e/cases.psd1`](../tests/e2e/cases.psd1) — do not write a
bespoke runner script.
