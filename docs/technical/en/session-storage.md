# Session identity and durable storage

Understand JSONL graphs, stable UUIDs, cold restoration, forks and the difference between context compaction and transcript reclamation.

<a id="layout-and-ids"></a>

## Storage layout and identities

Production transcripts live at <config-home>/projects/<sanitized-cwd>/<session-uuid>.jsonl. Long sanitized cwd names receive a deterministic DJB2 suffix. Large tool outputs live under the sibling <session-uuid>/tool-results directory, not beneath the project workspace. Rollout recording has a separate lower-level schema and policy; do not interchange the formats.

Session IDs identify mounted conversations. Outer JSONL UUIDs identify persisted rows; parentUuid links the graph. Provider response IDs identify remote responses. Host stream indexes preserve presentation ordering and use a monotonic watermark even after rows disappear. Text and visual position are unsuitable keys for restoring timestamps or resolving duplicates.

![Durable append and UUID graph restoration, fork and context compaction](../assets/runtime-session-lifecycle.svg)

A resume reuses the source session; a fork creates a new session identity. Context compaction does not erase the historical transcript by itself.

<a id="durability"></a>

## Append and publication barriers

The durable writer pins filesystem roots and handles, holds session-state authority, checks delivery identity, appends and fsyncs before acknowledging success. Retrying identical content with the same delivery identity returns AlreadyPresent; conflicting content for that identity fails. A write that reached the file but failed durability remains a failed publication until its retry crosses the barrier.

UI delivery, transcript snapshots and durable append are separate boundaries. Surface persistence warnings and preserve retry information. Shutdown drains the owning lifecycle coordinator; do not infer durability from MessageComplete or TurnEnded alone.

<a id="resume-and-fork"></a>

## Cold resume and fork

The reader separates graph rows from metadata, indexes rows by UUID and uses the final occurrence for duplicate UUIDs. Recovery reconstructs the selected parent chain and anchored tool results. Reading file lines directly is useful for diagnostics, but it is not the resume algorithm.

SessionResumer restores messages and metadata with an empty FileStateCache so historical Read state cannot authorize stale file assumptions. Runtime composition reattaches permissions, plugin/MCP connections, output style and accounting. Forking creates a new session JSONL and rewrites session identity/parent chain/sidechain facts; it is not simply renaming the original file.

In the CLI, /branch creates this sibling conversation and mounts it for the user; /fork names a different operation that spawns a detached background agent. Select the host command by its actual behavior rather than assuming every use of the word fork creates the same session lifecycle.

<a id="compaction"></a>

## Two different compactions

| Operation | Purpose and owner |
| --- | --- |
| Model context compaction | Orchestrator/compaction reduce the next request context using summaries, boundaries and retained state. It preserves the persisted history needed for replay and host projection. |
| Local transcript reclamation | session/jsonl/transcript_compact reclaims superseded metadata. Local GC is off by default and controlled by LINGXI_TRANSCRIPT_LOCAL_GC. It is not the model context window policy. |

Do not hand-edit a live JSONL to reduce token use. Use the orchestrator compaction interface, and preserve the writer authority and session epoch while switching or recovering a session.

<a id="inspection"></a>

## Read-only diagnostic example

The script counts record types and duplicate UUIDs without printing prompt text, credentials or tool payloads. Supply a copied transcript path. It intentionally reports malformed lines instead of silently presenting them as a valid restored session.

- Existing session tests cover golden JSONL, duplicate UUIDs, parent-chain recovery, fork names and transcript compaction wiring. Orchestrator tests separately cover durable append warnings and timestamp projection.
- Crash recovery on a target filesystem, interrupted writes and cold resume with actual attachments need end-to-end host acceptance. A parsed copy is diagnostic evidence only.

### Python · inspect a copied JSONL

```python
import collections
import json
import pathlib
import sys

counts = collections.Counter()
uuids = collections.Counter()
for line_no, line in enumerate(pathlib.Path(sys.argv[1]).read_text().splitlines(), 1):
    try:
        row = json.loads(line)
    except json.JSONDecodeError:
        print(f"malformed line: {line_no}")
        continue
    if not isinstance(row, dict):
        print(f"non-object line: {line_no}")
        continue
    counts[str(row.get("type", "unknown"))] += 1
    if isinstance(row.get("uuid"), str):
        uuids[row["uuid"]] += 1
print("record types:", dict(sorted(counts.items())))
print("duplicate UUID count:", sum(count > 1 for count in uuids.values()))
```

Verification method: Environment validation

