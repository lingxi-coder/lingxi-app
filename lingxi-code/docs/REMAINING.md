# What is actually left, 2026-07-26

Every audit backlog in `docs/` has been swept at the behaviour sites. This is
the residue, with the blocker named for each so the next session starts from
execution rather than re-derivation.

## The behaviour audit now has a harness: `scripts/parity_behaviour.py`

`parity_surface.py` compares which flags EXIST. `parity_behaviour.py` compares
what commands DO: it runs both binaries against a fresh sandboxed HOME per
command, with argv as a list and stdin closed, and diffs stdout+stderr+exit
status after folding known-legitimate branding to neutral tokens.

Current state: **12 of 12 probed commands identical, and 5 of 5 write probes
identical** — stdout, stderr, exit status, and the resulting config JSON.

It exists as a script because FOUR findings during this audit were artifacts of
an ad-hoc shell probe rather than facts about the code. zsh does not word-split
an unquoted variable, so `$cmd --help` with `cmd="mcp xaa setup"` arrives as ONE
argv element; the CLI falls back to ROOT help and the diff reads as "five
subcommands are missing". The same bug made both binaries treat
`"auto-mode config"` as a chat prompt and run a real model session. Two more
were interleaved stdout from concurrent processes sharing a terminal.

**The harness itself produced a false finding too** — its normalisation folded
`claude` before `.claude`, so `.claude` became `.«cli»` while `.lingxi` became
`«dotdir»`, and `auto-mode defaults` looked divergent immediately after being
regenerated to byte-identical. Normalisation order is now load-bearing and
commented as such.

## Fixed by the behaviour audit

- **Project `.mcp.json` discovery did not walk ancestors.** The oracle resolves
  the project config by walking UP from cwd; this port looked only at
  `<cwd>/.mcp.json`, so a user in any subdirectory of their repo silently got
  NO project MCP servers. One root cause, two symptoms — `mcp list` came up
  empty and `mcp reset-project-choices` suppressed its second line.

  Discovery walks; MUTATION deliberately does not. `mcp add --scope project`
  and `mcp remove` still target `<cwd>/.mcp.json`, because rewriting a
  `.mcp.json` that lives above the working directory — possibly in `$HOME`,
  shared by every repo underneath — is a side effect no one asked for.
  `scope_contains_server` is paired with the write path so `remove`'s
  precondition matches what `remove` actually does.

- **`auto-mode critique` graded the shipped defaults, never the user's rules.**
  Same verdict on every machine; it would report "no structural issues" to
  someone whose own rules were malformed.

- **`auto-mode defaults` was three versions stale** (2.1.191): 35 of 65
  `soft_deny`, 9 of 17 `allow`, 15 of 20 `environment` entries missing.
  Regenerated to byte-identical. DISPLAY state only — the runtime gate is
  `permission::classifier::classify_tool_call`, which reads no rules document,
  so this was never an under-ask.

- **`mcp xaa` was registered unconditionally.** The oracle gates the group on
  `CLAUDE_CODE_ENABLE_XAA` (`vZ()`), so a default install answers
  `error: unknown command 'xaa'`. Reproduced, including the hidden-from-help
  behaviour. Found by the harness against code written earlier the same day.

- **`mcp get` printed an empty `Configured servers:` list** when nothing was
  loaded but pending servers existed.

- **The `doctor` description was false about its own code.** It warned that
  "stdio servers from .mcp.json are spawned for health checks"; `doctor.rs`
  says "No connection is attempted". It also called the command a check of the
  "auto-updater" when it reports on much more, and the trust-prompt warning was
  lifted from the oracle's `-p/--print` option rather than its `doctor`. Now
  matches the oracle's wording, which is also true of this port. Overstating
  what a command touches is not harmlessly cautious — it steers people away
  from a safe command.

- **`mcp list` now health-checks.** `Checking MCP server health…` plus a
  per-server `✔ Connected` / `✘ Failed to connect — <error>`, matching the
  oracle's structure exactly (including the trailing space an argless stdio
  server renders: the oracle formats `{command} {args}` unconditionally).

  The check SPAWNS stdio servers, so it runs ONLY for servers the user has
  accepted: user- and local-scope servers came from an explicit `mcp add`, and
  a project `.mcp.json` server stays PENDING until approved. That bound is what
  makes it safe alongside ancestor discovery — an inherited `~/.mcp.json` is
  listed, never executed, until someone approves it. Probe connections are torn
  down, and a wedged server times out at 10s instead of hanging the listing.

  The error DETAIL after the em-dash is implementation-specific and will not
  match byte-for-byte (Rust `io` errors vs Node `ENOENT: … posix_spawn`).

- **User-facing text told users to run `claude`**, a binary that does not exist
  in this product — 9 strings across `mcp.rs` and `argv.rs`, plus the
  `plugin list` empty-case hint.

## 1. Surfaces still unprobed

The harness covers 12 read-only verbs and 5 write sequences. Not yet compared:
everything skipped BY NAME as interactive/network/server (`auth login`,
`setup-token`, `gateway`, `remote-control`, `install`, `update`, `mcp serve`)
and `plugin install` (network). These need either a mock registry/IdP or a
recorded-transcript fixture; running them for real reaches third-party services.

## 2. What remains different in `--help`

Section ORDER, preamble order, and 80-column wrapping now match the oracle
(`Usage:` → description → Arguments → Options → Commands), implemented as a
transform on clap's rendered help rather than per-command templates.

What cannot match: the binary name, the product description, the command set,
and clap's own option rendering (`-h, --help  Print help` vs commander's
`Display help for command`). `--help` is not a byte-parity target for a
differently-named product.

## Two decisions, not fixes

- **`--version` shape**: oracle `2.1.220 (Claude Code)` vs port
  `lingxi-cli 0.12.0`. The VALUE difference is correct — LingXi is its own
  product, and Claude-compat identifiers derive separately from
  `traits::CLAUDE_CODE_VERSION`. The SHAPE is a branding call not yet made.
- **Remote-session client** (`useRemoteSession`, `tengu_refusal_retraction_*`)
  is an accepted divergence: no Anthropic private relay / auth contract. If that
  changes, note `fzf` applies a retraction signal ONLY when
  `event.source === "worker"` — an unauthenticated retraction can erase history.

## The rules this session kept re-learning

**A comment or backlog heading that asserts a fact about OTHER code becomes a
lie the moment that code moves.** This file's own "XAA is absent as a concept"
was wrong — `mcp/src/xaa_idp.rs` had 1328 lines of working engine. Point at the
code; do not restate what it does.

**A generated absence is a hypothesis, not a finding.** Confirm at the call site
before recording anything.

**Do not report a state you could not read.** `setup` announced "keychain save
failed" when no secret was requested; `show` rendered a storage ERROR as
"Logged in: no". `unwrap_or(false)` on a fallible read is where this hides.

**A lock serializes access; it does not undo what you did while holding it.** A
process-global mutated under a lock must be RESTORED before release, or the
lock just serializes the corruption.
