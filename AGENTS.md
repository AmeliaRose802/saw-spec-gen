# Agent Instructions

This project uses **bd** (beads) for issue tracking — see the
auto-managed section at the bottom of this file, or run `bd prime`
for full workflow context.

## Non-Interactive Shell Commands

Shell commands like `cp`, `mv`, `rm` may be aliased to `-i` mode on
some systems and hang waiting for y/n. **Always pass non-interactive
flags:**

```bash
cp -f source dest       # NOT: cp source dest
mv -f source dest
rm -f file              # rm -rf for recursive
cp -rf source dest      # recursive copy
```

Other commands worth knowing: `scp`/`ssh` with `-o BatchMode=yes`,
`apt-get -y`, `HOMEBREW_NO_AUTO_UPDATE=1`.

## Project rules & workflow

This repo is a **Rust CLI** (`saw-spec-gen`) plus a set of PowerShell
driver scripts (`verify*.ps1`) that wrap `clang`/`rustc` → bitcode →
SAW. The full coding rules live in
[.github/copilot-instructions.md](.github/copilot-instructions.md) and
[CLAUDE.md](CLAUDE.md); the highlights every agent must follow:

### 500 non-whitespace lines per file (hard limit)

No source file may exceed **500 non-whitespace lines** (blank lines
don't count, comments do). Enforced by `scripts/check-line-count.ps1`
(or `.sh`), the `.githooks/pre-commit` hook, and the `line-count` CI
job. Split along clear seams (parsing vs emission, public API vs
helpers) instead of growing a file past the limit. **Never** add
entries to `.linecount-allow`.

### Build, test, lint

```powershell
cargo build
cargo test                                  # unit + integration
cargo clippy --all-targets -- -D warnings
cargo fmt                                   # or: cargo fmt -- --check
pwsh ./scripts/check-line-count.ps1
```

### End-to-end suite

The SAW/clang/rustc end-to-end tests live under `tests/e2e/`, driven by
a single manifest. Auto-skips when SAW isn't installed.

```powershell
pwsh tests/e2e/Run-E2ETests.ps1                  # full suite
pwsh tests/e2e/Run-E2ETests.ps1 -Tag cpp_stateful
pwsh tests/e2e/Run-E2ETests.ps1 -List            # dry run
```

Add new cases by appending to
[`tests/e2e/cases.psd1`](tests/e2e/cases.psd1) — see
[tests/e2e/README.md](tests/e2e/README.md) for the schema and tag list.
Do not write a bespoke runner script.

**No custom runners.** Do **not** add `Runner = 'custom'` or
`Script = ...` entries to `cases.psd1`. Use only `cpp`, `rust`, or
`equiv`. Extend a built-in runner if a capability is missing. CI
enforces this with the `no-custom-runners` job; run locally with:

```bash
bash scripts/check-no-custom-runners.sh
```

### Pre-commit hook

```powershell
git config core.hooksPath .githooks   # one-time, per clone
```

Runs the line-count check, `cargo fmt -- --check`, `cargo clippy -- -D
warnings`, and the e2e suite.

### Platform

PowerShell-first: on Windows use PowerShell 7 (`pwsh`), not Windows
PowerShell 5.1. Rust 1.85+ is required. Avoid
`Select-Object -First`/`-Last` in pipelines — it can hang.

<!-- BEGIN BEADS INTEGRATION v:1 profile:minimal hash:ca08a54f -->
## Beads Issue Tracker

This project uses **bd (beads)** for issue tracking. Run `bd prime` to see full workflow context and commands.

### Quick Reference

```bash
bd ready              # Find available work
bd show <id>          # View issue details
bd update <id> --claim  # Claim work
bd close <id>         # Complete work
```

### Rules

- Use `bd` for ALL task tracking — do NOT use TodoWrite, TaskCreate, or markdown TODO lists
- Run `bd prime` for detailed command reference and session close protocol
- Use `bd remember` for persistent knowledge — do NOT use MEMORY.md files

## Session Completion

**When ending a work session**, you MUST complete ALL steps below. Work is NOT complete until `git push` succeeds.

**MANDATORY WORKFLOW:**

1. **File issues for remaining work** - Create issues for anything that needs follow-up
2. **Run quality gates** (if code changed) - Tests, linters, builds
3. **Update issue status** - Close finished work, update in-progress items
4. **PUSH TO REMOTE** - This is MANDATORY:
   ```bash
   git pull --rebase
   bd dolt push
   git push
   git status  # MUST show "up to date with origin"
   ```
5. **Clean up** - Clear stashes, prune remote branches
6. **Verify** - All changes committed AND pushed
7. **Hand off** - Provide context for next session

**CRITICAL RULES:**
- Work is NOT complete until `git push` succeeds
- NEVER stop before pushing - that leaves work stranded locally
- NEVER say "ready to push when you are" - YOU must push
- If push fails, resolve and retry until it succeeds
<!-- END BEADS INTEGRATION -->
