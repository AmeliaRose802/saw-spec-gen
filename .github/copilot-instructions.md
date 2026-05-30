# Copilot / AI agent instructions

These rules apply to any AI agent (Copilot, Claude, etc.) working in this
repository. Human contributors should follow them too.

## Hard rules

### 500 non-whitespace lines per file

No source file may exceed **500 non-whitespace lines**. This is enforced by:

- `scripts/check-line-count.sh` (also available as `scripts/check-line-count.ps1`)
- The `.githooks/pre-commit` hook (activate with `git config core.hooksPath .githooks`)
- The `line-count` job in `.github/workflows/ci.yml`

When adding or modifying code:

1. **Do not create new files over the limit.** If a feature naturally needs
   more than ~500 lines, split it into multiple modules along clear seams
   (parsing vs. emission, public API vs. internal helpers, per-target
   backends, etc.).
2. **Refactor when an edit pushes a file over the limit.** Extract cohesive
   pieces into sibling modules before adding more code to a file that is
   already close to the threshold.
3. **Never add entries to `.linecount-allow`.** That file lists pre-existing
   legacy files that are scheduled to be refactored. Adding new entries
   defeats the purpose of the limit. If you believe an exception is
   required, surface it to a human reviewer and explain why splitting is
   infeasible — do not silently add the path.
4. **Prefer shrinking files in the allow-list.** When touching a legacy
   oversize file, opportunistically extract self-contained sections into
   new modules. Once a file drops below 500 non-whitespace lines, remove
   it from `.linecount-allow`.

The limit counts only lines containing at least one non-whitespace
character, so comments and doc-comments do count. Blank lines do not.

### Source extensions covered

The check applies to: `.rs`, `.py`, `.sh`, `.ps1`, `.psm1`, `.js`, `.ts`,
`.tsx`, `.jsx`, `.c`, `.cc`, `.cpp`, `.cxx`, `.h`, `.hh`, `.hpp`, `.hxx`,
`.saw`, `.cry`, `.java`, `.go`, `.rb`. Generated data files (JSON, LLVM
IR, bitcode) are intentionally excluded.

## Running the check locally

```bash
# All tracked source files
bash scripts/check-line-count.sh

# Specific files
bash scripts/check-line-count.sh src/main.rs src/cryptol_emit.rs
```

PowerShell equivalent:

```powershell
pwsh ./scripts/check-line-count.ps1
```

## Enabling the pre-commit hook

```bash
git config core.hooksPath .githooks
```

This is a one-time setup per clone.

## end-to-end test suite

End-to-end test regressions live under `tests/e2e/`:

- `cases.psd1` — declarative manifest (one entry per test case, with the
  expected `SAT`/`UNSAT`/`VERIFIED`/`DISPROVED`/`EQUIVALENT`/
  `NOT EQUIVALENT` verdict).
- `Run-E2ETests.ps1` — runner that loads the manifest, dispatches to
  `verify.ps1` / `verify-rust.ps1` / `verify-equiv.ps1` (or any custom
  per-test script), captures `RESULT:` lines, and emits a TAP-style
  summary. Returns non-zero if any case disagrees with its expected
  verdict.

Run locally:

```powershell
pwsh tests/e2e/Run-E2ETests.ps1               # full suite
pwsh tests/e2e/Run-E2ETests.ps1 -Tag cpp_havoc
pwsh tests/e2e/Run-E2ETests.ps1 -List         # dry run
```

The pre-commit hook (above) runs the line-count check, `cargo fmt --
--check` (matches CI; set `SKIP_FMT=1` to skip), `cargo clippy
-- -D warnings` (matches CI; set `SKIP_CLIPPY=1` to skip), **and** the
SAW suite. The runner auto-skips when SAW is not installed on the machine
(no-op exit 0). Set `SKIP_SAW_TESTS=1` in the environment to skip
explicitly. When adding a new test, append an entry to `cases.psd1` —
do not write a bespoke runner script. See
[tests/e2e/README.md](../tests/e2e/README.md) for the
manifest schema.
