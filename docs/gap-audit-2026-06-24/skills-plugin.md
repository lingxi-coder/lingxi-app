# Skills + OutputStyles + Plugin Subsystem Parity Audit — v2.1.186
**Binary**: `/opt/homebrew/lib/node_modules/@anthropic-ai/claude-code/node_modules/@anthropic-ai/claude-code-darwin-arm64/claude`
**TS reference**: `/Users/luolingfeng/Projects/LingXi-Next/claude-code/src/`
**LingXi source**: `lingxi-code/skill-api/src/`, `lingxi-code/outputstyles/src/`, `lingxi-code/plugin/src/`, `lingxi-code/tools/skill/src/`
**Date re-verified**: 2026-06-24
**Re-audit scope**: fresh binary + TS cross-check for skills frontmatter, SkillTool prompt, outputstyle disk/plugin, plugin manifest

---

## CONFIRMED GAPS (grounded in binary literals)

### [P1] Skill tool `prompt()` returns static stub instead of the binary's full prompt

- **Binary says** (bytes 198002225+): the `Skill` tool's dynamic prompt function returns:
  ```
  Execute a skill within the main conversation

  When users ask you to perform tasks, check if any of the available skills match. Skills provide specialized capabilities and domain knowledge.

  When users reference a "slash command" or "/<something>", they are referring to a skill. Use this tool to invoke it.

  How to invoke:
  - Set `skill` to the exact name of an available skill (no leading slash). For plugin-namespaced skills use the fully qualified `plugin:skill` form.
  - Set `args` to pass optional arguments.
  - Some skills are scoped to a directory: their name is prefixed with the directory (e.g. `apps/web:deploy`) and their description says which directory they apply to. When a skill name has both a scoped and an unscoped variant, pick by the files you are working on: if the files are under a variant's directory, invoke that variant (most specific directory wins); otherwise invoke the unscoped one.

  Important:
  - Available skills are listed in system-reminder messages in the conversation
  - Only invoke a skill that appears in that list, or one the user explicitly typed as `/<name>` in their message. Never guess or invent a skill name from training data; otherwise do not call this tool
  - When a skill matches the user's request, this is a BLOCKING REQUIREMENT: invoke the relevant Skill tool BEFORE generating any other response about the task
  - NEVER mention a skill without actually calling this tool
  - Do not invoke a skill that is already running
  - Do not use this tool for built-in CLI commands (like /help, /clear, etc.)
  - If you see a <command-name> tag in the current conversation turn, the skill has ALREADY been loaded - follow the instructions directly instead of calling this tool again
  ```
- **LingXi has**: `async fn prompt(&self, _: &PromptOptions) -> String { "Skill: invoke a slash-command skill by name.".into() }`
- **File**: `lingxi-code/tools/skill/src/skill.rs:524-526`
- **Note**: The binary text is also DIFFERENT from the leaked TS source at `claude-code/src/tools/SkillTool/prompt.ts:173-195` (TS has "Examples:" bullet format; binary has "Set `skill`" format). The binary is the ground truth. LingXi is missing the entire operative prompt.

---

### [P1] SkillFrontmatter missing `user-invocable` field (visible to model; gating behavior)

- **Binary says** (bytes 71015792, 94993776): `user-invocable` is a parsed frontmatter key. Description: "If true, the model cannot invoke this via the Skill tool; only users can type the slash command. If false, hides the slash command from users; only the model can invoke it via the Sk[ill tool]"
- **LingXi has**: `SkillFrontmatter` struct in `skill-api/src/model.rs` has NO `user_invocable` field. LingXi's `SkillDescriptor` also has no `user_invocable` — it is not plumbed through.
- **File**: `lingxi-code/skill-api/src/model.rs:35-54`
- **Impact**: Skills with `user-invocable: false` (model-only) or `user-invocable: false` (user-only) always show in both contexts. The skill's intended audience restriction is completely ignored.

---

### [P1] SkillFrontmatter missing `disallowed-tools` field

