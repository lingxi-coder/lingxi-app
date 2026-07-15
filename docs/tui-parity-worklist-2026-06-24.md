# TUI Parity — consolidated worklist (2026-06-24)

Total confirmed: 151 (+19 needs-binary-check). Two audit passes merged.

Status legend: ✅ DONE (Wave1, verified green) · ⬜ TODO · 🔶 JUDGMENT/large-rebuild


## agents-screen  (H=1 M=3 L=4)

- ⬜ **[HIGH]** `agents-03` — Detail empty-tools shows 'All tools' instead of 'None'
    - rust: `lingxi-code/tui/src/screens/agents.rs:138`  |  ts: `claude-code/src/components/agents/AgentDetail.tsx:71`
    - fix: Distinguish wildcard from empty: empty -> 'None', wildcard ('*') -> 'All tools'. AgentInfo wire (orchestrator.rs:176) lacks a wildcard flag, so a wildcard signal must be added to the wire.
- ⬜ **[MEDIUM]** `agents-02` — Empty-agents state text diverges (one line vs subtitle + three help lines)
    - rust: `lingxi-code/tui/src/screens/agents.rs:102`  |  ts: `claude-code/src/components/agents/AgentsList.tsx:27`
    - fix: When rows is empty, emit the dim subtitle 'No agents found' and the three help lines verbatim (the Create-specialized / own-context-window / Try-creating lines).
- ⬜ **[MEDIUM]** `agents-04` — Detail omits Model line when model is absent; TS always shows it
    - rust: `lingxi-code/tui/src/screens/agents.rs:144`  |  ts: `claude-code/src/utils/model/agent.ts:126`
    - fix: Always render Model: default 'Inherit from parent (default)' when unset, 'Inherit from parent' for 'inherit', else capitalize the model string.
- ⬜ **[MEDIUM]** `agents-05` — List navigation clamps at ends; TS wraps around
    - rust: `lingxi-code/tui/src/screens/agents.rs:66`  |  ts: `claude-code/src/components/agents/AgentsList.tsx:151`
    - fix: Wrap selection modulo row count for Up/Down (Up at top -> last row, Down at bottom -> first row).
- ⬜ **[LOW]** `agents-01` — List header has no count subtitle; not a styled Dialog title
    - rust: `lingxi-code/tui/src/screens/agents.rs:100`  |  ts: `claude-code/src/components/agents/AgentsList.tsx:81`
    - fix: Add a dim subtitle line `<N> agents` (row count) under the 'Agents' title. Title text itself already matches.
- ⬜ **[LOW]** `agents-06` — List accepts vim keys j/k/q that TS does not bind
    - rust: `lingxi-code/tui/src/screens/agents.rs:66`  |  ts: `claude-code/src/components/agents/AgentsList.tsx:135`
    - fix: Drop the j/k/q handlers in the list (arrow-only nav, Esc-only close) to match TS.
- ⬜ **[LOW]** `agents-07` — Detail appends 'esc to go back' footer; TS detail shows no footer line
    - rust: `lingxi-code/tui/src/screens/agents.rs:153`  |  ts: `claude-code/src/components/agents/AgentDetail.tsx:201`
    - fix: Remove the trailing 'esc to go back' line from the detail body.
- ⬜ **[LOW]** `agents-08` — List shows no source grouping or built-in section
    - rust: `lingxi-code/tui/src/root.rs:1463`  |  ts: `claude-code/src/components/agents/AgentsList.tsx:104`
    - fix: Extend the AgentInfo wire with source/baseDir/built-in, then group rows under bold-dim source headers plus a 'Built-in agents (always available)' section as TS does; otherwise it stays a data-model gap, not just a render gap.

## completion-palette  (H=2 M=5 L=1)

- ⬜ **[HIGH]** `cp-01` — Slash-palette shows placeholder "(unimplemented in v0.6.0)" for ~50 visible commands instead of real descriptions
    - rust: `lingxi-code/command-api/src/builtin_support/names.rs:504 (`_ => "(unimplemented in v0.6.0)"`) consumed at lingxi-code/tui/src/components/prompt_input/palette.rs:94 (`description: core_description(name)`)`  |  ts: `claude-code/src/utils/suggestions/commandSuggestions.ts:274-284 (description = formatDescriptionWithSource(cmd)); claude-code/src/commands/{diff,theme,keybindings,export,rename,resume,skills,review,...}/index.ts (each `description:` is a real one-liner)`
    - fix: Replace the core_description placeholder with the real per-command description for every visible (non-hidden, non-disabled) builtin, mirroring each claude-code command's `description` (and formatDescriptionWithSource for source-annotated ones). At minimum supply the one-line string for all visible commands so no palette row shows placeholder text.
- ⬜ **[HIGH]** `cp-05` — @-file completion only lists immediate cwd entries (non-recursive, dotfiles excluded, no directories), not the fuzzy-matched project file tree
    - rust: `lingxi-code/tui/src/components/prompt_input/completion.rs:183-193 (read_cwd_entries: single read_dir of cwd, filter !starts_with('.'), ASCII sort) + :88-93 (rows() flat subsequence filter); root.rs:1281-1284`  |  ts: `claude-code/src/hooks/fileSuggestions.ts:459-516 (git ls-files / ripgrep --hidden recursive), :692-708 (getTopLevelPaths trailing sep for dirs), :715-740 (generateFileSuggestions full-path match / top-level on empty)`
    - fix: Source @-completion from a recursive project file listing (git ls-files / ripgrep equivalent honoring gitignore), ranked by fuzzy score, include hidden files, append a trailing separator for directories, and match against the full relative path rather than a flat cwd basename list.
- ⬜ **[MEDIUM]** `cp-02` — Bare `/` palette ordering is by command-name length then ASCII, not alphabetical
    - rust: `lingxi-code/tui/src/components/prompt_input/fuzzy.rs:56-60 (sort: score desc, then a.1.len().cmp(b.1.len()), then ASCII) + empty-needle Some(0) at fuzzy.rs:19-21; called via palette.rs:85`  |  ts: `claude-code/src/utils/suggestions/commandSuggestions.ts:360-379 (sortAlphabetically = getCommandName(a).localeCompare(getCommandName(b)) applied per category, then concatenated)`
    - fix: For the empty-filter (bare `/`) case, sort visible builtins by case-insensitive name (localeCompare/ASCII-lowercase) instead of routing them through the length-weighted fuzzy ranker; reserve the length tiebreak for when a real needle is present.
- ⬜ **[MEDIUM]** `cp-03` — No command-alias matching or `(alias)` display in the slash palette
    - rust: `lingxi-code/tui/src/components/prompt_input/palette.rs:79-98 (rows() over BUILTIN_COMMAND_NAMES by name only; no alias data, no parenthetical)`  |  ts: `claude-code/src/utils/suggestions/commandSuggestions.ts:49 & 64-70 (Fuse aliasKey), :250-287 (findMatchedAlias + aliasText); claude-code/src/commands/{config,resume,permissions,rewind,desktop,exit,clear,mobile}/index.ts (`aliases:` arrays)`
    - fix: Add an alias table for the builtins, fold aliases into the fuzzy candidate set, and when a row matches via a typed alias append ` (alias)` to the displayed name, mirroring createCommandSuggestionItem.
- ⬜ **[MEDIUM]** `cp-04` — Palette/completion arrow navigation clamps at the ends instead of wrapping around
    - rust: `lingxi-code/tui/src/components/prompt_input/palette.rs:125-134 and lingxi-code/tui/src/components/prompt_input/completion.rs:120-129 (Down: selected+1<len; Up: saturating_sub(1) — clamp, no wrap)`  |  ts: `claude-code/src/hooks/useTypeahead.tsx:1242-1255 (previous wraps to length-1 at index 0; next wraps to 0 at last index)`
    - fix: In both PaletteState::handle_key and CompletionState::handle_key make Down at last index go to 0 and Up at index 0 go to len-1, matching the wrapping autocomplete navigation.
- ⬜ **[MEDIUM]** `cp-06` — @-completion renders "No matching files"/"Start typing to search…" empty-state rows that the inline overlay never shows
    - rust: `lingxi-code/tui/src/components/prompt_input/completion.rs:20-22 (EMPTY_WITH_QUERY/EMPTY_NO_QUERY) & :216-225 (renders the string when rows empty); rendered whenever open at lingxi-code/tui/src/screens/repl.rs:255-261`  |  ts: `claude-code/src/components/PromptInput/PromptInputFooterSuggestions.tsx:225-227 (`if (suggestions.length === 0) return null`); strings only in claude-code/src/components/QuickOpenDialog.tsx:227 (full-page finder)`
    - fix: When the inline @-overlay has no rows, render nothing (collapse the overlay) instead of an empty-state string; on a bare `@` show the top-level file/dir listing per cp-05 rather than 'Start typing to search…'.
- ⬜ **[MEDIUM]** `cp-07` — @-completion lacks Tab longest-common-prefix completion and directory drill-down
    - rust: `lingxi-code/tui/src/components/prompt_input/completion.rs:143-176 (Tab/Enter immediately insert `@{sel} ` with trailing space, close overlay)`  |  ts: `claude-code/src/hooks/useTypeahead.tsx:1041-1071 (Tab applies findLongestCommonPrefix, isComplete:false, re-runs updateSuggestions) and :1198-1217 (directory selection appends separator + re-queries)`
    - fix: Implement two-stage Tab: if all rows share a prefix longer than what's typed, insert that prefix without a trailing space and keep the overlay open; otherwise commit the selection. For directory entries, append a separator and re-query rather than closing.
- ⬜ **[LOW]** `cp-08` — Palette rows are a single concatenated `name – desc` string with no fixed name column, whitespace collapse, or width truncation
    - rust: `lingxi-code/tui/src/components/prompt_input/palette.rs:188 (`format!("/{} \u{2013} {}", row.name, row.description)` — one un-padded, un-truncated Text)`  |  ts: `claude-code/src/components/PromptInput/PromptInputFooterSuggestions.tsx:128-159,192 (name padded to min(maxColumnWidth, floor(columns*0.4)); description `.replace(/\s+/g,' ')` + truncateToWidth; whole row wrap="truncate")`
    - fix: Render the name in a fixed-width padded column (min(maxColumnWidth, floor(width*0.4))) and the description in a separate truncated span with `\s+`→' ' collapse, mirroring SuggestionItemRow's non-unified command layout.

## coordinator-team-tasks  (H=1 M=4 L=4)

- ⬜ **[HIGH]** `FOOTER-PILL-LABEL` — Footer pill always says "{n} background task[s]" instead of type-specific label (getPillLabel)
    - rust: `lingxi-code/tui/src/components/tasks/status_footer.rs:14-22 (filters non-teammate, then `format!("{n} background {noun}{VIEW_HINT}")` with noun=task/tasks only); consumed live at lingxi-code/tui/src/app.rs:1086`  |  ts: `claude-code/src/tasks/pillLabel.ts:10-67 (getPillLabel per-type labels) + claude-code/src/components/tasks/BackgroundTaskStatus.tsx:200,208 (compiled: `getPillLabel(runningTasks)` -> `<SummaryPill>`)`
    - fix: Port getPillLabel into render_task_footer: when all non-teammate tasks share task_type, emit the per-type label (shell/shells, local agent/local agents, monitor/monitors, background workflow/background workflows, dreaming); only fall back to "{n} background task[s]" for mixed types. For local_bash split shells vs monitors, which needs the shell `kind` threaded onto TaskRow (or treat all local_bash as shells until kind is available).
- ⬜ **[MEDIUM]** `FOOTER-CTA-ALWAYS-ON` — Footer appends " · ↓ to view" unconditionally; TS gates it behind pillNeedsCta (ultraplan-only)
    - rust: `lingxi-code/tui/src/components/tasks/status_footer.rs:8 (const VIEW_HINT = " · ↓ to view") + :22 (always appended)`  |  ts: `claude-code/src/components/tasks/BackgroundTaskStatus.tsx:218 (compiled: `pillNeedsCta(runningTasks) && <Text dimColor> · {figures.arrowDown} to view</Text>`) + claude-code/src/tasks/pillLabel.ts:74-82 (pillNeedsCta true only for single ultraplan remote_agent)`
    - fix: Drop the unconditional VIEW_HINT for ordinary tasks. Since the only condition that triggers the CTA (single ultraplan remote_agent) is an excluded cloud surface, the in-scope behavior is: emit the bare pill label with no "↓ to view" suffix. Keep the CTA only if/when an ultraplan/remote pill is implemented.
- ⬜ **[MEDIUM]** `BASH-ROW-USES-DESCRIPTION-NOT-COMMAND` — local_bash row/footer shows description; claude-code shows the shell command
    - rust: `lingxi-code/tui/src/multiagent/poller.rs:16-22 (task_row_from_record drops r.command) + lingxi-code/tui/src/multiagent/state.rs:11-20 (TaskRow has no command field) + lingxi-code/tui/src/components/tasks/mod.rs:26 (local_bash -> render_shell_progress_to_string(description,...))`  |  ts: `claude-code/src/components/tasks/BackgroundTask.tsx:27 (`task.kind === 'monitor' ? task.description : task.command`) + claude-code/src/components/tasks/BackgroundTasksDialog.tsx:498 (toListItem local_bash label = same)`
    - fix: Add `command: Option<String>` to TaskRow, copy r.command in task_row_from_record, and in render_task_row pass command.as_deref().unwrap_or(description) (description only for monitor kind) to render_shell_progress_to_string for local_bash.
- ⬜ **[MEDIUM]** `TASKS-DIALOG-FLAT-LIST-NO-SECTIONS` — /tasks dialog is a flat list missing the per-type section headers and running-count subtitle
    - rust: `lingxi-code/tui/src/screens/background_tasks.rs:104-118 (flat list, header "Background tasks", no subtitle, no group headers)`  |  ts: `claude-code/src/components/tasks/BackgroundTasksDialog.tsx:404-413 (running-count subtitle) + :437-481 (bold-dim section headers Shells/Monitors/Remote agents/Local agents/Workflows with marginTop blank line, each gated on other groups)`
    - fix: Group rows by type into sections with bold-dim "  <Section> (N)" headers and a blank line between groups (gate each header on another group being non-empty, matching the TS conditionals), and add the running-count subtitle ("N active shell[s] · N active agent[s]"). Exclude the teammate "Agents" group (team-frozen).
