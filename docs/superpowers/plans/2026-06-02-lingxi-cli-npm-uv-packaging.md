# `lingxi-cli` npm + uv Distribution — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make `lingxi-cli` (the TUI) installable via `npm install -g lingxi` (and `bun`) and `uv tool install lingxi` (and `pip`/`uvx`), by copying codex's packaging mechanism (`./codex`) and renaming to LingXi.

**Architecture:** Two thin launcher packages locate + exec a prebuilt `lingxi-cli` binary: an npm package (`lingxi` + 6 per-platform optional-dep packages bundling the binary) and a PyPI/uv package (6 platform-tagged wheels bundling the binary, with a console-script shim). A tagged-release GitHub workflow cross-builds the 6 binaries, stages both forms, and publishes (publish gated on secrets).

**Tech Stack:** Node ESM launcher (copied from codex `bin/codex.js`); Python 3.10+ / hatchling wheel (copied from codex `sdk/python-runtime`); a Python staging script (copied from codex `build_npm_package.py`); GitHub Actions; Rust 1.82 cross-compile.

**Spec:** `docs/superpowers/specs/2026-06-02-lingxi-cli-npm-uv-packaging-design.md`. Branch: `lingxi-cli-npm-uv-packaging` (off `main`). The codex source to copy is at `./codex` (untracked reference checkout).

**Mandate:** Copy codex's code verbatim where possible; the ONLY net-new code is the Python console-script `main()` (codex's bin wheel has none) and the LingXi release workflow.

---

## File-structure map

| Path | Origin | Responsibility |
|---|---|---|
| `lingxi-code/npm/bin/lingxi.js` | copy `codex/codex-cli/bin/codex.js` | Node launcher: detect platform → resolve platform pkg → exec binary |
| `lingxi-code/npm/package.json` | new (modeled on codex) | `lingxi` main package: bin + 6 optionalDependencies |
| `lingxi-code/npm/scripts/build_npm_package.py` | copy `codex/codex-cli/scripts/build_npm_package.py` | Stage main + 6 platform packages from built binaries |
| `lingxi-code/npm/.gitignore` | new | ignore staged `vendor/`, `dist/` |
| `lingxi-code/pypi/pyproject.toml` | new (modeled on codex `sdk/python-runtime`) | wheel-only, platform-tagged, `[project.scripts] lingxi` |
| `lingxi-code/pypi/hatch_build.py` | copy `codex/sdk/python-runtime/hatch_build.py` | platform-tag each wheel |
| `lingxi-code/pypi/src/lingxi_cli_bin/__init__.py` | copy `codex/sdk/python-runtime/src/codex_cli_bin/__init__.py` + new `main()` | locate bundled binary + console entrypoint |
| `lingxi-code/pypi/src/lingxi_cli_bin/__main__.py` | new | `python -m lingxi_cli_bin` → `main()` |
| `.github/workflows/lingxi-release.yml` | new (LingXi-authored) | cross-build 6 targets → stage → secret-gated publish |
| `lingxi-code/.gitignore` (append) | — | ignore packaging build output |

Target triples (from codex, identical): `x86_64-unknown-linux-musl`, `aarch64-unknown-linux-musl`, `x86_64-apple-darwin`, `aarch64-apple-darwin`, `x86_64-pc-windows-msvc`, `aarch64-pc-windows-msvc` → packages `lingxi-{linux-x64,linux-arm64,darwin-x64,darwin-arm64,win32-x64,win32-arm64}`.

---

## Task 1: Cross-build feasibility spike (gating — do FIRST)

`lingxi-cli` is only known to build for the host (macOS arm64). Determine which of the 6 triples build on Rust 1.82 before committing the matrix.

**Files:** none committed (produces a findings note appended to the plan / spec).