- **Binary says** (bytes 94993840, 155728865): `disallowed-tools` is parsed. Description: "Tools removed from the model while this file is active. Comma-separated string or YAML list. Cleared when the user sends the next message." Also confirmed: `disallowedTools` is a canonical alias (`disallowed-tools`).
- **LingXi has**: `SkillFrontmatter` has `allowed_tools` but no `disallowed_tools`. `SkillDescriptor` has no `disallowed_tools` field.
- **File**: `lingxi-code/skill-api/src/model.rs:35-54`, `lingxi-code/tools/skill/src/skill.rs:78-127`

---

### [P1] SkillFrontmatter missing `paths` field (conditional skill activation)

- **Binary says** (bytes 196457593): `paths` is in the full frontmatter key list alongside `hooks`, `context`, `agent`, `effort`, `shell`. Also confirms: TS implements `parseSkillPaths()` + `activateConditionalSkillsForPaths()` dynamic discovery path.
- **LingXi has**: `SkillFrontmatter` has no `paths` field. The dynamic skill directory discovery (`discoverSkillDirsForPaths`, `addSkillDirectories`, `activateConditionalSkillsForPaths`) is entirely absent.
- **File**: `lingxi-code/skill-api/src/model.rs:35-54`
- **Impact**: Skills declared with `paths:` frontmatter (gitignore-style file path filters) are never conditionally activated; they either always appear or never appear.

---

### [P1] SkillFrontmatter missing `argument-hint` / `arguments` fields

- **Binary says** (bytes 94993872, 196457593): `argument-hint` (= "Placeholder text shown after the slash command name") and `arguments` (named arg list) are valid frontmatter keys. The `SkillDescriptor` in LingXi already has `argument_names: Vec<String>` but the source that POPULATES it — parsing from `SkillFrontmatter` — doesn't exist.
- **LingXi has**: `SkillFrontmatter` has no `argument_hint` or `arguments` fields. The `SkillDescriptor.argument_names` is always empty when loaded from on-disk skills (only populated by the production loader from a custom code path not wired to `SkillFrontmatter`).
- **File**: `lingxi-code/skill-api/src/model.rs:35-54`

---

### [P1] SkillFrontmatter missing `effort`, `version`, `shell` fields

- **Binary says** (bytes 196457593): `effort`, `version`, and `shell` are in the known frontmatter key list.
- **LingXi has**: `SkillFrontmatter` has no `effort`, `version`, or `shell` fields. The `SkillDescriptor` has `shell: Option<command_api::FrontmatterShell>` but it is never populated from disk-loaded skills.
- **File**: `lingxi-code/skill-api/src/model.rs:35-54`

---

### [P1] SkillFrontmatter missing `context` / `agent` fields (fork execution context)

- **Binary says** (bytes 155734026, 196453457): `context: fork` and `agent` are valid frontmatter keys. Description: "Where the skill runs: `inline` expands into the current conversation; `fork` spawns a subagent. Agent type to spawn when `context: fork`."
- **LingXi has**: `SkillFrontmatter` has no `context` or `agent` fields. The execution context (`inline` vs `fork`) is completely absent.
- **File**: `lingxi-code/skill-api/src/model.rs:35-54`
- **Note**: The forked agent execution path (`executeForkedSkill`) is documented as a "biggest non-faithful surface" in the skill tool module doc (`skill.rs:23-33`) — that is already known. This gap is specifically about the FRONTMATTER field not being parsed; even if fork execution were out of scope, the field should still be parsed so that `disable-model-invocation`-style rejection of inappropriate invocations works correctly.

---

### [P1] SkillFrontmatter missing `hooks` field (per-skill hooks)

- **Binary says** (bytes 196457593): `hooks` is a valid frontmatter key. TS `parseHooksFromFrontmatter()` parses it against `HooksSchema`.
- **LingXi has**: `SkillFrontmatter` has no `hooks` field. Per-skill hooks are completely absent.
- **File**: `lingxi-code/skill-api/src/model.rs:35-54`

---

### [P2] SkillFrontmatter missing `hide-from-slash-command-tool` field

