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
| legacy `commands/`-as-skills | ✅ verified (§2.3) | ✅ **present** — via `command-api`, not `skill-api` |
| conditional / `paths:`-activated | ✅ verified (§2.1) | ✅ **ported** (`4c7fb33bd` + `f790dc179`) |

### 2.1 ✅ Conditional (`paths:`) skills — verified, 2026-09-10

The needle the old note asked for, found without relying on symbol names:
`"skill_paths"` is a real pattern-source tag (`Yf` in `src_161574736.js`, beside
`claudemd_rule_globs` / `permission_rules`), and the activator is `lhr`
(`src_163219561.js` @4573642):

```js
function lhr(e,n){ if((g_e()?.conditionalSkills.size??0)===0) return [];
  let r=[];
  for(let[o,d] of PR().conditionalSkills){
    if(d.type!=="prompt"||!d.paths||d.paths.length===0) continue;
    let p=uhr.default().add(y8(d.paths,"skill_paths"));      // gitignore-style
    for(let y of e){ let v=sCt(y)?dhr(n,y):y;
      if(!v||v.startsWith("..")||sCt(v)) continue;
      if(p.ignores(v)){ PR().dynamicSkills.set(MGe(d),d);
        PR().conditionalSkills.delete(o);
        PR().activatedConditionalSkillNames.add(o);
        r.push(o), t(`[skills] Activated conditional skill '${o}' (matched path: ${v})`);
        break } } }
  if(r.length>0) i("tengu_dynamic_skills_changed",{source:_("conditional_paths"),…}) }
```

So the shape is:

* a skill whose frontmatter carries `paths:` is loaded into **`conditionalSkills`**,
  NOT into the listed set — it is invisible until activated;
* when the session touches a file matching any of its patterns
  (**gitignore semantics**, the `ignore` library, same compiler as
  `claudemd_rule_globs`), it moves into `dynamicSkills` and becomes available;
* the move is **one-way and once** (`conditionalSkills.delete`, and the name is
  recorded in `activatedConditionalSkillNames`);
* relative paths escaping the root (`v.startsWith("..")`) are skipped;
* it emits `tengu_dynamic_skills_changed` with `source: "conditional_paths"`, and
  logs `[skills] Activated conditional skill '{name}' (matched path: {path})`.

`paths` is also in the recognized-frontmatter-key union `O` (`src_161416353.js`)
alongside `when_to_use` / `hooks` / `context`, and in the command-metadata shape
next to `whenToUse` — consistent with skills surfacing as commands.

### 2.2 Mechanism landed (`4c7fb33bd`), listing filter still to wire

`skill_api::ConditionalSkills` implements the state machine:
`is_conditional` (non-empty `paths`), `is_available`, and `activate_for_paths`,
which matches gitignore-style through the `ignore` crate — the same crate the
`.worktreeinclude` matcher uses, and reporting through the already-declared
`SITE_SKILL_PATHS` telemetry site. Activation is one-way and once, and a path
that escapes the root is skipped.

⚠️ **Two bugs worth knowing about, both caught by the tests:** `docs/` matched
nothing until the matcher moved to `matched_path_or_any_parents` (plain `matched`
only tests the final component), and a path outside the workspace activated a
repo-scoped skill because the first version fell back to the absolute path when
it could not be made relative.

**Wired end to end (`f790dc179`).** `paths` travels from skill frontmatter onto
the command record (upstream carries it on the command metadata too, beside
`whenToUse`), and both composition roots drop un-activated conditional skills
from the model-facing listing.

🚨 **Activation is remembered, not recomputed.** `read_file_state` is an LRU
capped at 100 entries, so the file that revealed a skill can age OUT of the
touched set. Recomputing availability per turn would make a skill the model has
already been shown disappear again; the state lives with the listing provider for
the session, and a test evicts the matching path and asserts the skill stays.

⚠️ The read-state map is session-owned, not a module global — the first attempt
reached for an accessor that does not exist. It has to be threaded from the root
that creates it (`read_state_map`). ⚠️ The sibling
`tengu_dynamic_skills_changed` `source: "file_operation"` belongs to DYNAMIC skill
discovery (`Dynamically discovered {n} skills from {m} directories`) — a
different mechanism that is also absent; do not conflate them.

### 2.3 ✅ `commands/`-as-skills — verified present, and the old row was misleading

Upstream's loader is at `src_163219561.js` @4565355: it walks the commands dirs
and builds skills with `loadedFrom: "commands_DEPRECATED"`, `paths: void 0`
(so a legacy command is never conditional) and the default description
`"Custom command"`, reporting through `skill_load_commands_dir` /
`skill_load_commands_parse_failed`.

**This port already does it.** `command-api/src/markdown_loader.rs` scans
`.lingxi/commands/**.md` (`SUBDIR = "commands"`, plus the managed dir) and tags
every command `loaded_from: Some("commands_DEPRECATED")`, and both listing
closures admit that tag alongside `bundled` / `skills`.

🚨 The old row said "`LoadedFrom::CommandsDeprecated` declared, never
constructed" and marked the tier absent. The declaration it named is
`skill_api::LoadedFrom`'s variant — a DEAD variant in a parallel type, while the
tier itself lives on `command_api`'s string-valued `loaded_from`. Reading one
type's unused variant as the feature's absence is the same mistake as reading a
0-hit grep for a foreign symbol as proof: the behaviour was never checked. The
dead variant is worth removing on its own, but it is not this feature. Their upstream identifiers
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

The port registers **14** through `register_bundled_skills` (11 + `update-config`,
`keybindings-help` and `explain-usage`, added 2026-09-10), plus `claude-api` as the `skill-api`
compiled-in builtin.