- ⬜ **[MEDIUM]** `TASKS-DIALOG-KEYHINTS` — /tasks key-hint footer differs in glyphs, labels, and missing the conditional 'x stop' hint
    - rust: `lingxi-code/tui/src/screens/background_tasks.rs:116 ("↑↓ move · enter open · esc close") + :126 ("← back · esc close")`  |  ts: `claude-code/src/components/tasks/BackgroundTasksDialog.tsx:414 (actions: "↑/↓ select", "Enter view", conditional "x stop" when selection running+killable, "←/Esc close")`
    - fix: Match the hint text/order: "↑/↓ select · Enter view · [x stop when selection is a running killable task] · ←/Esc close"; wire an x=stop key in handle_background_tasks_key for running tasks. Skip the team-only "stop all agents" / teammate "f foreground" hints.
- ⬜ **[LOW]** `BASH-ROW-NO-TRUNCATION` — Task rows are not truncated to maxActivityWidth; claude-code truncates every label
    - rust: `lingxi-code/tui/src/components/tasks/rows.rs:22-107 + shell_progress.rs:10-21 (no truncation) + lingxi-code/tui/src/screens/background_tasks.rs:110-114 (row pushed verbatim)`  |  ts: `claude-code/src/components/tasks/BackgroundTask.tsx:23,30,81,119,179,224,266 (truncate(label, activityLimit, true)) + BackgroundTasksDialog.tsx:561 (maxActivityWidth = Math.max(30, columns - 26))`
    - fix: Apply single-line trailing-ellipsis truncation to each row label before rendering: width 40 for the footer pill, max(30, columns-26) for the dialog Item, mirroring truncate(text, limit, true).
- ⬜ **[LOW]** `TASKS-DIALOG-EMPTY-TEXT` — Empty /tasks list says "(no background tasks)" vs claude-code "No tasks currently running"
    - rust: `lingxi-code/tui/src/screens/background_tasks.rs:107 (`out.push_str("(no background tasks)")`)`  |  ts: `claude-code/src/components/tasks/BackgroundTasksDialog.tsx:426 (`<Text dimColor>No tasks currently running</Text>`)`
    - fix: Change the empty-state text to "No tasks currently running" (dimmed).
- ⬜ **[LOW]** `TASKS-DIALOG-SELECTION-MARKER` — List selection marker is "> " instead of figures.pointer (❯ )
    - rust: `lingxi-code/tui/src/screens/background_tasks.rs:111 (`let marker = if i == state.selected { "> " } else { "  " }`)`  |  ts: `claude-code/src/components/tasks/BackgroundTasksDialog.tsx:571 (`isSelected ? figures.pointer + " " : "  "`; figures.pointer = ❯ U+276F)`
    - fix: Use "\u{276F} " (❯ + space) for the selected-row marker to match figures.pointer; keep two spaces for unselected.
- ⬜ **[LOW]** `TASKS-DIALOG-SORT-ORDER` — List does not sort running-first then by start time; preserves wire order
    - rust: `lingxi-code/tui/src/screens/background_tasks.rs:110 (iterates tasks in feed order) + lingxi-code/tui/src/multiagent/state.rs:39 (tasks kept in feed order)`  |  ts: `claude-code/src/components/tasks/BackgroundTasksDialog.tsx:184-192 (sort: running-first, then startTime descending)`
    - fix: Sort rows before rendering: status==running/pending first, then by start time descending. startTime is not on TaskRow today, so add it (from TaskRecord) or at minimum apply the running-first partition to stop completed tasks floating above running ones.

## diff-markdown-syntax  (H=0 M=8 L=3)

- ⬜ **[MEDIUM]** `diff-01` — Removed (-) lines are syntax-highlighted in Rust but plaintext in claude-code
    - rust: `lingxi-code/tui/src/render/diff.rs:145 (content_spans always syntax::highlight); 236 (plain_row highlights remove rows)`  |  ts: `claude-code/src/native-ts/color-diff/index.ts:915-918 (marker==='-' -> [[defaultStyle(theme),code]], else highlightLine); default path confirmed via components/StructuredDiff.tsx:50-65,114 and components/StructuredDiff/colorDiff.ts:18-27`
    - fix: In plain_row/word fallback, when row.kind==LineKind::Remove render content with default fg (skip syntax::highlight), mirroring the TS marker==='-' branch. Note: TS still word-diffs removed lines (applyBackground), it just skips syntax color.
- ⬜ **[MEDIUM]** `diff-02` — Diff line number + marker gutter is dim-grey in Rust, green/red decoration in claude-code
    - rust: `lingxi-code/tui/src/render/diff.rs:125-135 (gutter_span: fg always StyleColor::Named(BrightBlack) for number+sigil, all kinds)`  |  ts: `claude-code/src/native-ts/color-diff/index.ts:747-756 (addMarker decorationColor), 726-745 (addLineNumber: changed-line number = decorationColor, only context/null dimmed), 390-399/306/323 (decorationColor add green/remove red)`
    - fix: In gutter_span, color the sigil and the line-number with green (Add) / red (Remove) decoration colors matching index.ts buildTheme; keep BrightBlack/dim only for Context lines.
- ⬜ **[MEDIUM]** `diff-03` — Changed-line background does not extend to the right edge in Rust
    - rust: `lingxi-code/tui/src/render/diff.rs:229-238,241-255 (no right-edge pad span); 333 (render() has no width param to pad to)`  |  ts: `claude-code/src/native-ts/color-diff/index.ts:713-723 (wrapText pads changed lines with lineBackground to full width)`
    - fix: Thread render width into diff::render and append a pad span (spaces) carrying the line bg on changed (Add/Remove) rows, as TS wrapText does. (Context rows have terminal-default bg, so no pad needed there.)
- ⬜ **[MEDIUM]** `diff-05` — Rust emits unified-diff '@@ -a,b +c,d @@' hunk headers that claude-code never renders
    - rust: `lingxi-code/tui/src/render/diff.rs:267-277,356 (hunk_header pushed before each hunk; no '...' separator anywhere)`  |  ts: `claude-code/src/native-ts/color-diff/index.ts:860-932 (no @@ emitted); claude-code/src/components/StructuredDiffList.tsx:24-28 (hunks separated by dim '...' via intersperse)`
    - fix: Drop the @@ hunk_header lines; instead render hunks like StructuredDiffList — render each hunk's rows, separated by a single dim '...' line between hunks.
- ⬜ **[MEDIUM]** `diff-07` — Edit/Write diff and theme preview are not wrapped in the dashed top/bottom border frame claude-code draws
    - rust: `lingxi-code/tui/src/components/messages/user_tool_result.rs:227-272 (diff rows in plain View(Column), no dashed border); lingxi-code/tui/src/screens/theme.rs:215-234 (preview rows, no dashed border)`  |  ts: `claude-code/src/components/FileEditToolDiff.tsx:81-104 (DiffFrame: Box borderStyle=dashed borderColor=subtle borderLeft/Right=false); claude-code/src/components/ThemePicker.tsx (~246, Box borderTop/borderBottom dashed subtle around StructuredDiff)`
    - fix: Wrap the Edit/Write diff rows (user_tool_result) and the theme preview rows in a top+bottom dashed 'subtle' border (no side borders). Confirm the iocraft dashed border glyph matches Ink's dashed style.
- ⬜ **[MEDIUM]** `md-01` — Blockquote bar glyph is '|' (U+2502) in Rust but U+258E in claude-code
    - rust: `lingxi-code/tui/src/render/markdown.rs:33 (BLOCKQUOTE_BAR = "│" U+2502)`  |  ts: `claude-code/src/constants/figures.ts:34 (BLOCKQUOTE_BAR = '▎' ▎); claude-code/src/utils/markdown.ts:64-70 (chalk.dim(BLOCKQUOTE_BAR) prefix)`
    - fix: Change BLOCKQUOTE_BAR in markdown.rs from "│" to "\u{258e}" (▎). Update the blockquote_has_bar_prefix test's expected starts_with accordingly.
- ⬜ **[MEDIUM]** `md-02` — Ordered-list numbering does not switch to letters (depth 2) / roman numerals (depth 3)
    - rust: `lingxi-code/tui/src/render/markdown.rs:322-329 (ordered marker always format!("{n}. "))`  |  ts: `claude-code/src/utils/markdown.ts:347-359 (getListNumber: depth2->numberToLetter, depth3->numberToRoman), 310-345 (numberToLetter/numberToRoman), 202 (used in list_item)`
    - fix: Port getListNumber: pick marker by list nesting depth — number at depth 0/1, lowercase letters at depth 2, lowercase roman at depth 3 (with the same +1 depth indexing TS uses).
- ⬜ **[MEDIUM]** `md-03` — Links render as plain 'text (url)' instead of OSC-8 blue hyperlink; mailto not special-cased
    - rust: `lingxi-code/tui/src/render/markdown.rs:245-253 (End(Link) pushes " ({url})" after plain link text; no mailto handling, no OSC-8/blue)`  |  ts: `claude-code/src/utils/markdown.ts:141-161 (mailto->plain email; createHyperlink with link text); claude-code/src/utils/hyperlink.ts:24-41 (OSC-8 + chalk.blue when supported, else bare URL)`
    - fix: Strip 'mailto:' to a plain email; for other links emit an OSC-8 hyperlink span (blue fg) carrying the URL with link text as display when display differs from URL, else show just the URL; do NOT append ' (url)' to the text. needs_binary_check only on terminal OSC-8 gating; the 'just URL (not text (url))' and mailto behaviors are unconditional.
- ⬜ **[LOW]** `diff-04` — Word-diff dissimilarity threshold uses word-count ratio in Rust vs character-length ratio in claude-code
    - rust: `lingxi-code/tui/src/render/diff.rs:178-198 (changed/total are word counts; too_dissimilar = changed/total > 0.4)`  |  ts: `claude-code/src/native-ts/color-diff/index.ts:609,617-624,632 (changedLen += token char length; totalLen = oldStr.length+newStr.length; changedLen/totalLen > 0.4)`
    - fix: Accumulate changed as sum of inserted+removed token char lengths and total as old.len()+new.len() (chars), then compare changed/total > 0.4 to match TS exactly.
- ⬜ **[LOW]** `syntax-01` — Syntax highlighting omits shebang/first-line language detection
    - rust: `lingxi-code/tui/src/render/syntax.rs:72-95 (detect_language: info-string + extension only, no firstLine); lingxi-code/tui/src/render/diff.rs:345 (detect_language(None, path) — firstLine never threaded)`  |  ts: `claude-code/src/native-ts/color-diff/index.ts:422-451 (shebang bash/sh/python/node->javascript/ruby/perl, <?php, <?xml, BOM strip)`
    - fix: Add a firstLine/shebang branch to detect_language mirroring index.ts, and thread the file's first line into the diff/file highlight callers.
- ⬜ **[LOW]** `syntax-02` — Filename-based language detection (Dockerfile/Makefile/CMakeLists/Gemfile/Rakefile) missing
    - rust: `lingxi-code/tui/src/render/syntax.rs:86-93 (only Path::extension consulted); test at syntax.rs:230 asserts Makefile -> None`  |  ts: `claude-code/src/native-ts/color-diff/index.ts:414-432 (FILENAME_LANGS by basename and stem)`
    - fix: Add a filename/stem lookup table (Dockerfile/Makefile/Rakefile/Gemfile/CMakeLists) to detect_language before the extension fallback.

## help-doctor  (H=0 M=1 L=6)

- ⬜ **[MEDIUM]** `help-1` — Newline-instructions row is hardcoded to 'shift + ⏎' instead of terminal-dependent text
    - rust: `lingxi-code/tui/src/screens/help.rs:140-145 (ShortcutRow{action:None, fallback:"shift + \u{23CE}", label:"for newline"} rendered verbatim); backslash-return fallback exists at lingxi-code/tui/src/events/keymap.rs:91-93 and lingxi-code/tui/src/root.rs:1215`  |  ts: `claude-code/src/components/PromptInput/PromptInputHelpMenu.tsx:232 ({getNewlineInstructions()}); claude-code/src/components/PromptInput/utils.ts:17-32 (Apple_Terminal/darwin || isShiftEnterKeyBindingInstalled() → shift+⏎, else hasUsedBackslashReturn()? '\⏎' : 'backslash (\) + return (⏎) for newline')`
    - fix: Make the newline row text dynamic: when on Apple_Terminal+darwin or with a detected shift-enter keybinding render 'shift + ⏎ for newline', otherwise render 'backslash (\) + return (⏎) for newline' (and '\⏎ for newline' once the user has used backslash+return), mirroring getNewlineInstructions(). Apply the same fix to footer.rs:129.
- ⬜ **[LOW]** `help-2` — 'to toggle fast mode' shown unconditionally instead of gated on fast-mode availability
    - rust: `lingxi-code/tui/src/screens/help.rs:188-193 (ShortcutRow chat:fastMode 'to toggle fast mode', always present in SHORTCUTS; no availability gate anywhere in the TUI crate)`  |  ts: `claude-code/src/components/PromptInput/PromptInputHelpMenu.tsx:296 (isFastModeEnabled() && isFastModeAvailable() && <Box>…to toggle fast mode</Box>); claude-code/src/utils/fastMode.ts isFastModeEnabled/isFastModeAvailable/getFastModeUnavailableReason`
    - fix: If/when LingXi gains a fast-mode availability notion, gate the row on the analogue of isFastModeEnabled() && isFastModeAvailable(); until then, since LingXi exposes fast mode universally via @fast, accept as a deliberate always-on choice.
- ⬜ **[LOW]** `help-3` — 'ctrl + z to suspend' shown on all platforms instead of non-Windows only
    - rust: `lingxi-code/tui/src/screens/help.rs:176-181 (ShortcutRow{action:None, fallback:"ctrl + z", label:"to suspend"}, no platform guard)`  |  ts: `claude-code/src/components/PromptInput/PromptInputHelpMenu.tsx:270 (getPlatform() !== "windows" && <Box>…ctrl + z to suspend</Box>)`
    - fix: Suppress the 'ctrl + z to suspend' row when cfg!(windows) (or a runtime platform check), matching getPlatform() !== 'windows'.
- ⬜ **[LOW]** `help-4` — Help screen title is 'Help' instead of 'Claude Code v<version>'
    - rust: `lingxi-code/tui/src/screens/help.rs:57 (pub const TITLE: &str = "Help"); emitted first at help.rs:360 and rendered at lingxi-code/tui/src/app.rs:983`  |  ts: `claude-code/src/components/HelpV2/HelpV2.tsx:141 (<Tabs title={false ? "/help" : `Claude Code v${MACRO.VERSION}`} color="professionalBlue" defaultTab="general">)`
    - fix: Render the title as 'Claude Code v<version>' using env!("CARGO_PKG_VERSION") (the doctor.rs cli_version pattern), or accept 'Help' as a deliberate LingXi rename if the version is surfaced elsewhere.
