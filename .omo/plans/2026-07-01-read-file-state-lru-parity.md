# Read-File-State Byte-Budgeted LRU — Parity Plan (2026-07-01)

- **Status:** Ready (blocker resolved). Pre-implementation.
- **Gap ID:** core-parity `read-file-state store` (was 🟡 PARTIAL, self-documented divergence).
- **Severity:** LOW — only manifests once a single session caches >100 files or >25 MB of
  file content. Correctness/parity item, not a live bug. Worth doing because it's now
  unblocked, self-contained, and has a clean seam.

## 1. Problem

`lingxi-code/tool-api/src/read_file_state.rs` stores read-state in an **unbounded**
`Arc<Mutex<HashMap<PathBuf, ReadFileEntry>>>`. Claude Code uses a **byte-budgeted LRU**
(`claude-code/src/utils/fileStateCache.ts` `FileStateCache`). The port comment (lines 16-28)
deferred this because the `max`/`maxSize` constants were thought to be "config-indirected
in the binary." **They are not** — they are plain module constants:

| CC constant | Value | Rust equivalent |
|---|---|---|
| `READ_FILE_STATE_CACHE_SIZE` (`max` entries) | `100` | `const READ_FILE_STATE_MAX_ENTRIES: usize = 100;` |
| `DEFAULT_MAX_CACHE_SIZE_BYTES` (`maxSize`) | `25 * 1024 * 1024` | `const READ_FILE_STATE_MAX_BYTES: u64 = 25 * 1024 * 1024;` |
| `sizeCalculation(v)` | `Math.max(1, Buffer.byteLength(v.content))` | `entry.content.len().max(1) as u64` |

CC semantics to match: **MRU-promote on `get`**; on `set`, insert/update then evict the
least-recently-used entries while `entries > max` **OR** `total_bytes > maxSize`.

## 2. Design (minimal blast radius)

Keep the **existing public API unchanged** — `new_read_file_state_map()`, `set(map, path, entry)`,
`get(map, path) -> Option<ReadFileEntry>`, `mtime_ms_floor(...)`, type alias `ReadFileStateMap`.
Only the inner data structure changes, so **no consumer changes** (orchestrator
`conversation.rs`, `tools/file/{read,edit,write,multi_edit,lib}.rs`, `turn_loop_test.rs` all
keep compiling verbatim).

Replace the inner `HashMap` with a small byte-accounting LRU struct:

```rust
struct ReadFileStateLru {
    map: /* insertion/recency-ordered */,   // e.g. indexmap::IndexMap or lru::LruCache
    total_bytes: u64,
    max_entries: usize,   // 100
    max_bytes: u64,       // 25 MiB
}
```
`ReadFileStateMap = Arc<Mutex<ReadFileStateLru>>`.

- **`get`**: look up; on hit, move key to MRU position; return clone. (LRUCache promotes on get.)
- **`set`**: compute `new_size = entry.content.len().max(1)`. If key exists, subtract its old
  size first. Insert/overwrite at MRU. Add `new_size`. Then **evict LRU** while
  `map.len() > max_entries || total_bytes > max_bytes` (never evict the just-inserted key —
  matches lru-cache, which evicts *other* LRU entries; a single entry larger than `maxSize`
  stays, per lru-cache behavior).
- **Dependency:** prefer an LRU already in `Cargo.lock`; else add `lru` (count-LRU) and layer
  byte accounting on top, or hand-roll with `indexmap` (both are common; pick per workspace
  policy — Task 1 decides).

### Scope boundary
- **IN:** byte-budgeted eviction + MRU + the two constants. This is the faithful `FileStateCache`
  eviction behavior.
- **OUT (separate follow-ups, file tracking items):**
  - `dump()`/`load()` + `cloneFileStateCache` for forked-agent state cloning
    (`claude-code/src/tools/AgentTool/runAgent.ts:377`) — only needed once forked agents
    snapshot read-state; wire when that path lands.
  - `mergeFileStateCaches` (timestamp merge) for compact restore.
  - `normalize(key)` path normalization — Rust callers already pass canonicalized absolute
    `PathBuf` keys (see `tools/file/src/lib.rs:158` canon), so this is a no-op today; add only
    if a non-canonical caller appears.
  - telemetry `file_state_cache:{entries, bytes}` — add if/when the telemetry surface wants it.

## 3. Tasks (atomic, verifiable)

