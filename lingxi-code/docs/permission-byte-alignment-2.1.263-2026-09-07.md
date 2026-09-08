# Permission subsystem — byte-level alignment vs Claude Code 2.1.263

**Date:** 2026-09-07
**Oracle:** `~/.local/share/claude/versions/2.1.263` (npm `latest` — confirmed
`npm view @anthropic-ai/claude-code dist-tags` → `latest: 2.1.263`, so this IS
"latest Claude Code"; `stable` is 2.1.236).
**Baseline:** `claude-code-2.1.232-permission-parity-closure-2026-08-14.md` —
the last time this subsystem was declared aligned. 2.1.232 was re-fetched with
`npm pack @anthropic-ai/claude-code-darwin-arm64@2.1.232` for the delta.
**Port surface:** `lingxi-code/permission/` — 73 files, 75 933 lines.

## Headline

Permission was closed against **2.1.232**. Thirty-one releases later the
**2.1.232 → 2.1.263 permission delta is essentially unported**:

| | |
|---|---|
| permission-relevant prose strings NEW in 2.1.263 (after de-noising) | **458** |
| …already present in the port's Rust sources (whole workspace, not just `permission/`) | **24** |
| …**absent** | **434** |

Full list: `permission-delta-2.1.263/absent-strings.json`.

This is a delta-porting programme, not a byte-polish pass.

## Method (reproducible; scripts committed)

1. `scripts/oracle_prose_delta.py` — prose-string set difference between two
   oracle binaries, then filter by permission keywords and drop code-shaped runs.
2. `scripts/perm_extract_literals.py` — pull every shipped string literal from
   `permission/src` (skips `#[cfg(test)]` blocks and comments).
3. `scripts/perm_verify_literals.py` — verify each literal against the oracle
   binary **placeholder-aware**: split on `{…}`, require every fixed segment to
   appear. Tries raw / `\n`-escaped / `\uXXXX` / `\xXX` / quote-escaped forms.

### Two tooling traps this pass had to fix (both produced false "divergences")

- **Rust `\u{2014}` escapes.** A naive un-escaper turns `\u{2014}` into the
  literal `u{2014}`. Every em-dash string then "misses".
- **🚨 Rust line-continuation `\`+newline.** A backslash at end-of-line inside a
  string literal removes the newline **and all leading whitespace on the next
  line**. Before handling this, `denial_tracking.rs`, `shadow.rs` and
  `loader.rs` all looked like they were shipping 20 spaces of source
  indentation inside user-visible messages. They are **not** — the values are
  single-line and correct (`loader.rs`'s own test pins the flat form). Do not
  "fix" these.

Non-ASCII in the binary is stored escaped and **not uniformly**: em-dash is
`—`, but `×` is `\xD7`. A raw byte grep for either returns zero.

## Verified state of the pre-2.1.232 surface

Of 1 462 shipped literals in `permission/src`, 451 verify byte-exact against the
oracle. The rest are, on inspection of the decision core:

- **port-internal diagnostics** — `tracing::warn!`/panic strings with no oracle
  counterpart (`Hook returned 'allow' for …` ×2, `permission check for … crashed
  under an active bashCommandClamp`, `auto-mode classifier saw … host-context
  line(s)`, `resolve_outcome returned Some for Invalid`, …);
- **documented intentional divergences** — `prompting_gate.rs` is a LingXi
  plain-REPL `[Y/n]` surface (its module docs record that claude-code has no
  stdin permission prompt at all); Restricted mode; local-app boundaries;
  `LingXi is running in don't ask mode`;
- **composed-in-the-binary strings** that a naive matcher splits, e.g.
  `DENIAL_WORKAROUND_GUIDANCE` is two adjacent string-table entries in the
  oracle (`IMPORTANT: You *may* attempt …denial. ` + `If you believe this
  capability is essential…`) and the port's single const is their exact
  concatenation — **not** a divergence.

Two items in the decision core are still genuinely unresolved and want an
oracle-side answer before any edit:

