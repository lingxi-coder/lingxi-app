# What is actually left, 2026-07-26

`main` @ `406f35cec`. Every audit backlog in `docs/` has been swept at the
behaviour sites; this is the residue, with the blocker named for each so the
next session starts from execution rather than re-derivation.

Ordered by what I would do first.

## 1. 2.1.220 BEHAVIOUR audit — the real remaining work

Everything done so far compares SURFACE (does the flag/command exist). Nobody
has compared what a flag DOES. Two sides can advertise `--scope` and write to
different tiers.

Start with `scripts/parity_surface.py` — it self-verifies its traversal and
refuses to report a broken walk — then, per command, construct inputs and
compare outputs against the oracle.

**First slice done 2026-07-26** (pure-output commands, both binaries executed
and diffed — evidence, not inference):

- `--version` SHAPE differs: oracle `2.1.220 (Claude Code)` vs port
  `lingxi-cli 0.12.0`. Version-then-parenthesised-product vs
  product-then-version. The VALUE difference is correct — LingXi is its own
  product with its own version, and the Claude-compat identifiers derive
  separately from `traits::CLAUDE_CODE_VERSION`. The SHAPE is a branding call
  that has not been made explicitly.
- `--help` layout differs (clap vs commander ordering: the oracle leads with
  `Usage:`, the port with the description). This is the known accepted
  divergence from the 2.1.216 audit, now confirmed by execution rather than
  inferred.
- No behaviour DEFECT found in this slice.

**Two false alarms were caught inside this slice before being recorded**: a
"materially different `mcp --help`" and an "`--help` performs network I/O", both
of which were interleaved stdout from concurrent processes sharing the
terminal. Run each comparison with `< /dev/null` and in isolation; a shared
terminal is enough to manufacture a finding.

**Method warning, earned three times this session:** a generated absence looks
identical whether the probe worked or not. Confirm every candidate at the call
site (`<cli> <sub> --help`, or the code path) before recording it. A one-level
walk produced 39 phantom flag gaps; a grep for two identifiers that never share
a line reported a wired feature as missing; a `--help` comparison read the ROOT
help as 20 subcommands' descriptions because an unrecognised path silently
falls back to it.

## 2. `mcp xaa` — the XAA (SEP-990) IdP connection subsystem

Absent as a concept. `mcp xaa setup` / `login` / `show` / `clear` manage an IdP
connection so XAA-enabled MCP servers authenticate silently:

```
xaa login  "Cache an IdP id_token so XAA-enabled MCP servers authenticate
            silently. Default: run the OIDC browser login. With --id-token:
            write a pre-obtained JWT directly (used by conformance/e2e tests
            where the mock IdP does not serve /authorize)."
           --force      ignore any cached id_token and re-login
           --id-token   write a pre-obtained JWT directly
setup      configure the IdP connection (one-time, all XAA servers)
           --client-id --client-secret --callback-port --scope
show       show the current IdP connection config
clear      clear the connection config and cached id_token
```

Storage side: `saveIdpClientSecret(issuer, secret)` and
`saveIdpIdTokenFromJwt(issuer, jwt)`. The port has neither — repo-wide,
`id_token` appears only as a JWT the OAuth handles DECODE and as a redaction
key, never as stored state. `secret::SecretKind` would gain the IdP variants.

**CORRECTION.** An earlier revision of this file listed `login --id-token` as a
separate item under `auth login`, and a comment in `LoginArgs` explained why it
was "deliberately not added". Both were wrong: `--id-token` belongs to
`mcp xaa login`, `auth login` never had it, and the oracle's own text says the
flag exists for conformance/e2e tests against a mock IdP — not the
headless-enterprise-auth affordance I described. The comment has been removed
rather than corrected, since `auth.rs` was never the right place for it.

## 3. `--help` TEXT layout

A known accepted divergence from the 2.1.216 audit, not a new finding. Listed
so it is not re-discovered as one.

## Two decisions, not fixes

- **`doctor` description** diverges beyond branding: the port additionally
  warns that stdio servers from `.mcp.json` are spawned. That is more
  informative than the oracle's text. Keep or align — a product call.
- **Remote-session client** (`useRemoteSession`, the
  `tengu_refusal_retraction_*` family) is an accepted divergence: no Anthropic
  private relay / auth contract. If that ever changes, note that `fzf` applies
  a retraction signal ONLY when `event.source === "worker"` — a trust boundary,
  since an unauthenticated retraction can erase history.

## The rule this session kept re-learning

A comment or backlog heading that asserts a fact about OTHER code becomes a lie
the moment that code moves. Four false findings this session traced to exactly
that — including 2.1.215's M-13, where an auditor read a stale module doc
saying a surface was "not wired" and filed a gap against working code.

When deferring, point at the code; do not restate what it does.
