## cost-pricing fix — STATUS: COMPLETE

**Commit**: see `git log -1` (branch: main worktree gap-sweep)
**Tests**: 76 passed, 0 failed (`cargo test -p cost`)

### Prices/tiers added (all binary-verified via audit):
| Model | Standard | Fast |
|---|---|---|
| claude-opus-4-7 | $5/$25 (Voe) | $30/$150 (H6s, shared with 4-6) |
| claude-opus-4-8 | $5/$25 (Voe) | $10/$50 (Ypn) |
| claude-fable-5  | $10/$50 (Ypn) | none (binary has no fast branch) |
| claude-mythos-5 | $10/$50 (Ypn) | none |

### Cache rates (all 4 models):
- 5m cache-write: proportional to standard input rate (125% of input)
- cache-read: 10% of input rate (matching existing pattern)
- 1h cache-write: `CacheWrite1h` token class added; rates from audit table
  (Voe→$10, Ypn→$20, H6s→$60 per Mtok)

### 1h cache tier (gap #5):
- `TokenClass::CacheWrite1h` added to `pricing.rs` with per-model rates
- `TokenUsage.cache_write_1h` field added to `usage.rs` (defaults 0)
- `tokens_for()` handles the new class; `total_tokens()` + `add()` updated
- All `insert_anthropic()` calls now auto-derive 1h rates from the 5m rate
- **TODO wire**: `ephemeral_1h_input_tokens` parsing in
  `llm-client/src/providers/anthropic.rs` — the rate table is correct but
  the API field isn't yet parsed, so `cache_write_1h` stays 0 until that
  is wired. Documented in `CacheWrite1h` doc comment.

### Deferred:
- `/cost` per-model output format (M8 gap, already documented)
- `ephemeral_1h_input_tokens` API-field parsing in llm-client (requires
  touching anthropic.rs usage extraction — separate PR)