- `policy.rs` `"Command exceeds maximum length of 10000 characters and cannot be
  statically analyzed"` — `10000 characters` has **zero** hits in 2.1.263. The
  oracle's neighbouring texts are `Contains shell syntax (${type}) that cannot
  be statically analyzed`, `timeout with ${x} flag cannot be statically
  analyzed`. Does 2.1.263 cap command length at all?
- `policy.rs:2164` prefix-matches `"Dangerous rm operation"` / `"Dangerous rmdir
  operation"`. The oracle's producer is
  `HL(e,t,r) → decisionReason.reason = \`Dangerous ${e} operation ${r}\``
  (`classifierApprovable:!1`, `circuitBreaker:"dangerousRemoval"`). The prefix
  match is compatible; the **message** (`t`) still needs a side-by-side.

## The 2.1.232 → 2.1.263 delta, by theme

### P0 — a whole new setting family, entirely absent
**`permissions.blockReadsOutsideWorkingDirectories`** (12 strings). Reads outside
the working directories are blocked; the copy steers the user to `/add-dir` or to
removing the setting, and there is a distinct message for a command the shell
parser cannot analyse under the read block, and another for a path that "cannot
be checked against the read block". Nothing in the port mentions it.

### P1
- **PermissionRequest hook decision shape** (11) — `hookSpecificOutput.permissionDecision`
  vs the legacy top-level `approve|block`; the validation copy
  `(PermissionRequest decision must be {"behavior": "allow"} or {"behavior":
  "deny", "message": "..."})`; `PermissionRequest allow ignored: a confined
  session takes grants only from its command line`.
- **Plan-mode approval / consent floor** (28) — `Plan-mode artifact consent
  floor`, "in plan mode the approval must come from the user, not the
  auto-permission classifier", artifact delete/upload refusals from plan mode,
  `Only the team lead can approve plans`.
- **Sandbox filesystem isolation** (11) — `sandbox.filesystem` additional
  read/write deny paths merged with `Read(...)`/`Edit(...)` deny rules;
  enterprise-policy-requires-sandbox on Windows; `Sandbox: ignoring permission
  rules and sandbox.filesystem entries from disabled setting source`.
- **cd / working-directory interaction** (14) — `Compound command contains cd
  with a relative file read while a Read() deny rule exists`; deny evaluation
  after an unresolvable `cd`.

### P2
- **MCP approval** (13), **subagent/teammate permission surface** (8)
  (`Subagent declared permissionMode: bypassPermissions but this session is not
  running in a contained no-internet environment…`), **remote-control /
  `--print` permission host** (4), **host-asserted `classifierContext`** (2).
- 343 further strings not yet themed — see the JSON.

## Suggested order

1. `blockReadsOutsideWorkingDirectories` — new user-facing setting, self-contained.
2. PermissionRequest hook decision shape — protocol-visible, affects hook authors.
3. Plan-mode consent floor.
4. sandbox.filesystem deny-path merge.
5. Re-run `perm_verify_literals.py` after each; it is the regression gate.

## Not done in this pass

No permission code was changed. This pass built and validated the tooling,
re-established the baseline, and produced the worklist. The two decision-core
questions above should be answered at the oracle before any edit — the
line-continuation trap above is exactly how a "fix" ends up worse than the
defect.

---

# P0 reverse-engineering — `blockReadsOutsideWorkingDirectories`

Fully extracted from 2.1.263. **It is not "add a setting": the gate it hangs off
(`sc`) does not exist in the port at all.**

## Oracle constants (byte-verified)

```js
ov = "Reads outside the working directories are blocked (permissions.blockReadsOutsideWorkingDirectories). Add the directory with /add-dir, or remove that setting."

ic = { why: "--restricted confines the file tools to the working directory.",
       reason: Ctt }                       // Ctt = "--restricted: path outside the working directory"
Ep = { why: "the permissions.blockReadsOutsideWorkingDirectories setting blocks reads outside the working directories. Ask the user to add the directory with /add-dir, or to remove that setting.",
       reason: ov }
```

