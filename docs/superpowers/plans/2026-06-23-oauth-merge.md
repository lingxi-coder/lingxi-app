# OAuth Merge Implementation Plan (Plan C)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Fold the `anthropic-oauth` and `openai-oauth` crates into `llm_client::oauth::{anthropic,openai}`, delete the two standalone crates, and re-point the 6 consumers — completing the auth stack inside llm-client.

**Architecture:** Both crates already depend on `llm-client` and implement `llm_client::CredentialProvider`; the auth *abstractions* live in llm-client. This moves their concrete OAuth machinery (PKCE, callback server, refresh, profile/subscription, device-code/PAT) *down* into `llm-client` as the `oauth` module. Pure relocation + crate deletion. No new heavy deps (the code uses only `tokio` net + `rand`/`sha2`/`base64`/`urlencoding` for PKCE — no browser/keyring).

**Tech Stack:** Rust (MSRV 1.82). Independent of Plans A/B (can run before or after), except it shares the `telemetry` dep that Plan B also adds (add-if-absent).

## Global Constraints

- **MSRV 1.82** — no crate may raise the floor.
- **Byte-identical behavior.** Pure relocation; OAuth wire flows (scopes, grant types, PKCE, refresh timing) unchanged. Parity preserved.
- **No dependency cycle:** `llm-client` may now depend on `secret`/`telemetry`/`protocol`/`traits`/`rand`/`urlencoding`/`tracing` — all verified NOT to depend on llm-client. Confirm with a cycle check (Task 1 Step 1).
- **Build env (guarded-ff):** prefix every cargo command with `CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0`.
- **Green at every task:** `cargo build --workspace --tests` passes before each commit.
- **Atomic per crate:** each crate's move + consumer re-point + crate deletion happen in ONE task that ends green (a deleted crate with dangling consumers won't compile).

---

## File Structure

- **Modify** `lingxi-code/llm-client/Cargo.toml` — add `secret`, `telemetry`, `rand`, `urlencoding`, `tracing`; extend `tokio` features.
- **Create** `lingxi-code/llm-client/src/oauth/mod.rs` + `oauth/anthropic/` (14 files) + `oauth/openai/` (13 files).
- **Modify** `lingxi-code/llm-client/src/lib.rs` — `pub mod oauth;` + re-exports.
- **Delete** `lingxi-code/anthropic-oauth/`, `lingxi-code/openai-oauth/` and their `lingxi-code/Cargo.toml` members.
- **Modify** consumers: `apps/cli`, `apps/engine-desktop`, `apps/engine-mobile`, `migrations`, `platforms/android-minijail`, `test-harness` (imports + Cargo deps).

---

## Task 1: Fold `anthropic-oauth` → `llm_client::oauth::anthropic`

**Files:**
- Modify: `lingxi-code/llm-client/Cargo.toml`, `lingxi-code/llm-client/src/lib.rs`
- Create: `lingxi-code/llm-client/src/oauth/mod.rs`, `lingxi-code/llm-client/src/oauth/anthropic/` (moved files)
- Delete: `lingxi-code/anthropic-oauth/`; remove its `lingxi-code/Cargo.toml` member
- Modify: consumers importing `anthropic_oauth::…`

**Interfaces:**
- Produces: `llm_client::oauth::anthropic::{client, config, handle, refresh, resolver, limits, profile, subscription, credential_provider, pkce, callback, scope_upgrade, testsupport, …}` — every public item the crate exposed (e.g. `OAuthCredentialProvider`, `RefreshDriver`, `ClaudeAiOAuthClient`, `ClaudeAiOAuthConfig`, `OAuthHandle`, `OAuthError`, `SubscriptionType`, `subscription_from_scopes`, `OAuthProfileResponse`, `UserRolesResponse`), re-exported at `llm_client::oauth::anthropic::…` matching the crate's old `lib.rs` re-exports.

- [ ] **Step 1: Cycle check (gate) + add deps**

```bash
cd lingxi-code
for c in secret telemetry protocol traits; do
  grep -qE 'llm-client|llm_client' $c/Cargo.toml && echo "CYCLE via $c — STOP" || echo "$c clean";
done
```
All must be "clean". Then in `lingxi-code/llm-client/Cargo.toml` `[dependencies]` add (skip any already present — `telemetry` may exist from Plan B):
```toml
secret = { path = "../secret" }
telemetry = { path = "../telemetry" }
rand = "0.9"
urlencoding = "2"
tracing.workspace = true
```
And extend the existing `tokio` line to include the OAuth features (union with current): `tokio = { workspace = true, features = ["sync", "time", "net", "io-util", "macros", "rt-multi-thread"] }`.

