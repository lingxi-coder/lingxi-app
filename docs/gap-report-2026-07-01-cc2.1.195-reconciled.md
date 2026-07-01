# LingXi ↔ Claude Code Parity — Reconciled Gap Report (2026-07-01)

**Supersedes:** `docs/tui-parity-worklist-2026-06-24.md` (151 items),
`docs/gap-audit-2026-06-24/` (26 docs), and the status portions of
`.omo/plans/2026-06-29-cc2.1.195-core-parity-corrected.md`.

## Baseline

- **Parity target:** pinned `claude-code/` TypeScript reference (CC ~2.1.195 /
  2026-03-31 snapshot; dir last touched 2026-04-22). No newer Anthropic source
  is checked out — "latest claude code" = this reference.
- **LingXi version:** `lingxi-code` v0.12.0.
- **Why this doc exists:** ~446 commits landed since the 2026-06-24 audit, closing
  the large majority of tracked gaps. Those docs are stale. This report reconciles
  them against current HEAD (verified via 6 parallel探查 agents + direct checks on
  2026-07-01) so only the *real* remaining gap surface is listed.

Legend: ✅ DONE · 🟡 PARTIAL · 🔴 OPEN · ⚪ UNVERIFIED · ⏸ DEFERRED-BY-DESIGN

---

## A. TUI parity (the 151-item worklist — now ~85% closed)

Fully reconciled clusters — **all DONE**, no action needed:
- **agents-screen** (8/8) — `screens/agents.rs`
- **completion-palette** (8/8) — `palette.rs`, `completion.rs`, `fuzzy.rs`
- **coordinator-team-tasks** (9/9, minus cosmetic `startTime` sort — wire lacks `start_time`)

### Remaining TUI gaps

> ⚠️ **CORRECTION (direct code re-verification, 2026-07-01):** several agent-reported
> OPEN items were **false positives** — the agents read stale doc-comments or searched
> wrong literals. Verified directly by reading the code:
> - **PERM-1 = ✅ DONE, not open.** `screens/permissions.rs` implements the full
>   interactive manager (4 tabs Allow/Ask/Deny/Workspace, add-input, delete-confirm,
>   `AddRule`/`DeleteRule`/`AddWorkspaceDir`/`RemoveWorkspaceDir` outcomes, `rule_description`),
>   consumed at `root.rs:829-853`, persisted by `pump_permission_add/delete/workspace_dir`
>   → real `persist_permission_update`/`remove_permission_update`/`persist_workspace_directory`.
>   15 passing tests. Only the doc-comment header (lines 18-22) is stale. Residual vs CC
>   is *rich chrome only* (RecentDenialsTab, styled Dialog, input autocomplete) — low-priority polish.

| ID | Sev | Gap | Location |
|---|---|---|---|
| SS-07 / RRS-03 | 🟡 MED | Spinner has no elapsed-time / token-count status suffix | `tui/src/components/spinner.rs:511` |
| RRS-01 | 🟡 MED | PageUp/PageDown scroll a full page; CC scrolls half | `tui/src/app.rs:1314` |
| RRS-02 | 🟡 MED | `Esc` does not interrupt a streaming turn (only Ctrl-C mapped) | `tui/src/root.rs` live-key path |
| PIC-02 | 🟡 MED | Footer "? for shortcuts" not suppressed during history-search / vim | `tui/src/components/prompt_input/footer.rs:218` |
| md-03 | 🟡 MED | Links render `text (url)` not OSC-8 blue hyperlink; mailto not special-cased | `tui/src/render/markdown.rs:245` |
| syntax-01 | 🟡 LOW | No shebang/first-line language detection | `tui/src/render/syntax.rs:73` |
| syntax-02 | 🟡 LOW | No filename-based lang (Dockerfile/Makefile/Gemfile/…) | `tui/src/render/syntax.rs:86` |
| mcp-* | 🟡 LOW | MCP vocabulary: title "Manage MCP servers", count subtitle, status icons, `failed` wording | `tui/src/screens/mcp.rs`, `root.rs:1509` |
| help-4/5/6, doctor-1 | 🟡 LOW | Help title "Help" vs "Claude Code v{ver}"; missing docs-URL line; footer wording; Doctor Esc/q vs Enter | `tui/src/screens/help.rs`, `doctor.rs` |
| ~~MEM-1~~ | ✅ DONE | Git-conditional description ("Checked in at" / "Saved in"), tested. Uses `LINGXI.md` branding (intentional). | `tui/src/screens/memory.rs:62-66` |
| ~~TRUST-1~~ | ✅ DONE | Dimmed "Enter to confirm · Esc to cancel" footer present + tested | `tui/src/startup_trust.rs:113-115` |