- ⬜ **[LOW]** `help-5` — Missing 'For more help: <docs URL>' line in the help screen
    - rust: `lingxi-code/tui/src/screens/help.rs:359-376 (render_help_to_string_with emits TITLE/INTRO/rows/indicator/FOOTER only — no docs URL line)`  |  ts: `claude-code/src/components/HelpV2/HelpV2.tsx:148-150 (<Box marginTop={1}><Text>For more help:{" "}<Link url="https://code.claude.com/docs/en/overview" /></Text></Box>)`
    - fix: Add a 'For more help: https://code.claude.com/docs/en/overview' line to the help body (above the footer).
- ⬜ **[LOW]** `help-6` — Help dismiss-hint wording: 'Esc to close' vs TS 'esc to cancel' (italic, lowercase)
    - rust: `lingxi-code/tui/src/screens/help.rs:65 (pub const FOOTER: &str = "Esc to close"; emitted verbatim at help.rs:374, no italic, not keymap-resolved)`  |  ts: `claude-code/src/components/HelpV2/HelpV2.tsx:54 (dismissShortcut = useShortcutDisplay('help:dismiss','Help','esc')) + :156 (<Text italic>{dismissShortcut} to cancel</Text>)`
    - fix: Render the footer as '<resolved help:dismiss chord> to cancel' in lowercase italic, resolving the chord from the live keymap (the same bindings already passed to render_help_to_string_with).
- ⬜ **[LOW]** `doctor-1` — Doctor dismiss interaction and footer hint differ (Esc/q + 'Press Esc or q to return' vs TS Enter + 'Press Enter to continue…')
    - rust: `lingxi-code/tui/src/screens/doctor.rs:160 (Text(content:"Press Esc or q to return", color: Color::DarkGrey)); screen closes on Esc/q`  |  ts: `claude-code/src/screens/Doctor.tsx:482 (<PressEnterToContinue/>) + claude-code/src/components/PressEnterToContinue.tsx:8 (<Text color="permission">Press <bold>Enter</bold> to continue…</Text>); dismiss on confirm:yes/confirm:no (Enter)`
    - fix: To match claude-code, change the Doctor dismiss affordance to Enter with a permission-colored 'Press Enter to continue…' hint (Enter bold). Otherwise accept Esc/q as LingXi's intentional screen-close convention, since all other LingXi screens use it.

## mcp-hooks-skills  (H=1 M=8 L=5)

- ⬜ **[HIGH]** `hooks-lists-hooks-not-events` — Hooks menu lists hooks flatly not events-first
    - rust: `lingxi-code/tui/src/screens/hooks.rs:96-124 (flat HookRow list keyed by synthetic `name`)`  |  ts: `claude-code/src/components/hooks/SelectEventMode.tsx:80-98 (event list with (count) + summary; drill to matcher→hook→detail)`
    - fix: Restructure to an event-first list (with per-event count badge + summary), then matcher list, then hook list, then ViewHookMode detail. Drop the synthetic per-hook `name`; identify hooks by event+matcher+config. This needs a richer list_hooks seam exposing event metadata and grouping.
- ⬜ **[MEDIUM]** `skills-section-subtitle-missing` — Skills menu omits per-section path subtitle
    - rust: `lingxi-code/tui/src/screens/skills.rs:96-101 (SkillSection has only `title`), 253 (pushes bare title)`  |  ts: `claude-code/src/components/skills/SkillsMenu.tsx:33-46 (getSourceSubtitle), 136 (renders ' ({subtitle})')`
    - fix: Add a `subtitle: Option<String>` to SkillSection populated with getDisplayPath(skills dir) for project/user sections, and render '{title} ({subtitle})' as the section header line in content_lines.
- ⬜ **[MEDIUM]** `mcp-title-text` — MCP title is 'MCP Servers' not 'Manage MCP servers'
    - rust: `lingxi-code/tui/src/screens/mcp.rs:97`  |  ts: `claude-code/src/components/mcp/MCPListPanel.tsx:461 (Dialog title="Manage MCP servers")`
    - fix: Change the List-mode header literal from "MCP Servers\n" to "Manage MCP servers\n".
- ⬜ **[MEDIUM]** `mcp-count-subtitle-missing` — MCP list missing N server(s) count subtitle
    - rust: `lingxi-code/tui/src/screens/mcp.rs:96-114`  |  ts: `claude-code/src/components/mcp/MCPListPanel.tsx:373-380 (totalServers plural), 461 (subtitle={t21})`
    - fix: After the title line, emit a subtitle line '{N} {plural(N,"server")}' using the existing row count.
- ⬜ **[MEDIUM]** `mcp-status-icon-missing` — MCP rows omit colored status icon
    - rust: `lingxi-code/tui/src/screens/mcp.rs:108-111 (no icon between name and status)`  |  ts: `claude-code/src/components/mcp/MCPListPanel.tsx:305-337 (statusIcon figures), 337 (' · {statusIcon} {statusText}')`
    - fix: Map McpStatus to a colored figure (Connected→green tick, Error→red cross, Disconnected→inactive radioOff) and render ' · {icon} {status}' between the name and status text.
- ⬜ **[MEDIUM]** `mcp-status-vocabulary` — MCP status words differ (disconnected/error vs failed)
    - rust: `lingxi-code/tui/src/root.rs:1509-1511 (Disconnected→"disconnected", Error(e)→"error: {e}")`  |  ts: `claude-code/src/components/mcp/MCPListPanel.tsx:306-336 (statusText: connected/failed/disabled/needs authentication/connecting…)`
    - fix: Map McpStatus::Error→'failed' (the figures.cross case) and McpStatus::Disconnected→'failed' to match claude-code's vocabulary; drop the 'error:'/'disconnected' literals. Richer connecting/needs-auth/disabled states require extending the McpStatus seam.
- ⬜ **[MEDIUM]** `hooks-count-subtitle-missing` — Hooks menu missing N hooks configured subtitle
    - rust: `lingxi-code/tui/src/screens/hooks.rs:99-117`  |  ts: `claude-code/src/components/hooks/SelectEventMode.tsx:38-45 (subtitle), 117 (Dialog subtitle={subtitle})`
    - fix: Emit a subtitle line '{N} {plural(N,"hook")} configured' after the 'Hooks' title using the total hook count.
- ⬜ **[MEDIUM]** `hooks-readonly-banner-missing` — Hooks menu omits read-only info banner
    - rust: `lingxi-code/tui/src/screens/hooks.rs:96-124 (no read-only banner)`  |  ts: `claude-code/src/components/hooks/SelectEventMode.tsx:54-60 (always-shown 'This menu is read-only...' info line)`
    - fix: Render a dim line 'ⓘ This menu is read-only. To add or modify hooks, edit settings.json directly or ask Claude. Learn more' near the top of the hooks list screen (the link can be plain text in the TUI).
- ⬜ **[MEDIUM]** `hooks-detail-fields-divergent` — Hook detail fields differ (Timeout shown; Type/Source/content missing)
    - rust: `lingxi-code/tui/src/screens/hooks.rs:127-136 (title=name; Event/Matcher/Timeout only)`  |  ts: `claude-code/src/components/hooks/ViewHookMode.tsx:26-153 (Event/Matcher/Type/Source/Plugin/content-box/Status-message/edit-hint; title 'Hook details')`
    - fix: Title the dialog 'Hook details'; drop the Timeout field; render Event, Matcher (conditional), Type, Source, optional Plugin, the command/prompt/URL content in a bordered box with a dim label, optional Status message, and the edit hint. Requires extending HookInfo with type/source/content/plugin fields.
- ⬜ **[LOW]** `skills-gap-between-sections` — Skills menu lacks blank line between groups
    - rust: `lingxi-code/tui/src/screens/skills.rs:247-258 (content_lines, no inter-section separator)`  |  ts: `claude-code/src/components/skills/SkillsMenu.tsx:196 (<Box ... gap={1}> wrapping the groups)`
    - fix: In content_lines, push an empty String between consecutive non-empty sections (but not before the first or after the last).
- ⬜ **[LOW]** `mcp-footer-text` — MCP footer wording differs
    - rust: `lingxi-code/tui/src/screens/mcp.rs:113`  |  ts: `claude-code/src/components/mcp/MCPListPanel.tsx:471 (Byline: navigate / confirm / cancel, no 'Press')`
    - fix: Change the list footer to '↑↓ to navigate · Enter to confirm · Esc to cancel' (drop 'Press '). Note claude-code's actual glyph format is shortcut+action via Byline; matching the verbs and dropping the prefix is the load-bearing fix.
- ⬜ **[LOW]** `mcp-detail-footer-case` — Detail footer lowercase 'esc to go back'
    - rust: `lingxi-code/tui/src/screens/mcp.rs:130 ("\nesc to go back"); lingxi-code/tui/src/screens/hooks.rs:135`  |  ts: `claude-code/src/components/hooks/ViewHookMode.tsx:153,168 (inputGuide => <Text>Esc to go back</Text>)`
    - fix: Capitalize the detail footers to 'Esc to go back' in both mcp.rs:130 and hooks.rs:135.
- ⬜ **[LOW]** `mcp-empty-state-text` — MCP empty state truncated vs claude-code guidance
    - rust: `lingxi-code/tui/src/screens/mcp.rs:98-100 ("No MCP servers configured.")`  |  ts: `claude-code/src/components/mcp/MCPSettings.tsx:149-151 (onComplete full guidance string)`
    - fix: When list_mcp_servers returns empty, surface the full guidance string ('No MCP servers configured. Please run /doctor if this is unexpected. Otherwise, run `claude mcp --help` or visit https://code.claude.com/docs/en/mcp to learn more.') rather than opening the panel with the truncated line.
- ⬜ **[LOW]** `hooks-matcher-placeholder` — Hook detail uses (any tool) vs claude-code (all)
    - rust: `lingxi-code/tui/src/screens/hooks.rs:132 (unwrap_or("(any tool)"))`  |  ts: `claude-code/src/components/hooks/ViewHookMode.tsx:34 (selectedHook.matcher || "(all)")`
    - fix: Change the empty-matcher placeholder from '(any tool)' to '(all)' in render_hook_detail.

## memory-perms-connect-bgtasks  (H=1 M=4 L=8)

- ⬜ **[HIGH]** `PERM-1` — /permissions is a read-only viewer; TS is a full interactive add/remove/delete tabbed manager
    - rust: `lingxi-code/tui/src/screens/permissions.rs:18-22 (read-only viewer doc), :165-199 (handle_permissions_key only Up/Down/Enter/Esc), :203-228 (flat list render)`  |  ts: `claude-code/src/components/permissions/rules/PermissionRuleList.tsx:1070-1117 (Tabs Recently-denied/Allow/Ask/Deny/Workspace), :603-606 (Add a new rule…), :190-227 (Delete Yes/No), :986/:1029/:1108 (workspace add/remove); commands/permissions/permissions.tsx:5-8 (command → PermissionRuleList)`
    - fix: Build the tabbed manager: Allow/Ask/Deny/Workspace tabs grouped by behavior, an 'Add a new rule…' Select entry per editable tab feeding a rule-input + AddPermissionRules persist (3c persist_permission_update exists), a delete-confirmation detail, and workspace add/remove. Defer the Recently-denied tab to PERM-5/TRANSCRIPT_CLASSIFIER.
- ⬜ **[MEDIUM]** `PERM-2` — Permission rule detail missing the human-readable rule description and uses different field layout/labels
    - rust: `lingxi-code/tui/src/screens/permissions.rs:232-240 (render_rule_detail: rule + 'Behavior: {b}' + 'Source: {s} settings' + 'esc to go back')`  |  ts: `claude-code/src/components/permissions/rules/PermissionRuleList.tsx:50-53/:103/:119/:127 (bold value + PermissionRuleDescription + 'From {source}'); PermissionRuleDescription.tsx:29-63 (per-tool subtitle strings)`
    - fix: Port PermissionRuleDescription's per-tool subtitle generation; render bold rule value + dimmed description + 'From {sourceDisplayString}' to match RuleDetails.
- ⬜ **[MEDIUM]** `MEM-1` — Project memory description hardcoded 'Checked in at'; TS is git-conditional
    - rust: `lingxi-code/tui/src/screens/memory.rs:53 (description: "Checked in at ./CLAUDE.md" unconditional)`  |  ts: `claude-code/src/components/memory/MemoryFileSelector.tsx:88,93 (isGit ? 'Checked in at' : 'Saved in' + ' ./CLAUDE.md')`
    - fix: Make the project-tier description git-aware: 'Checked in at ./CLAUDE.md' when cwd is inside a git repo, else 'Saved in ./CLAUDE.md'.
- ⬜ **[MEDIUM]** `TRUST-1` — Trust dialog missing the 'Enter to confirm · Esc to cancel' footer hint line
    - rust: `lingxi-code/tui/src/startup_trust.rs:100-113 (render_lines, no footer), :184-216 (draw_dialog paints only body + 2 options)`  |  ts: `claude-code/src/components/TrustDialog/TrustDialog.tsx:248 (dimmed 'Enter to confirm · Esc to cancel' / 'Press {keyName} again to exit'), :257 (rendered in body)`
    - fix: Append a dimmed 'Enter to confirm · Esc to cancel' footer line after the two options in render_lines/draw_dialog (and a 'Press <key> again to exit' variant if exit-pending is modeled).
- ⬜ **[MEDIUM]** `BGTASK-3` — Background-tasks footer hints and subtitle diverge from TS Byline shortcut hints
    - rust: `lingxi-code/tui/src/screens/background_tasks.rs:116 ('↑↓ move · enter open · esc close'), :104-105 (header, no subtitle)`  |  ts: `claude-code/src/components/tasks/BackgroundTasksDialog.tsx:414 (Byline KeyboardShortcutHints incl. conditional x stop / f foreground / stop-all), :404-413 (subtitle running counts), :425 (Dialog title+subtitle)`
    - fix: Render the running-count subtitle and Byline-style hints with TS verbs ('select'/'view'/'close'), add the conditional 'x stop' hint and wire an actual stop action for running local_bash/local_agent tasks; '←/Esc' (not just 'esc') closes.
