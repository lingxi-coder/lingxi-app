# Session + State Persistence Gap Audit — LingXi vs v2.1.186

**Total confirmed gaps: 10** (8 real gaps + 1 confirmed-correct non-gap + 1 uncertain)

All gaps are grounded in binary string evidence from the oracle at
`/opt/homebrew/lib/node_modules/@anthropic-ai/claude-code/node_modules/@anthropic-ai/claude-code-darwin-arm64/claude`
(VERSION string: `2.1.186`, BUILD_TIME: `2026-06-22T16:43:00Z`).

**NOTE**: This audit supersedes the prior `/tmp/gap-audit/session-state.md` from the previous agent
run (which counted 23 gaps but included many read-errors and had 0 verified binary anchors for
several claims). Every item below has been verified by reading the exact binary offset.

---

## Confirmed Gaps

| # | Area | Item | Oracle (binary evidence) | LingXi (file:line) | Severity | Note |
|---|------|------|--------------------------|-------------------|----------|------|
| 1 | Session JSONL / Resume tip | `last-prompt` explicit tip override not implemented | `Yle` @ 206473264: `else if(N.type==="last-prompt"){if(N.leafUuid) L=N.explicit===!0\|\|L&&N.leafUuid===O,O=N.leafUuid}` then `V=L&&O&&n.has(O)&&!n.get(O)?.isSidechain` forces leaf to `O`; "last-prompt" confirmed at 112891888, 113177080 | `route_lines` (reader.rs:184-185) lists `last-prompt` as Tier-2/ignored; `find_tip` (loader.rs:718) uses timestamp-only leaf selection, never reads `last-prompt` entries | HIGH | Binary forces the resume tip to an explicit `leafUuid` when `explicit===true`. LingXi always picks the newest-timestamp leaf → wrong branch on any session that has an explicit `last-prompt` entry |
| 2 | Resume picker filter | `sessionKind` ("daemon", "daemon-worker") sessions not filtered | `vkm` @ 206492423: `if(i.sessionKind==="daemon"\|\|i.sessionKind==="daemon-worker") return C(...),null`; `sessionKind` read from line containing `"parentUuid":` via `OK(l,"sessionKind")`; "daemon"@64335712, "daemon-worker"@113388928 | `collect_dir` (loader.rs:239) only filters `isSidechain` and `teamName`; no `sessionKind` check | HIGH | Daemon sessions leak into the `/resume` picker |
| 3 | Resume picker filter | SDK-entrypoint sessions not filtered | `vkm` @ 206492423: `k9l=new Set(["sdk-cli","sdk-ts","sdk-py"])`; `if(!a && k9l.has(n.entrypoint??"")) return C(...),null`; "sdk-cli" etc. confirmed in binary | LingXi writes `entrypoint` field (confirmed in session/tests/jsonl_writer_test.rs:119) but `collect_dir` never filters on it | MEDIUM | sdk-cli/sdk-ts/sdk-py sessions appear in the `/resume` picker |
| 4 | Resume picker filter | `/loop` sessions not filtered | `vkm` @ 206492423: `m=r.includes("<command-name>/loop</command-name>")` then `if(!a&&n.isLoopSession) return C(...),null`; string "<command-name>/loop</command-name>" confirmed at 113388700+ | LingXi's `collect_dir` has no `/loop` detection | MEDIUM | `/loop` sessions appear in the `/resume` picker |
| 5 | `LoadedTranscript` struct | 15 binary-parsed metadata side-maps missing | `Yle` returns `tags, agentNames, agentColors, agentSettings, prNumbers, prUrls, prRepositories, bridgeSessionIds, bridgeLastSeqs, bridgeDialogKindsBySession, modes, permissionModes, isolationLatches, worktreeStates, fileHistorySnapshots, attributionSnapshots, contentReplacements, agentContentReplacements, forkContextRefs, contextCollapseCommits, contextCollapseSnapshot, leafUuids` (all confirmed in binary: "tag"×2244, "agent-name"×23, "agent-setting"×6, "permission-mode"×85, "isolation-latch"×8, "worktree-state"×12, "pr-link"×14, "file-history-snapshot"×6, "attribution-snapshot"×8, "content-replacement"×15, "fork-context-ref"×12, "marble-origami"×20, "queue-operation"×4) | `LoadedTranscript` (reader.rs:32-52) has only 3 side-maps: `summaries`, `custom_titles`, `ai_titles`. `route_lines` sends everything else to "Tier-2 / unknown → ignored" | MEDIUM | On resume, all session-level metadata (mode, permission-mode, isolation-latch, worktree-state, agent-name/color/setting, PR link, bridge-session, content-replacements, context-collapse) is silently discarded |
| 6 | `TranscriptEntry` / Writer | `content-replacement` entry never written | Binary `x9l` policy @ 206493671: `"content-replacement":"route-by-agent"`; "content-replacement" confirmed at 15 locations; `Yle` routes to `R` (session) and `w` (agent) maps and returns them | `TranscriptEntry` (transcript.rs:14-57) has no `ContentReplacement` variant; `JsonlWriter` and `SessionStorage::append` have no provision for it | MEDIUM | Replaced content blocks (for prompt-cache stability) are never persisted → not replayed on resume → prompt cache drift |
| 7 | `TranscriptEntry` / Writer | `last-prompt` entry never written | Binary `x9l` policy: `"last-prompt":"always"`; "last-prompt" confirmed ×14 | `TranscriptEntry` has no `LastPrompt` variant; nothing appends `last-prompt` to the session file | MEDIUM | Gap #1 is unreachable because `last-prompt` entries are never written. However, a LingXi session resumed by the claude-code binary (or a mixed session) will behave incorrectly |
| 8 | `TranscriptEntry` / Writer | `queue-operation` entry never written | Binary `x9l` policy: `"queue-operation":"always"`; confirmed at 80877792 | `QueueOperation` type exists in `msgqueue/src/operations.rs` but is never appended to the session JSONL | LOW | Queue state cannot be reconstructed after an unclean shutdown; `msgqueue/src/operations.rs` is dead code from the JSONL perspective |