## Sources and documentation baseline

[Source repository](https://github.com/lingxi-coder/harness-runtime)

- [harness-runtime/crates/session/src/jsonl/path.rs](https://github.com/lingxi-coder/harness-runtime/blob/f975ab5804bb3af0e72beb3974e5c911ece44d53/crates/session/src/jsonl/path.rs) — Committed source (f975ab5804bb3af0e72beb3974e5c911ece44d53)
- [harness-runtime/crates/session/src/jsonl/schema.rs](https://github.com/lingxi-coder/harness-runtime/blob/f975ab5804bb3af0e72beb3974e5c911ece44d53/crates/session/src/jsonl/schema.rs) — Committed source (f975ab5804bb3af0e72beb3974e5c911ece44d53)
- [harness-runtime/crates/session/src/jsonl/reader.rs](https://github.com/lingxi-coder/harness-runtime/blob/f975ab5804bb3af0e72beb3974e5c911ece44d53/crates/session/src/jsonl/reader.rs) — Committed source (f975ab5804bb3af0e72beb3974e5c911ece44d53)
- [harness-runtime/crates/session/src/jsonl/recovery.rs](https://github.com/lingxi-coder/harness-runtime/blob/f975ab5804bb3af0e72beb3974e5c911ece44d53/crates/session/src/jsonl/recovery.rs) — Committed source (f975ab5804bb3af0e72beb3974e5c911ece44d53)
- [harness-runtime/crates/session/src/jsonl/durable_writer.rs](https://github.com/lingxi-coder/harness-runtime/blob/f975ab5804bb3af0e72beb3974e5c911ece44d53/crates/session/src/jsonl/durable_writer.rs) — Committed source (f975ab5804bb3af0e72beb3974e5c911ece44d53)
- [harness-runtime/crates/session/src/jsonl/message_identity.rs](https://github.com/lingxi-coder/harness-runtime/blob/f975ab5804bb3af0e72beb3974e5c911ece44d53/crates/session/src/jsonl/message_identity.rs) — Committed source (f975ab5804bb3af0e72beb3974e5c911ece44d53)
- [harness-runtime/crates/session/src/jsonl/transcript_compact.rs](https://github.com/lingxi-coder/harness-runtime/blob/f975ab5804bb3af0e72beb3974e5c911ece44d53/crates/session/src/jsonl/transcript_compact.rs) — Committed source (f975ab5804bb3af0e72beb3974e5c911ece44d53)
- [harness-runtime/crates/session/src/resumer.rs](https://github.com/lingxi-coder/harness-runtime/blob/f975ab5804bb3af0e72beb3974e5c911ece44d53/crates/session/src/resumer.rs) — Committed source (f975ab5804bb3af0e72beb3974e5c911ece44d53)
- [harness-runtime/crates/session/src/branch.rs](https://github.com/lingxi-coder/harness-runtime/blob/f975ab5804bb3af0e72beb3974e5c911ece44d53/crates/session/src/branch.rs) — Committed source (f975ab5804bb3af0e72beb3974e5c911ece44d53)
- [harness-runtime/crates/orchestrator/src/conversation/compaction.rs](https://github.com/lingxi-coder/harness-runtime/blob/f975ab5804bb3af0e72beb3974e5c911ece44d53/crates/orchestrator/src/conversation/compaction.rs) — Committed source (f975ab5804bb3af0e72beb3974e5c911ece44d53)
- [harness-runtime/crates/client/src/adapter/lowering/messages.rs](https://github.com/lingxi-coder/harness-runtime/blob/f975ab5804bb3af0e72beb3974e5c911ece44d53/crates/client/src/adapter/lowering/messages.rs) — Committed source (f975ab5804bb3af0e72beb3974e5c911ece44d53)
- [harness-runtime/crates/runtime/src/desktop/session_state/resume.rs](https://github.com/lingxi-coder/harness-runtime/blob/f975ab5804bb3af0e72beb3974e5c911ece44d53/crates/runtime/src/desktop/session_state/resume.rs) — Committed source (f975ab5804bb3af0e72beb3974e5c911ece44d53)

- harness-runtime: HEAD f975ab5804bb3af0e72beb3974e5c911ece44d53; Referenced input fingerprint sha256:92c2108dc63215bc69ea9c11ea081795da883fdcb1c9c28b060aa9221a1b95a2; Product pin e04c1837508014d68ea042d5c9fe3c8c15af5aa0

<!-- Generated from apps/web/src/data/docs. Do not edit this file directly. -->