Settings schema (`Hs(e)`), `permissions.blockReadsOutsideWorkingDirectories`:
> `Refuse file-tool reads (Read, Grep, Glob, LSP) outside the working directories in every permission mode; true in any settings source wins. Also set when the user picks "block" on the one-time auto-mode prompt for a read outside the working directories.`

Managed policy: `{path:["permissions","blockReadsOutsideWorkingDirectories"], restrictive:!0}`;
merge is `if (e.permissions.blockReadsOutsideWorkingDirectories === !0) d.… = !0`
— **OR across sources, not last-wins.**

## The gate

```js
function sc(path, forms, ctx, ruleCheck, why, workingDirs){
  if (Bh(path, ctx, forms, workingDirs) || ruleCheck().behavior === "allow") return null;
  return { behavior:"deny",
           message:`${path} is outside ${[...workingDirs].join(", ")}; ${why.why}`,
           decisionReason:{ type:"other", reason: why.reason } };
}
```
called from the read path as
```js
if (ctx.restricted || ctx.blockReadsOutsideWorkingDirectories)
  sc(path, forms, ctx,
     () => BK(…, {restricted, blockOutsideReads: flag, readBlockFence: flag}),
     ctx.restricted ? ic : Ep,
     ctx.blockReadsOutsideWorkingDirectories ? mEt(ctx) : rb(ctx));
```

🚨 **`mEt` is not `rb`:**
```js
mEt(e) = new Set([ originalCwd,
                   ...e.additionalWorkingDirectories.values()
                       .filter(t => t.source !== "projectSettings")
                       .map(t => t.path) ])
```
Under the read block a **project-settings `additionalDirectories` entry does not
widen the allowed set**. Only cwd and non-project sources (user settings,
`/add-dir`, CLI) count.

`readBlockFence && !restricted` carve-outs inside the rule check (all still
readable): `~/.claude/CLAUDE.md` ("The user memory file is allowed for reading"),
`~/.claude/{skills,plugins,rules,agents,commands}/…` (`User ${dir} files are
allowed for reading`), bundled skill reference files; auto-memory files are
allowed unless `blockOutsideReads && kzt()`.

A second, distinct producer is used by the Bash-side path checks:
`ZCe(path, ctx, forms)` → `{allowed:false, decisionReason:{type:"safetyCheck",
reason: ov, classifierApprovable:false, circuitBreaker:"outsideReadsBlocked"}}`.
Note the two shapes: the file-tool gate denies with `type:"other"`, the Bash-side
check reports a `safetyCheck` with a circuit breaker.

## Two blockers found in the port

1. **The `sc` gate is entirely absent.** `filesystem.rs` and
   `path_constraints.rs` contain zero `restricted` references; `policy.rs`'s
   restricted handling is only `is_restricted_protected_mutation` (Editor tools
   + `Config`). So **`--restricted` does not confine the file READ tools today**
   — even though `apps/cli/src/argv.rs:280` `RESTRICTED_HELP` promises "Also
   confines the file tools to the working directories (--add-dir included)".
   That is a pre-existing, security-relevant gap this P0 surfaced, and it must
   be fixed in the same change because `sc` is one function with two triggers.
2. **`PermissionContext.additional_working_dirs: Vec<PathBuf>` carries no rule
   source**, so `mEt`'s `source !== "projectSettings"` filter cannot be
   expressed. Shipping the gate without it makes the read block silently
   **more permissive** than the oracle whenever project settings add a
   directory. Populated from 3 composition roots
   (`engine-desktop/src/lib.rs:11750`, `engine-mobile/src/host.rs:3826`,
   `tools/skill/src/prompt_shell.rs:564`).

Also missing: `PermissionDecisionReason::SafetyCheck` has no `circuit_breaker`
field (oracle `circuitBreaker`), needed for `outsideReadsBlocked` — 17 non-test
construction sites, 42 with tests.

## Landed 2026-09-07 (steps 1, 3, 4 of the order below)

`permission` (lib + 8 integration binaries): **1429 passed, 0 failed**;
`tool-workflow`, `tool-skill`, `engine-desktop`, `engine-mobile` all build with
tests. Disabling the gate turns exactly the
five gate-dependent new tests red and nothing else; the file was restored
byte-identically afterwards (`diff -q`). All four copy constants re-verified
present in the 2.1.263 binary.