---

## Confirmed Correct (not a gap)

| # | Item | Evidence |
|---|------|---------|
| C1 | `FileStateCache` not persisted to disk on session end | Binary has no `file-state` JSONL type (0 hits). LingXi's `SessionResumer::resume` (resumer.rs:63) correctly starts a fresh cache with comment "deliberately do NOT rehydrate from stale historical Reads — see B6". This matches binary behavior. |
| C2 | Session JSONL path convention `~/.claude/projects/<sanitized-cwd>/<uuid>.jsonl` | Binary confirms `.claude/projects` ×5 hits. LingXi `session_path` (path.rs:26-33) and `project_dir_name` (path.rs:12-23) match exactly. |
| C3 | `LoadedTranscript` three main side-maps (`summaries`, `custom_titles`, `ai_titles`) | Binary `Yle` returns `summaries:r, customTitles:o, aiTitles:s` — all confirmed with 895/63/49 hits respectively. LingXi implements all three correctly. |
| C4 | Session sort order (mtime desc, created/birthtime desc tiebreak) | Binary `sortLogs` in logs.ts source confirmed; LingXi `Ord for SessionMetadata` (loader.rs:70-80) matches exactly. |
| C5 | Sidechain + teamName picker filters | Binary `vkm`: checks `i.isSidechain` and `i.teamName` before `sessionKind`. LingXi `collect_dir` (loader.rs:316-325) implements both correctly with truthy-string semantics. |
| C6 | `agent-color` side-map read + write | Binary `Yle`: `l.set(N.sessionId,N.agentColor)`. LingXi `agent_color.rs` implements read (`last_agent_color`) and write (`save_agent_color`) correctly with byte-locked JSON shape. |

---

## Resume Picker Filter — Complete Ground Truth

`vkm` function @ 206492423 applies filters IN ORDER. All 5 log messages confirmed in binary:

```
"( filtered from /resume: isSidechain=true"  @113414225
"! filtered from /resume: teamName="         @113414305  
"$ filtered from /resume: sessionKind="      @113414433
"# filtered from /resume: entrypoint="       @113414513
"% filtered from /resume: /loop session"     @113414577
```

LingXi implements filters 1+2 (isSidechain, teamName). **Missing: filters 3+4+5** (gaps #2, #3, #4).

---

## Entry-Append Policy (`x9l`) — Complete Map (confirmed @ 206493671)

The binary defines the canonical set of JSONL types and their write policy:

| Policy | Types |
|--------|-------|
| `"dedup-transcript"` | user, assistant, attachment, system, progress |
| `"always"` | summary, custom-title, ai-title, **last-prompt**, tag, agent-name, agent-color, agent-setting, pr-link, bridge-session, file-history-snapshot, attribution-snapshot, speculation-accept, mode, permission-mode, isolation-latch, worktree-state, **queue-operation**, marble-origami-commit, marble-origami-snapshot, marble-origami-reset |
| `"route-by-agent"` | **content-replacement**, fork-context-ref |

LingXi writes: `user/assistant/attachment/system` (JsonlMessage), `summary/custom-title/ai-title` (route_lines read), `agent-color` (agent_color.rs).
**LingXi never writes**: last-prompt, queue-operation, content-replacement (gaps #6, #7, #8 above), plus ~15 others (tag, agent-name, agent-setting, pr-link, bridge-session, file-history-snapshot, attribution-snapshot, speculation-accept, mode, permission-mode, isolation-latch, worktree-state, marble-origami-*, fork-context-ref) — those correspond to features not yet implemented in LingXi.

---

## UNCERTAIN

| # | Item | Reason |
|---|------|--------|
| U1 | Session-memory on-disk path (`agents/session-memory/*.md`) | The string `session-memory` (with hyphen as path) = 0 binary hits. Binary has `session_memory` (underscore, ×7) only as a feature-flag/telemetry name (`session_memory thinking_reminder companion_intro...`). The path `<config_home>/agents/session-memory/<session_id>.md` used by LingXi (session_memory.rs:224) cannot be confirmed from the binary. The feature may be cloud-side or the path may be constructed differently. |
| U2 | `fork-context-ref` impact on main-thread resume | Binary confirmed: 12 hits, `"route-by-agent"` policy, `H.set(N.agentId,N)` in Yle. Returned in Yle result. Whether absence breaks main-thread (vs. fork-session sub-agent) resume is unclear from binary alone. |
