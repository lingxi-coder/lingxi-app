# `lingxi-cli` npm + uv Distribution — Design

> Make the LingXi TUI (`lingxi-cli`) installable via `npm install -g lingxi`
> (and `bun`) and `uv tool install lingxi` (and `pip` / `uvx`), by **copying
> codex's packaging mechanism** (`./codex`) and adapting names to LingXi. Copy
> code verbatim wherever possible; invent only the one piece codex lacks (a
> Python console-script entrypoint).

## §0 · Context & decisions

The Rust binary is `lingxi-cli` (`lingxi-code/apps/cli`, `[[bin]] name = "lingxi-cli"`),
which embeds the `tui` crate — i.e. the TUI *is* this binary. LingXi has CI
(`.github/workflows/ci.yml`) but **no release pipeline and no npm/python
packaging** today.

codex (`./codex`) distributes the same kind of Rust CLI two ways, which we mirror:

- **npm** — thin `@openai/codex` package (`codex-cli/`, only `bin/codex.js`) + 6
  per-platform optional-dependency packages carrying `vendor/<triple>/bin/codex`;
  `scripts/build_npm_package.py` stages them.
- **PyPI/uv** — `openai-codex-cli-bin` (`sdk/python-runtime/`): wheel-only,
  per-platform, hatchling + a custom `hatch_build.py` that platform-tags each
  wheel; binary bundled under `src/codex_cli_bin/bin/`. It exposes
  `bundled_codex_path()` but **no console script** (it feeds codex's Python SDK).

**Locked decisions (brainstorm):**

| # | Decision | Choice |
|---|---|---|
| Names | package + command | npm `lingxi` (+ platform pkgs `lingxi-<os>-<arch>`); PyPI `lingxi`; command `lingxi`; bundled binary file `lingxi` |
| Scope | how far | packaging scaffolding **+** a GitHub release workflow (cross-build → stage → publish), publish steps gated on secrets |
| Platforms | which targets | all 6 codex targets (Linux musl x64/arm64, macOS x64/arm64, Windows msvc x64/arm64) |

**Target-triple → package map (identical to codex):**

| Triple | os | cpu | npm platform pkg | wheel platform tag (example) |
|---|---|---|---|---|
| `x86_64-unknown-linux-musl` | linux | x64 | `lingxi-linux-x64` | `manylinux*_x86_64` / `musllinux*_x86_64` |
| `aarch64-unknown-linux-musl` | linux | arm64 | `lingxi-linux-arm64` | `musllinux*_aarch64` |
| `x86_64-apple-darwin` | darwin | x64 | `lingxi-darwin-x64` | `macosx_*_x86_64` |
| `aarch64-apple-darwin` | darwin | arm64 | `lingxi-darwin-arm64` | `macosx_*_arm64` |
| `x86_64-pc-windows-msvc` | win32 | x64 | `lingxi-win32-x64` | `win_amd64` |
| `aarch64-pc-windows-msvc` | win32 | arm64 | `lingxi-win32-arm64` | `win_arm64` |

The wheel platform tag is computed by the copied `hatch_build.py` (via
`packaging.tags.sys_tags()` or the `CODEX_CLI_BIN_PLATFORM_TAG`-analogue env
override); the table's tag column is illustrative.

---

## §1 · Goals & non-goals

### Goals
- `npm install -g lingxi` / `bun install -g lingxi` → working `lingxi` command.
- `uv tool install lingxi` / `uvx lingxi` / `pip install lingxi` → working `lingxi` command.
- A tagged release (`v*`) cross-builds the 6 binaries and stages both package
  forms; publishing to npm + PyPI runs when the respective token secrets exist.
- Maximum code reuse from `./codex` — launchers, staging script, hatch hook,
  and binary-locator are copied and renamed, not reinvented.

### Non-goals
- Homebrew / cargo-binstall / other channels (codex has brew; out of scope here).
- Signing / notarization of binaries.
- Changing `lingxi-cli` itself (no Rust source changes beyond what cross-build needs).
- Bundling a Python SDK (codex's `sdk/python` is separate; we only need the CLI).

---

## §2 · Components

### §2.1 · npm — `lingxi-code/npm/` (copy of `codex/codex-cli/`)

```
lingxi-code/npm/
  package.json                 # name "lingxi", bin lingxi→bin/lingxi.js, optionalDependencies x6
  bin/lingxi.js                # copied from codex.js, renamed
  scripts/build_npm_package.py # copied from codex's, platform table re-pointed
  .gitignore                   # ignore staged vendor/ + platform-pkg output
```

- **`bin/lingxi.js`** — `cp codex/codex-cli/bin/codex.js`, then:
  - `PLATFORM_PACKAGE_BY_TARGET` values → `lingxi-linux-x64` … `lingxi-win32-arm64`
    (same 6 triple keys).
  - `codexBinaryName` → `lingxiBinaryName` = `process.platform === "win32" ? "lingxi.exe" : "lingxi"`.
  - `CODEX_MANAGED_BY_NPM` / `CODEX_MANAGED_BY_BUN` / `CODEX_MANAGED_PACKAGE_ROOT`
    → `LINGXI_*`; the `npm install -g @openai/codex@latest` hint string →
    `npm install -g lingxi@latest`.
  - Everything else (platform detection, `resolveNativePackage`, async `spawn`,
    `SIGINT/SIGTERM/SIGHUP` forwarding, PATH augmentation, exit-code/signal
    mirroring, bun detection) copied unchanged.
- **`package.json`** — `name: "lingxi"`, `version: "0.0.0-dev"` placeholder,
  `bin: { "lingxi": "bin/lingxi.js" }`, `type: "module"`, `engines.node >=16`,
  `files: ["bin/lingxi.js"]`, and `optionalDependencies` listing all 6
  `lingxi-<os>-<arch>` at the release version (added/rewritten by the staging
  script). The launcher's `require.resolve(\`${platformPackage}/package.json\`)`
  depends on these being optional deps.
- **Platform package** (generated, one per triple): `package.json` with
  `name`, `version`, `os: [<os>]`, `cpu: [<cpu>]`, and the file
  `vendor/<triple>/bin/lingxi` (the cross-built `lingxi-cli`, renamed to `lingxi`).
- **`scripts/build_npm_package.py`** — `cp codex/codex-cli/scripts/build_npm_package.py`,
  then re-point its platform table (the `CODEX_PLATFORM_PACKAGES` dict) to the 6
  `lingxi-*` names/triples, set `CODEX_NPM_NAME` → `lingxi`, and source binaries
  from `lingxi-code/target/<triple>/release/lingxi-cli` (copied to
  `vendor/<triple>/bin/lingxi`). Drop codex-only components
  (`responses-api-proxy`, `codex-sdk`, `codex-resources`/`codex-path` if unused
  by lingxi). Keep `--package` / `--version` / `--release-version` / staging-dir
  flags as-is.

### §2.2 · uv/PyPI — `lingxi-code/pypi/` (copy of `codex/sdk/python-runtime/`)

```
lingxi-code/pypi/
  pyproject.toml               # hatchling, wheel-only, name "lingxi", [project.scripts] lingxi
  hatch_build.py               # copied verbatim (platform-tags the wheel)
  src/lingxi_cli_bin/__init__.py   # copied locators (renamed) + NEW main() entrypoint
```

- **`pyproject.toml`** — copy of codex's, with `name = "lingxi"`,
  `wheel.packages = ["src/lingxi_cli_bin"]`, include
  `src/lingxi_cli_bin/bin/**`, and **add**:
  ```toml
  [project.scripts]
  lingxi = "lingxi_cli_bin:main"
  ```
  Keep `[tool.hatch.build.targets.wheel.hooks.custom]` (the platform-tag hook)
  and the sdist-disabled behavior.
- **`hatch_build.py`** — `cp codex/sdk/python-runtime/hatch_build.py` verbatim
  (it sets `build_data["tag"] = "py3-none-<platform_tag>"`, honoring a
  `*_PLATFORM_TAG` env override). Rename the env var
  `CODEX_CLI_BIN_PLATFORM_TAG` → `LINGXI_CLI_BIN_PLATFORM_TAG`.
- **`src/lingxi_cli_bin/__init__.py`** — copy codex's `bundled_package_dir()` /
  `bundled_codex_path()` / `bundled_path_dir()` (renamed `bundled_lingxi_path()`,
  binary file `lingxi`/`lingxi.exe`, metadata filename
  `lingxi-package.json`), **plus the new `main()`** — the only invented piece:
  ```python
  def main() -> int:
      """Console entrypoint: exec the bundled lingxi binary with argv passthrough."""
      import sys, subprocess, signal
      binary = bundled_lingxi_path()
      # forward all argv; inherit stdio; mirror exit code/signal (Windows: no execv)
      proc = subprocess.Popen([str(binary), *sys.argv[1:]])
      for sig in (signal.SIGINT, signal.SIGTERM):
          ...  # forward to child (POSIX); Windows handles Ctrl-C natively
      return proc.wait()
  ```
  Semantics mirror `bin/lingxi.js` (signal forwarding, exit-code passthrough).
  On POSIX, `os.execv` is acceptable too (replaces the process); use `subprocess`
  for cross-platform signal/exit parity.

### §2.3 · Release workflow — `.github/workflows/lingxi-release.yml`

Adapted from codex's `rust-release.yml` + `rust-release-windows.yml` +
`python-sdk-release.yml`. Trigger: push tag `v*` (and `workflow_dispatch`).

1. **build matrix** (6 jobs): each `cargo build --release -p cli --target <triple>`
   from `lingxi-code/` on the appropriate runner:
   - Linux musl x64/arm64: `ubuntu-latest` + `rustup target add` + `musl-tools`
     (arm64 via `cross` or the gnu→musl cross linker — copy codex's setup).
   - macOS x64/arm64: `macos-latest` (arm64 native, x64 via `--target`).
   - Windows msvc x64/arm64: `windows-latest` (arm64 cross via the MSVC arm64 toolchain).
   - Toolchain pinned to **Rust 1.82** (workspace `rust-toolchain`). Upload each
     `target/<triple>/release/lingxi-cli` as an artifact.
2. **package job**: download artifacts; run `npm/scripts/build_npm_package.py`
   (stages main + 6 platform npm packages); build 6 platform wheels from
   `pypi/` (`uv build --wheel` per target with `LINGXI_CLI_BIN_PLATFORM_TAG`).
3. **publish job** (gated): `npm publish` for the main + 6 platform packages
   when `secrets.NPM_TOKEN` is set; `uv publish` / `twine upload` to PyPI when
   `secrets.PYPI_TOKEN` is set. `if:` guards skip publish cleanly when secrets
   are absent, so the build+package path always runs.

---

## §3 · Cross-build feasibility (spike — first implementation step)

`lingxi-cli` pulls `engine-desktop` + `platform-posix-minimal` and is only known
to build for the host (macOS arm64) today. Cross-compiling to musl + Windows on
the pinned **Rust 1.82** is unproven and is the project's main risk (same class
as the AWS-on-1.82 issue). Therefore **Task 1 is a spike**: attempt
`cargo build --release -p cli --target <triple>` for each of the 6 triples and
record pass/fail. Any target that cannot build on 1.82 (after reasonable
linker/dep setup) is **dropped from the matrix and the package tables with a
documented note** (the npm `optionalDependencies` / launcher map and the wheel
set simply omit it). The mechanism ships for whatever subset builds; the others
are tracked follow-ups. This keeps the deliverable honest and unblocked.

---

## §4 · Error handling

- **npm, no matching platform pkg:** the copied launcher already throws
  `Missing optional dependency lingxi-<os>-<arch>. Reinstall: npm install -g lingxi@latest`.
- **npm, unsupported platform/arch:** copied launcher throws
  `Unsupported platform: <platform> (<arch>)`.
- **uv, missing bundled binary:** `bundled_lingxi_path()` raises
  `FileNotFoundError("lingxi is installed but missing its packaged binary at …")`
  (copied behavior).
- **child exit/signals:** both launchers mirror the child's exit code and
  re-raise terminating signals (copied semantics).

---

## §5 · Testing

- **Launcher unit tests (npm):** a Node test creates a temp `vendor/<triple>/bin/lingxi`
  stub and asserts `bin/lingxi.js` resolves it; and that a missing package
  yields the documented error. (Mirror any tests codex ships; otherwise a small
  smoke test.)
- **Locator unit test (uv):** pytest asserts `bundled_lingxi_path()` finds a
  stubbed binary and raises on absence; `main()` returns the child's exit code
  for a fake binary that exits N.
- **Staging test:** run `build_npm_package.py --package lingxi --version 0.0.0`
  against a fixture binary; assert the staged tree (main `package.json`
  optionalDependencies, each platform `package.json` `os`/`cpu`, vendor binary
  path) byte-matches expectations.
- **End-to-end host smoke (CI + local):** build the host triple, stage both
  packages, then `node npm/bin/lingxi.js --version` and
  `uvx --from ./pypi lingxi --version` (or `python -c "import lingxi_cli_bin; lingxi_cli_bin.main()"`)
  print the `lingxi-cli` version.
- **Workflow:** `act`-style dry-run not required; the publish steps are
  secret-gated so the build+package path is exercised on every tag.

---

## §6 · File-structure summary

| Path | Origin | Change |
|---|---|---|
| `lingxi-code/npm/bin/lingxi.js` | `codex/codex-cli/bin/codex.js` | rename codex→lingxi (map, binary name, env vars, hint) |
| `lingxi-code/npm/package.json` | `codex/codex-cli/package.json` | name `lingxi`, 6 optionalDependencies |
| `lingxi-code/npm/scripts/build_npm_package.py` | codex's | platform table → lingxi, binary source path, drop codex-only components |
| `lingxi-code/pypi/pyproject.toml` | `codex/sdk/python-runtime/pyproject.toml` | name `lingxi`, add `[project.scripts]` |
| `lingxi-code/pypi/hatch_build.py` | codex's | rename env var only |
| `lingxi-code/pypi/src/lingxi_cli_bin/__init__.py` | codex's `codex_cli_bin/__init__.py` | rename + **add `main()`** |
| `.github/workflows/lingxi-release.yml` | codex `rust-release*.yml` + `python-sdk-release.yml` | lingxi targets, `-p cli`, secret-gated publish |
| `lingxi-code/.gitignore` (append) | — | ignore staged `npm/vendor/`, `pypi/dist/`, platform-pkg output |

---

## §7 · Open questions / future work
- Homebrew tap + cargo-binstall metadata (codex has brew) — later.
- Binary signing / macOS notarization — later.
- A combined "meta" that also ships the Python SDK — out of scope (CLI only).