| in both (8) | LingXi-only (4) | upstream-only (13) |
|---|---|---|
| batch, claude-api, code-review, dataviz, explain-usage, fewer-permission-prompts, keybindings-help, loop, run, run-skill-generator, update-config | cron, deep-research, simplify, verify | artifact-components, claude-in-chrome, debug, design-sync, memory-types, setup-claude, whiteboard, workflow-authoring, workshop |

⚠️ **The 13 are not a backlog.** Most ride surfaces this port does not have:
`artifact-components` / `whiteboard` / `workshop` / `design-sync` need the
Artifact and Design surfaces (the Artifact tool here is a deliberate
register-but-disabled skeleton pinned at 2.1.207), and `claude-in-chrome` needs
the browser extension. Each needs adjudicating on its own substrate before
anyone ports it. The ones with no obvious blocker and therefore worth triaging
first are `update-config`, `keybindings-help`, `explain-usage` and `doctor`.

### ✅ `update-config` (`29a8e64a4`) and `keybindings-help` (`f7fb8840a`) — ported

Both were recorded as needing "dynamic-prompt plumbing plus a branding decision".
**Neither blocker was real.** `SlashCommandKind::Bundled` already carries a
`prompt_fn`, and the `branding` crate already fixes the product name, config dir
and env prefix — there was nothing left to decide.

What makes both worth having is the same property: their prompts carry LIVE data
rather than prose.

* `update-config` injects the settings schema generated from the very
  `SettingsJson` the loader parses (`schemars::schema_for!`), so it cannot
  describe a key the loader would reject. Both prompt shapes are ported,
  including the `[hooks-only]` one that swaps the entire prompt and carries no
  schema.
* `keybindings-help` builds its contexts / actions / reserved tables from
  `command_core::keybindings` — the same tables the validator uses. The action
  column INVERTS the default-binding table, so a moved default shows its new key
  with no prose to update. `userInvocable:!1` is kept verbatim: model-invocable
  only, the user route stays `/keybindings`.

⚠️ Both bodies are the binary's, rebranded (`Claude Code`→`LingXi`,
`.claude/`→`.lingxi/`, `claude --debug`→`lingxi-cli --debug`), with a test per
skill asserting none of those strings survive.

🚨 The name-set lock caught BOTH additions, and caught `keybindings-help` going
in out of alphabetical order (the list is compared sorted). Update it
deliberately; do not re-bless it.

### ✅ `explain-usage` (`724b9e52b`) — ported

A single-prompt skill, so the only judgement in it is what NOT to rebrand:
`${CLAUDE_CONFIG_DIR:-$HOME/.claude}` becomes the LingXi pair, but
`mcp__claude-in-chrome__` stays **verbatim** — that is an MCP server id on the
wire, not a product reference, and rewriting it would point the skill at a
server that does not exist. A test pins each direction.

Also pinned: the line telling the model to treat transcript contents as data
rather than instructions. It is the skill's only defence against a transcript
that contains instruction-shaped text.

### ⛔ `doctor` — NOT a port; this surface already exists by another mechanism

Upstream's `doctor` is a bundled SKILL (`uo({name:"doctor",aliases:["checkup"],
survivesBundledKillSwitch:!0,requires:{workspace:!0},terminalOriented:!0,…})`) —
an LLM-driven health check that reads local data and proposes fixes.

**This port already ships `/doctor`**, but as a deterministic builtin command
handler (`commands/core/src/doctor.rs`) rendering a LOCKED report, plus the
`lingxi-cli doctor` subcommand. Registering a bundled skill under the same name
would collide with that builtin registration.

So this is an adjudication, not a backlog item: the user-facing capability is
present, delivered differently. ⛔ Do not "port" it by registering a second
`doctor` — decide first whether this port wants the deterministic report, the
model-driven one, or both under distinct names. Same shape as the
`commands/`-as-skills row above: a feature judged absent because one mechanism
was missing, when another already covers it.

⚠️ **A near miss worth recording.** `stuck` was reported out of this audit as a
fourth portable name. It is not a bundled skill — it has ~50 occurrences in the
2.1.267 binary and **no `uo({name:"stuck"` registration**; they are the English
word. The list above never contained it, because it was built from
registrations; the claim came from reading a substring count as if it were one.
🚨 A bundled name is only a bundled name when a registration says so — literal
`uo({name:"…"})` or a resolved variable. `skillify` and `lorem-ipsum` fail the
same test from the other direction (present in the stale TS mirror, absent from
the binary).

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

## 5b. The two portable bundled skills — sized

`update-config` and `keybindings-help` are the two upstream-only names with no
substrate blocker, and both are genuinely LIVE upstream:

| | gate | user-invocable |
|---|---|---|
| `update-config` | none — always on | yes |
| `keybindings-help` | `OF(){return H("tengu_keybinding_customization_release",!0)}` — default **true** | **no** (model-only) |

⚠️ **Neither is a quick win, and an earlier note here implied otherwise.** Both
carry a DYNAMIC prompt: `getPromptForCommand(e)` assembled from live host state,
not a static body. `update-config` additionally branches on a `[hooks-only]`
prefix; `keybindings-help` composes roughly nine sections plus the session's
actual keybindings. The port has the substrate for this (`dynamic_body` /
`BundledPromptFn` on `SkillDescriptor`), so it is a port rather than an
invention — but it is a feature each, not an afternoon.

They also both name Claude Code and `~/.claude/` paths in copy the model reads,
so porting them lands on the established branding divergence and needs the
LingXi rebrand rather than a byte-exact copy. That is a decision to take
deliberately, not incidentally.

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
