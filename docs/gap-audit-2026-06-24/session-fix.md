# Session Resume Parity Gap Fixes — Report

**Status:** ALL assigned gaps FIXED (gaps #1–#5, reachable subset).  
**Commit:** `cd797842` on branch `worktree-gap-sweep`  
**Test result:** 27 new + all prior tests GREEN (0 failures across all session test suites)

---

## Gaps Fixed

### Gap #1 — `last-prompt` JSONL entry + `find_tip` explicit override [HIGH]

**Files:** `session/src/jsonl/reader.rs`, `session/src/jsonl/loader.rs`

Two parts:

**Part A — Write/parse side:** `route_lines` now routes `type:"last-prompt"` lines
into new `LoadedTranscript` fields: `last_prompt_leaf_uuid: Option<String>` and
`last_prompt_explicit: bool`. The accumulation mirrors the binary `Yle` function
exactly:
```
L = N.explicit===true || L && N.leafUuid===O
O = N.leafUuid
```

**Part B — `find_tip` read side:** Before the timestamp-based leaf race (steps 1–4),
`find_tip` now checks:
```rust
if loaded.last_prompt_explicit {
    if let Some(lp_msg) = by_uuid.get(lp_uuid) {
        if !lp_msg.is_sidechain && (user or assistant) { return Some(lp_msg); }
    }
}
```
This mirrors binary `V = L&&O&&n.has(O)&&!n.get(O)?.isSidechain`.

**Corner cases handled:** non-explicit entry (falls through), sidechain target (falls
through), unknown uuid (falls through), accumulation with same uuid (explicit carries
over), accumulation with new uuid (explicit resets).

---

### Gap #2 — `sessionKind` daemon filter [HIGH]

**File:** `session/src/jsonl/loader.rs` — `collect_dir`

Added after the existing isSidechain/teamName check:
```rust
let session_kind = first.extra.get("sessionKind")...;
if session_kind == "daemon" || session_kind == "daemon-worker" { continue; }
```
`sessionKind` is not a named struct field on `JsonlMessage` but lands in `extra`
via `#[serde(flatten)]` — correct.

---

### Gap #3 — SDK-entrypoint filter [MEDIUM]

**File:** `session/src/jsonl/loader.rs` — `collect_dir`

Added after daemon check:
```rust
let entrypoint = first.entrypoint.as_deref().unwrap_or("");
if matches!(entrypoint, "sdk-cli" | "sdk-ts" | "sdk-py") { continue; }
```
`entrypoint` IS a named struct field on `JsonlMessage` — direct access.

---

### Gap #4 — `/loop` session filter [MEDIUM]

**File:** `session/src/jsonl/loader.rs` — `collect_dir`

Added after the sidechain/teamName/daemon/SDK block (outside `if let Some(first)`
so it reads as a separate top-level check). Re-serializes the first message to
JSON string and scans for the XML tag:
```rust
if raw_first_line.contains("<command-name>/loop</command-name>") { continue; }
```
Binary detects this via raw first-line `.includes()` on the exact string confirmed
at offset 113388700. Test verifies that plain text "loop" does NOT trigger the filter.

---

### Gap #5 — `LoadedTranscript` side-maps [MEDIUM]

**File:** `session/src/jsonl/reader.rs`

**Added maps** (tied to features LingXi HAS):
| Rust field | JSONL type | Binary key | Note |
|---|---|---|---|
| `tags: HashMap<String, Vec<String>>` | `"tag"` | `N.sessionId→[N.tag]` | accumulates per session |
| `agent_names: HashMap<String, String>` | `"agent-name"` | `N.agentId→N.agentName` | |
| `agent_settings: HashMap<String, Value>` | `"agent-setting"` | `N.agentId→N` (whole entry) | |
| `modes: HashMap<String, String>` | `"mode"` | `N.sessionId→N.mode` | last-write-wins |
| `permission_modes: HashMap<String, String>` | `"permission-mode"` | `N.sessionId→N.permissionMode` | last-write-wins |
| `worktree_states: HashMap<String, Value>` | `"worktree-state"` | `N.agentId→N` (whole entry) | |
| `last_prompt_leaf_uuid: Option<String>` | `"last-prompt"` | see gap #1 | |
| `last_prompt_explicit: bool` | `"last-prompt"` | see gap #1 | |

**Deferred maps** (documented in `LoadedTranscript` struct comment with reason):
- `prNumbers/prUrls/prRepositories` → PR subsystem absent in LingXi
- `bridgeSessionIds/bridgeLastSeqs/bridgeDialogKindsBySession` → bridge subsystem absent
- `contextCollapseCommits/contextCollapseSnapshot` → context-collapse REFUTED as inert (prior audit `mainloop-parity-2026-06-23.md`)
- `contentReplacements/agentContentReplacements` → context-collapse-tied
- `attributionSnapshots` → attribution subsystem absent
- `forkContextRefs` → fork-context subsystem absent
- `isolationLatches` → isolation/worktree out-of-single-process-scope
- `fileHistorySnapshots` → file-history-snapshot subsystem absent
- `agentColors` → already handled correctly by `agent_color.rs` (confirmed C6)

---

## Deferred (as specified, not implemented)

### Gap #6 — `content-replacement` entry write [MEDIUM]
**Reason:** Context-collapse-tied. LingXi lacks context-collapse (REFUTED as inert
in `mainloop-parity-2026-06-23.md`). Not implemented per spec.

### Gap #8 — `queue-operation` entry write [LOW]
**Reason:** Crash-recovery only (LOW priority). The `QueueOperation` type exists in
`msgqueue/src/operations.rs` but is dead from the JSONL perspective. Not implemented
per spec — trivial additive when that subsystem is activated.

---

## Test Results

```
session_gap_fixes_test.rs: 27 passed; 0 failed
All existing session test suites: 0 failures
```

All 27 new tests are in `lingxi-code/session/tests/session_gap_fixes_test.rs`:
- 4 tests for explicit/non-explicit/sidechain/unknown-uuid `find_tip` override
- 7 tests for `last-prompt` `route_lines` accumulation logic
- 6 tests for gap #5 side-map parsing
- 3 tests for daemon/daemon-worker filter
- 4 tests for sdk-cli/sdk-ts/sdk-py + cli non-filter
- 2 tests for /loop detection + false-positive guard
- 1 test for all-filtered-returns-EmptyDirectory invariant

---

## Concerns / Notes

- The `/loop` detection (gap #4) re-serializes the first `JsonlMessage` back to
  JSON rather than preserving the raw JSONL line, because `route_lines` discards
  raw text after parsing. This is functionally correct — `serde_json::to_string`
  reproduces the exact field values including the XML tag — but is not a
  byte-literal scan of the raw line. Any future optimization that changes the
  serialization order could theoretically affect this; however the tag is in
  `message.content` (a `Value`), which serializes deterministically.

- `sessionKind` is captured via `JsonlMessage::extra` (the flatten catch-all),
  not a dedicated field. This is correct per the existing schema design; adding a
  named field would require migration of callers and is not warranted by LingXi's
  current feature surface.

- The `last-prompt` write side (part A of gap #1 — `TranscriptEntry::LastPrompt`
  variant and writer call site) is NOT implemented. The gap audit (gap #7) notes
  that "gap #1 is unreachable because `last-prompt` entries are never written" by
  LingXi. The read side is implemented so LingXi correctly handles sessions written
  by the claude-code binary (mixed sessions or sessions resumed by claude-code).
  The write side requires identifying where in the orchestrator to emit the entry
  (on every prompt submit), which is outside the `session` crate scope.