- [ ] **Step 1 — add rustup targets + attempt host-adjacent builds.** From `lingxi-code/`, for each target run a build and record pass/fail:
```bash
cd lingxi-code
for T in aarch64-apple-darwin x86_64-apple-darwin x86_64-unknown-linux-musl aarch64-unknown-linux-musl x86_64-pc-windows-msvc aarch64-pc-windows-msvc; do
  rustup target add "$T" 2>/dev/null || true
  echo "=== $T ==="
  cargo build --release -p cli --target "$T" 2>&1 | tail -3
done
```
Native host (`aarch64-apple-darwin`) must pass. Cross targets that need a linker (musl, windows) will likely fail locally without toolchains — that is EXPECTED; the authoritative cross-build happens in CI (Task 6) on the right runners. The goal here is to (a) confirm the host build and the binary name `target/<triple>/release/lingxi-cli`, and (b) catch any source-level portability blocker (e.g. a posix-only dependency that breaks `cfg(windows)` *compilation* of `-p cli`).
- [ ] **Step 2 — record findings.** Append a short "Cross-build findings" note to this plan: which targets compiled, which need CI runners, and any target that has a hard source blocker on 1.82 (to be dropped from the matrix + package tables + launcher map with a documented note). Do NOT drop a target merely because it needs a cross-linker locally — only for a genuine source/MSRV incompatibility.
- [ ] **Step 3 — no commit** (findings only). Proceed with the full 6-target set unless Step 2 found a hard blocker.

### Cross-build findings (RESOLVED 2026-06-02 — keep all 6 targets)

