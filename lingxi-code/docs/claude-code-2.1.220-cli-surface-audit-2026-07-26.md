# Claude Code 2.1.220 — CLI surface audit, 2026-07-26

LingXi baseline: `main` @ `9ea1bc506`
Oracle: `~/.local/share/claude/versions/2.1.220` (256,908,272 bytes)

## What this is, and what it is not

This is a **surface** audit: which flags and commands 2.1.220 advertises that
the port does not. It is mechanical and verifiable — extract every
`.option("--x"` and `.command("y"` from the binary, extract the same from the
release CLI's `--help` across its 17 subcommands, diff.

It is **NOT** a behaviour audit. A flag present in both can still diverge in
what it does; this pass says nothing about that. The 2.1.215 audit's method
(cross-checking behaviour at the code) is the other half and is still the
expensive part. Recording the boundary because a surface diff that gets read as
a parity verdict is worse than no diff.

## Method

```
oracle: rg '\.option\("(--[a-z0-9-]+)' over the binary          → 83 long flags
        rg '\.command\("([a-z0-9:_-]+)'                          → 45 command names
port:   lingxi-cli --help, then <sub> --help for each of 17 subs → 77 long flags
```

Flags are attributed to a command by scanning backwards to the nearest
`.command(` within 12 KB — good enough to group, not authoritative for a flag
that sits far from its registration.

## Result: 39 oracle flags absent from the port, grouped by owner

| Owner | Missing flags | Assessment |
|---|---|---|
| `eval` | `--ablation --allow-tools --case --judge-model --keep-temp --max-cost-usd --no-scaffold --output-dir --publish-report --report --runs --scaffold --tag --threshold` | **Internal benchmark harness.** 14 of the 39 — a third of the whole diff — belong to one command that is Anthropic's own eval tooling, not a user surface. Not a parity gap. |
| `login` | `--claudeai --console --email --id-token --no-browser --sso` | **Candidate gaps.** `--no-browser` and `--console` are ordinary headless-auth affordances; `--sso` / `--id-token` / `--client-id` touch enterprise auth the port may not model. Worth a behaviour pass. |
| `init` | `--author --author-email --description --with` | **Candidate gap** — plugin/project scaffolding metadata. |
| `add` / `remove` / `setup` | `--scope --client-id --client-secret --callback-port --sparse` | **Candidate gaps** — MCP server registration options. `--scope` (user/project/local) is the notable one: it decides WHERE a server is written. |
| `import` / `prune` / `purge` / `tag` | `--dry-run --yes --push --remote` | **Candidate gaps** — destructive-operation affordances. `--dry-run` on four destructive commands is the highest-value item here: it is the difference between previewing a purge and performing one. |
| `uninstall` | `--keep-data --prune` | Candidate gap. |
| `list` / `status` / `validate` / `defaults` | `--available --text --strict --label` | Output/validation modes; low value individually. |

## The honest headline

39 sounds large; the shape matters more than the count.

- **14 belong to `eval`**, an internal harness. Excluding it, the real surface
  delta is **25 flags across 12 commands**.
- **The single highest-value item is `--dry-run`**, which the oracle offers on
  `import`, `import-conversations`, `prune`, `purge` and `tag`. A destructive
  command with no preview mode is a usability and safety gap, not a cosmetic
  one.
- **`--scope` on `mcp add`/`remove`** is second: it selects the settings tier a
  server is written to, and its absence means the port can only write one.

## Command names

45 oracle `.command(` names vs 17 port top-level commands, but that comparison
is misleading and should not be quoted as a gap count: most oracle names are
SUBcommands (`add`, `list`, `get`, `enable`…) of groups the port also has
(`mcp`, `plugin`, `project`, `auth`). The names genuinely absent as concepts are
`eval`, `critique` and `xaa` — the first is the internal harness above; the
other two were not investigated in this pass.

## Not done

- No behaviour comparison for flags present in both.
- No `--help` TEXT diff (the 2.1.216 audit already records help LAYOUT as a
  deferred item).
- `critique` and `xaa` unexamined.
- Hidden/undocumented commands not enumerated (this pass only sees
  `.command(` registrations).
