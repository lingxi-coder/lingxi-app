# Skills vs Claude Code 2.1.267

First alignment audit of this subsystem. There has never been one: no prior doc
covers skills, and `grep -rl '2\.1\.26[0-9]' docs/` finds nothing for it.

Oracle: `~/.local/share/claude/versions/2.1.267`, sha256
`a681f3008f0050029aeebcab3af51bb6a55ddeb625a3af3141a4416d43cd2558`, extracted to
`~/.claude/oracle-chunks/2.1.267/` (1657 chunks). Tree at `2d081b3dd`.

🚨 **Every claim below is checked against the executable.** An early pass of
this audit used `claude-code/src/skills/*.ts`, and that mirror is stale enough to
produce wrong findings — it lists `skillify` and `lorem-ipsum` as bundled skills
and both are **0 hits in the 2.1.267 binary**. Skill names survive minification
as string literals, so that is decisive: they do not exist upstream and must not
be ported. Anything sourced from the mirror was re-derived or dropped.

---

## 1. Shape: there are two parsers and two scanners, and the obvious one is not live

| | parses | reaches the model? |
|---|---|---|
| `skill-api/src/{model,frontmatter}.rs` | the FULL key set (21 keys) | **no** — feeds bundled/plugin registration and the `/skills` listing |
| `command-api/src/markdown_loader.rs` | a subset | **yes** — this is what turns `SKILL.md` into an invocable `SlashCommand` |

A skill reaches the model as a `SlashCommand`, through
`load_skill_markdown_files_with_roots` → `load_skill_dir` → `build_skill_command`.
Reading `skill-api` and concluding a key is supported is the trap this subsystem
sets; two of the three defects below were exactly that.

## 2. Source tiers

| tier | upstream | port |
|---|---|---|
| managed / policy | ✅ | ✅ |
| user (`~/<DOT_DIR>/skills`) | ✅ | ✅ |
| project, walking UP to the repo root | ✅ | ✅ `project_dirs_up_to_home` |
| `--add-dir` roots | ✅ | ✅ **fixed in `e63f84d11`** — see §4.2 |
| bundled | ✅ 21 | ⚠️ 11 + 1, see §3 |
| plugin | ✅ | ✅ |
| MCP-derived | ✅ | partial — resolves as `Other` and is rejected by the tool |
| legacy `commands/`-as-skills | ❓ unverified | ❌ `LoadedFrom::CommandsDeprecated` declared, never constructed |
| conditional / `paths:`-activated | ❓ unverified | ❌ absent |

⛔ The last two are marked unverified deliberately. Their upstream identifiers
(`loadSkillsFromCommandsDir`, `activateConditionalSkillsForPaths`,
`getDynamicSkills`) are **0 hits in the binary — which proves nothing**, because
minification erases source-level function names. They must be established by a
behaviour needle or a string literal before anyone builds them.

**The single-level `read_dir` is CORRECT — do not "fix" it.** Upstream's scan is
also one level (`skill-name/SKILL.md` only). This is *not* the shape of hole the
agent audit found in the agent-catalog loader, and the two should not be
conflated.

## 3. Bundled skills — the name set

Upstream 2.1.267 registers **21** (`uo({name:…})`, variable names resolved):
`artifact-components`, `batch`, `claude-api`, `claude-in-chrome`, `code-review`,
`dataviz`, `debug`, `design-sync`, `doctor`, `explain-usage`,
`fewer-permission-prompts`, `keybindings-help`, `loop`, `memory-types`, `run`,
`run-skill-generator`, `setup-claude`, `update-config`, `whiteboard`,
`workflow-authoring`, `workshop`.

The port registers 11 through `register_bundled_skills`, plus `claude-api` as
the `skill-api` compiled-in builtin.

| in both (8) | LingXi-only (4) | upstream-only (13) |
|---|---|---|
| batch, claude-api, code-review, dataviz, fewer-permission-prompts, loop, run, run-skill-generator | cron, deep-research, simplify, verify | artifact-components, claude-in-chrome, debug, design-sync, doctor, explain-usage, keybindings-help, memory-types, setup-claude, update-config, whiteboard, workflow-authoring, workshop |

