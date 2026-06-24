# Cost / Pricing / Model-Metadata Parity Gap Audit
**Oracle binary**: v2.1.186 at `/opt/homebrew/lib/node_modules/@anthropic-ai/claude-code/node_modules/@anthropic-ai/claude-code-darwin-arm64/claude`  
**LingXi files**: `lingxi-code/cost/src/pricing.rs`, `lingxi-code/llm-client/src/model/context_window.rs`, `lingxi-code/commands/core/src/usage.rs`  
**Total confirmed gaps: 7** (4 HIGH, 2 MEDIUM, 1 LOW)  
**Uncertain items: 1**

---

## Confirmed Gaps

| # | Area | Item | Oracle (binary evidence) | LingXi (file:line) | Severity | Note |
|---|------|------|--------------------------|---------------------|----------|------|
| 1 | Model pricing | `claude-opus-4-7` missing from catalog | `mHr={firstParty:"claude-opus-4-7"}` → `Voe` ($5/$25 standard) in `ukt`; `H6s` ($30/$150) in fast mode (`$2u`, offset 195362400) | `pricing.rs` — no entry for `claude-opus-4-7` | HIGH | Would fall through to `UnpricedModel`; binary bills $5/$25 standard / $30/$150 fast |
| 2 | Model pricing | `claude-opus-4-8` missing from catalog | `fHr={firstParty:"claude-opus-4-8"}` → `Voe` ($5/$25 standard) in `ukt`; `Ypn` ($10/$50) in fast mode (`$2u`) | `pricing.rs` — no entry for `claude-opus-4-8` | HIGH | Would fall through to `default_unknown_pricing` ($5/$25) for standard, but fast-mode ($10/$50) is absent |
| 3 | Model pricing | `claude-fable-5` missing from catalog | `iCe={firstParty:"claude-fable-5"}` → `Ypn` ($10/$50) in `ukt` (offset 195364719) | `pricing.rs` — no entry; `context_window.rs:179` has output-token tier | HIGH | Would fall through to `default_unknown_pricing` ($5/$25) — 2× underbilling |
| 4 | Model pricing | `claude-mythos-5` missing from catalog | `yFs={firstParty:"claude-mythos-5"}` → `Ypn` ($10/$50) in `ukt` | `pricing.rs` — no entry; `context_window.rs:179` has output-token tier | HIGH | Same as fable-5: 2× underbilling at $5/$25 vs correct $10/$50 |
| 5 | Cost calculation | Ephemeral 1-hour cache write tier (`promptCacheWrite1hTokens`) missing | Binary `B2u` function (offset 195362200) uses `cache_creation.ephemeral_1h_input_tokens` with a separate higher rate (e.g. sonnet: $6/Mtok vs standard $3.75/Mtok); all model tiers carry `promptCacheWrite1hTokens` (D0r=1.6, P0r=2, yme=6, w6s=30, Voe=10, H6s=60, Ypn=20) | `cost/src/usage.rs:18` only has `cache_write: u64`; `pricing.rs` has no `CacheWrite1h` token class; `calculator.rs` uses only standard `CacheWrite` rate | MEDIUM | If any API response returns `cache_creation.ephemeral_1h_input_tokens`, those tokens are silently billed at the cheaper standard rate instead of the 1h rate |
| 6 | Cost calculation | Fast mode for `opus-4-7` missing | Binary `$2u`: `if(n==="claude-opus-4-6"\|\|n==="claude-opus-4-7")return H6s` when `speed==="fast"` (offset 195362400) | `cost/src/calculator.rs` — fast-mode path only checks `opus-4-6` (mirrors only `CLAUDE_OPUS_4_6_CONFIG`) | MEDIUM | Fast-speed `opus-4-7` usage would be billed at $5/$25 instead of $30/$150 |
| 7 | /cost output format | `/cost` slash command output differs significantly from binary | Binary `formatTotalCost()` (cost-tracker.ts offset 88409280): `"Total cost: {cost}\nTotal duration (API): ...\nTotal duration (wall): ...\nTotal code changes: N lines added, N lines removed\nUsage by model:\n  {shortName}: N input, N output, N cache read, N cache write (cost)"` | `commands/core/src/usage.rs:50`: `"Usage\nTotal cost: $x.xxxx\nInput tokens: N\nOutput tokens: N\nAPI calls: N\nSession duration: Ns\nPer-model cost breakdown is not available yet (M8).\nEsc to close"` | LOW | Documented M8 divergence; per-model breakdown, API/wall duration split, and lines-changed are absent |

---

## Uncertain / Not Confirmed as Gaps

| # | Area | Item | Why Uncertain |
|---|------|------|---------------|
| U1 | Context window | `getSonnet1mExpTreatmentEnabled` (`coral_reef_sonnet` GrowthBook A/B flag) — if enabled, `sonnet-4-6` gets 1M context without the explicit beta header | LingXi `context_window.rs` doc (line 21) explicitly lists this as omitted (`GrowthBook config has no Rust equivalent`). Whether the binary's GrowthBook flag is live in production is unknown. LingXi cannot mirror a runtime A/B flag. Flag-gated/inert-by-default paths are not counted as confirmed gaps per audit policy. |

---

## Areas That Looked Clean

- **All currently-priced Anthropic models** (sonnet 3.5/3.7/4/4.5/4.6, opus 4/4.1/4.5/4.6, haiku 3.5/4.5): rates match binary exactly — $3/$15, $15/$75, $5/$25, $30/$150 (fast), $0.8/$4, $1/$5.
- **Cache write and cache read rates** for all priced models: match binary tiers.
- **Opus 4.6 fast-mode tier** ($30/$150): `pricing.rs:690` matches binary `H6s`.
- **Default unknown-model pricing** ($5/$25): `pricing.rs:631` matches binary `F2u=Voe`.
- **Context window default** (200k): `context_window.rs:34` matches binary `b0t=200000`.
- **Max output token table** (per-model default/upper): LingXi's `model_max_output_tokens()` matches the binary's `YCe` function exactly for all 14 named models (200k else-branch default=32k/upper=128k also matches binary `kQu=32000,HQu=128000`).
- **1M context detection** (`[1m]` suffix, `CONTEXT_1M_BETA_HEADER`, capable-model list): matches binary.
- **Model ID canonicalization** (`first_party_name_to_canonical`): matches binary `bo/firstPartyNameToCanonical`.
- **`/cost` alias for `/usage`**: wired correctly (`register.rs:343`).