- `policy.rs` — `OUTSIDE_READS_BLOCKED_REASON` (`ov`), `RESTRICTED_OUTSIDE_WHY`
  (`ic.why`), `RESTRICTED_OUTSIDE_REASON` (`Ctt`), `READ_BLOCK_WHY` (`Ep.why`),
  all byte-locked; `PermissionPolicy::{project_settings_working_dirs,
  block_reads_outside_working_directories}` + builders; `read_block_working_dirs`
  (= `mEt`); `outside_working_dirs_denial` (= `sc`) wired as an
  `authorize_with_mode_and_workspace_lease` post-pass next to RESTRICTED-01, so
  it applies in EVERY permission mode.
- `loader.rs` — `permissions.blockReadsOutsideWorkingDirectories` +
  `block_reads_outside_working_directories_from_settings_json` +
  `fold_block_reads_outside_working_directories` (OR across sources).
- This also gives `--restricted` the file-READ confinement it never had, which
  `RESTRICTED_HELP` has been promising (blocker 1).

**Blocker 2 was solved by the real refactor** (the subset workaround was
reverted): `permission/src/working_dirs.rs` now models claude-code's
`ToolPermissionContext.additionalWorkingDirectories` faithfully — a path-keyed,
insertion-ordered set of `WorkingDirectory { path, source }` with `Map`
semantics (`insert` re-sets an existing key's source in place, `remove`,
`contains`). `PermissionPolicy.additional_working_dirs` is that type;
`paths()` is `rb`, `read_block_paths()` is `mEt`.

The source is now carried end to end rather than reconstructed:

- `engine-desktop` records the tier source at all four assembly points
  (user/project/local loop, `flagSettings`, managed `policySettings`, and CLI
  `--add-dir` → `cliArg`); `engine-mobile` at its tier loop.
- `policy_gate`'s `addDirectories`/`removeDirectories` reducer now USES the
  update's `destination` as the entry source — it previously parsed it only as a
  validity check and threw it away.
- The `working_directory` permission layer inserts with source `session` and
  keeps the oracle's `!has(dir)` guard, so an existing entry keeps its original
  source.

This also fixed a latent gap the subset hack would have papered over: a
`/add-dir` at runtime and a `projectSettings` `additionalDirectories` entry were
previously indistinguishable once folded.

### 🚨 Correction: `ruleCheck()` is `BK`, not the allow-rule bucket

The first cut mapped `sc`'s `ruleCheck().behavior === "allow"` escape onto "the
policy produced an Allow that a RULE matched". **That was wrong and permissive.**
In the oracle's read gate the order is

```js
for (rule of denyRules) …                       // deny rules
if (restricted || blockReads) { sc(…) }         // ← short-circuits here
… allow rules are consulted only after this …
```

so an `sc` denial returns before any allow rule is reached: **an explicit
`Read(<path>)` allow rule does NOT escape the block.** `ruleCheck()` is `BK`, the
read-side FILESYSTEM allowance walk, which is a different mechanism entirely.

Fixed: `read_block_allowance` (`BK`) is now the escape, and
`read_block_beats_an_explicit_allow_rule` pins it — it asserts the same read is
allowed without the block and denied with it, so the test cannot pass by the
rule simply failing to match.

### `BK` read-side allowance walk

Under the block the oracle computes `k = remoteSurface || restricted ||
blockOutsideReads`, which **suppresses** the `!k`-gated carve-outs (agent memory,
`~/.claude/tasks`, `~/.claude/teams`). What survives is the
`readBlockFence && !restricted` group, now ported byte-exact:

- `<configHome>/CLAUDE.md` → `The user memory file is allowed for reading`
- `<configHome>/{skills,plugins,rules,agents,commands}[/**]` →
  `` `User ${dir} files are allowed for reading` `` (template literal in the
  binary, so the string table holds `User ` and ` files are allowed for reading`
  as separate fragments — a whole-string grep MISSES; the rendered form is what
  matters)

