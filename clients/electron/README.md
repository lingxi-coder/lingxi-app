# LingXi Code Desktop — Internal Beta

The macOS desktop client runs the real LingXi Rust engine through a bundled,
loopback-only bridge. The renderer shows only host-authoritative workspaces,
sessions, models, tasks, permission requests, and diagnostics. It has no
production mock-data fallback.

## Beta scope

- Apple Silicon macOS (`arm64`)
- Native workspace selection and explicit workspace trust
- Curated multi-provider connection setup (Anthropic, OpenAI, DeepSeek, Gemini,
  OpenRouter, Z.AI, GLM Coding Plan, and GitHub Copilot token) with per-provider
  encrypted storage
- New, listed, and resumed local JSONL sessions
- Streaming text, thinking, tool activity, cancellation, and permission prompts
- Engine-reported model selection and background tasks
- Sanitized in-memory diagnostics and controlled engine restart
- Manual, checksum-verified updates

Cloud accounts, billing, chat/cowork modes, automatic updates, Windows, and
Linux are outside this Beta. Voice input is an optional local-browser control
and depends on the macOS speech-recognition permission.

Internal testers should follow [INTERNAL_BETA.md](./INTERNAL_BETA.md).

## Security model

Electron main is the local authority. The sandboxed renderer receives a narrow,
runtime-validated API and never receives the stored credential, bridge bearer
token, process environment, or filesystem access.

Workspace project settings are fingerprinted. Trust requires an explicit native
confirmation and is revoked when `.mcp.json`, `.claude/settings*.json`, or
`.lingxi/settings*.json` changes. Until trust is granted, the bridge ignores
project executable configuration and the host rejects prompts and session/model/
task commands.

Provider credentials are stored as per-provider generic-password items in the
macOS Keychain and written once to bridge stdin as a bounded envelope. They are
absent from arguments and environment variables. Legacy Safe Storage blobs are
migrated once after a successful read, then removed. The bridge receives an
allowlisted environment, publishes discovery data inside a private per-launch
directory, requires protocol hello before commands, and accepts one
authenticated client. OAuth/device sign-in remains a CLI/TUI-only flow in this
Beta and is shown as such in the provider picker.

## Development

Prerequisites: Node.js/npm, the Rust toolchain, and an Apple Silicon Mac for the
release artifact.

```bash
cd clients/shared
npm install
npm run build

cd ../electron
npm install
npm run typecheck
npm test
npm run build
```

For development launch, build the sidecar first:

```bash
cd lingxi-code
cargo build -p bridge-server --bin bridge-server

cd ../clients/electron
npm run dev
```

`LINGXI_BRIDGE_SERVER_BIN` is accepted only in development. Packaged builds
resolve the sidecar exclusively from their signed resources.

## Build the internal Beta artifact

```bash
cd lingxi-code
REPO_ROOT="$(git rev-parse --show-toplevel)"
RUSTFLAGS="--remap-path-prefix=${REPO_ROOT}=. \
--remap-path-prefix=${CARGO_HOME:-$HOME/.cargo}=/cargo-home \
--remap-path-prefix=$HOME/.rustup=/rustup" \
  cargo build --locked --release -p bridge-server --bin bridge-server

cd ../clients/electron
npm run package:mac
npm run verify:package
```

`npm run verify:package` now runs both the static bundle audit and a packaged
runtime smoke test. The smoke runner copies the final `.app` to a temporary
path outside the repository, launches it under an isolated temporary `HOME`
with renderer URL / sidecar override / API-key environment variables removed,
uses a random remote-debugging port to confirm `window.lingxi`, truthful
onboarding copy, renderer security invariants, and a keyless bundled-sidecar
session/listing flow, then verifies the app and sidecar cleaned up their exact
temporary runtime directories and processes.

The gitignored `dist/` directory receives:

- `LingXi-Code-<version>-mac-arm64/LingXi Code.app`
- `LingXi-Code-<version>-mac-arm64.zip`
- `LingXi-Code-<version>-mac-arm64.zip.sha256`

The in-repo packager uses Electron's official application skeleton, bundles the
release Rust sidecar at `Contents/Resources/bin/bridge-server`, removes
development metadata, checks both binaries are arm64, scans for credentials and
developer paths, applies an ad-hoc signature, and emits a SHA-256 checksum.

The ad-hoc signature is intended only for approved internal distribution. A
public or wider external release still requires Developer ID signing,
notarization, stapling, and a separate release approval.

## Install, update, and roll back

1. Verify the ZIP using the accompanying SHA-256 file.
2. Quit LingXi Code completely.
3. Replace the existing `/Applications/LingXi Code.app` with the verified app.
4. Launch it and complete the workspace/provider onboarding if required.

Updates never delete `~/.lingxi` sessions or Electron user data. To roll back,
quit the app and replace it with the previously retained verified Beta artifact.
Because internal builds are ad-hoc signed, macOS may require an explicit local
approval after the artifact is transferred to a different machine.

## Verification gates

```bash
npm run typecheck
npm test
npm run build
npm audit
npm run verify:package
```

The packaged smoke coverage is intentionally limited to what can be asserted
non-interactively on one machine. Manual follow-up is still required for
Gatekeeper approval on transferred builds, Developer ID signing/notarization,
and native trust-dialog copy on a tester's host.

Rust release gates are run from `lingxi-code/`:

```bash
cargo fmt --check
cargo test -p bridge -p bridge-server
REPO_ROOT="$(git rev-parse --show-toplevel)"
RUSTFLAGS="--remap-path-prefix=${REPO_ROOT}=. \
--remap-path-prefix=${CARGO_HOME:-$HOME/.cargo}=/cargo-home \
--remap-path-prefix=$HOME/.rustup=/rustup" \
  cargo build --locked --release -p bridge-server --bin bridge-server
```
