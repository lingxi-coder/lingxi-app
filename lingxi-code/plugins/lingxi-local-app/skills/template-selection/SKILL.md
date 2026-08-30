---
name: template-selection
description: Pick the simplest workable Local App runtime profile from the Host's read-only catalog, and echo back its id, reasons, revision, and digest exactly as read.
---

# Select a Local App runtime profile

Own reading the Host's runtime-profile catalog and reasoning to the simplest
workable candidate for what the user already confirmed (a `dom` or `canvas`
surface, and what the brief actually needs). This skill's output is an id,
the reasons for it, and the catalog's own fields relayed exactly as read —
never a path, a recomputed hash, or a claim that the pick is final.

## Reading the catalog

`LocalAppRuntimeProfiles` is the only tool this skill calls. It takes no
arguments and returns every runtime profile the host knows about — both
scaffoldable ones and ones the host is deliberately holding back — each with
`family`, `revision`, `surface` (`dom` or `canvas`), `core_packages`,
`contract_sha256`, `available`, `availability_reason`, `cache_status`, and
`download_status`. The tool's own description says to read it before
choosing a non-default profile and that the catalog is authoritative — never
infer availability from source code or package names, and never reuse a
family/revision/digest remembered from an earlier turn or an earlier app;
call it fresh and read this turn's response.

## Narrowing to a candidate

1. Drop every entry with `available: false` outright — it is not a
   candidate, no matter how well it fits the brief. One entry is gated this
   way for real-device validation reasons the host, not this skill, owns;
   never talk a user into it or suggest a workaround.
2. Keep only entries whose `surface` matches what the user already confirmed
   (a multi-screen app needs `dom`; a single drawing surface needs
   `canvas`) — surface is fixed by that earlier confirmation, not a second
   decision this skill makes.
3. Among what is left, prefer the entry with the fewest `core_packages`
   beyond the shared baseline, unless the brief names something only a
   larger profile's packages provide (3D rendering, a game engine's sprite
   and physics/scene abstractions). Point at the specific package that
   justifies the larger pick rather than asserting "this one is fancier."
4. If nothing available matches the confirmed surface, say so plainly —
   do not fall back to a surface the user did not confirm.

## Handing back the result

Report the chosen family's id exactly as the catalog spelled it, the reasons
from step 3, and the catalog's own `revision` and `contract_sha256` for that
entry, unedited. Treat those two fields as an echo, not a value this skill
computed or is vouching for: the host recomputes and re-checks its own
digest independently at every later step that matters, so relaying anything
other than what this turn's tool call returned can only go stale, never
help.

This skill's job ends here. Turning a candidate into an app's actual runtime
binding is a separate, user-confirmed step the Local App workspace's own
guidance calls out on its own — this skill does not perform it, does not
mint or claim to mint whatever token that step produces, and does not call
`LocalAppScaffold` itself.

## Boundaries

- Never present an `available: false` entry as workable, and never guess
  around why it is gated — relay `availability_reason` verbatim if asked.
- Never state a `family`, `revision`, or `contract_sha256` value that did
  not come from this turn's `LocalAppRuntimeProfiles` response — not a
  remembered value, not one inferred from the template's source paths, and
  not one guessed from the family name's shape.
- Never claim this skill's pick is the app's final runtime — only the
  user-confirmed step after this one, and the host's own re-derivation at
  scaffold time, decide that.
- Never touch a template's own source files, `package.json`, or lockfile
  from here — this skill reasons about the catalog's own fields only.