- [ ] **Step 2: Move the source files**

```bash
cd lingxi-code
mkdir -p llm-client/src/oauth/anthropic
git mv anthropic-oauth/src/lib.rs llm-client/src/oauth/anthropic/mod.rs
for f in callback client config credential_provider handle limits pkce profile refresh resolver scope_upgrade subscription testsupport; do
  git mv anthropic-oauth/src/$f.rs llm-client/src/oauth/anthropic/$f.rs
done
# any integration tests:
ls anthropic-oauth/tests/ 2>/dev/null && git mv anthropic-oauth/tests/* llm-client/tests/ 2>/dev/null || true
```

- [ ] **Step 3: Rewrite intra-crate paths in the moved files**

The moved files used `crate::` (rooted at the old anthropic-oauth crate) and `llm_client::` (the dep). Rewrite BOTH, in this exact order, per file (single sed, ordered — the first expr's `crate::` output is not re-matched by the second):
```bash
cd lingxi-code
for f in llm-client/src/oauth/anthropic/*.rs; do
  sed -i '' -e 's/\bcrate::/crate::oauth::anthropic::/g' -e 's/\bllm_client::/crate::/g' "$f"
done
```
Then in `mod.rs` (the old lib.rs), the top-level `pub mod callback; pub mod client; …` lines are correct as-is (siblings). `super::` references inside submodules are relative and need NO change. Spot-check doc comments for over-rewritten `crate::oauth::anthropic::` text (harmless, fix only if glaring).

- [ ] **Step 4: Wire the module into llm-client**

Create `lingxi-code/llm-client/src/oauth/mod.rs`:
```rust
//! OAuth flows folded in from the former `anthropic-oauth` / `openai-oauth`
//! crates. These implement `crate::CredentialProvider` over PKCE/device-code
//! refresh machinery; the auth abstractions already live in this crate.

pub mod anthropic;
```
(Task 2 adds `pub mod openai;`.) In `lingxi-code/llm-client/src/lib.rs` add `pub mod oauth;`.

- [ ] **Step 5: Re-point anthropic consumers + drop the crate dep**

Find every consumer: `grep -rnE 'use anthropic_oauth|anthropic_oauth::' lingxi-code --include='*.rs' | grep -v '/target/'`. In each (migrations, test-harness, apps/engine-desktop, apps/engine-mobile, apps/cli, platforms/android-minijail as applicable), replace `anthropic_oauth::` → `llm_client::oauth::anthropic::`. In each consumer's `Cargo.toml`, remove `anthropic-oauth = …` and ensure `llm-client = { path = … }` is present (add if missing). `telemetry/src/tengu/oauth.rs` has only a doc-comment mention — update the text for accuracy.

- [ ] **Step 6: Delete the crate + workspace member**

```bash
cd lingxi-code
rm -rf anthropic-oauth
# remove the "anthropic-oauth", line from lingxi-code/Cargo.toml members
```
Edit `lingxi-code/Cargo.toml` to delete the `"anthropic-oauth",` member line.

- [ ] **Step 7: Build + test**

```bash
cd lingxi-code
CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 cargo build --workspace --tests
CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 cargo test -p llm-client -p migrations -p test-harness
```
Expected: PASS. If a moved file fails on an unresolved path, it's almost always a `crate::`/`llm_client::` rewrite miss — fix the specific path. If a consumer fails, a re-point was missed.

- [ ] **Step 8: Commit**

```bash
git add -A
git commit -m "refactor(llm-client): fold anthropic-oauth into llm_client::oauth::anthropic; delete crate"
```

---

## Task 2: Fold `openai-oauth` → `llm_client::oauth::openai`

Identical method to Task 1, for the second crate. Deps were added in Task 1 (openai-oauth's deps are the same set).

**Files:**
- Create: `lingxi-code/llm-client/src/oauth/openai/` (moved files); Modify: `llm-client/src/oauth/mod.rs`
- Delete: `lingxi-code/openai-oauth/`; remove its member
- Modify: consumers importing `openai_oauth::…` (notably `apps/engine-desktop/src/connect.rs`)

**Interfaces:**
- Produces: `llm_client::oauth::openai::{client, config, handle, refresh, device_code, external_tokens, pat, token_data, credential_provider, pkce, callback, testsupport, …}` (e.g. `OpenAiOAuthHandle`, `OpenAiOAuthClient`), matching the crate's old `lib.rs` re-exports.

- [ ] **Step 1: Move the source files**

```bash
cd lingxi-code
mkdir -p llm-client/src/oauth/openai
git mv openai-oauth/src/lib.rs llm-client/src/oauth/openai/mod.rs
for f in callback client config credential_provider device_code external_tokens handle pat pkce refresh testsupport token_data; do
  git mv openai-oauth/src/$f.rs llm-client/src/oauth/openai/$f.rs
done
ls openai-oauth/tests/ 2>/dev/null && git mv openai-oauth/tests/* llm-client/tests/ 2>/dev/null || true
```

- [ ] **Step 2: Rewrite intra-crate paths**

```bash
cd lingxi-code
for f in llm-client/src/oauth/openai/*.rs; do
  sed -i '' -e 's/\bcrate::/crate::oauth::openai::/g' -e 's/\bllm_client::/crate::/g' "$f"
done
```

- [ ] **Step 3: Wire into the oauth module**

In `lingxi-code/llm-client/src/oauth/mod.rs` add `pub mod openai;`.

- [ ] **Step 4: Re-point openai consumers + drop the crate dep**

`grep -rnE 'use openai_oauth|openai_oauth::' lingxi-code --include='*.rs' | grep -v '/target/'`. Replace `openai_oauth::` → `llm_client::oauth::openai::` (notably `apps/engine-desktop/src/connect.rs:62,217,224,228`). Remove `openai-oauth = …` from each consumer's `Cargo.toml`; ensure `llm-client` dep present.

- [ ] **Step 5: Delete the crate + member**

```bash
cd lingxi-code
rm -rf openai-oauth
```
Remove the `"openai-oauth",` member line from `lingxi-code/Cargo.toml`.

- [ ] **Step 6: Build + test**

```bash
cd lingxi-code
CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 cargo build --workspace --tests
CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 cargo test -p llm-client -p engine-desktop
```
Expected: PASS.

- [ ] **Step 7: Commit**

```bash
git add -A
git commit -m "refactor(llm-client): fold openai-oauth into llm_client::oauth::openai; delete crate"
```

---

## Task 3: Workspace-wide green + cleanup

**Files:** none expected; integration gate + dead-dep/import sweep.

- [ ] **Step 1: Full workspace build + test**

```bash
cd lingxi-code
CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 cargo build --workspace --tests
CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 cargo test --workspace --no-fail-fast
```
Expected: PASS (known-flaky powershell/web_fetch may need isolated re-run).

- [ ] **Step 2: Confirm the crates are gone + no dangling refs**

```bash
ls lingxi-code/anthropic-oauth lingxi-code/openai-oauth 2>/dev/null && echo "STILL PRESENT — investigate" || echo "both crates deleted ✓"
grep -rnE 'anthropic_oauth::|openai_oauth::|anthropic-oauth|openai-oauth' lingxi-code --include='*.rs' --include='Cargo.toml' | grep -v '/target/' | grep -v 'oauth::anthropic\|oauth::openai' || echo "no dangling references ✓"
```
Any remaining hit (outside doc comments) is a missed re-point — fix it.

- [ ] **Step 3: Commit (if any straggler fixes)**

```bash
cd lingxi-code
git add -A
git commit -m "refactor(llm-client): oauth merge — workspace-wide green; straggler re-points"
```

---

## Self-Review

**Spec coverage (Plan C = spec Phase 6):**
- ✅ anthropic-oauth → llm_client::oauth::anthropic — Task 1.
- ✅ openai-oauth → llm_client::oauth::openai — Task 2.
- ✅ crates deleted, members removed, ~6 consumers re-pointed — Tasks 1/2.
- ✅ no new heavy deps; cycle-checked (secret/telemetry/protocol/traits clean) — Task 1 Step 1.
- ✅ byte-identical OAuth flows; green per task.

**Placeholder scan:** No TBD. The path-rewrite is a concrete ordered sed (not "fix imports"); the consumer re-point is a concrete grep+replace with the known sites listed.

**Type consistency:** module paths `llm_client::oauth::{anthropic,openai}::…` are used identically in the Interfaces blocks, the consumer re-points, and the dangling-ref grep in Task 3.

**Risk note:** the sed path-rewrite is the main hazard — it also rewrites `crate::`/`llm_client::` inside doc comments and strings (harmless for the build). The build+test gate per task catches any rewrite miss. If a moved file references a `secret::`/`protocol::` item that turns out to need a feature not enabled, add it to llm-client's dep and note it.