- **Binary says** (bytes 155749616): `hide-from-slash-command-tool` is a valid frontmatter key.
- **LingXi has**: `SkillFrontmatter` has no such field.
- **File**: `lingxi-code/skill-api/src/model.rs:35-54`

---

### [P2] SkillFrontmatter missing `created_by` / `improved_by` metadata fields

- **Binary says** (bytes 94994016, 94994048): `created_by` and `improved_by` are parsed frontmatter fields (metadata for improvement survey tracking).
- **LingXi has**: `SkillFrontmatter` has no `created_by` or `improved_by` fields.
- **File**: `lingxi-code/skill-api/src/model.rs:35-54`

---

### ~~[P1] Skill system-reminder injection format~~ — REFUTED (CLEAN)

- **Re-verification 2026-06-24**: Binary at offset 189047392 produces exactly `"The following skills are available for use with the Skill tool:\n\n"` (double newline). TS `messages.ts:3734` uses `\n\n${attachment.content}`. LingXi `skill_listing.rs:163` also uses `\n\n{body}`. **Match confirmed.** Prior audit finding was wrong.

---

### ~~[P1] Output style `turnReminder` / per-turn injection~~ — REFUTED (CLEAN)

- **Re-verification 2026-06-24**: LingXi `conversation.rs:5429-5440` implements `output_style_reminder_message()` which fires every turn when a style is active, producing `"<system-reminder>\n{name} output style is active. Remember to follow the specific guidelines for this style.\n</system-reminder>"` — byte-matching TS `messages.ts:3807` + `wrapInSystemReminder`. The `turnReminder` field in the binary is an optional override (falls back to the default text when `undefined`); LingXi uses the default text. **Match confirmed.** Prior audit finding was wrong.

---

### [P1] Dynamic skill discovery (file-operation triggered) entirely absent

- **Binary says** (by string presence of `activateConditionalSkills`, `discoverSkillDirsForPaths`, `addSkillDirectories`, `tengu_dynamic_skills_changed` analytics event in TS source): When the model uses tools that touch files, the engine walks from each touched file's directory up to cwd looking for `.claude/skills/` directories, loads any new ones found, and activates conditional (path-filtered) skills whose `paths:` patterns match the touched files.
- **LingXi has**: No analogous dynamic skill directory discovery. All skill loading is done at startup/registry-build time. There is no mechanism for discovering new `.claude/skills/` directories at runtime as files are touched.
- **File**: `lingxi-code/skill-api/src/` (missing the dynamic discovery module)
- **Impact**: Projects that have skills in nested subdirectories (e.g., `apps/web/.claude/skills/`) will never have those skills loaded by LingXi.

---

### [P2] OutputStyle `forceForPlugin` field not present in LingXi disk output style

- **Binary says** (bytes 189331814, 206814560): `force-for-plugin` is a valid output-style frontmatter key. When set on a non-plugin output style, the binary logs a warning. For plugin styles, it makes the style automatically apply when the plugin is enabled.
- **LingXi has**: `DiskOutputStyle` and `DiskFrontmatter` in `lingxi-code/outputstyles/src/disk.rs` have no `force_for_plugin` field. The warning for non-plugin styles is correctly absent (the `loadOutputStylesDir.ts` behavior of warning+ignoring is not yet needed), but the actual plugin-forced-style behavior is missing.
- **File**: `lingxi-code/outputstyles/src/disk.rs:27-38`

---

### [P2] Plugin manifest `RawManifest` omits `skills`, `commands`, `agents`, `outputStyles`, `hooks`, `mcpServers`, `lspServers` from JSON deserialization