The group is gated on `!restricted`, so `--restricted` gets no fence carve-out —
pinned by the same test.

### Still open for this P0

- The **session-scoped** `BK` allowances that also survive the block are not
  ported: plan files, tool-result files, scratchpad, job `tmp/`, project temp,
  and `Rzt()` bundled skill reference files. The `permission` crate has no
  session-directory plumbing to reach them (its deps are branding / protocol /
  telemetry / platform-api). Their absence makes the block STRICTER than the
  oracle, never looser — so it is a usability gap, not a security one.
### Step 5 landed 2026-09-08 — `circuit_breaker` + the Bash-side shapes

- `PermissionDecisionReason::SafetyCheck` gained `circuit_breaker:
  Option<SafetyCircuitBreaker>`. The enum is the COMPLETE 2.1.263 value set,
  taken from a grep of the binary for `circuitBreaker:"…"`: `backgroundOperator`,
  `dangerousRemoval`, `isolatePeerMachines`, `outsideReadsBlocked`,
  `restrictedMode`, `suspiciousWindowsPath`.
- Two sites carry a verified value: `ask_dangerous_removal` →
  `DangerousRemoval` (oracle `HL`) and `ask_background_operator` →
  `BackgroundOperator` (verified in the binary). Everything else is `None` —
  the port's restricted-mode and rm-variable-path asks use LingXi wording that
  does not appear in the binary at all, so attaching a breaker there would be
  invention.
- New `permission/src/read_block.rs`: `is_outside_reads_blocked` (`uL`) plus the
  byte-locked builders `ask_runtime_computed_path` (`Op`), `ask_unanalyzable`
  (`zU`), `ask_sed_off_allowlist` (`vtt`), `ask_uncheckable_named_path`,
  `ask_outside_path` (the three `MovesLaterReads` / `NamesAPath` /
  `NamesResolved` shapes), and `deny_rule_message`. `Op`/`zU` put the SAME
  string in `message` and `decisionReason.reason`; the tests assert that.

`permission` **1433 passed, 0 failed**; `tool-worktree` 69/0; `cli`,
`orchestrator`, `engine-desktop`, `engine-mobile`, `tool-skill`, `tool-workflow`
all build with tests.

### Step 6, first wiring landed 2026-09-08 — the `zU` escalation

`Pmo`'s unanalysable-command escalation is now live on both AST verdicts:

```js
if (o.blockReadsOutsideWorkingDirectories === !0 && !(jS(e) && Nz())) return zU(p.reason);  // TooComplex
… same on the checkSemantics Deny path …
```

`PermissionPolicy::read_block_unanalyzable_ask` sits in front of
`ask_bash_safety` at both sites, so under the read block a command the parser
cannot analyse becomes the `outsideReadsBlocked` safetyCheck instead of the
ordinary `Other` ask. `shell_bash_safety_ask` became a method to reach the
policy.

The `jS(e) && Nz()` sandbox escape is applied on
`SandboxAutoAllowConfig::would_sandbox` alone — `Nz()` (`Wmt() && tVe()`) has no
port-side equivalent. With no sandbox runtime wired (the default) nothing is
exempt, i.e. the STRICTER direction.

`permission --features bash-ast`: **1596 passed, 0 failed**; default features
**1433 passed, 0 failed** across all binaries. Disabling the escalation turns
exactly `read_block_escalates_an_unanalyzable_command` red; restored
byte-identically.

The escalation test asserts the BASELINE too — the same command without the
block must produce the ordinary `Other` ask — so it cannot pass by the command
simply failing to parse.

### `cd` wired 2026-09-08

`check_path_constraints` gained a `read_block_dirs: Option<&[PathBuf]>`
parameter (the `mEt` set when the block is armed) and `PathConstraintAsk` an
`outside_reads_blocked` flag; `ask_path_constraint` renders a flagged ask as the
`outsideReadsBlocked` safetyCheck instead of `type:"other"`, which is `ppo`
passing `PE`'s own `decisionReason` through.

