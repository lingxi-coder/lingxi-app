# What is actually left, 2026-07-26

Every audit backlog in `docs/` has been swept at the behaviour sites. This is
the residue, with the blocker named for each so the next session starts from
execution rather than re-derivation.

## The behaviour audit now has a harness: `scripts/parity_behaviour.py`

`parity_surface.py` compares which flags EXIST. `parity_behaviour.py` compares
what commands DO: it runs both binaries against a fresh sandboxed HOME per
command, with argv as a list and stdin closed, and diffs stdout+stderr+exit
status after folding known-legitimate branding to neutral tokens.

Current state: **11 of 12 probed commands identical, 1 differing** — and that
one is a deliberate divergence (below), not an unfixed defect.

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

- **User-facing text told users to run `claude`**, a binary that does not exist
  in this product — 9 strings across `mcp.rs` and `argv.rs`, plus the
  `plugin list` empty-case hint.

## 1. The one remaining behaviour difference: MCP health checks

`claude mcp list` prints `Checking MCP server health…` and appends a per-server
status (`✔ Connected` / `✘ Failed to connect`). This port prints neither.

**This is a deliberate architectural stance, documented independently in three
places** (`mcp list`, `mcp get`, and `doctor`, which says "No connection is
attempted — this is purely the parsed config view"). Health-checking means
spawning every configured stdio server, i.e. executing arbitrary configured
commands as a side effect of a listing command.

It is now MORE consequential than before: this wave made ancestor `.mcp.json`
discoverable, so a spawn-on-list would execute commands from an inherited
`~/.mcp.json` in any subdirectory of `$HOME`. The oracle bounds this by only
health-checking APPROVED servers (pending ones are skipped), so the risk is
containable — but turning it on is a product decision about executing inherited
config, not a parity cleanup. **Do not implement it without deciding that
question first.**

## 2. Surfaces still unprobed

The harness covers 12 read-only verbs. Not yet compared: commands that write
(`mcp add`, `plugin install`, `project` mutations), and everything skipped by
name as interactive/network/server (`auth login`, `setup-token`, `gateway`,
`remote-control`, `doctor`, `install`, `update`, `mcp serve`). Extending it
means constructing fixtures and asserting on resulting FILES, not just stdout —
two sides can print identical bytes and write different files.

## 3. `--help` section order

`Usage:` leads, the description follows, and help wraps at 80 columns with
hanging indents — matching the oracle. What remains is that clap's `{all-args}`
orders Commands → Options where commander emits Arguments → Options → Commands.
Reordering needs per-command templates, since a literal `Arguments:` header in
a shared template would print for the ~40 subcommands that have no positionals.

`--help` can never be byte-identical regardless: the binary name, product
description, and command set legitimately differ.

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
