# What is actually left, 2026-07-26

Every audit backlog in `docs/` has been swept at the behaviour sites; this is
the residue, with the blocker named for each so the next session starts from
execution rather than re-derivation.

## DONE since the previous revision

- **`mcp xaa` — the XAA (SEP-990) IdP connection subsystem. COMPLETE.**
  `setup` / `login` / `show` / `clear`, wired at `apps/cli/src/commands/mcp_xaa.rs`.

  **The previous revision of this file said XAA was "absent as a concept". That
  was wrong, and wrong in the way this session kept being wrong:** the whole
  engine already existed — `mcp/src/xaa_idp.rs`, 1328 lines of OIDC discovery,
  the PKCE browser flow, the id_token cache and its 60s expiry buffer — with no
  way to reach it from a terminal. What was missing was three storage writers
  (`save_id_token_from_jwt`, `save_idp_client_secret`, `clear_idp_client_secret`)
  and the CLI surface. Checking `mcp/src/` before writing the sentence would
  have cost one command.

  Verified by execution against a sandboxed HOME, not by inference: every
  validation message is byte-identical to the oracle, `setup` preserves
  unrelated settings keys, client-id rotation clears the old credentials, and
  `clear` removes only `xaaIdp`.

  One deliberate divergence: remediation hints name `lingxi-cli mcp xaa setup`,
  not `claude mcp xaa setup`. Telling a user to run a binary that does not exist
  would be a defect, not fidelity.

  One known behaviour delta, small and safer: `--callback-port 9000abc` is
  REJECTED here, where the oracle's `parseInt` would accept it as 9000.

- **`--help` layout.** `Usage:` now leads, the description follows, and help
  wraps at 80 columns with hanging indents (clap `help_template` +
  `wrap_help` + `term_width = 80`) — matching the oracle's presentation.

  Still divergent, and structurally so: clap orders `{all-args}` as
  Commands → Options, where commander emits Arguments → Options → Commands.
  Reordering needs per-command templates, because a literal `Arguments:` header
  in a shared template would print for the ~40 subcommands that have no
  positionals. Not worth that for section order.

  Note that `--help` can never be byte-identical regardless: the binary name,
  product description, and command set legitimately differ.

## 1. 2.1.220 BEHAVIOUR audit — the real remaining work

Everything done so far compares SURFACE (does the flag/command exist). Nobody
has compared what a flag DOES across the board. Two sides can advertise
`--scope` and write to different tiers.

Start with `scripts/parity_surface.py` — it self-verifies its traversal and
refuses to report a broken walk — then, per command, construct inputs and
compare outputs against the oracle.

**The harness the audit needs now exists in practice**: run the port against a
temp `HOME` + `LINGXI_HOME` with a seeded `settings.json`, drive each
subcommand with `< /dev/null`, and diff stdout/stderr/exit-code against the
oracle run the same way. That is exactly how `mcp xaa` was verified above, and
it caught two defects a unit test would not have (see below).

**Slices done** (both binaries executed and diffed — evidence, not inference):

- Pure-output commands: no behaviour DEFECT found.
- `--version` SHAPE differs: oracle `2.1.220 (Claude Code)` vs port
  `lingxi-cli 0.12.0`. The VALUE difference is correct — LingXi is its own
  product, and the Claude-compat identifiers derive separately from
  `traits::CLAUDE_CODE_VERSION`. The SHAPE is a branding call not yet made.
- `mcp xaa`: full lifecycle diffed against the oracle's strings. Green.

**Still uncovered:** `plugin`, `auto-mode`, `project`, `agents`, `auth`, and the
`mcp` verbs other than `xaa`.

## Two decisions, not fixes

- **`doctor` description** diverges beyond branding: the port additionally
  warns that stdio servers from `.mcp.json` are spawned. That is more
  informative than the oracle's text. Keep or align — a product call.
- **Remote-session client** (`useRemoteSession`, the
  `tengu_refusal_retraction_*` family) is an accepted divergence: no Anthropic
  private relay / auth contract. If that ever changes, note that `fzf` applies
  a retraction signal ONLY when `event.source === "worker"` — a trust boundary,
  since an unauthenticated retraction can erase history.

## The rules this session kept re-learning

**A comment or backlog heading that asserts a fact about OTHER code becomes a
lie the moment that code moves.** Five false findings this session traced to
exactly that — including this file's own "XAA is absent as a concept", and
2.1.215's M-13 where an auditor read a stale module doc and filed a gap against
working code. When deferring, point at the code; do not restate what it does.

**A generated absence is a hypothesis, not a finding.** A one-level walk
produced 39 phantom flag gaps; a grep for two identifiers that never share a
line reported a wired feature as missing; a `--help` probe reported five
missing commands that were all present, because zsh does not word-split an
unquoted variable and the whole path arrived as one argument. Confirm at the
call site — `<cli> <sub> --help` with the arguments spelled out — before
recording anything.

**Do not report a state you could not read.** Two defects found reviewing the
`mcp xaa` code against its own oracle, both of the same shape: `setup` reported
"keychain save failed" when no secret had been requested, and `show` rendered a
credential-storage ERROR as "Logged in: no". Both would have sent a user to fix
something that was not broken. `unwrap_or(false)` on a fallible read is where
this hides.