Two things this changes for `cd` under the block:

1. the target is validated against **`mEt`**, so a `projectSettings`-sourced
   additional directory no longer makes `cd` there acceptable (an `/add-dir`
   one still does);
2. the refusal carries the read block's byte-locked copy (`cd moves later reads
   to a directory outside the working directories, …`) and the
   `outsideReadsBlocked` safetyCheck, instead of LingXi's generic containment
   ask.

`mEt ⊆ rb`, so once armed this branch subsumes the generic containment branch
for `cd` — matching the binary, where `PE(target,…,"read")` reports `ov` before
the generic path is reached.

Both tests assert the BASELINE (same command without the block ⇒ the generic
`Other` containment ask), so neither can pass on a `cd` that simply fails to
resolve. `permission --features bash-ast`: **1598 passed, 0 failed**; default
features 1435/0. Disabling the branch turns exactly the two `read_block_cd_*`
tests red; restored byte-identically.

### `pushd` / `env -C` closed 2026-09-08

`parse_dir_change` ports `ppo`'s argv extraction: the verb is the BASENAME of
argv[0] (so `/usr/bin/env` counts), `pushd` takes the first positional, and `env`
scans for `-C`, `--chdir`, `--chdir=X`, `-CX`. All six spellings are covered by
a test.

🚨 **Deliberately wired into the READ-BLOCK branch only.** With the block off,
`PE(target,…,"read")` ALLOWS a plain outside path for these verbs — it refuses
only on a deny rule, `--restricted`, or the block — so routing them into the
port's generic `cd` containment ask would ask where the binary allows. A second
test (`pushd_and_env_chdir_are_untouched_without_the_read_block`) pins that.

A run-time-computed target takes the `Op` copy (`… names a path that is computed
at run time …`), matching `if (hi(p)) return Op(d)`.

#### One assertion I had to weaken, and why

`read_block_covers_pushd_and_env_chdir` first asserted that an INSIDE-the-working
-dirs `env -C /proj/sub ls` produces no read-block reason at all. That passed on
default features and failed under `--features bash-ast`, because the read block
ALSO escalates any command the parser cannot analyse — `env` with options is
exactly the oracle's `Eun` → `zU("an environment variable prefix outside the safe
list cannot be checked against the read block")` case. The assertion was
asserting something the binary does not promise. It now checks that the
OUTSIDE-PATH copy specifically is absent, which is the property the `ppo` branch
actually owns.

### `ln`/`link`, `cp`/`mv` and the generic path walker wired 2026-09-08

`check_command_path_containment` took the same `read_block_dirs` parameter, and
the read block now owns the refusal at its containment point, with the oracle's
TWO message shapes:

- `ln`/`link` (`mpo`) and `cp`/`mv` → `${verb} names a path outside the working
  directories, …`
- everything else (`gmo`) → `${verb} names '${resolved}', outside the working
  directories, …`

selected by `read_block::outside_path_shape_for`.

🚨 **`ln`/`link` needed their own handler.** They are absent from the oracle's
`qU` action-verb table *and* from the port's `command_spec`, which is exactly why
the binary gives them a separate function (`mpo`). The port's walker skipped them
entirely, so the first version of this wiring silently did nothing for `ln` — the
test caught it. `mpo`'s positional rule is now ported: when any flag was present,
or there is exactly one positional, EVERY positional is read-checked; otherwise
the LAST one (the link name, created rather than read) is dropped.

Like `ppo`, both are **read-block only** — with the block off `PE(…,"read")`
allows a plain outside path for these verbs, so running them generally would ask
where the binary allows. `read_block_path_walker_baseline_and_met` pins the
baseline (generic `was blocked. For security…` copy, `type:"other"`) and the
`mEt` filter.

Disabling either branch turns exactly the two `read_block_path_walker_*` tests
red; restored byte-identically.

### git global path flags wired 2026-09-08

`ymo`'s git branch:

```js
if (C === "git") {
  for (let N=1; N<argv.length; N++) { let F=argv[N];
    if ((F==="-C"||F==="--git-dir"||F==="--work-tree"||F==="--file"||F==="-f") && argv[N+1]!==undefined) { D.push(argv[N+1]); N++ }
    else if (/^--(git-dir|work-tree|file)=/.test(F)) D.push(F.slice(F.indexOf("=")+1));
    else if (F.startsWith("-C") && F.length>2) D.push(F.slice(2)); }
  return D.length===0 ? undefined : gmo(C, D, cwd, ctx);
}
```

All eight spellings are covered by a test (`-C dir`, `-Cdir`, `--git-dir dir`,
`--git-dir=dir`, `--work-tree` ×2, `--file`, `-f`). The refusal takes `gmo`'s
`NamesResolved` shape.

🚨 Read-block only, and here the reason is sharper than for `ppo`/`mpo`: these
flags appear ONLY in `ymo`, never in the `qU` table extractor — which is exactly
why the port's `extract_git` covers just `git diff --no-index`. Wiring them into
the generic walker would ask on `git -C /etc log` in a plain session, where the
binary is silent. `git_path_flags_are_untouched_without_the_read_block` pins it.

Disabling the branch turns exactly `read_block_covers_git_path_flags` red (the
baseline test stays green); restored byte-identically.

### Known deliberate divergence in all three walkers

`gmo` skips a path that does not exist (`if (!existsSync(_)) continue`). The port
does not apply that gate: the existing containment walker has never consulted the
filesystem, and adding it would make a permission decision depend on FS state.
The effect is that the port checks strictly MORE paths than the binary — the safe
direction — but it is a real difference and is recorded here rather than in a
comment only.

### Interpreter / stdin guards wired 2026-09-08 — P0's shell side closed

`ymo`'s interpreter guards and `Pmo`'s stdin guard, in the binary's order:

| oracle | reason text |
|---|---|
| `args.includes("-")` | `${verb} runs code from stdin, which cannot be checked against the read block` |
| `C === "xargs"` | `Op(xargs)` — argv is built at run time |
| an `o2e` inline-code flag | `${verb} runs inline code, which cannot be checked against the read block` |
| `mmo(argv)` + heredoc / pipe-into | `code on stdin cannot be checked against the read block` |

All four go through `zU` (except `xargs`, which is `Op`). `o2e` is ported whole
as `INLINE_CODE_FLAGS` — 19 interpreters — and the lookup strips a trailing
version suffix, so `python3.11 -c` is caught (tested).

`mmo`'s own gate is ported too: the basename must be an `o2e` interpreter AND
either a bare `-` appears or EVERY argument is a flag — so a plain
`python script.py` is not swept up.

Read-block only again, with `interpreter_guards_are_untouched_without_the_read_block`
pinning it: `zU` exists only under the block. Disabling the three branches turns
exactly the three `read_block_escalates_*` tests red, and the baseline test stays
green; restored byte-identically.

### P0 status

| surface | |
|---|---|
| file tools (Read/Grep/Glob/LSP) + `mEt` + fence carve-outs + allow-rule does not escape | ✅ |
| setting + OR fold + all three composition roots | ✅ |
| `cd` | ✅ |
| `pushd` / `env -C\|--chdir` (4 spellings) | ✅ |
| `ln` / `link` (`mpo` positional rule) | ✅ |
| `cp` / `mv` + the generic walker (both message shapes) | ✅ |
| git `-C`/`--git-dir`/`--work-tree`/`--file`/`-f` (8 spellings) | ✅ |
| unanalysable command (`zU`) + sandbox exemption | ✅ |
| interpreters: stdin `-`, inline-code flags, `xargs`, heredoc/pipe | ✅ |
| glob-containing paths → `ymo`'s `_tt` upward-escape guard | ✅ |
| `env FOO=bar` outside the safe list (`eO`) | ❌ — the safe list was not located in the binary |

### Glob upward-escape guard wired 2026-09-08

The base-directory reduction itself was ALREADY in the port — `validate_path`
reduces a READ glob to `glob_base_directory` before containment, which is what
`ymo`'s `ve = V.slice(0, firstMetachar)` does. What was missing is the guard that
decides whether reducing is legitimate at all:

```js
if (V.split(/[\/]+/).some(_tt)) return Op(C);   // a segment whose glob could match ".."
```

`glob_segment_can_match_dotdot` ports `_tt` including both of its non-obvious
branches: `/\[[:=.]/` (POSIX class / equivalence / collating) short-circuits to
true, and a segment STARTING with `*` or `?` short-circuits to false — a leading
`*` would match `..`, but the binary deliberately does not treat it as an escape.
An unparsable pattern is treated as matching (the binary's `catch { return true }`
— fail closed).

Without it, `cat /proj/.*/secret` reduced to the base `/proj`, which is inside
the working directories, and passed — while the shell expands `.*` to `..` and
reads upward. Now it takes the `Op` refusal. A harmless `/proj/sub/*.rs` keeps
the ordinary reduction (tested both ways).

`permission`: **1449 passed, 0 failed** (default) and green under
`--features bash-ast`. 29 tests were added for this feature, every one of them
carrying a BASELINE assertion (the same command with the block off, or the
harmless sibling input), so none can pass by the command merely failing to
parse. (The env-var-prefix guard is reached indirectly through the
`zU` escalation, but not as its own byte-exact `Eun` message.)

### Verification note — a red suite that is NOT this work

The final full run showed 77 failures. Every one of them is the single
`debug_assert_eq!(m.len(), 83, "tool defaults table must list all 83 tools")` in
`defaults_per_tool.rs` (the map holds 81), a file this work never touched and
whose mtime is minutes old — a concurrent session is mid-edit on the tool
registry (`tool-api/src/lib.rs`, `todo_tools_gate.rs`, a new
`send_message_contract.rs`). Running this work's own 20 tests in isolation:
**20 passed, 0 failed**. Do not attribute that red suite here. Their builders exist and are byte-locked
(`ask_outside_path`, `ask_runtime_computed_path`, `ask_uncheckable_named_path`,
`deny_rule_message`) but nothing calls them yet; those sites live in
`command_path_containment.rs` and need the same `read_block_dirs` thread-through
`check_path_constraints` just got.

#### Adding a field to a widely-matched enum: what went wrong

A blanket regex over `classifier_approvable: <expr>,` produced 18 insertions and
**four were wrong**, in three distinct shapes the regex could not see:

1. a `pub classifier_approvable: Option<bool>` FIELD DECLARATION on an unrelated
   struct (`PermissionCheckContext`, twice);
2. `AutoEditSafety::Unsafe`'s own `classifier_approvable: bool` — a different
   type that merely shares the field name (4 insertions);
3. `matches!` PATTERNS that already ended in `..`, where the added
   `circuit_breaker: None` silently turned a shape assertion into a value
   assertion — this one COMPILED and was caught only by
   `dangerous_rm_asks_even_with_matching_allow_rule` going red.

(3) is the dangerous class: it type-checks. If that test had not existed the
regex would have quietly narrowed an assertion. Audit any such mass edit by
checking the line AFTER each insertion for a `..`, not just by compiling.

## Implementation order

1. ~~Add source attribution to additional working dirs~~ — done properly
   (`working_dirs.rs`, sources threaded through both composition roots and the
   `policy_gate` reducer).
2. Add `circuit_breaker` to `SafetyCheck`.
3. `permissions.blockReadsOutsideWorkingDirectories` in `loader.rs`
   `PermissionsBlock` + OR-across-sources accessor + managed-policy restrictive entry.
4. Port `Bh` / `mEt` / `sc` into `filesystem.rs`; wire into the Reader path with
   BOTH triggers (`ic` for `--restricted`, `Ep` for the read block).
5. `ZCe` + the `outsideReadsBlocked` safetyCheck for the Bash-side checks.
6. Bash-side call sites (`cd`/`pushd`/`env --chdir`, `ln`/`link`, `cp`/`mv`,
   git `-C`/`--git-dir`, glob paths, heredoc/stdin code, env-var prefixes,
   too-complex AST fallback) — each has its own byte-exact ask message.