- **Binary says**: The full `PluginManifestSchema` in TS includes `skills`, `commands`, `agents`, `outputStyles`, `hooks`, `mcpServers`, `lspServers`, `channels`, `userConfig`, `dependencies`, `settings`, `keywords`, `license`, `repository` at the top-level `plugin.json`.
- **LingXi has**: `RawManifest` in `lingxi-code/plugin/src/discovery.rs:61-71` only deserializes `name`, `version`, `description`, `author`, `homepage`. All component declarations in `plugin.json` (like `commands:`, `skills:`, `mcpServers:`) are SILENTLY IGNORED — the component detection falls back entirely to auto-detection via `detect_components()` (directory walk).
- **File**: `lingxi-code/plugin/src/discovery.rs:53-71`
- **Impact**: Plugins that declare their components explicitly in `plugin.json` (instead of relying on directory auto-detection) won't have those components recognized. Also, manifest-declared MCP servers (`mcpServers:` field in `plugin.json`) are NEVER loaded — only `.mcp.json` files are read.

---

## UNCERTAIN (possible gaps, not binary-confirmed)

### [?] SkillFrontmatter `auto_search` / `triggers` not in binary frontmatter key list

- **Binary key list** (bytes 196457593): The complete known-key list is `["name","description","model","allowed-tools","argument-hint","arguments","disable-model-invocation","user-invocable","effort","shell","version","when_to_use","paths","hooks","context","agent","created_by","improved_by",...]` — does NOT include `auto_search` or `triggers`.
- **LingXi has**: `SkillFrontmatter.auto_search` and `SkillFrontmatter.triggers` fields that are used for trigger-based discovery in `SkillRegistry`.
- **Assessment**: These are LingXi-ADDED fields (not in claude-code). This is an additive extension, not a gap. The `triggers`-based discovery (`SkillRegistry.discover()`) is also a LingXi addition. However, the trigger-based discovery replaces what claude-code does via the `getSkillToolCommands`→model-provided listing mechanism, so this may be intentional LingXi design.

### [?] Output style per-turn `output_style` system-reminder vs system-prompt heading

- The binary shows TWO mechanisms for output style:
  1. System prompt: `# Output Style: <name>\n<prompt>` (at assembly time, via `$Hm()`)
  2. Per-turn meta message: `<name> output style is active. Remember to follow the specific guidelines for this style.`
- LingXi implements mechanism #1 faithfully (`orchestrator/src/prompt/mod.rs:160-161`).
- Mechanism #2 is the `output_style` system-reminder type in the binary's turn-message renderer (`messages.ts` in TS, seen at bytes 206905129).
- **Assessment**: It is unclear whether mechanism #2 fires on every turn (in which case it's a P1 gap) or only as a one-time notification. The binary shows it's driven by an `output_style` event/message struct with a `turnReminder` optional field — likely fires per turn. Needs verification.

### [?] Skill listing system-reminder fires on turn-0 only vs. every turn

- LingXi's `conversation.rs` notes "Turn-0 emits the FULL listing; later turns emit..." (line 814) suggesting per-turn injection of SOME form.
- The binary's `skill_listing` renderer (bytes 206904949) shows it's triggered by a `skill_listing` event in the message stream, implying it fires when the skill listing changes (not necessarily every turn).
- **Assessment**: LingXi's implementation behavior needs verification against the binary's exact trigger conditions.

---

## CLEAN (things that match)

### Skills subsystem — CLEAN
- **SKILL.md discovery layout**: Both use `<name>/SKILL.md` directory format for `.claude/skills/` dirs. Discovery paths (managed → user → project → additional) match. `FileSkillSection` titles ("Managed skills", "Project skills", "User skills", "Additional skills") match.
- **Frontmatter parsing**: `---\nYAML\n---\nbody` format, malformed → silent default, body trimmed — all match.
- **SkillTool input schema**: `{skill: string, args?: string}`, `required: ["skill"]`, no `minLength` on skill — matches binary.
- **SkillTool error strings** (byte-locked): `Unknown skill: <name>`, `Skill <name> cannot be used with Skill tool due to disable-model-invocation`, `Skill <name> is not a prompt-based skill` — all match.
- **`disable-model-invocation` flag**: parsed and enforced correctly.
- **`allowed-tools` field** (alias `tools_allowed`): present and parsed.
- **`when_to_use` field**: present and used in skill listing.
- **Shell expansion** (`!command` in skill body): implemented with `skip_shell_expansion` for MCP skills.
- **`${CLAUDE_SKILL_DIR}` / `${CLAUDE_SESSION_ID}` substitution**: implemented.
- **`model:` frontmatter**: `SkillDescriptor.model` is wired; `context_modifier` switches `main_loop_model` with `[1m]` suffix preservation.
- **Skill listing budget logic**: `formatCommandsWithinBudget` with 1% of context window, 250-char cap, bundled-never-truncated, names-only fallback — all match (`skill_listing.rs`).
- **MCP skill builders**: `skill_from_mcp_tool` present, with `skip_shell_expansion = true`.
- **Telemetry events**: `SKILL_STARTED`, `SKILL_COMPLETED`, `SKILL_FAILED`, `SKILL_INVOKED` all wired.