- ⬜ **[LOW]** `PERM-3` — Permissions footer/hint and empty-state text diverge from TS
    - rust: `lingxi-code/tui/src/screens/permissions.rs:208 ('No permission rules configured.'), :221 (static hint)`  |  ts: `claude-code/src/components/permissions/rules/PermissionRuleList.tsx:390-399 (per-tab subtitles), :1131 (4-state footer hint)`
    - fix: Add the three per-tab subtitle strings and the context-sensitive footer-hint variants once the tabbed manager (PERM-1) exists.
- ⬜ **[LOW]** `MEM-2` — Imported/nested memory file rows missing the 'L ' prefix + indentation
    - rust: `lingxi-code/tui/src/screens/memory.rs:74-80 (flat path label, no prefix/indent, always '@-imported'); memory hierarchy.rs:74-87 (HierarchyEntry has no depth/parent/isNested)`  |  ts: `claude-code/src/components/memory/MemoryFileSelector.tsx:70-84 (indent + 'L ' prefix), :95-99 ('@-imported' vs 'dynamically loaded')`
    - fix: Apply depth indentation + 'L ' prefix to non-top-level entries; use 'dynamically loaded' for nested files and '@-imported' for parent-imported files.
- ⬜ **[LOW]** `MEM-3` — Selecting a memory tier opens an inline editor; TS opens the file in the external editor
    - rust: `lingxi-code/tui/src/screens/memory.rs:230-235 (Enter → inline open_editor), :262-306 (inline 'Edit memory' buffer with Ctrl-S save)`  |  ts: `claude-code/src/commands/memory/memory.tsx:42-56 (editFileInEditor + 'Opened memory file at …' editor hint); components/memory/MemoryFileSelector.tsx:374 (onSelect forwards path)`
    - fix: For strict parity, on Enter spawn $EDITOR/$VISUAL on the selected path and emit the 'Opened memory file at … > Using $EDITOR' message; otherwise document the inline editor as an intentional LingXi divergence.
- ⬜ **[LOW]** `TRUST-2` — Trust dialog selection marker uses '> ' instead of the figures pointer '❯ '
    - rust: `lingxi-code/tui/src/startup_trust.rs:211 (marker = if selected { "> " } else { "  " }); compare permissions.rs:213 / memory.rs:283 which use \u{276F}`  |  ts: `claude-code/src/components/TrustDialog/TrustDialog.tsx:240 (<Select>); components/CustomSelect/select.tsx:522 (focused row → figures.pointer = ❯ U+276F)`
    - fix: Use '\u{276F} ' (❯) as the selected-row marker to match CustomSelect's pointer glyph (same fix as BGTASK-2).
- ⬜ **[LOW]** `BGTASK-1` — Background-tasks empty state text differs ('(no background tasks)' vs 'No tasks currently running')
    - rust: `lingxi-code/tui/src/screens/background_tasks.rs:106-109 (out.push_str("(no background tasks)"))`  |  ts: `claude-code/src/components/tasks/BackgroundTasksDialog.tsx:426 (<Text dimColor>No tasks currently running</Text>)`
    - fix: Change the empty-state string to 'No tasks currently running' (dimmed).
- ⬜ **[LOW]** `BGTASK-2` — Background-tasks selection pointer '> ' instead of figures pointer '❯ '
    - rust: `lingxi-code/tui/src/screens/background_tasks.rs:111 (marker = if i == state.selected { "> " } else { "  " })`  |  ts: `claude-code/src/components/tasks/BackgroundTasksDialog.tsx:571 (isSelected ? figures.pointer + ' ' : '  ')`
    - fix: Use '\u{276F} ' (❯) for the selected-row marker.
- ⬜ **[LOW]** `BGTASK-4` — Background-tasks list not grouped into category sections with bold headers/counts
    - rust: `lingxi-code/tui/src/screens/background_tasks.rs:110-115 (flat enumerated list, no headers)`  |  ts: `claude-code/src/components/tasks/BackgroundTasksDialog.tsx:437-444 (bold '  Shells' (N), conditional), :465-472 (bold '  Local agents' (N))`
    - fix: Group rows by type and emit the bold dimmed '  Shells' + ' (N)' (conditional on other groups present) and '  Local agents' + ' (N)' headers for the in-scope groups, with the marginTop blank-line gap between groups.
- ⬜ **[LOW]** `BGTASK-5` — Background-task detail header text diverges and single-task auto-skip not modeled
    - rust: `lingxi-code/tui/src/components/tasks/detail.rs:14-21 (detail_header: 'Shell details' for local_bash|monitor_mcp; 'agent › {desc}' hardcodes 'agent'); screens/background_tasks.rs:56-89 (always opens in List, no single-task auto-skip)`  |  ts: `claude-code/src/components/tasks/BackgroundTasksDialog.tsx:152-159 (single-task auto-skip to detail); ShellDetailDialog.tsx:164 ('Monitor details' vs 'Shell details'); AsyncAgentDetailDialog.tsx:102-106 ('{agentType} › {description}')`
    - fix: Use 'Monitor details' for monitor_mcp; for local_agent use '{agentType} › {description || "Async agent"}' (carry agentType into TaskRow); add single-task auto-skip-to-detail so opening the dialog with exactly one task lands on detail, with back returning to the list only if a second task appeared.

## messages-assistant  (H=3 M=3 L=2)

- ⬜ **[HIGH]** `ma-01` — Tool-use preview dumps raw JSON instead of per-tool renderToolUseMessage
    - rust: `lingxi-code/tui/src/components/messages/assistant_tool_use.rs:43`  |  ts: `claude-code/src/components/messages/AssistantToolUseMessage.tsx:163`
    - fix: Route each tool through a per-tool userFacingName + renderToolUseMessage preview instead of JSON.stringify of the whole input.
- ⬜ **[HIGH]** `ma-02` — Tool-use header colored uniformly (Claude accent) instead of dot=success/error/dim + bold default-text name
    - rust: `lingxi-code/tui/src/components/messages/assistant_tool_use.rs:84`  |  ts: `claude-code/src/components/ToolUseLoader.tsx:19`
    - fix: Split the header into dot (resolution-state color) + bold default-text name + default-text preview; drop the blanket cyan/accent.
- ⬜ **[HIGH]** `ma-03` — BLACK_CIRCLE marker is `●` everywhere; claude-code uses `⏺` (U+23FA) on macOS
    - rust: `lingxi-code/tui/src/components/messages/assistant_text.rs:48`  |  ts: `claude-code/src/constants/figures.ts:4`
    - fix: Make the marker platform-aware: `⏺` (U+23FA) on macOS, `●` (U+25CF) otherwise, mirroring figures.BLACK_CIRCLE, across all four marker constants.
- ⬜ **[MEDIUM]** `ma-04` — Expanded thinking body rendered italic; claude-code dims it only (not italic)
    - rust: `lingxi-code/tui/src/components/messages/thinking.rs:75`  |  ts: `claude-code/src/components/messages/AssistantThinkingMessage.tsx:69`
    - fix: Italicize only the header; render the expanded markdown body dim without italic.
- ⬜ **[MEDIUM]** `ma-05` — Expanded thinking missing the gap=1 blank line between header and body
    - rust: `lingxi-code/tui/src/components/messages/thinking.rs:42`  |  ts: `claude-code/src/components/messages/AssistantThinkingMessage.tsx:77`
    - fix: Emit a blank line between the `∴ Thinking…` header and the indented markdown body when expanded.
- ⬜ **[MEDIUM]** `ma-06` — Advisor `Advising` rendered dim + no leading dot; claude-code uses bold default text plus a ToolUseLoader dot
    - rust: `lingxi-code/tui/src/components/messages/advisor.rs:99`  |  ts: `claude-code/src/components/messages/AdvisorMessage.tsx:73`
    - fix: Render `Advising` bold in default text color (not dim), keep the descriptor runs dim, and prepend the resolution-colored dot glyph.
- ⬜ **[LOW]** `ma-07` — Advisor `using {model}` shows raw model id instead of renderModelName
    - rust: `lingxi-code/tui/src/components/messages/advisor.rs:62`  |  ts: `claude-code/src/components/messages/AdvisorMessage.tsx:80`
    - fix: Run the advisor model id through the same model-name display mapping (renderModelName) before appending.
- ⬜ **[LOW]** `ma-09` — SystemAPIError footer omits the API_TIMEOUT_MS hint suffix
    - rust: `lingxi-code/tui/src/components/messages/system_api_error.rs:47`  |  ts: `claude-code/src/components/messages/SystemAPIErrorMessage.tsx:106`
    - fix: If the API_TIMEOUT_MS env var is set, append ` · API_TIMEOUT_MS={n}ms, try increasing it` to the retry-countdown footer.

## messages-user  (H=3 M=3 L=5)

- ✅ **[HIGH]** `tool-result-gutter-glyph` — Tool-result gutter uses '└ ' (U+2514) instead of claude-code's '  ⎿  ' (2 spaces + U+23BF + space + nbsp)
    - rust: `lingxi-code/tui/src/components/messages/user_tool_result.rs:23 (pub const MARKER: &str = "└ "; U+2514) — used at lines 167,176,236,283; wired via lingxi-code/tui/src/components/scrollback.rs:108-135`  |  ts: `claude-code/src/components/MessageResponse.tsx:22 (`{"  "}⎿  ` — U+23BF arc glyph; python-decoded codepoint U+23BF)`
    - fix: Change MARKER to `  \u{23BF}  ` (2 spaces + U+23BF + space + nbsp/space) to match local_command_output.rs::GUTTER; the TS second trailing char is a non-breaking space (U+00A0) though a regular space renders identically. Apply to all 4 use-sites (string render + Bash + diff header).
- ⬜ **[HIGH]** `tool-error-result-formatting` — Errored tool results are not rendered as red 'Error: …' with tag-stripping and 10-line cap (FallbackToolUseErrorMessage)
    - rust: `lingxi-code/tui/src/events/orchestrator_bridge.rs:43-51 (TurnEvent::ToolUseResult has no is_error) + 181-192 (emit_tool_result drops it); lingxi-code/tui/src/components/messages/user_tool_result.rs:324-329 renders body in TuiTheme::DIM regardless`  |  ts: `claude-code/src/components/messages/UserToolResultMessage/UserToolResultMessage.tsx:71-86 (is_error → UserToolErrorMessage) → claude-code/src/components/FallbackToolUseErrorMessage.tsx:11,35-55,76,86`
    - fix: Add an is_error field to TurnEvent::ToolUseResult (or re-derive from the result JSON), thread it into RenderedMessage::UserToolResult, and add an error-render branch that strips <tool_use_error>/<error>/sandbox-violation tags, applies the `Error: ` prefix (unless already Error:/Cancelled:), maps InputValidationError→'Invalid tool parameters' non-verbose, colors with TuiTheme::ERROR, and caps at 10 lines with a `… +N line(s) (ctrl+o to see all)` footer.
- ⬜ **[HIGH]** `tool-reject-cancel-interrupted` — Rejected/canceled/interrupted tool results dump the verbose model-facing REJECT_MESSAGE instead of 'Interrupted · What should Claude do instead?'
    - rust: `lingxi-code/orchestrator/src/streaming_executor.rs:17,55 (emits bare REJECT_MESSAGE); lingxi-code/tui/src/streaming.rs:61-74 + components/messages/user_tool_result.rs:324-329 render it as plain dim body under `└ ``  |  ts: `claude-code/src/components/messages/UserToolResultMessage/UserToolResultMessage.tsx:40-70 + UserToolErrorMessage.tsx:33-41,63-72 → claude-code/src/components/InterruptedByUser.tsx:8 (`Interrupted ` + `· What should Claude do instead?`)`
    - fix: Before rendering UserToolResult, detect content starting with CANCEL_MESSAGE / REJECT_MESSAGE / REJECT_MESSAGE_WITH_REASON_PREFIX (and == INTERRUPT_MESSAGE_FOR_TOOL_USE) and substitute an InterruptedByUser line (`Interrupted ` + `· What should Claude do instead?`) inside the `  ⎿  ` gutter, mirroring UserToolCanceledMessage / FallbackToolUseRejectedMessage.
- ✅ **[MEDIUM]** `tool-result-continuation-indent` — Tool-result continuation lines indent by 2 spaces instead of aligning under the content column (~5)
    - rust: `lingxi-code/tui/src/components/messages/user_tool_result.rs:25 (pub const INDENT: &str = "  ";) + lines 179,185 push INDENT for non-first lines`  |  ts: `claude-code/src/components/MessageResponse.tsx:22,29,37 — fixed flexShrink=0 gutter column (`  ⎿  `, 5 cols) + adjacent flexGrow children column`
    - fix: Set INDENT to 5 spaces to match the `  ⎿  ` gutter display width so continuation lines align under the content column.
- ⬜ **[MEDIUM]** `fileedit-result-added-removed-header` — Edit/Write tool result diff is missing the 'Added N lines, removed M lines' summary header
    - rust: `lingxi-code/tui/src/components/messages/user_tool_result.rs:227-273 (diff branch: header `└ ` + StructuredDiff rows only; no count line)`  |  ts: `claude-code/src/components/FileEditToolUpdatedMessage.tsx:32-62 (Added <b>N</b> line(s)[, removed <b>M</b> line(s)]) above StructuredDiffList:90-91`
    - fix: In the diff branch compute additions/removals from the StyledLine diff (count + vs - rows, or from old/new strings) and prepend the `Added N line(s)[, removed M line(s)]` summary line above the diff rows (capitalize 'Removed' only when additions==0; counts bold), inside the gutter — matching FileEditToolUpdatedMessage.
- ⬜ **[MEDIUM]** `rate-limit-missing-gutter` — RateLimitMessage renders without the '  ⎿  ' MessageResponse gutter
    - rust: `lingxi-code/tui/src/components/messages/rate_limit.rs:59-64 (bare Column, no gutter)`  |  ts: `claude-code/src/components/messages/RateLimitMessage.tsx:152 (<MessageResponse> wrapping error text + upsell)`
    - fix: Wrap the RateLimitMessage Column in the `  ⎿  ` gutter (MessageResponse equivalent); the gutter renders once on the first row, upsell as a child line.
- ⬜ **[LOW]** `hook-progress-missing-gutter` — HookProgressMessage renders without the '  ⎿  ' MessageResponse gutter
    - rust: `lingxi-code/tui/src/components/messages/hook_progress.rs:53-62 (bare Column, no gutter; {event} bold deferred)`  |  ts: `claude-code/src/components/messages/HookProgressMessage.tsx:66,107 (<MessageResponse> wrapping the row) + 49,90 (bold {event})`
    - fix: Wrap the rendered line in the `  ⎿  ` gutter (MessageResponse equivalent); optionally split {event} into a bold dim run.