> **Migration note:** the iocraft→ratatui plan (`.omo/plans/2026-07-01-...`) is only
> scaffolded (`tui-core` partial, **no `tui-rata`**). The live iocraft `tui` crate
> still owns all the above, so these gaps are current, not superseded.

---

## B. Core engine parity (cc2.1.195 plan)

| Area | Verdict | Evidence / Note |
|---|---|---|
| Permission classifier / auto-mode | ✅ DONE | Trait + `ClassifierApproved/Rejected` wired (`policy_gate.rs:297-336`); flag returns `true` (`classifier.rs:25`). Divergence: deterministic **offline** classifier, not sidequery-LLM. |
| Compaction — cached microcompact | ✅ DONE | Real `cache_hit` hit/miss + tests (`cached_microcompact.rs`). `context_collapse` stays a stub — OK (verified DCE). |
| Read token limit + env override | ✅ DONE | `DEFAULT_MAX_OUTPUT_TOKENS=25_000` + `LINGXI_FILE_READ_MAX_OUTPUT_TOKENS` (`tools/file/src/read.rs`). |
| Read-before-write staleness | ✅ DONE | `check_read_before_write` + byte-exact messages (`tools/file/src/lib.rs`). |
| Streaming executor schema validation | ✅ DONE | Malformed schema = PASS-by-design (`schema_validation.rs`). |
| Telemetry `OTEL_LOG_ASSISTANT_RESPONSES` | ✅ DONE | Default-off env gate, retains request_id/model/stop/usage (`turn_loop.rs:775`). |
| **Memory per-memory surfacing** | 🟡 DIVERGENCE (judgment call) | `surfacing.rs:41-52` **documents** wrapping joined per-memory blocks in ONE `<system-reminder>` envelope vs CC's N separate `isMeta` messages — author argues byte-equivalent text. Content shape (preamble/`h6n`/staleness) verified 1:1 with CC v2.1.193+. Wire *structure* (1 msg vs N) differs; only matters under strict JSONL-byte parity. **Not a clear bug.** |
| Bash write-marker invalidation | ✅ DONE | `bash.rs:591` `invalidate_written_read_state()` called at `bash.rs:1617`; `command_semantics.rs:79` `parsed_written_paths()` covers `>`/`>>`/`tee`/heredoc/`touch`/`cp` + tests. (Agent false-negative: searched wrong literal.) |
| Sandbox missing-companion hard-fail | ⏸ DEFERRED-by-design | socat domain-filter companion explicitly deferred (`sandbox/src/wrap.rs:108`); missing socat deliberately must NEVER disable sandbox (`dependency_check.rs:50`). Not a bug — the whole allowlist companion is a deferred feature. |
| Read-file state store | ✅ DONE (2026-07-01) | Byte-budgeted LRU (100 entries / 25 MiB, size=`content.len().max(1)`), MRU-on-`get`, evict-on-`set`, lone-oversized retained — 1:1 with `fileStateCache.ts`. Zero new deps; public API unchanged (consumers incl. `conversation.rs` `drain()` compile verbatim). 13 unit tests green. `tool-api/src/read_file_state.rs`. Plan: `.omo/plans/2026-07-01-read-file-state-lru-parity.md`. |
| WebFetch host flow | 🟡 PARTIAL | Preapproved-host + `domain_info` blocklist preflight present; binary-body fidelity (String vs `Vec<u8>`) + image resize deferred. |
| Bash image resize/re-encode | ⏸ DEFERRED | Emits IMAGE block; resize "PARTIALLY DEFERRED" (needs image-decode dep). |
| Sidequery ProviderSideQueryClient | ✅ DONE (by design) | `provider_side_query.rs:14-16,294-297` forwards `tool_choice`+`stop_sequences`; `max_retries` is caller-side budget, `thinking_budget` dropped because sidequery model table is `reasoning:false`. Deliberate + tested (line 791). Not a gap. |
| Forked agent | 🟡 PARTIAL | No-backend returns `"[forked-agent-stub]"` sentinel not error; multi-turn lives in AgentTool path, no `ForkedAgentLoop` seam. (May also be intentional — needs owner call.) |
| Plugin secrets / marketplace | ⏸ DEFERRED (no consumer path) | `resolve_user_config` is a documented stub (`_credentials` unused, sensitive→`Value::Null`, `loader.rs:29-50`, "Plan 16"). **But the read-side alone would be dead code:** the sole caller discards the result (`let _user_config = …`, `manager.rs:467`) and no plugin-activation code consumes user_config. No CC reference for the consumer path exists under `userConfig`/`resolveUserConfig` (grep empty). Implement only once the consumer (plugin config → MCP-server env) + write path exist; not a standalone win. Marketplace: git-only, network arms deferred. |

