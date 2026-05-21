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