- ⬜ **[LOW]** `agent-notification-black-circle-darwin` — UserAgentNotificationMessage marker is '●' (U+25CF) instead of macOS '⏺' (U+23FA)
    - rust: `lingxi-code/tui/src/components/messages/user_agent_notification.rs:14-15 (MARKER = "\u{25CF} "; comment locks non-darwin form)`  |  ts: `claude-code/src/constants/figures.ts:4 (BLACK_CIRCLE = darwin ? '⏺' (U+23FA) : '●' (U+25CF))`
    - fix: Make the BLACK_CIRCLE marker platform-conditional (⏺ U+23FA on macOS, ● U+25CF otherwise) per figures.ts; ideally centralize so assistant/system markers follow too.
- ⬜ **[LOW]** `user-plan-missing-blank-line` — UserPlanMessage omits the blank line between 'Plan to implement' header and the markdown body
    - rust: `lingxi-code/tui/src/components/messages/plan.rs:69-78 (header Text directly above body Text, no margin)`  |  ts: `claude-code/src/components/messages/UserPlanMessage.tsx:18 (<Box marginBottom={1}> around the header)`
    - fix: Insert a blank line (marginBottom-equivalent empty row) between the header Text and the body Text inside the bordered Column. Consider also adding the paddingX={1} inset that TS applies (separate divergence).
- ⬜ **[LOW]** `compact-boundary-marginy` — CompactBoundaryMessage omits the marginY (blank line above and below)
    - rust: `lingxi-code/tui/src/components/messages/compact_boundary.rs:22-28 (Column, no vertical margin); messages stacked with no gap in virtual_message_list.rs:736-740`  |  ts: `claude-code/src/components/messages/CompactBoundaryMessage.tsx:10 (<Box marginY={1}>)`
    - fix: Add a blank line above and below the boundary line (marginY={1} equivalent), e.g. by emitting empty Text rows or a wrapping View with vertical margin — the scrollback inserts no uniform inter-message spacing, so this is needed for parity.
- ⬜ **[LOW]** `memory-input-saving-line-pinned` — UserMemoryInputMessage saving acknowledgement is pinned to 'Got it.' instead of randomly sampling 3 phrases
    - rust: `lingxi-code/tui/src/components/messages/memory_input.rs:24 (SAVING_MESSAGE = "Got it."; pinned for snapshot determinism)`  |  ts: `claude-code/src/components/messages/UserMemoryInputMessage.tsx:8-9 (getSavingMessage() = sample(['Got it.', 'Good to know.', 'Noted.']))`
    - fix: Sample one of the three phrases at construction time (e.g. seeded/once-per-message RNG) instead of pinning 'Got it.', or accept the pin as an intentional determinism tradeoff if snapshot stability is required.

## permissions  (H=4 M=4 L=1)

- ⬜ **[HIGH]** `perm-01` — Option labels are '[1] Allow Once / [2] Allow Always / [N] Deny' instead of claude-code's 'Yes / Yes, and don't ask again... / No'
    - rust: `lingxi-code/tui/src/components/permissions/tool_use_confirm.rs:83-85 (literal '[1] Allow Once'/'[2] Allow Always'/'[N] Deny'); confirmed rendered by lingxi-code/tui/tests/snapshot_permission_dialogs.rs:31-33`  |  ts: `claude-code/src/components/permissions/FallbackPermissionRequest.tsx:160,185,198 (labels Yes / Yes, and don't ask again for <name> commands in <cwd> / No); gate claude-code/src/utils/permissions/permissionsLoader.ts:42-43 (shouldShowAlwaysAllowOptions defaults true)`
    - fix: Replace the three hardcoded labels with 'Yes', a conditional 'Yes, and don't ask again for <userFacingName> commands in <cwd>' (shown only when shouldShowAlwaysAllowOptions() i.e. not allowManagedPermissionRulesOnly), and 'No'. Drop the numbered-bracket scheme.
- ⬜ **[HIGH]** `perm-02` — No per-tool permission dialogs — Bash/Edit/Write/Fetch/Skill/MCP all fall through to one generic dialog
    - rust: `lingxi-code/tui/src/app.rs:739-771 (single ToolUseConfirm renderer, serde_json::to_string_pretty input); prompting_gate.rs:31-50 (only 3 request variants)`  |  ts: `claude-code/src/components/permissions/PermissionRequest.tsx:47-81 (permissionComponentForTool switch); FileEditPermissionRequest, FileWritePermissionRequest, BashPermissionRequest, WebFetchPermissionRequest, SkillPermissionRequest, FilesystemPermissionRequest dirs`
    - fix: Introduce per-tool permission renderers (tool-discriminated) mirroring permissionComponentForTool: tool-specific title (Edit file/Create file/Overwrite file/Fetch/Tool use), subtitle, 'Do you want to ...?' question, and diff/command body instead of raw JSON.
- ⬜ **[HIGH]** `perm-03` — Header text 'Claude needs your permission to use {tool}' + 'Input: {json}' replaces claude-code's titled dialog with question line
    - rust: `lingxi-code/tui/src/components/permissions/tool_use_confirm.rs:72-73,95-96 (header 'Claude needs your permission to use {tool}', 'Input: {pretty}'); snapshot_permission_dialogs.rs:28`  |  ts: `claude-code/src/components/permissions/FallbackPermissionRequest.tsx:323 (<PermissionDialog title="Tool use">) + sourcesContent body {userFacingName}(renderToolUseMessage) + dim truncateToLines(description,3); PermissionPrompt.tsx:54 (question)`
    - fix: Drop the 'Claude needs your permission to use' body line and the 'Input:' JSON line. Render a titled dialog ('Tool use') with the rendered tool-use message + dim 3-line description, and a 'Do you want to proceed?' question above the options.
- ⬜ **[HIGH]** `perm-06` — ExitPlanMode dialog uses generic Allow Once/Always/Deny instead of the rich plan-approval option list
    - rust: `lingxi-code/tui/src/components/permissions/exit_plan_mode.rs:54 (header 'Claude Code needs your approval for the plan'),64-66 (generic [1]/[2]/[N]); snapshot_permission_dialogs.rs:48-51`  |  ts: `claude-code/src/components/permissions/ExitPlanModePermissionRequest/ExitPlanModePermissionRequest.tsx:627 (PermissionDialog color planMode title 'Ready to code?'),635 (Markdown plan),646-649 (dim proceed text),723,728,737-744 (option labels + No-keep-planning input)`
    - fix: Render a planMode-colored 'Ready to code?' dialog with the plan as Markdown, and a Select with 'Yes, auto-accept edits' / 'Yes, manually approve edits' / 'No, keep planning' (input + placeholder 'Tell Claude what to change' + 'shift+tab to approve with this feedback'). Context-clear/bypass variants can be deferred.
- ⬜ **[MEDIUM]** `perm-04` — Missing the 'Do you want to proceed?' / per-tool question line above the options
    - rust: `lingxi-code/tui/src/components/permissions/tool_use_confirm.rs:86-103 (no question Text between input and buttons)`  |  ts: `claude-code/src/components/permissions/PermissionPrompt.tsx:54 (question default 'Do you want to proceed?'),266-267 (rendered above Select)`
    - fix: Add a question Text line above the options defaulting to 'Do you want to proceed?', overridable per tool variant.
- ⬜ **[MEDIUM]** `perm-05` — Dialog border chrome differs: full round box vs claude-code's top-only 'permission'-colored border
    - rust: `lingxi-code/tui/src/components/permissions/tool_use_confirm.rs:87-91; lingxi-code/tui/src/components/permissions/exit_plan_mode.rs:67-72 (full round border, padding 1, no color)`  |  ts: `claude-code/src/components/permissions/PermissionDialog.tsx:62 (borderLeft/Right/Bottom=false, borderColor permission/planMode, marginTop 1),29 (default color 'permission')`
    - fix: Render only a top border (borderLeft/Right/Bottom=false) colored with the theme 'permission' (or 'planMode' for plan dialogs) and marginTop=1. Remove the four-sided box.
- ⬜ **[MEDIUM]** `perm-08` — AskUserQuestion permission UI is entirely missing
    - rust: `lingxi-code/tools/ui/src/ask_user_question.rs:130-142 (FirstOptionResolver auto-picks first label),495-498 (check_permissions returns Allow); no with_resolver wiring in lingxi-code/tui or apps`  |  ts: `claude-code/src/tools/AskUserQuestionTool/AskUserQuestionTool.tsx:182-184 (checkPermissions behavior 'ask'); claude-code/src/components/permissions/AskUserQuestionPermissionRequest/ (interactive multiple-choice UI)`
    - fix: Wire an interactive AskUserQuestion resolver in the TUI that presents the multiple-choice questions (per-question views, navigation bar, multi-select, submit) and feeds the user's selections back to with_resolver, instead of FirstOptionResolver auto-picking the first option.
- ⬜ **[MEDIUM]** `perm-09` — Worker attribution rendered as a separate '● @name' line instead of a dim '· @name' suffix in the title row
    - rust: `lingxi-code/tui/src/components/permissions/tool_use_confirm.rs:92-94 (separate Text line); lingxi-code/tui/src/components/permissions/worker.rs:28-30 (render_worker_badge = '● @name', U+25CF); app.rs:748-751`  |  ts: `claude-code/src/components/permissions/PermissionRequestTitle.tsx:32 (dim '\xB7 @name' inline),40 (row gap 1 right of bold title)`
    - fix: Render worker attribution as a dim '· @<name>' inline suffix on the title row (middot U+00B7, dimColor, gap 1) instead of a separate '● @name' line above the header.
- ⬜ **[LOW]** `perm-10` — Worker badge circle glyph hardcodes non-darwin '●' (U+25CF); claude-code uses '⏺' (U+23FA) on macOS
    - rust: `lingxi-code/tui/src/components/permissions/worker.rs:13 (BADGE_CIRCLE = '\u{25CF} ' all platforms),123 (test asserts E2 97 8F)`  |  ts: `claude-code/src/constants/figures.ts:4 (BLACK_CIRCLE darwin '⏺' U+23FA else '●'); WorkerBadge.tsx:40 (uses BLACK_CIRCLE)`
    - fix: Make the worker-badge circle platform-dependent: '⏺' (U+23FA) on darwin, '●' (U+25CF) elsewhere, and update the byte-assertion test accordingly.

## pickers  (H=2 M=3 L=9)

- ⬜ **[HIGH]** `resume-old-form-vs-logselector` — Resume picker is the OLD simple list, not the shipped modern LogSelector (search/tree/relative-time)
    - rust: `lingxi-code/tui/src/screens/resume.rs:163 (header "Resume which session?"), :166-180 (flat numbered row list)`  |  ts: `claude-code/src/screens/ResumeConversation.tsx:314 (renders <LogSelector/>); claude-code/src/components/LogSelector.tsx:1266 (header "Resume Session" + "({focusedIndex} of {N})" counter), :1051/:1116 (viewMode "search" type-to-search), :1357 (TreeSelect rows)`
    - fix: Rebuild the resume screen on the LogSelector model: bold suggestion-colored header literal "Resume Session" with a "(idx of N)" counter when the list overflows the visible window, a `/`-activated type-to-search box, and tree/list rows. Drop the legacy "Resume which session?" header (it does not exist in current claude-code).
- ⬜ **[HIGH]** `resume-metadata-absolute-vs-relative` — Resume rows show absolute RFC3339 timestamp + "(N messages)"; shipped shows relative "<time ago> · <branch> · N messages"
    - rust: `lingxi-code/tui/src/screens/resume.rs:51-56 (absolute format_rfc3339_seconds + "(N messages)"), :172-178 (both packed onto title line in [ ] and ( ))`  |  ts: `claude-code/src/utils/format.ts:213-235 (formatLogMetadata: relativeTimeAgo · gitBranch? · "N messages", join ' · '); claude-code/src/components/LogSelector.tsx:139 + :597/:674 (set as option description); claude-code/src/components/design-system/ListItem.tsx:226 (description on own dim line, paddingLeft=2)`
    - fix: Render the title on one line and a separate dim metadata line below it formatted as relativeTimeAgo(short) + optional gitBranch + "N messages", joined with " · " (no brackets, no parens), matching formatLogMetadata.