---

## C. Structural (whole-subsystem) gaps

| CC subsystem | Rust state |
|---|---|
| `buddy`, `upstreamproxy`, `moreright` | 🔴 Missing entirely |
| `vim` mode | 🟡 Parity subset only (TUI) |
| `remote`, `memdir` | ⏸ Minimal — RemoteAgent/cloud deferred by design |
| `voice`, `query` | 🟡 Referenced, not full crates |
| Slash commands | ✅ Surface complete — all ~76 visible builtins have real descriptions; placeholder only for hidden/dev-gated commands (`names.rs:519-593`). 94 registered vs ~102 CC modules = hidden/deferred delta only. |

---

## Priority ranking (after direct re-verification)

**Key finding:** every item originally ranked 🔴 HIGH turned out to be DONE, a
documented divergence, or deferred-by-design. The真实 actionable surface is small
and low-severity.

1. 🟡 **Genuinely-PARTIAL wiring** (owner should confirm intent, then wire):
   - Plugin `CredentialManager` for sensitive `user_config` (`plugin/src/loader.rs`).
   - Read-file state byte-budgeted LRU eviction/persistence (`tool-api/src/read_file_state.rs`).
   - WebFetch binary-body fidelity (String→`Vec<u8>`).
   - Forked-agent no-backend→error semantics (or confirm sentinel is intentional).
2. 🟡 **TUI polish** (verify each individually — agents unreliable here):
   spinner status suffix · Esc-interrupt streaming · half-page PageUp/Down ·
   markdown OSC-8 links · MCP vocabulary. *(syntax shebang/filename detection may
   already be DONE — cluster-B agent contradicted itself; needs direct check.)*
3. ⏸ **Deferred-by-design** (not gaps): sandbox socat companion, bash image resize,
   RemoteAgent/cloud, buddy, upstreamproxy, full vim.
4. ✅ **Confirmed DONE** (previously mis-flagged): PERM-1 interactive manager,
   bash write-marker invalidation, sidequery budget forwarding, permission classifier,
   cached microcompact, MEM-1, TRUST-1, slash-command surface, agents/palette/tasks clusters.

## ⚠️ Verification caveat

The 6 background探查 agents had a **high false-positive rate** — they flagged
DONE code as OPEN by reading stale doc-comments (PERM-1), searching wrong literals
(bash write-marker), or missing documented divergences (memory, sidequery). **Every
remaining 🟡 item in this report should be confirmed by directly reading the cited
code before any implementation work.** Treat agent verdicts as leads, not conclusions.