- **Host** `aarch64-apple-darwin`: BUILDS (`lingxi-cli 0.12.0`). `x86_64-apple-darwin`: BUILDS (cross from arm64 mac).
- **musl x64/arm64** + **windows-msvc x64/arm64**: `NEEDS_CI_LINKER` only — **no Rust source blockers**. Verified: `platform-posix-minimal` is not unix-only (deps all cross-platform; the lone `#[cfg(unix)]` in `fs.rs:110` is paired with a `#[cfg(windows)]` arm), and `cargo check -p cli --target x86_64-pc-windows-gnu` finished clean (whole CLI type-checks for Windows).
- **The one cross-compile complication is `ring v0.17.14`** — it compiles C, so every non-host target needs a platform C compiler: musl-gcc (linux), the Windows SDK/MSVC (windows). **Task 6 must ensure each runner has the right C toolchain** (musl-tools / `cross` for linux; the `windows-latest` runner's VS Build Tools for windows). Not a source issue; standard CI setup.
- **Decision:** all 6 targets stay in the matrix.

---

## Task 2: npm launcher + package.json

**Files:**
- Create: `lingxi-code/npm/bin/lingxi.js` (copy of `codex/codex-cli/bin/codex.js`)
- Create: `lingxi-code/npm/package.json`
- Create: `lingxi-code/npm/.gitignore`

- [ ] **Step 1 — copy the launcher verbatim.**
```bash
mkdir -p lingxi-code/npm/bin
cp codex/codex-cli/bin/codex.js lingxi-code/npm/bin/lingxi.js
```
- [ ] **Step 2 — apply the rename edits to `bin/lingxi.js`.** Exactly these, nothing else:
  1. `PLATFORM_PACKAGE_BY_TARGET` values: `@openai/codex-linux-x64` → `lingxi-linux-x64`, …, `@openai/codex-win32-arm64` → `lingxi-win32-arm64` (keep the 6 triple keys unchanged).
  2. `const codexBinaryName = … "codex.exe" : "codex";` → `const lingxiBinaryName = process.platform === "win32" ? "lingxi.exe" : "lingxi";` and update its two uses (`packageBinaryPath`, `legacyBinaryPath`).
  3. Inside those path helpers: `"codex"` directory segment in `legacyBinaryPath` → `"lingxi"`; `codex-path` → `lingxi-path` (the `pathDir` for both branches).
  4. The reinstall hint: `@openai/codex@latest` → `lingxi@latest` (both bun + npm branches).
  5. Env vars: `CODEX_MANAGED_BY_BUN` → `LINGXI_MANAGED_BY_BUN`, `CODEX_MANAGED_BY_NPM` → `LINGXI_MANAGED_BY_NPM`, `CODEX_MANAGED_PACKAGE_ROOT` → `LINGXI_MANAGED_PACKAGE_ROOT`.
  All platform-detection, `resolveNativePackage`, async `spawn`, `SIGINT/SIGTERM/SIGHUP` forwarding, PATH/exit-code/signal logic stays byte-for-byte.
- [ ] **Step 3 — write `lingxi-code/npm/package.json`:**
```json
{
  "name": "lingxi",
  "version": "0.0.0-dev",
  "description": "LingXi coding agent CLI (TUI).",
  "license": "Apache-2.0",
  "bin": { "lingxi": "bin/lingxi.js" },
  "type": "module",
  "engines": { "node": ">=16" },
  "files": ["bin/lingxi.js"],
  "optionalDependencies": {
    "lingxi-linux-x64": "0.0.0-dev",
    "lingxi-linux-arm64": "0.0.0-dev",
    "lingxi-darwin-x64": "0.0.0-dev",
    "lingxi-darwin-arm64": "0.0.0-dev",
    "lingxi-win32-x64": "0.0.0-dev",
    "lingxi-win32-arm64": "0.0.0-dev"
  }
}
```
(Set `license` to the workspace's license; read `lingxi-code/Cargo.toml` `[workspace.package] license` and match it. The staging script rewrites all `0.0.0-dev` to the release version.)
- [ ] **Step 4 — `lingxi-code/npm/.gitignore`:**
```
vendor/
dist/
*.tgz
node_modules/
```
- [ ] **Step 5 — sanity: launcher errors cleanly with no binary.**
```bash
cd lingxi-code/npm && node bin/lingxi.js --version; echo "exit=$?"
```
Expected: throws `Missing optional dependency lingxi-<os>-<arch>. Reinstall Lingxi: npm install -g lingxi@latest` (no vendor/, no platform pkg installed) with non-zero exit. This confirms platform detection + the error path.
- [ ] **Step 6 — commit.**
```bash
git add lingxi-code/npm/bin/lingxi.js lingxi-code/npm/package.json lingxi-code/npm/.gitignore
git commit -m "feat(packaging): npm launcher (lingxi) + package.json (copied from codex bin/codex.js)

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 3: npm staging script

**Files:**
- Create: `lingxi-code/npm/scripts/build_npm_package.py` (copy + adapt codex's)

- [ ] **Step 1 — copy verbatim.**
```bash
mkdir -p lingxi-code/npm/scripts
cp codex/codex-cli/scripts/build_npm_package.py lingxi-code/npm/scripts/build_npm_package.py
```
- [ ] **Step 2 — adapt the constants + platform table.** Edit the head of the file:
  1. Path anchors: `CODEX_CLI_ROOT` → the `npm/` dir (`SCRIPT_DIR.parent`); `REPO_ROOT` → `lingxi-code/` (`CODEX_CLI_ROOT.parent`). Remove `RESPONSES_API_PROXY_NPM_ROOT` and `CODEX_SDK_ROOT` (codex-only).
  2. `CODEX_NPM_NAME = "@openai/codex"` → `LINGXI_NPM_NAME = "lingxi"`.
  3. Replace the `CODEX_PLATFORM_PACKAGES` dict with the 6 `lingxi-*` entries: each `{ "npm_name": "lingxi-<os>-<arch>", "npm_tag": "<os>-<arch>", "target_triple": "<triple>", "os": "<os>", "cpu": "<cpu>" }` for the 6 triples in the spec table.
  4. `PACKAGE_EXPANSIONS`/`PACKAGE_NATIVE_COMPONENTS`/`PACKAGE_TARGET_FILTERS`: keep the `lingxi` (main, no native component) + the 6 platform packages (each native component = a single binary). Drop the `codex-responses-api-proxy` and `codex-sdk` entries entirely.
  5. In `stage_sources` + `main`: replace `codex.js`→`lingxi.js`, `@openai/codex`→`lingxi`, and the per-binary copy so each platform package gets `vendor/<triple>/bin/lingxi` sourced from `lingxi-code/target/<triple>/release/lingxi-cli` (rename `lingxi-cli`→`lingxi` on copy). Remove the responses-api-proxy / SDK branches of the staging/print logic.
- [ ] **Step 3 — test: stage the main package with a fixture.** Create a throwaway test:
```bash
cd lingxi-code/npm
mkdir -p /tmp/lx_stage_main
python3 scripts/build_npm_package.py --package lingxi --version 1.2.3 --staging-dir /tmp/lx_stage_main
test -f /tmp/lx_stage_main/bin/lingxi.js && echo "launcher staged OK"
python3 -c "import json;d=json.load(open('/tmp/lx_stage_main/package.json'));assert d['version']=='1.2.3';assert d['name']=='lingxi';assert set(d['optionalDependencies'])>= {'lingxi-darwin-arm64'};print('main package.json OK')"
```
Expected: both checks print OK.
- [ ] **Step 4 — test: stage a platform package from a fixture binary.**
```bash
cd lingxi-code
mkdir -p target/aarch64-apple-darwin/release && printf '#!/bin/sh\necho lingxi 1.2.3' > target/aarch64-apple-darwin/release/lingxi-cli && chmod +x target/aarch64-apple-darwin/release/lingxi-cli
rm -rf /tmp/lx_stage_plat && mkdir -p /tmp/lx_stage_plat
python3 npm/scripts/build_npm_package.py --package lingxi-darwin-arm64 --version 1.2.3 --staging-dir /tmp/lx_stage_plat --vendor-src target
python3 -c "import json;d=json.load(open('/tmp/lx_stage_plat/package.json'));assert d['os']==['darwin'] and d['cpu']==['arm64'];print('platform package.json OK')"
test -f /tmp/lx_stage_plat/vendor/aarch64-apple-darwin/bin/lingxi && echo "vendored binary OK"
```
(Adapt `--vendor-src` flag/path to whatever the copied script expects after Step 2; the assertion is: platform `package.json` carries `os`/`cpu` and the binary lands at `vendor/<triple>/bin/lingxi`.)
- [ ] **Step 5 — commit.**
```bash
git add lingxi-code/npm/scripts/build_npm_package.py
git commit -m "feat(packaging): npm staging script (copied + adapted from codex build_npm_package.py)

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 4: uv/PyPI wheel package

**Files:**
- Create: `lingxi-code/pypi/hatch_build.py` (copy codex's, rename env var)
- Create: `lingxi-code/pypi/src/lingxi_cli_bin/__init__.py` (copy locators + new `main()`)
- Create: `lingxi-code/pypi/src/lingxi_cli_bin/__main__.py` (new)
- Create: `lingxi-code/pypi/pyproject.toml` (new)
- Create: `lingxi-code/pypi/README.md` (short)

- [ ] **Step 1 — copy the hatch build hook + rename its env var.**
```bash
mkdir -p lingxi-code/pypi/src/lingxi_cli_bin
cp codex/sdk/python-runtime/hatch_build.py lingxi-code/pypi/hatch_build.py
```
Then edit: `CODEX_CLI_BIN_PLATFORM_TAG` → `LINGXI_CLI_BIN_PLATFORM_TAG` (the only change; the class still sets `build_data["tag"] = f"py3-none-{platform_tag}"` and forbids sdist).
- [ ] **Step 2 — write `src/lingxi_cli_bin/__init__.py`** (copy codex's locators, renamed, + the new `main()`):
```python
import os
import signal
import subprocess
import sys
from pathlib import Path

PACKAGE_NAME = "lingxi"
PACKAGE_METADATA_FILENAME = "lingxi-package.json"


def bundled_package_dir() -> Path:
    path = Path(__file__).resolve().parent
    metadata_path = path / PACKAGE_METADATA_FILENAME
    if not metadata_path.is_file():
        raise FileNotFoundError(
            f"{PACKAGE_NAME} is installed but missing its package metadata at {metadata_path}"
        )
    return path


def bundled_lingxi_path() -> Path:
    exe = "lingxi.exe" if os.name == "nt" else "lingxi"
    path = bundled_package_dir() / "bin" / exe
    if not path.is_file():
        raise FileNotFoundError(
            f"{PACKAGE_NAME} is installed but missing its packaged binary at {path}"
        )
    return path


def bundled_path_dir() -> Path | None:
    path = bundled_package_dir() / "lingxi-path"
    return path if path.is_dir() else None


def main() -> int:
    """Console entrypoint: exec the bundled lingxi binary with argv passthrough,
    forwarding termination signals and mirroring the child's exit code."""
    binary = bundled_lingxi_path()
    env = dict(os.environ)
    path_dir = bundled_path_dir()
    if path_dir is not None:
        sep = ";" if os.name == "nt" else ":"
        env["PATH"] = f"{path_dir}{sep}{env.get('PATH', '')}"
    env["LINGXI_MANAGED_BY_PIP"] = "1"

    proc = subprocess.Popen([str(binary), *sys.argv[1:]], env=env)

    def _forward(signum, _frame):
        try:
            proc.send_signal(signum)
        except ProcessLookupError:
            pass

    forwardable = [signal.SIGINT, signal.SIGTERM]
    if hasattr(signal, "SIGHUP"):
        forwardable.append(signal.SIGHUP)
    for sig in forwardable:
        try:
            signal.signal(sig, _forward)
        except (ValueError, OSError):
            pass

    return proc.wait()


__all__ = [
    "PACKAGE_NAME",
    "bundled_lingxi_path",
    "bundled_package_dir",
    "bundled_path_dir",
    "main",
]
```
- [ ] **Step 3 — write `src/lingxi_cli_bin/__main__.py`:**
```python
from . import main

if __name__ == "__main__":
    raise SystemExit(main())
```
- [ ] **Step 4 — write `pyproject.toml`** (modeled on codex `sdk/python-runtime`, + the console script):
```toml
[build-system]
requires = ["hatchling>=1.24.0", "packaging"]
build-backend = "hatchling.build"

[project]
name = "lingxi"
version = "0.0.0-dev"
description = "LingXi coding agent CLI (TUI)."
readme = "README.md"
requires-python = ">=3.10"
license = { text = "Apache-2.0" }
authors = [{ name = "LingXi" }]
classifiers = [
  "Programming Language :: Python :: 3",
  "Programming Language :: Python :: 3.10",
]

[project.scripts]
lingxi = "lingxi_cli_bin:main"

[tool.hatch.build]
exclude = [".venv/**", "dist/**", "build/**"]

[tool.hatch.build.targets.wheel]
packages = ["src/lingxi_cli_bin"]
include = [
  "src/lingxi_cli_bin/lingxi-package.json",
  "src/lingxi_cli_bin/bin/**",
  "src/lingxi_cli_bin/lingxi-path/**",
]

[tool.hatch.build.targets.wheel.hooks.custom]

[tool.hatch.build.targets.sdist]

[tool.hatch.build.targets.sdist.hooks.custom]
```
(Set `license` to match the workspace license. The `[…hooks.custom]` activates `hatch_build.py`.)
- [ ] **Step 5 — `pypi/README.md`:** one paragraph: "Platform wheel bundling the `lingxi` CLI binary; `uv tool install lingxi` / `pip install lingxi` exposes the `lingxi` command."
- [ ] **Step 6 — test: locator + `main()` exit code** (`lingxi-code/pypi/tests/test_bin.py`):
```python
import os, stat, json, sys
from pathlib import Path
import lingxi_cli_bin

def _install_fake_binary(tmp_path, monkeypatch, script):
    pkg = tmp_path / "lingxi_cli_bin"
    (pkg / "bin").mkdir(parents=True)
    (pkg / "lingxi-package.json").write_text("{}")
    exe = pkg / "bin" / ("lingxi.exe" if os.name == "nt" else "lingxi")
    exe.write_text(script)
    exe.chmod(exe.stat().st_mode | stat.S_IEXEC)
    monkeypatch.setattr(lingxi_cli_bin, "__file__", str(pkg / "__init__.py"))
    return exe

def test_bundled_path_found(tmp_path, monkeypatch):
    _install_fake_binary(tmp_path, monkeypatch, "#!/bin/sh\nexit 0\n")
    assert lingxi_cli_bin.bundled_lingxi_path().is_file()

def test_main_mirrors_exit_code(tmp_path, monkeypatch):
    _install_fake_binary(tmp_path, monkeypatch, "#!/bin/sh\nexit 7\n")
    monkeypatch.setattr(sys, "argv", ["lingxi"])
    assert lingxi_cli_bin.main() == 7

def test_missing_binary_raises(tmp_path, monkeypatch):
    pkg = tmp_path / "lingxi_cli_bin"; (pkg).mkdir()
    (pkg / "lingxi-package.json").write_text("{}")
    monkeypatch.setattr(lingxi_cli_bin, "__file__", str(pkg / "__init__.py"))
    import pytest
    with pytest.raises(FileNotFoundError):
        lingxi_cli_bin.bundled_lingxi_path()
```
Run (skip the exec test on Windows shells that can't run `/bin/sh`):
```bash
cd lingxi-code/pypi && python -m pytest tests/ -q
```
Expected: 3 passed (the exit-code test is POSIX; mark it `@pytest.mark.skipif(os.name=="nt", …)` if needed).
- [ ] **Step 7 — commit.**
```bash
git add lingxi-code/pypi
git commit -m "feat(packaging): uv/PyPI wheel (lingxi) — copied locator + new console-script main()

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 5: Host end-to-end smoke

**Files:** none (verification only; optionally a `lingxi-code/npm/scripts/smoke_host.sh` helper).

- [ ] **Step 1 — build the host binary.** `cd lingxi-code && cargo build --release -p cli` → `target/release/lingxi-cli`.
- [ ] **Step 2 — stage + run the npm launcher against a local vendor dir** (the launcher's local-vendor fallback):
```bash
cd lingxi-code
HOST_TRIPLE=$(rustc -vV | sed -n 's/host: //p')
mkdir -p npm/vendor/$HOST_TRIPLE/bin
cp target/release/lingxi-cli npm/vendor/$HOST_TRIPLE/bin/lingxi
node npm/bin/lingxi.js --version
```
Expected: prints the `lingxi-cli --version` output (proves detection → vendor resolution → exec). (`npm/vendor/` is gitignored.)
- [ ] **Step 3 — build + run the uv wheel locally.**
```bash
cd lingxi-code/pypi
mkdir -p src/lingxi_cli_bin/bin && cp ../target/release/lingxi-cli src/lingxi_cli_bin/bin/lingxi
echo '{}' > src/lingxi_cli_bin/lingxi-package.json
uv tool install --from . lingxi 2>/dev/null || pip install --force-reinstall .
lingxi --version   # the console script
```
Expected: prints the version. Then clean up the staged binary (gitignored).
- [ ] **Step 4 — no commit** (smoke only), unless adding the optional `smoke_host.sh` helper:
```bash
git add lingxi-code/npm/scripts/smoke_host.sh 2>/dev/null && git commit -m "chore(packaging): host smoke-test helper" || true
```

---

## Task 6: Release workflow

**Files:**
- Create: `.github/workflows/lingxi-release.yml`
- Append: `lingxi-code/.gitignore` (packaging build output)

- [ ] **Step 1 — write `.github/workflows/lingxi-release.yml`.** Trigger on tag `v*` + `workflow_dispatch`. Concrete workflow:
```yaml
name: lingxi-release
on:
  push:
    tags: ["v*.*.*"]
  workflow_dispatch:
    inputs:
      version:
        description: "Release version (defaults to the tag without leading v)"
        required: false

concurrency:
  group: lingxi-release
  cancel-in-progress: true

jobs:
  build:
    name: build ${{ matrix.target }}
    runs-on: ${{ matrix.os }}
    strategy:
      fail-fast: false
      matrix:
        include:
          - { target: aarch64-apple-darwin,        os: macos-latest }
          - { target: x86_64-apple-darwin,         os: macos-latest }
          - { target: x86_64-unknown-linux-musl,   os: ubuntu-latest }
          - { target: aarch64-unknown-linux-musl,  os: ubuntu-latest, cross: true }
          - { target: x86_64-pc-windows-msvc,      os: windows-latest }
          - { target: aarch64-pc-windows-msvc,     os: windows-latest }
    steps:
      - uses: actions/checkout@v4
      - name: Install Rust 1.82 + target
        run: |
          rustup toolchain install 1.82.0 --profile minimal
          rustup default 1.82.0
          rustup target add ${{ matrix.target }}
      - name: Linux musl deps
        if: runner.os == 'Linux'
        run: |
          sudo apt-get update && sudo apt-get install -y musl-tools
          cargo install cross --locked || true
      - name: Build
        working-directory: lingxi-code
        shell: bash
        run: |
          if [ "${{ matrix.cross }}" = "true" ]; then
            cross build --release -p cli --target ${{ matrix.target }}
          else
            cargo build --release -p cli --target ${{ matrix.target }}
          fi
      - name: Stage binary
        working-directory: lingxi-code
        shell: bash
        run: |
          mkdir -p dist
          BIN=target/${{ matrix.target }}/release/lingxi-cli
          [ "${{ runner.os }}" = "Windows" ] && BIN="$BIN.exe"
          cp "$BIN" "dist/lingxi-${{ matrix.target }}$([ "${{ runner.os }}" = "Windows" ] && echo .exe)"
      - uses: actions/upload-artifact@v4
        with:
          name: bin-${{ matrix.target }}
          path: lingxi-code/dist/*

  package-and-publish:
    needs: build
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: actions/download-artifact@v4
        with: { path: artifacts }
      - name: Reassemble target/<triple>/release layout
        shell: bash
        run: |
          cd lingxi-code
          for d in ../artifacts/bin-*; do
            t=$(basename "$d" | sed 's/^bin-//')
            mkdir -p target/$t/release
            f=$(ls "$d")
            cp "$d/$f" target/$t/release/lingxi-cli
          done
      - uses: actions/setup-node@v4
        with: { node-version: 20, registry-url: "https://registry.npmjs.org" }
      - uses: astral-sh/setup-uv@v5
      - name: Resolve version
        id: ver
        shell: bash
        run: echo "v=${GITHUB_REF_NAME#v}" >> "$GITHUB_OUTPUT"
      - name: Stage npm packages
        working-directory: lingxi-code
        run: |
          for p in lingxi lingxi-linux-x64 lingxi-linux-arm64 lingxi-darwin-x64 lingxi-darwin-arm64 lingxi-win32-x64 lingxi-win32-arm64; do
            python3 npm/scripts/build_npm_package.py --package "$p" --release-version "${{ steps.ver.outputs.v }}" --staging-dir "/tmp/npm/$p" --vendor-src target
          done
      - name: Build wheels (one per target)
        working-directory: lingxi-code/pypi
        shell: bash
        run: |
          declare -A TAG=( [x86_64-apple-darwin]=macosx_11_0_x86_64 [aarch64-apple-darwin]=macosx_11_0_arm64 \
            [x86_64-unknown-linux-musl]=musllinux_1_2_x86_64 [aarch64-unknown-linux-musl]=musllinux_1_2_aarch64 \
            [x86_64-pc-windows-msvc]=win_amd64 [aarch64-pc-windows-msvc]=win_arm64 )
          for t in "${!TAG[@]}"; do
            rm -rf src/lingxi_cli_bin/bin && mkdir -p src/lingxi_cli_bin/bin
            ext=""; [[ "$t" == *windows* ]] && ext=".exe"
            cp ../target/$t/release/lingxi-cli "src/lingxi_cli_bin/bin/lingxi$ext"
            echo '{}' > src/lingxi_cli_bin/lingxi-package.json
            LINGXI_CLI_BIN_PLATFORM_TAG="${TAG[$t]}" uv build --wheel --out-dir dist
          done
      - name: Publish to npm
        if: ${{ env.NPM_TOKEN != '' }}
        env:
          NODE_AUTH_TOKEN: ${{ secrets.NPM_TOKEN }}
          NPM_TOKEN: ${{ secrets.NPM_TOKEN }}
        run: |
          for p in lingxi-linux-x64 lingxi-linux-arm64 lingxi-darwin-x64 lingxi-darwin-arm64 lingxi-win32-x64 lingxi-win32-arm64 lingxi; do
            (cd "/tmp/npm/$p" && npm publish --access public)
          done
      - name: Publish to PyPI
        if: ${{ env.PYPI_TOKEN != '' }}
        env:
          PYPI_TOKEN: ${{ secrets.PYPI_TOKEN }}
          UV_PUBLISH_TOKEN: ${{ secrets.PYPI_TOKEN }}
        run: uv publish lingxi-code/pypi/dist/*.whl
```
(Adapt `--vendor-src`/flag names to the Task-3 script. The `if: ${{ env.X != '' }}` guards make publish steps no-ops without secrets, so build+stage always runs. `aarch64-pc-windows-msvc` and `aarch64-unknown-linux-musl` are the riskiest cross targets — if Task 1 flagged a hard blocker, remove that matrix row + its npm/wheel/optionalDependency entries with a comment.)
- [ ] **Step 2 — append packaging output to `lingxi-code/.gitignore`:**
```
# packaging build output
/npm/vendor/
/npm/dist/
/pypi/dist/
/pypi/src/lingxi_cli_bin/bin/
/pypi/src/lingxi_cli_bin/lingxi-package.json
/dist/
```
- [ ] **Step 3 — validate the workflow YAML parses.** `python3 -c "import yaml,sys; yaml.safe_load(open('.github/workflows/lingxi-release.yml'))" && echo OK` (or `actionlint` if available).
- [ ] **Step 4 — commit.**
```bash
git add .github/workflows/lingxi-release.yml lingxi-code/.gitignore
git commit -m "feat(packaging): tagged-release workflow — cross-build 6 targets, stage npm+wheels, secret-gated publish

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 7: Install docs

**Files:** Append an "Install" section to `README.md` (and/or `docs/`).

- [ ] **Step 1 — document both channels:**
```markdown
## Install the CLI

    npm install -g lingxi      # or: bun install -g lingxi
    uv tool install lingxi     # or: uvx lingxi  /  pip install lingxi

Both install the `lingxi` command (the TUI). They download the prebuilt
`lingxi-cli` binary for your platform (Linux x64/arm64, macOS x64/arm64,
Windows x64/arm64).
```
- [ ] **Step 2 — commit.**
```bash
git add README.md
git commit -m "docs(packaging): document npm + uv install of the lingxi CLI

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Self-Review

**Spec coverage:** npm launcher+pkg (Task 2, spec §2.1) ✓; npm staging script (Task 3, §2.1) ✓; uv wheel+hook+shim (Task 4, §2.2) ✓; release workflow (Task 6, §2.3) ✓; cross-build spike (Task 1, §3) ✓; error handling exercised (Task 2 Step 5, Task 4 test) (§4) ✓; testing — launcher error, locator/main, staging, host e2e (Tasks 2/3/4/5, §5) ✓; gitignore + docs (Tasks 6/7, §6) ✓.

**Placeholder scan:** Copied files (`lingxi.js`, `build_npm_package.py`, `hatch_build.py`) carry exact rename instructions against on-disk codex sources (verifiable, not "TBD"). New files (`package.json`, `pyproject.toml`, `__init__.py` `main()`, `__main__.py`, workflow, tests) are given in full. The `--vendor-src`/flag-name caveats point at the copied script's resolved API (adapt-and-verify, gated by the staging tests) — matching this repo's plan convention. The one judgement call (drop a target) is conditioned on Task 1's finding.

**Type/name consistency:** `lingxi` command, `lingxi-cli` cargo binary, bundled file `lingxi`(`.exe`), platform packages `lingxi-<os>-<arch>`, Python module `lingxi_cli_bin`, metadata `lingxi-package.json`, env vars `LINGXI_*`, wheel-tag env `LINGXI_CLI_BIN_PLATFORM_TAG`, 6 triples consistent across launcher map / optionalDependencies / staging table / workflow matrix.

**Risk:** Cross-compilation on Rust 1.82 (Task 1 gates it; risky targets are droppable with a documented note). Publish is secret-gated so the pipeline is safe/testable without tokens.