- ⬜ **[MEDIUM]** `model-header-not-bold-no-subheader` — /model header "Select model" is plain (not bold/colored) and omits the dim sub-header line
    - rust: `lingxi-code/tui/src/screens/model.rs:326 (plain "Select model\n") + :327 ("Search: <q>" instead of sub-header); rendered as plain Text in lingxi-code/tui/src/app.rs:898-904`  |  ts: `claude-code/src/components/ModelPicker.tsx:263 (<Text color="remember" bold>Select model</Text>), :268 (default dim sub-header "Switch between Claude models. Applies to this session and future Claude Code sessions. For other/previous model names, specify with --model.")`
    - fix: Make the "Select model" title bold with the remember/accent color and add a dim sub-header line "Switch between Claude models. Applies to this session and future Claude Code sessions. For other/previous model names, specify with --model." (note: this is the actual snapshot text, not the auditor's quoted variant).
- ⬜ **[MEDIUM]** `model-no-row-descriptions` — /model rows show no per-model description line (shipped Select shows a dim description under each label)
    - rust: `lingxi-code/tui/src/screens/model.rs:348-359 (row = pointer + display_model + " · provider_label" + badge; no description line)`  |  ts: `claude-code/src/utils/model/modelOptions.ts:101/138/186 (per-option description); claude-code/src/components/design-system/ListItem.tsx:226 (description rendered dim, paddingLeft=2, color "inactive")`
    - fix: Add a dim per-row description line (paddingLeft=2, inactive color) below each model label, sourced from the catalog/listing, matching the shared ListItem layout.
- ⬜ **[MEDIUM]** `theme-missing-syntax-status-line` — Theme picker omits the dim "Syntax theme: …" / "Syntax highlighting disabled" status line below the preview
    - rust: `lingxi-code/tui/src/screens/theme.rs:242-264 (no syntax-status line)`  |  ts: `claude-code/src/components/ThemePicker.tsx:252-255 (dim status line: "Syntax theme: <name> (<shortcut> to disable)" / "Syntax highlighting disabled (<shortcut> to enable)" / env-disabled / enabled variants)`
    - fix: Add a dim status line under the preview reporting the active syntax theme ("Syntax theme: <name> (<shortcut> to disable)") or the disabled/enabled state, matching the shipped variants.
- ⬜ **[LOW]** `resume-preview-pane-not-in-shipped` — Rust resume screen renders a bordered Title/Session/Messages/Modified preview pane that the shipped picker does not
    - rust: `lingxi-code/tui/src/screens/resume.rs:184-215 (always-on Round-bordered "Title:/Session:/Messages:/Modified:" key:value box)`  |  ts: `claude-code/src/components/SessionPreview.tsx:142 (preview = <Messages screen="transcript"> transcript); claude-code/src/components/LogSelector.tsx:1227 (preview shown only when viewMode==="preview" && isResumeWithRenameEnabled)`
    - fix: Drop the always-on labeled Title/Session/Messages/Modified box. If a preview is added, gate it behind a Ctrl+V toggle and render an actual message transcript (matching SessionPreview), not a key:value metadata card.
- ⬜ **[LOW]** `resume-footer-hint-wording` — Resume footer hint text differs from shipped Byline shortcut hints
    - rust: `lingxi-code/tui/src/screens/resume.rs:196 ("Up/Down select   Enter resume   Esc cancel")`  |  ts: `claude-code/src/components/LogSelector.tsx:1412 (Byline footer: Ctrl+A/Ctrl+B/Ctrl+W/Ctrl+V preview/Ctrl+R rename/"Type to search"/Esc cancel)`
    - fix: Render the footer as a Byline of the shipped shortcut hints (Type to search, Esc cancel, plus Ctrl+V preview / Ctrl+R rename when those features exist) instead of the fixed "Up/Down select   Enter resume   Esc cancel" sentence.
- ⬜ **[LOW]** `model-no-visible-count-overflow` — /model list renders all rows with no visible-window cap or "and N more…" overflow indicator
    - rust: `lingxi-code/tui/src/screens/model.rs:333-363 (iterates all visible_lines, no window cap, no overflow indicator)`  |  ts: `claude-code/src/components/ModelPicker.tsx:135-136 (visibleCount=min(10,len), hiddenCount), :311 (<Text dimColor>and {hiddenCount} more…</Text>)`
    - fix: Cap visible rows (e.g. 10), render up/down scroll-arrow indicators on the first/last visible rows when overflowing, and add an "and N more…" dim line (paddingLeft 3) for the remainder.
- ⬜ **[LOW]** `model-footer-static-vs-byline` — /model footer is a fixed sentence; shipped renders Byline shortcut hints (Enter confirm / Esc exit) only in standalone
    - rust: `lingxi-code/tui/src/screens/model.rs:364-366 ("Press ↑↓ to navigate · type to search · Enter to select · Esc to go back")`  |  ts: `claude-code/src/components/ModelPicker.tsx:358 (isStandaloneCommand && <Byline><KeyboardShortcutHint shortcut="Enter" action="confirm"/><ConfigurableShortcutHint action="select:cancel" fallback="Esc" description="exit"/></Byline>)`
    - fix: Render the footer as a dim italic Byline of Enter/confirm + Esc/exit shortcut hints; retain a "type to search" token only if LingXi keeps the search feature, and drop the "to navigate"/"go back" wording.
- ⬜ **[LOW]** `theme-subheader-dimmed` — Theme sub-header is dimmed in Rust but rendered bold (non-dim) in shipped
    - rust: `lingxi-code/tui/src/screens/theme.rs:245 (Text(content: SUB_HEADER, color: theme.dim, weight: Bold))`  |  ts: `claude-code/src/components/ThemePicker.tsx:150 (compiled t12 = <Text bold={true}>Choose the text style…</Text>; readable source confirms bold, no dimColor)`
    - fix: Render the theme sub-header bold WITHOUT the theme.dim color so it matches the shipped full-brightness bold text.
- ⬜ **[LOW]** `theme-unselected-rows-dimmed` — Theme picker dims all non-highlighted option rows; shipped renders them in normal foreground
    - rust: `lingxi-code/tui/src/screens/theme.rs:248-249 (unselected rows colored theme.dim; selected theme.suggestion)`  |  ts: `claude-code/src/components/design-system/ListItem.tsx:148-161 (non-focused/non-selected textColor = undefined → default foreground; focused = 'suggestion')`
    - fix: Leave unselected option rows in the default foreground (no theme.dim); color only the highlighted row with the suggestion/accent color.
- ⬜ **[LOW]** `theme-preview-border-round-vs-dashed` — Theme diff preview uses a Round full border; shipped uses dashed top+bottom only
    - rust: `lingxi-code/tui/src/screens/theme.rs:252-259 (BorderStyle::Round full border around the diff preview)`  |  ts: `claude-code/src/components/ThemePicker.tsx:246 (borderTop+borderBottom only, borderLeft/Right=false, borderStyle="dashed", borderColor="subtle")`
    - fix: Wrap the diff preview in a dashed top+bottom-only rule (no left/right borders) with the subtle color, instead of a Round full border.
- ⬜ **[LOW]** `theme-missing-syntax-toggle` — Theme picker is missing the Ctrl+T toggle for syntax highlighting
    - rust: `lingxi-code/tui/src/screens/theme.rs:128-151 (key handler: Up/Down/Enter/Esc/q only; no syntax toggle)`  |  ts: `claude-code/src/components/ThemePicker.tsx:76 + :109 (useKeybinding "theme:toggleSyntaxHighlighting", default ctrl+t, persists syntaxHighlightingDisabled)`
    - fix: Add a ctrl+t binding that toggles syntaxHighlightingDisabled (persist to settings) and updates the preview + the syntax-status line.
- ⬜ **[LOW]** `theme-footer-wording` — Theme footer is a fixed sentence; shipped uses Byline Enter/select + Esc/cancel hints
    - rust: `lingxi-code/tui/src/screens/theme.rs:261 ("Up/Down select   Enter apply   Esc cancel")`  |  ts: `claude-code/src/components/ThemePicker.tsx:300 (dim italic <Byline> Enter=select + Esc=cancel; "Press <key> again to exit" when exit pending)`
    - fix: Render the footer as a dim italic Byline of Enter/select + Esc/cancel hints (note "select", not "apply"); show the "Press <key> again to exit" message when ctrl+c/d is pending.

## prompt-input-core  (H=2 M=3 L=2)

- ✅ **[HIGH]** `PIC-01` — Footer always renders "shift + ⏎ for newline" row; claude-code never shows it in the resting footer
    - rust: `lingxi-code/tui/src/components/prompt_input/footer.rs:127-130 (unconditional 2nd hint row: "? for shortcuts" + "shift + ⏎ for newline")`  |  ts: `claude-code/src/components/PromptInput/utils.ts:17-32 (getNewlineInstructions), consumed only at PromptInput/PromptInputHelpMenu.tsx:10,232; PromptInputFooter.tsx + PromptInputFooterLeftSide.tsx never render any 'for newline' text`
    - fix: Delete the permanent "shift + ⏎ for newline" row from the resting footer (footer.rs:129). The newline instruction belongs only on the help surface (LingXi already has it at screens/help.rs:144). If ever surfaced, make it terminal-dependent per getNewlineInstructions (Apple_Terminal/darwin or shift-enter keybinding installed => 'shift + ⏎ for newline'; else hasUsedBackslashReturn => '\⏎ for newline' : 'backslash (\) + return (⏎) for newline').
- ✅ **[HIGH]** `PIC-02` — "? for shortcuts" shown unconditionally; claude-code suppresses it when input non-empty / vim INSERT / searching / custom status line
    - rust: `lingxi-code/tui/src/components/prompt_input/footer.rs:128 ("? for shortcuts" unconditional; is_empty only gates placeholder at 106-110)`  |  ts: `claude-code/src/components/PromptInput/PromptInput.tsx:2274 (suppressHint={input.length>0}); PromptInputFooter.tsx:122; PromptInputFooterLeftSide.tsx:197 (showHint=!suppressHint&&!showVim), 409-413 (push only if parts/tasksPart/modePart empty AND showHint), 464-466 (return null otherwise)`
    - fix: Thread a suppress_hint signal (= prompt non-empty) plus is_searching and vim-INSERT-shown into the footer, and only render "? for shortcuts" when the buffer is empty, no mode/task/team part is shown, vim INSERT indicator is not shown, and history search is inactive — matching showHint = !(input.length>0 || statusLineShouldDisplay || isSearching) && !showVim.
- ⬜ **[MEDIUM]** `PIC-03` — Footer renders "-- NORMAL --" / "-- VISUAL --" / "-- VISUAL LINE --"; claude-code only ever shows "-- INSERT --"
    - rust: `lingxi-code/tui/src/components/prompt_input/footer.rs:34-39 + wired at 113-118; vim.rs:1565-1571 (mode_indicator emits NORMAL/INSERT/VISUAL)`  |  ts: `claude-code/src/components/PromptInput/PromptInputFooterLeftSide.tsx:170 (showVim requires vimMode==='INSERT' && !isSearching), 191 (renders '-- INSERT --' only, else null)`
    - fix: Only render the vim mode label when vim_enabled AND vim_mode == Insert AND not searching (text '-- INSERT --'); suppress the label entirely for Normal/Visual/VisualLine.
- ⬜ **[MEDIUM]** `PIC-05` — Large / multi-line text paste inserted raw; claude-code collapses to a "[Pasted text #N +M lines]" pill
    - rust: `lingxi-code/tui/src/components/prompt_input/image_paste.rs:172 (only image segs pilled), 173-176 (text inserted verbatim), 241-244 (deferred comment)`  |  ts: `claude-code/src/components/PromptInput/PromptInput.tsx:1214-1239 (onTextPaste pill when text.length>PASTE_THRESHOLD || numLines>maxLines, maxLines=min(rows-10,2)); claude-code/src/history.ts:51-55 (formatPastedTextRef => '[Pasted text #N]' / '[Pasted text #N +M lines]')`
    - fix: In apply_paste_block, when the inserted text exceeds the char threshold OR exceeds maxLines (min(rows-10,2)), store the content in a pasted-contents map keyed by an incrementing id and insert '[Pasted text #N]' / '[Pasted text #N +M lines]' instead of the raw text (port formatPastedTextRef + getPastedTextRefNumLines), expanding at submit.
- ⬜ **[MEDIUM]** `PIC-07` — Loading-state footer hint "esc to interrupt" is missing
    - rust: `lingxi-code/tui/src/components/prompt_input/footer.rs:127-130 (static hint, no isLoading); components/spinner.rs:432-441 (spinner line has no interrupt hint); grep: no 'esc to interrupt' in tui/src`  |  ts: `claude-code/src/components/PromptInput/PromptInputFooterLeftSide.tsx:375,382-384 (hintParts appended when showHint) + 506-507 (getSpinnerHintParts pushes esc/interrupt when isLoading)`
    - fix: Thread an is_loading signal into the footer-left and, when loading and the hint is not suppressed, render 'esc to interrupt' (KeyboardShortcutHint shortcut=esc action=interrupt) in place of '? for shortcuts'.
- ⬜ **[LOW]** `PIC-10` — Permission-mode footer indicator ("{symbol} {mode} on (shift+tab to cycle)") not rendered in the footer
    - rust: `lingxi-code/tui/src/components/status_line.rs:70-101 (mode rendered as short mode_label in top status row); screens/repl.rs:215 (permission_mode passed to StatusLine, not the footer); footer.rs has no permission-mode part`  |  ts: `claude-code/src/components/PromptInput/PromptInputFooterLeftSide.tsx:348-355 (modePart '{symbol} {title lowercased} on (shift+tab to cycle)' in getModeColor); utils/permissions/PermissionMode.ts:62-139 (permissionModeSymbol '⏵⏵', permissionModeTitle, getModeColor)`
    - fix: Add a footer-left permission-mode part for non-default modes: '{permissionModeSymbol} {title-lowercased} on' + a dim '(shift+tab to cycle)' KeyboardShortcutHint, colored with getModeColor — or document the top-status-row label as the intended LingXi equivalent.
- ⬜ **[LOW]** `PIC-14` — History-search input renders no cursor and is placed above the prompt rather than in the footer-left
    - rust: `lingxi-code/tui/src/components/prompt_input/history_search.rs:169-174 (static dim Text 'label query', no cursor); mounted above the prompt at screens/repl.rs:268-274`  |  ts: `claude-code/src/components/PromptInput/HistorySearchInput.tsx:30 (TextInput showCursor={true} cursorOffset={value.length} dimColor); PromptInputFooterLeftSide.tsx:180 (isSearching && <HistorySearchInput/> inside footer-left)`
    - fix: Render a block cursor at the end of the search query (match showCursor=true), and relocate the search input into the footer-left during ctrl-r to match claude-code's placement (replacing the mode indicator).

## repl-root-scroll  (H=1 M=6 L=1)

- ⬜ **[HIGH]** `RRS-02` — Esc does not interrupt a streaming turn in the main REPL (claude-code's chat:cancel)
    - rust: `lingxi-code/tui/src/root.rs:327 ('chat:cancel' => None) + root.rs:135-178 (map_iocraft_key, no Esc arm) + root.rs:1126 (Esc only consumed for viewing_teammate); only app.rs:494 Cancel (Ctrl+C) interrupts`  |  ts: `claude-code/src/keybindings/defaultBindings.ts:66 (escape: 'chat:cancel'); hooks/useCancelRequest.ts:97-101,150-167 (chat:cancel → onCancel when canCancelRunningTask, not fullscreen-gated)`
    - fix: In handle_live_key, after the screen/overlay/teammate traps and before editor input, treat KeyCode::Esc as a turn interrupt when st.in_flight_turn.is_some() (mirror the Ctrl+C Cancel branch: cancel the token, push the interrupt marker). Do NOT also render '(esc to interrupt)' in the spinner — claude-code only shows that for teammates.
- ⬜ **[MEDIUM]** `RRS-01` — PageUp/PageDown scroll a FULL viewport in Rust, but HALF a viewport in claude-code
    - rust: `lingxi-code/tui/src/app.rs:1314-1315 (ScrollDir::PageUp => cur + viewport_height as i64; PageDown => cur - viewport_height as i64)`  |  ts: `claude-code/src/components/ScrollKeybindingHandler.tsx:451,459 (d = ±Math.max(1, Math.floor(getViewportHeight()/2))); defaultBindings.ts:198-199 (pageup/pagedown→scroll:pageUp/pageDown); gated fullscreen-only at REPL.tsx:4561 / fullscreen.ts:128`
    - fix: In scroll_with_viewport, for ScrollDir::PageUp/PageDown use a step of viewport_height.max(2)/2 (i.e. max(1, viewport_height/2)) to match Math.floor(getViewportHeight()/2) — a less-style half-page jump that keeps half a screen of overlap.
- ⬜ **[MEDIUM]** `RRS-03` — Streaming spinner row omits elapsed-time and token-count status that claude-code shows
    - rust: `lingxi-code/tui/src/components/spinner.rs:432-441 (line = '{frame} {verb}…'; only Text rendered, no timer/tokens)`  |  ts: `claude-code/src/components/Spinner/SpinnerAnimationRow.tsx:19 (SHOW_TOKENS_AFTER_MS=30_000), :179 (wantsTimerAndTokens), :190-225 (builds '(<elapsed> · <N> tokens)' dim Byline); rendered via SpinnerWithVerb at REPL.tsx:4587, not fullscreen-gated`
    - fix: Thread the turn's elapsed time and streamed token count into SpinnerWithVerb and append a dim '(<formatDuration> · <N> tokens)' suffix once elapsed > 30s (or always in verbose), matching SpinnerAnimationRow's parts/Byline. Keep it dim and parenthesized.
- ⬜ **[MEDIUM]** `RRS-05` — Empty-prompt j/k/g/G scroll the transcript instead of typing the character
    - rust: `lingxi-code/tui/src/root.rs:153-164 (Char('j')/'k'/'g'/'G' with prompt_empty → ScrollStep); same in lingxi-code/tui/src/events/keymap.rs:177-188`  |  ts: `claude-code/src/components/ScrollKeybindingHandler.tsx:580-582 (isActive: isActive && isModal), :939-959 (modalPagerAction g/G/j/k), :566 (comment 'g/G → printable chars'); defaultBindings.ts Chat context 63-97 binds no j/k/g/G`
    - fix: Remove the empty-prompt j/k/g/G → ScrollStep mappings from both the live (root.rs) and legacy (keymap.rs) main-REPL key paths so those letters InsertChar like any printable. Route g/G/j/k pager nav only into a future ctrl+o transcript-modal reading view.
- ⬜ **[MEDIUM]** `RRS-06` — Ctrl+O (app:toggleTranscript) does nothing; compact-boundary hint references a non-functional shortcut
    - rust: `lingxi-code/tui/src/components/messages/compact_boundary.rs:12 ('Conversation compacted (ctrl+o for history)'); no ctrl+o handler anywhere (root.rs:316 notes app:toggleTranscript has no live KeyAction)`  |  ts: `claude-code/src/keybindings/defaultBindings.ts:44 (ctrl+o→app:toggleTranscript); hooks/useGlobalKeybindings.tsx:124,188-190 (unconditional setScreen('transcript')); screens/REPL.tsx:4392-4398 (transcript renders for non-fullscreen via dump-to-scrollback)`
    - fix: Either implement a ctrl+o transcript/history toggle (scrollable read-only verbose dump of the full message log, exit on ctrl+o/esc, ctrl+e show-all), or, if intentionally unsupported, drop/replace the misleading '(ctrl+o for history)' substring in compact_boundary.rs and any other ctrl+o hints (CompactSummary equivalents).
- ⬜ **[MEDIUM]** `RRS-07` — Ctrl+D is unbound in the live REPL (claude-code's app:exit double-press)
    - rust: `lingxi-code/tui/src/root.rs:135-178 + 313-346 (no Char('d')+CONTROL arm; only ctrl+c→Cancel); legacy classify() at events/keymap.rs:40 not used by live path`  |  ts: `claude-code/src/keybindings/defaultBindings.ts:41 (ctrl+d→app:exit); hooks/useExitOnCtrlCD.ts:80-92 (app:exit → Ctrl-D double-press exit); hooks/useDoublePress.ts:6 (800ms)`
    - fix: Add a Char('d')+CONTROL arm to the live key path that arms/triggers the same double-press exit flow as Ctrl+C (keyName 'Ctrl-D' in the hint), mirroring useExitOnCtrlCD. Note Ctrl+D should NOT interrupt a turn — it only exits via double-press.
- ⬜ **[MEDIUM]** `RRS-08` — Ctrl+C exit confirmation is a pushed scrollback message instead of a footer hint, with different text and window
    - rust: `lingxi-code/tui/src/app.rs:514-518 (scrollback '^C (press Ctrl-C again or type /exit to quit)') + :500 ('^C interrupted by user') + :509 (SIGINT_WINDOW_SECS=2)`  |  ts: `claude-code/src/components/PromptInput/PromptInputFooterLeftSide.tsx:147-150 ('Press {key} again to exit' footer hint); PromptInput.tsx:2186 (setExitMessage); hooks/useDoublePress.ts:6 (800ms); utils/messages.ts:207 ('[Request interrupted by user]')`
    - fix: Render the 'Press Ctrl-C again to exit' confirmation as a transient footer hint cleared after the window (not a scrollback line), shorten the double-press window to 800ms, and emit the '[Request interrupted by user]' marker (a user-role transcript message) on turn interrupt instead of '^C interrupted by user'.
- ⬜ **[LOW]** `RRS-04` — Spinner is missing the marginTop blank line above it
    - rust: `lingxi-code/tui/src/components/spinner.rs:437-441 (View{Text} no margin); lingxi-code/tui/src/screens/repl.rs:230-244 (spinner mounted directly after VirtualMessageList, no spacer)`  |  ts: `claude-code/src/components/Spinner/SpinnerAnimationRow.tsx:226 (<Box ref … marginTop={1} …>)`
    - fix: Add margin_top: 1 to the SpinnerWithVerb root View (or render an empty spacer row above it in repl.rs) to match marginTop={1}.

## spinner-status  (H=3 M=3 L=2)

- ⬜ **[HIGH]** `SS-01` — Built-in 'model cwd $cost ctx% mode' status row is rendered unconditionally; claude-code shows no default status line (command-only)
    - rust: `lingxi-code/tui/src/components/status_line.rs:175-192 (StatusLine renders built-in format_status_line row when custom is None) + lingxi-code/tui/src/screens/repl.rs:210-223 (mounted at top of column) + repl.rs:162 (status_line_text default None)`  |  ts: `claude-code/src/components/StatusLine.tsx:30-35 (statusLineShouldDisplay = settings?.statusLine !== undefined) + StatusLine.tsx:314-316 (render: statusLineText ? <Ansi> : isFullscreenEnvEnabled() ? <Text> </Text> : null) + components/PromptInput/PromptInputFooter.tsx:141 (StatusLine mounted in footer, gated by statusLineShouldDisplay)`
    - fix: When no `statusLine` command is configured (custom is None), render nothing (empty View/null) to match statusLineShouldDisplay. If a default built-in row is an intentional LingXi feature, at minimum relocate it from the top of the column to the footer near the prompt, matching claude-code's PromptInputFooter placement.
- ✅ **[HIGH]** `SS-02` — Spinner glyph and verb are hardcoded Color::Cyan; should be the theme 'claude' accent (Claude orange)
    - rust: `lingxi-code/tui/src/components/spinner.rs:437-441 (Text color: Color::Cyan; no theme threaded into SpinnerWithVerbProps) + theme.rs:204/244/284 (claude accent already present in palette)`  |  ts: `claude-code/src/components/Spinner.tsx:211-213 (defaultColor='claude'; messageColor = overrideColor ?? defaultColor) + Spinner/SpinnerGlyph.tsx:71 (<Text color={messageColor}>) + utils/theme.ts:118 (claude:'rgb(215,119,87)')`
    - fix: Thread the active Theme into SpinnerWithVerb (as StatusLine already does) and replace Color::Cyan with theme.claude so the glyph+verb render in the Claude accent: rgb(215,119,87) dark, rgb(255,153,51) colorblind, ansi:redBright for ANSI themes.
- ✅ **[HIGH]** `SS-03` — Spinner verb rotates every 4s through the pool; claude-code picks ONE verb on mount and keeps it for the whole turn
    - rust: `lingxi-code/tui/src/components/spinner.rs:346 (VERB_ROTATE_MS=4000) + spinner.rs:418-430 (use_future increments verb index every VERB_ROTATE_MS) + spinner.rs:432-436 (renders pool_verb_at_index(verb.get()))`  |  ts: `claude-code/src/components/Spinner.tsx:166 (const [randomVerb] = useState(() => sample(getSpinnerVerbs()))) + Spinner.tsx:169-171 (leaderVerb=...??randomVerb; message=effectiveVerb+'…'; never re-sampled)`
    - fix: Keep the random-on-mount pick (initial_verb_index/live_seed) but remove the VERB_ROTATE_MS use_future so the verb stays fixed for the spinner's lifetime; a fresh random verb is selected only on the next mount (next turn).
- ⬜ **[MEDIUM]** `SS-05` — No blank line (marginTop={1}) above the spinner row
    - rust: `lingxi-code/tui/src/components/spinner.rs:437-441 (View flex Row, no top margin) + lingxi-code/tui/src/screens/repl.rs:240-244 (SpinnerWithVerb rendered inline after VirtualMessageList, no leading blank row)`  |  ts: `claude-code/src/components/Spinner/SpinnerAnimationRow.tsx:226 (<Box ... marginTop={1} width="100%">) + Spinner.tsx:231,245 (idle/teammate fallbacks also marginTop={1})`
    - fix: Add a one-row top margin above the spinner — either a leading blank Text/View row in repl.rs before SpinnerWithVerb, or margin_top:1 on the spinner's container if iocraft supports it — matching marginTop={1}.
- ⬜ **[MEDIUM]** `SS-06` — prefersReducedMotion setting is not honored; claude-code swaps to a slow-flashing dot and stops shimmer
    - rust: `lingxi-code/tui/src/components/spinner.rs:395-442 (always renders animated SPINNER_FRAMES; no reduced-motion branch) + no reduced_motion/prefersReducedMotion occurrence anywhere in lingxi-code/tui`  |  ts: `claude-code/src/components/Spinner.tsx:98 (reducedMotion = settings.prefersReducedMotion ?? false) + Spinner/SpinnerGlyph.tsx:8-9,36-47 (REDUCED_MOTION_DOT '●', 2000ms cycle, isDim toggle) + SpinnerAnimationRow.tsx:103 (useAnimationFrame(reducedMotion?null:50) disables clock)`
    - fix: Read prefersReducedMotion from settings; when true, render a static '●' (U+25CF) in theme.claude that toggles dim on a 2000ms cycle (floor(time/1000)%2===1) and suppress the frame/shimmer animation, matching SpinnerGlyph's reduced-motion branch.
- ⬜ **[MEDIUM]** `SS-07` — Spinner status suffix '(esc to interrupt · {elapsed} · ↓{N} tokens)' (+ thinking/effort) is entirely missing
    - rust: `lingxi-code/tui/src/components/spinner.rs:432-441 (renders only '{frame} {verb}…'; no timer, token count, thinking, or effort byline)`  |  ts: `claude-code/src/components/Spinner/SpinnerAnimationRow.tsx:162-172 (timerText=formatDuration; tokensText=`${figures.arrowDown} ${tokenCount} tokens`; thinkingText 'thinking{effortSuffix}'/'thought for Ns') + :179 (wantsTimerAndTokens = verbose||hasRunningTeammates||elapsed>30_000) + byline render '(' + <Byline>{parts}</Byline> + ')' (note: 'esc to interrupt' is the teammate-only path, not the leader byline)`
    - fix: Track turn start time and response length; once verbose || elapsed>30s render a dim byline '(' + formatDuration(elapsed) + ' · ↓{formatNumber(tokens)} tokens' + ')' joined by ' · ', and show 'thinking{effort}' / 'thought for Ns' whenever a thinking status is active. Drop the 'esc to interrupt' phrasing for the non-teammate path.
- ⬜ **[LOW]** `SS-08` — Spinner verb ignores the active task's activeForm/subject override
    - rust: `lingxi-code/tui/src/components/spinner.rs:374-382 (SpinnerWithVerbProps has only frame_override/verb_override; no todo input) + spinner.rs:432-436 (always renders pool_verb_at_index of the random index)`  |  ts: `claude-code/src/components/Spinner.tsx:162 (currentTodo = first non-pending/non-completed task) + Spinner.tsx:169 (leaderVerb = overrideMessage ?? currentTodo?.activeForm ?? currentTodo?.subject ?? randomVerb)`
    - fix: Thread the current tasks/todo list into SpinnerWithVerb; when an in-progress todo exists, use its activeForm ?? subject as the verb (falling back to the random verb), matching the leaderVerb resolution order.
- ⬜ **[LOW]** `SS-09` — Spinner frames hardcode the darwin variant; Linux/ghostty substitutions are absent
    - rust: `lingxi-code/tui/src/components/spinner.rs:26 (SPINNER_FRAMES hardcoded darwin: ·✢✳✶✻✽ + reverse) + spinner.rs:8-11 (doc: 'locked here as the darwin variant')`  |  ts: `claude-code/src/components/Spinner/utils.ts:4-11 (getDefaultCharacters: xterm-ghostty → ✽→*; darwin → ✽; else Linux → ✳→*) consumed by Spinner.tsx:40-41 and SpinnerGlyph.tsx:6-7`
    - fix: Select the frame set at runtime like getDefaultCharacters before building the forward+reverse cycle: if TERM=xterm-ghostty substitute the 6th glyph ✽→*, else if not macOS (cfg!(target_os) != "macos") substitute the 3rd glyph ✳→*.

## stats-usage-settings  (H=4 M=7 L=3)

- ⬜ **[HIGH]** `stats-heatmap-grid` — Activity heatmap is a 1-line compact strip instead of the GitHub-style 7-row week grid with month/day labels
    - rust: `lingxi-code/tui/src/screens/stats.rs:668-687 (heatmap); rendered into Overview via overview_lines stats.rs:764-766`  |  ts: `claude-code/src/utils/heatmap.ts:39-166 (generateHeatmap: month line :138, 7 day rows :145-150, Mon/Wed/Fri labels :147)`
    - fix: Reimplement heatmap() to produce the 7-row x N-week grid: anchor the last column on the current week's Sunday, walk back (width-1)*7 days, fill grid[day][week] with heatmap_char(intensity), pad future days with ' ', emit the '    '+monthLabels line (padEnd(floor(width/uniqueMonths))), the 7 weekday rows with 'Mon'/'Wed'/'Fri' (padEnd(3)) labels on day 1/3/5 and '   ' otherwise, then the legend. width = min(52, max(10, terminalWidth-4)). Requires a real calendar walk (today, weekday-of, date arithmetic) the current date-string aggregation does not do.
- ⬜ **[HIGH]** `stats-model-name-raw` — Favorite model and per-model rows show the raw model id instead of the friendly display name
    - rust: `lingxi-code/tui/src/screens/stats.rs:767-768 (favorite) and 815 (per-model row)`  |  ts: `claude-code/src/components/Stats.tsx:446,864 (renderModelName) + claude-code/src/utils/model/model.ts:349-384,395-415`
    - fix: Map the model id through LingXi's public-display-name table (provider catalog / model resolution) before rendering it in favorite_model output (stats.rs:768) and the Models-tab rows (stats.rs:815).
- ⬜ **[HIGH]** `stats-overview-missing-fields` — Overview tab is missing Longest session, Longest/Current streak, Active-days /N range, and the fun factoid
    - rust: `lingxi-code/tui/src/screens/stats.rs:762-783 (overview_lines: only Favorite/Total tokens/Sessions/Active days/Most active day)`  |  ts: `claude-code/src/components/Stats.tsx:458-512 (Longest session/streaks/Active days /rangeDays), 573-576 (factoid)`
    - fix: Add Longest session (needs session-duration tracking), longest/current consecutive-active-day streak computation, the '/rangeDays' suffix on Active days, and a factoid generator (BOOK_COMPARISONS/TIME_COMPARISONS). These need extra aggregation (session durations, streaks, total-days) absent from StatsData. The factoid and streaks are all-time, so do not depend on the date-range work.
- ⬜ **[HIGH]** `stats-date-range-selector` — Date-range selector (All time / Last 7 days / Last 30 days) and the 'r' cycle key are entirely absent
    - rust: `lingxi-code/tui/src/screens/stats.rs:36-39 (documented omission), 574-588 (handle_stats_key has no 'r')`  |  ts: `claude-code/src/components/Stats.tsx:48-57, 215-217 ('r' key), 315-355 (DateRangeSelector), 438 + 776 (rendered in both tabs)`
    - fix: Add a date-range state + a selector line ('All time · Last 7 days · Last 30 days', selected bold+claude) at the top of both tabs, and an 'r' key in handle_stats_key cycling all->7d->30d, filtering daily_messages/daily_model_tokens/model_usage to the window. Requires range-filtered aggregation; the heatmap stays all-time (TS always passes allTimeStats to generateHeatmap).
- ⬜ **[MEDIUM]** `stats-footer-text` — Stats footer text differs: 'Tab to switch · Esc to close' vs 'Esc to cancel · r to cycle dates · ctrl+s to copy'
    - rust: `lingxi-code/tui/src/screens/stats.rs:72 (FOOTER const)`  |  ts: `claude-code/src/components/Stats.tsx:295 ('Esc to cancel · r to cycle dates · ctrl+s to copy{copyStatus}')`
    - fix: Align the literal to 'Esc to cancel · r to cycle dates · ctrl+s to copy' once r/ctrl+s land; if keeping the deferral, at minimum use 'Esc to cancel' to match the verb and drop the Rust-invented 'Tab to switch' (no TS equivalent).
- ⬜ **[MEDIUM]** `stats-models-row-bullet` — Models-tab per-model rows lack the leading bullet glyph and bold/dim styling
    - rust: `lingxi-code/tui/src/screens/stats.rs:814-820 (models_lines: '{model} ({pct}%)' no bullet)`  |  ts: `claude-code/src/components/Stats.tsx:888 (<Text>{figures.bullet} {bold name} {subtle (pct%)}</Text>)`
    - fix: Prefix each model row with the figures.bullet glyph '● ' to match; bold the model name and dim the '(pct%)' and the In/Out line once per-segment coloring is wired into the stats render path.
- ⬜ **[MEDIUM]** `stats-models-scroll-hint` — Models tab has no '{start}-{end} of {N} models (↑↓ to scroll)' hint and uses a different two-column / scroll model
    - rust: `lingxi-code/tui/src/screens/stats.rs:789-823 (single-column models_lines) + scroll.rs:226-240 (generic '↑ N more · ↓ N more')`  |  ts: `claude-code/src/components/Stats.tsx:767-773 (two-column 4-visible window), 801 (scroll-hint line '{start}-{end} of {N} models (↑↓ to scroll)')`
    - fix: Add a models-specific scroll-hint line with the '{start}-{end} of {N} models (↑↓ to scroll)' format and up/down arrow indicators when >4 models. The two-column layout is hard in the single-string oracle; at minimum match the hint text and the 4-visible window semantics.
- ⬜ **[MEDIUM]** `stats-chart-sparkline-vs-asciichart` — Tokens-per-Day chart is a single 1-line sparkline instead of the multi-series asciichart with y-axis, x-axis date labels, and a colored legend
    - rust: `lingxi-code/tui/src/screens/stats.rs:624-655 (sparkline), 796-811 (single favorite-model series, no axes/legend)`  |  ts: `claude-code/src/components/Stats.tsx:811 (chart+xAxisLabels+legend render), 940-1018 (generateTokenChart: asciichart, top-3 :976, y-axis k/M :998-1008), 1019+ (generateXAxisLabels)`
    - fix: Port generateTokenChart: an 8-row asciichart-style chart for the top-3 models with padStart(6) y-axis token labels, an x-axis date-label line (generateXAxisLabels), and a legend line of colored bullets + display names. Significant work; at minimum add the x-axis date labels and the legend line below the existing sparkline.
- ⬜ **[MEDIUM]** `stats-empty-state-color` — Empty-state and value strings are monochrome; TS colors the empty line 'warning' and headline values 'claude'
    - rust: `lingxi-code/tui/src/app.rs:964-971 (Text(content: line) with no color — whole stats body monochrome)`  |  ts: `claude-code/src/components/Stats.tsx:245 (empty color=warning), 445-447/453/463/480/500 (values color=claude, favorite bold)`
    - fix: Move the stats render from a single plain-string oracle to per-segment colored Text (or post-color known lines in app.rs): color the empty-state line warning/yellow and the metric values claude-orange (bold for favorite model), dim the '/N' suffixes. Requires structured render output rather than plain lines.
- ⬜ **[MEDIUM]** `settings-tab-order-and-missing-default` — Settings overlay tab set/order differs: TS is Status→Config→Usage with a command-driven default tab; Rust is Config→Settings→Status→Usage
    - rust: `lingxi-code/tui/src/screens/settings/mod.rs:69-76 (all(): Config,Settings,Status,Usage) + 17-32 (default-tab hook deferred)`  |  ts: `claude-code/src/components/Settings/Settings.tsx:20 (defaultTab prop), 29 (useState(defaultTab)), 104-114 (order status,config,usage; gates gated by external==='ant')`
    - fix: Reorder the visible tabs to Status, Config, Usage to match TS (keep the Rust-only Settings/provenance tab if desired, ideally after Usage), and wire the open path to honor a default tab so /status opens on Status and /config opens on Config.
- ⬜ **[MEDIUM]** `settings-status-missing-mcp-and-setting-sources` — Status tab omits the 'Setting sources' row TS lists and adds non-parity Messages/Started rows; the 'per-MCP-server rows' claim is REFUTED
    - rust: `lingxi-code/tui/src/screens/settings/status.rs:37 (Messages, non-parity), 38-41 (MCP summary — matches TS summary), 44 (Started, non-parity); no Setting-sources row`  |  ts: `claude-code/src/utils/status.tsx:89-114 (buildMcpProperties is a SUMMARY), 126+ (buildSettingSourcesProperties) + Status.tsx:52 (secondary section)`
    - fix: Add a 'Setting sources' row enumerating the loaded settings files (mirroring buildSettingSourcesProperties). Do NOT add a per-server MCP list — TS itself only shows a summary, so the existing aggregate row is fine. Drop or relabel the non-parity Messages/Started rows if strict parity is desired.
- ⬜ **[LOW]** `stats-heatmap-legend-indent` — Heatmap legend lacks the 4-space indent (and is uncolored); the 'different separator' claim is wrong
    - rust: `lingxi-code/tui/src/screens/stats.rs:681-686 ('Less {} {} {} {} More' no indent)`  |  ts: `claude-code/src/utils/heatmap.ts:153-163 (lines.push(''); '    Less '+...+' More', claudeOrange glyphs)`
    - fix: Prefix the legend with '    ' (4 spaces) and a leading blank line to match heatmap.ts; color the four glyphs claude-orange (#da7756) once per-segment coloring is available. The separator already matches (single space) — do not change it.
- ⬜ **[LOW]** `stats-loading-spinner` — Loading state shows a static text line instead of an animated spinner + 'Loading your Claude Code stats…'
    - rust: `lingxi-code/tui/src/screens/stats.rs:62-63 (LOADING_LINE), 852-854 (rendered static)`  |  ts: `claude-code/src/components/Stats.tsx:96-101 ('<Spinner/><Text> Loading your Claude Code stats…</Text>')`
    - fix: Use the existing TUI spinner component for the stats loading state and align the literal toward 'Loading your Claude Code stats…' (or keep the more descriptive text but add the animated spinner).
- ⬜ **[LOW]** `settings-status-label-bold` — Status row labels are not bold; TS renders each 'Label:' in bold
    - rust: `lingxi-code/tui/src/screens/settings/status.rs:58-67 (whole body one DIM Text block)`  |  ts: `claude-code/src/components/Settings/Status.tsx:196 (<Text bold>{label}:</Text>), 215 (bold 'System Diagnostics'), 239 (figures.warning color=error)`
    - fix: Render each row as bold label + normal-weight value (structured rows rather than one dim string). Optionally add the System Diagnostics section (bold header + warning-icon lines) when diagnostics exist.

## theme-colors-designsystem  (H=1 M=1 L=0)

- ✅ **[HIGH]** `theme-01` — Streaming spinner glyph/verb rendered cyan instead of theme 'claude' (orange) accent
    - rust: `lingxi-code/tui/src/components/spinner.rs:439 (Text(content: line, color: Color::Cyan)); mounted at lingxi-code/tui/src/screens/repl.rs:240-241; theme.claude defined but unused at lingxi-code/tui/src/theme.rs:204`  |  ts: `claude-code/src/components/Spinner.tsx:211-213 (defaultColor='claude'); claude-code/src/components/Spinner/SpinnerGlyph.tsx:69-71 (<Text color={messageColor}>{spinnerChar}); claude-code/src/components/Spinner/SpinnerAnimationRow.tsx:227-228; claude-code/src/utils/theme.ts:118 (claude:'rgb(215,119,87)')`
    - fix: Add a theme (or AppState.theme) prop to SpinnerWithVerbProps, thread it from repl.rs:241, and color the spinner Text with theme.claude instead of the literal Color::Cyan, matching Spinner.tsx defaultColor='claude'. The static base color is theme.claude; the shimmer/stalled animation is an optional follow-up.
- ⬜ **[MEDIUM]** `theme-02` — 'auto' theme setting always resolves to Dark — ignores $COLORFGBG and terminal background
    - rust: `lingxi-code/tui/src/theme.rs:112-117 (ThemeSetting::Auto => ThemeName::Dark unconditionally); picker offers 'auto' at lingxi-code/tui/src/theme.rs:80-88 and lingxi-code/tui/src/screens/theme.rs:279`  |  ts: `claude-code/src/utils/systemTheme.ts:42-47 (resolveThemeSetting -> getSystemThemeName), :24-29 (seed from detectFromColorFgBg), :109-119 (COLORFGBG parse: bg<=6||==8 dark, else light); claude-code/src/components/design-system/ThemeProvider.tsx:53`
    - fix: Implement the synchronous $COLORFGBG branch in ThemeSetting::Auto.resolve() (or a resolve_with_env helper): read COLORFGBG, split on ';', parse the last component as int 0..=15, dark if <=6 or ==8 else light, fall back to Dark when absent/unparseable. OSC-11 round-trip is an optional later addition; default theme stays 'dark' so only explicit-auto users change.


## TOTALS: H=29 M=66 L=56  (done so far: 7)


## needs_binary_check

- `diff-06` [diff-markdown-syntax] Diff has a 100-line truncation footer that claude-code's diff renderer does not have
- `md-04` [diff-markdown-syntax] owner/repo#NNN issue references are not linkified
- `perm-07` [permissions] EnterPlanMode permission dialog is entirely missing
- `PIC-04` [prompt-input-core] Placeholder is hardcoded None — the input-empty placeholder never renders
- `PIC-06` [prompt-input-core] Bash mode ("!" glyph + "! for bash mode" footer hint) never shown — footer mode hardcoded to Prompt
- `PIC-08` [prompt-input-core] Footer states "Pasting text…" and "Press {key} again to exit" not rendered
- `PIC-11` [prompt-input-core] PromptInputStashNotice ("› Stashed (auto-restores after submit)") not rendered
- `PIC-12` [prompt-input-core] Queued-commands preview above the prompt not rendered
- `PIC-13` [prompt-input-core] Sandbox-violation footer hint ("⧈ Sandbox blocked N operations · ctrl+o for details · /sandbox to disable") not rendered
- `RRS-09` [repl-root-scroll] No 'N new messages / Jump to bottom' pill when scrolled up and new content arrives
- `RRS-10` [repl-root-scroll] No sticky-prompt header showing the current turn's user prompt while scrolled up
- `model-current-badge-text-vs-tick` [pickers] Selected/current model marked with "(current)" text instead of a success-colored tick (figures.tick)
- `model-no-effort-indicator` [pickers] /model picker omits the reasoning-effort indicator line shown for effort-capable models
- `output-style-picker-missing` [pickers] No /output-style picker exists in LingXi (shipped has a "Preferred output style" picker)
- `settings-usage-flat-cost-vs-ratelimit` [stats-usage-settings] Usage tab shows flat cumulative cost instead of the rate-limit utilization bars TS renders
- `help-7` [help-doctor] '/keybindings to customize' row shown unconditionally vs TS feature-gate
- `help-8` [help-doctor] Slash-command list is a curated 19-item subset in custom order with in-tree descriptions, vs TS full alphabetical built-in catalog
- `theme-04` [theme-colors-designsystem] No 256-color / truecolor clamp for tmux and Apple Terminal — colors may render wrong under tmux
- `ma-08` [messages-assistant] SystemAPIError renders unconditionally; claude-code hides attempts < 4 on external builds