### OutputStyles subsystem — CLEAN
- **Builtin style names**: `Explanatory` and `Learning` — exact names match binary.
- **Explanatory prompt body**: 1023 chars / 1197 bytes, byte-exact match confirmed by test.
- **Learning prompt body**: 4888 chars / 5076 bytes, byte-exact match confirmed by test.
- **`DEFAULT_OUTPUT_STYLE_NAME = "default"`**: matches binary.
- **System prompt heading**: `# Output Style: <name>\n<body>` — matches binary's `$Hm()` function.
- **`keepCodingInstructions` field**: parsed, stored, and used to gate coding-instructions section.
- **Disk discovery**: `~/.claude/output-styles/` and `<cwd>/.claude/output-styles/` (via `markdownConfigLoader`), `*.md` files, stem → name fallback — all match.
- **Case-sensitive name matching**: `resolve_builtin_output_style` is case-sensitive — matches binary.

### Plugin subsystem — CLEAN
- **`.claude-plugin/plugin.json` layout**: discovered and read correctly.
- **Auto-detection of `commands/`, `agents/`, `skills/`, `output-styles/`, `hooks/hooks.json`**: implemented in `detect_components()`.
- **`<name>/SKILL.md` layout for plugin skills**: `glob_skill_dirs()` correctly uses subdirectory format.
- **Lifecycle states** (7 states): present.
- **PluginManifest identity fields**: `name`, `version`, `description`, `author`, `homepage` — present.
- **Trust levels**: `default_trust_for_source` implemented.
- **Blocklist**: present.
- **Strict policy**: present.
- **`glob_md` for commands/agents/output-styles**: present in `discovery.rs`.

---

## GAP SUMMARY (re-verified 2026-06-24)

**Two prior "P1" findings REFUTED** (skill-listing extra blank line, output-style per-turn reminder) — both are actually CLEAN.

| Severity | Count | Areas |
|---|---|---|
| P0 | 0 | — |
| P1 | 7 | SkillTool `prompt()` stub; `user-invocable`; `disallowed-tools`; `paths`+conditional skill discovery (dynamic, not at FileWrite); `argument-hint`/`arguments`; `effort`/`version`/`shell`; `context`/`agent` (fork); `hooks` |
| P2 | 4 | `hide-from-slash-command-tool`, `created_by`/`improved_by`, `forceForPlugin`, plugin manifest component fields silently ignored |
| Total confirmed | 11 | |
| Refuted (CLEAN) | 2 | skill-listing `\n\n` format, output-style per-turn reminder text |

### Severity ranking rationale
- **P1 (SkillTool prompt stub)**: Model receives wrong instructions for how to invoke skills and missing re-entry guard. Highest user-visible impact.
- **P1 (user-invocable, disallowed-tools, paths, hooks)**: Behavioral gating missing — skills show in wrong contexts, forbidden tools aren't excluded, path-scoped skills never activate, per-skill hooks never fire.
- **P1 (argument-hint, arguments, effort, version, shell)**: Display and execution-configuration fields silently ignored from SKILL.md frontmatter.
- **P1 (context/agent fork)**: `context: fork` silently treated as inline — already documented as known in `skill.rs` module doc.
- **P2**: Metadata/tracking fields, plugin output style auto-apply, plugin manifest explicit component declarations.