⚠️ **The 13 are not a backlog.** Most ride surfaces this port does not have:
`artifact-components` / `whiteboard` / `workshop` / `design-sync` need the
Artifact and Design surfaces (the Artifact tool here is a deliberate
register-but-disabled skeleton pinned at 2.1.207), and `claude-in-chrome` needs
the browser extension. Each needs adjudicating on its own substrate before
anyone ports it. The ones with no obvious blocker and therefore worth triaging
first are `update-config`, `keybindings-help`, `explain-usage` and `doctor`.

**Now locked.** `the_bundled_skill_name_set_is_locked`
(`commands/core/src/bundled/mod.rs`) pins the set in BOTH directions — it
enumerates `list_all()` rather than filtering a hardcoded list, so an addition
fails as loudly as a removal — and separately asserts that five
surface-dependent upstream names stay absent. Nothing pinned this before: the
per-skill tests each check one skill, so gaining or losing a whole skill changed
no assertion. Assert names, never a count.

## 4. Defects found and fixed

### 4.1 `user-invocable: false` was a no-op — `e6d65a403`

`build_skill_command` hardcoded `user_invocable: Some(true)`, and the production
frontmatter reader never parsed the key at all. Upstream:
`let dt=v["user-invocable"], en=dt===void 0?!0:htt(dt)` with
`htt(e)=c1(e)??!1` — absent means invocable, and a value that coerces to neither
boolean set means `false`. Load-bearing on two surfaces that filter on it
(`CommandRegistry`'s listing and the TUI slash menu), so a skill asking to be
hidden now is.

### 4.2 The `--add-dir` tier was plumbed and never populated — `e63f84d11`

All three desktop registration sites passed `Vec::new()`. Wiring it naively
would still have found nothing: upstream joins each root with `.claude/skills`
(`for(let e of Up()){ let S=P.join(e,".claude","skills"); … }`,
`src_172414592.js` @5180) while the port's loader takes already-resolved skill
dirs. This repo already agreed — the repo-root reload path has always built
`root.join(branding::DOT_DIR).join("skills")`. Only initial registration was
empty. Desktop-only; engine-mobile has no `add_dir` concept.

### 4.3 A skill's declared `effort` never reached its fork — `2d081b3dd`

`ForkedSkillScoping.effort` exists, validates against a faithful port of
`union([enum(low|medium|high|xhigh|max), int().min(1).max(1000)])`, is persisted
beside the fork's transcript and replayed on resume — and was **always `None`**,
because nothing above it parsed the key. Upstream's generic skill→command
builder carries it (`wXe({… effort: De …})`, `src_163219561.js` @4555866) and
the scoping spreads it in conditionally.

The raw carrier is an untagged enum because `effort: high` arrives as YAML text
and `effort: 500` as an integer. Conversion is deliberately lenient, mirroring
`Gx`: outside the union yields `None`, so a typo means "declared no effort" and
the fork still launches.

## 5. ⛔ Seven keys that are NOT worth porting the way they look

`version`, `hooks`, `paths`, `metadata`, `created_by`, `improved_by`,
`hide-from-slash-command-tool` are absent from the production reader — and
**already parsed, with tests, in `skill-api`, where nothing reads any of them**
(`skill-api/src/model.rs:39` labels one "P2 gap" in so many words).

Adding them to `CommandFrontmatter` would produce a second parsed-and-unread
copy: the exact "named, computed, never wired" shape this audit exists to find,
doubled. **Build the consumer first, then the plumbing.** `effort` was the one
member of this group with a live consumer, which is why it is in §4 and these
are not.

## 6. Gates

- `scripts/check_skill_frontmatter.py` is a real fail-closed gate (name == dir,
  description ≤ 180 display columns, frontmatter parsable) but `SKILL_ROOTS`
  covers only `plugins/lingxi-local-app/skills` — not the bundled skills and not
  `<DOT_DIR>/skills`. Widening it is cheap and unclaimed.
- No fixture anywhere pins bundled skill BODIES against the oracle. The
  `*_body.md` files are `include_str!`-ed and never diffed.

## 7. Open, with blockers

| item | blocker |
|---|---|
| conditional / `paths:`-activated skills | ⛔ establish the behaviour at the binary first; the symbol-name greps prove nothing |
| legacy `commands/`-as-skills tier | same |
| the 13 upstream-only bundled skills | most need a surface this port does not ship; triage `update-config` / `keybindings-help` / `explain-usage` / `doctor` first |
| MCP-derived skills | resolve as `Other` and are rejected by the tool |
| the seven unread keys (§5) | need a consumer before they need a parser |