1. **`tool-api/Cargo.toml`: choose the LRU backing.** Check `Cargo.lock` for an existing
   `lru`/`indexmap`/`hashlink` dep; reuse it. If none, add `lru` (MIT). — *expect:* one dep
   line, `cargo tree -p tool-api` shows it.
2. **`read_file_state.rs`: add the two constants** `READ_FILE_STATE_MAX_ENTRIES = 100`,
   `READ_FILE_STATE_MAX_BYTES = 25 * 1024 * 1024`, with doc-comments citing
   `fileStateCache.ts:18,22`. — *expect:* `cargo build -p tool-api`.
3. **`read_file_state.rs`: introduce `ReadFileStateLru`** inner struct with `total_bytes` +
   `map`; change `ReadFileStateMap` alias to wrap it; update `new_read_file_state_map()` to seed
   the constants. — *expect:* type-checks; public API signatures unchanged.
4. **`read_file_state.rs`: implement `set` with byte accounting + eviction** (subtract old size
   on overwrite, add new, evict LRU while over `max_entries` OR `max_bytes`, never the just-set
   key). — *expect:* new unit tests below pass.
5. **`read_file_state.rs`: implement MRU-promote in `get`.** — *expect:* recency test passes.
6. **Delete the "DEFERRED divergence" comment block** (lines 16-28) and replace with a short
   "1:1 with `FileStateCache` (max=100 entries, maxSize=25 MiB, size=byteLen(content))" note.
7. **Run the file-tools + orchestrator suites** to prove no consumer regressed.

## 4. Test plan (add to `read_file_state.rs` `#[cfg(test)]`)

Keep the existing 6 tests (roundtrip, missing, overwrite, arc-share, mtime floor) — they must
stay green (proves API compatibility). Add:

- `evicts_lru_when_entry_count_exceeds_max`: insert 101 distinct paths (small content) → `size()`
  == 100; the **first-inserted** key is gone, the 101st present.
- `evicts_by_byte_budget`: with a test-injected small `max_bytes` (add a
  `new_read_file_state_map_with_limits(max, bytes)` test constructor), insert entries whose
  content sums past the budget → oldest evicted until under budget; `total_bytes <= max_bytes`.
- `get_promotes_mru_so_it_survives_eviction`: insert A,B,C at count-cap=... ; `get(A)`; insert
  D that forces one eviction → **B** (now LRU) evicted, **A** survives.
- `overwrite_updates_byte_total_not_count`: set path P (size 10), then P (size 3) → `size()`==1,
  `total_bytes`==3 (old size subtracted, not double-counted).
- `single_oversized_entry_is_retained`: one entry with content > `max_bytes` stays (lru-cache
  keeps a lone over-budget entry). — matches `Math.max(1, byteLength)` + lru-cache semantics.

## 5. Verification commands

```
cargo test -p tool-api read_file_state::            # new + existing unit tests
cargo test -p tool-file                             # staleness guard + read/edit/write set-sites
cargo test -p orchestrator files_in_context         # /files snapshot + post-compact path
cargo build --workspace                             # no consumer signature drift
```
Plus `lsp_diagnostics` clean on `read_file_state.rs`.

## 6. Risks / notes

- **lru-cache eviction edge:** lru-cache evicts on `set` *after* inserting the new key, and will
  not evict the key it just set even if that single entry exceeds `maxSize`. The Task-4 loop must
  mirror this (evict *other* LRU keys only). The `single_oversized_entry_is_retained` test locks it.
- **Determinism:** eviction order must be strict insertion/recency, not `HashMap` iteration order,
  so tests are deterministic. Use an ordered structure (`lru::LruCache` or `IndexMap` + manual
  recency), never raw `HashMap`.
- **JSONL byte-parity:** unaffected — this cache is in-memory session state, never serialized to
  the session JSONL. No locked fixtures change.
- **`from_read` vs CC `isPartialView`:** orthogonal to this change; the Rust `ReadFileEntry` field
  set is unchanged. (CC's `isPartialView` for auto-injected partial content is a *separate* gap,
  not in scope here.)
```
```

## 7. Definition of done

- [ ] `read_file_state.rs` backed by a byte-budgeted LRU (100 entries / 25 MiB), MRU on get.
- [ ] Deferred-divergence comment removed; replaced with 1:1 parity note + constant citations.
- [ ] 5 new eviction/MRU tests + 6 existing tests green.
- [ ] `tool-file` + `orchestrator files_in_context` suites green; workspace builds; diagnostics clean.
- [ ] Report row updated: read-file-state store → ✅ DONE.
