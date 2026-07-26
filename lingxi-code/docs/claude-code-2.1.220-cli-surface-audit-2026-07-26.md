# Claude Code 2.1.220 — CLI surface audit, 2026-07-26

LingXi baseline: `main` @ `bbb2bb719`
Oracle: `~/.local/share/claude/versions/2.1.220` (256,908,272 bytes)

## CORRECTION — this document's first revision was wrong

The first revision reported **39 missing flags** and named `--dry-run` (on five
destructive commands) and `mcp --scope` as the highest-value gaps.

**Every one of those already exists.** The extraction script walked only the 17
TOP-LEVEL subcommands; the oracle registers most of its flags on NESTED ones
(`plugin prune`, `plugin tag`, `mcp add`, …). Walking one level and diffing
against a two-level oracle manufactures a gap list out of a traversal bug.

`lingxi-cli plugin prune --help` shows `--dry-run`, `--scope`, `--yes`;
`plugin tag --help` shows `--dry-run`, `--push`, `--remote`; `plugin uninstall`
shows `--keep-data`, `--prune`; `plugin validate` shows `--strict`. All of it
was there before I filed any of it.

Corrected below with a recursive walk (51 command paths, depth 3).

## Method

```
oracle: every `.option("--x"` in the binary                     → 83 long flags
port:   lingxi-cli --help, recursively through every "Commands:"
        block to depth 3                                        → 51 paths, 106 long flags
```

## Result: 16 oracle flags absent, and 14 of them are one internal command

| Owner | Missing | Assessment |
|---|---|---|
| `eval` | `--ablation --allow-tools --case --judge-model --keep-temp --max-cost-usd --no-scaffold --output-dir --publish-report --report --runs --scaffold --tag --threshold` | Anthropic's internal benchmark harness. Not a user surface; the port has no `eval` command at all, deliberately. **Not gaps.** |
| `login` | `--id-token <jwt>` | *"Write this pre-obtained id_token directly to cache, skipping the OIDC browser login"* — an enterprise/CI affordance for a headless OIDC flow. The port models `id_token` inside the OAuth handles but exposes no way to inject one. **Real, small.** |
| `defaults` | `--label <prefix>` | *"Show only rules whose label starts with this prefix (case-insensitive)"* — a filter on a rules listing. **Real, cosmetic.** |

**So the genuine user-facing CLI surface delta against 2.1.220 is two flags**,
one of which only matters for headless enterprise auth.

## Command names

45 oracle `.command(` names vs 17 port top-level commands is a MISLEADING
comparison and should not be quoted as a gap count — most oracle names are
subcommands of groups the port also has. Absent as concepts: `eval` (above),
`critique`, `xaa`. The latter two were not investigated.

## What this pass does NOT cover

- **Behaviour.** A flag present in both can still diverge in what it does. This
  is a surface diff and says nothing about that. The 2.1.215 audit's method —
  cross-checking behaviour at the code — remains the expensive half.
- `--help` TEXT (layout is a known deferred item from the 2.1.216 audit).
- Hidden commands not registered through `.command(`.
- `critique` and `xaa`.

## The lesson, since I filed the bad version myself

A tool-generated gap list is only as good as the traversal. I diffed a
one-level walk against a two-level oracle, got 39 plausible-looking gaps, ranked
them by severity, and committed a document recommending work that was already
done — in the same session where I had corrected four other stale artifacts for
exactly this failure mode. **Spot-check a generated list against the running
binary before ranking it**: one `plugin prune --help` would have caught it.
