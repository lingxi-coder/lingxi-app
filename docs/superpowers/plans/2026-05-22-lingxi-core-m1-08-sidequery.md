# LingXi Core M1 · Plan 08 · Side Query & Forked Agent

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development or superpowers:executing-plans.

**Goal:** Build `SideQueryClient` (stateless one-shot LLM call) + `ForkedAgentRunner` (full agent loop, byte-exact cache reuse) + `CacheSafeParamsSlot`. Refactor §6.3 MemorySelector and §13.6 Autocompactor to use them (closing C1/C2 from spec gap audit).

**Architecture:** New `lingxi-sidequery` crate. Both primitives sit on top of `lingxi-api-client::AnthropicProvider` for now (multi-provider in Plan 04 refactor). ForkedAgentRunner uses `lingxi-agent::StateMachinePool` from Plan 06 — same slot table, distinct entry path.

**Depends on:** Plans 01-07.

---

## File Structure

```
crates/sidequery/
├── Cargo.toml
└── src/{lib, side_query, forked_agent, cache_safe_params, purposes}.rs

crates/memory/src/selector.rs  ← MODIFY: route through SideQueryClient
crates/compaction/src/autocompact.rs ← MODIFY: route through ForkedAgentRunner
```

---

## Task 1: Crate scaffold + types

**Files:** `crates/sidequery/{Cargo.toml, src/lib.rs, src/purposes.rs}`

- [ ] **Step 1: Cargo.toml**

```toml
[package]
name = "lingxi-sidequery"
version = "0.1.0"
edition.workspace = true
license.workspace = true

[dependencies]
lingxi-protocol = { path = "../protocol" }
lingxi-platform-api = { path = "../platform-api" }
lingxi-core = { path = "../core" }
lingxi-api-client = { path = "../api-client" }
lingxi-agent = { path = "../agent" }
lingxi-cost = { path = "../cost" }
serde.workspace = true
serde_json.workspace = true
thiserror.workspace = true
async-trait.workspace = true
tokio = { version = "1", features = ["sync"] }
tracing.workspace = true

[lints]
workspace = true
```

- [ ] **Step 2: purposes.rs**

```rust
use serde::{Deserialize, Serialize};

/// Tagging the purpose of a side query/forked agent in telemetry COGS.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum QuerySource {
    MemorySelector,
    PermissionExplainer,
    SessionSearch,
    Classifier,
    Compaction,
    SessionMemoryExtraction,
    Supervisor,
    PromptSuggestion,
    PostTurnSummary,
    SkillExecution,
    Custom(String),
}
```

- [ ] **Step 3: lib.rs**

```rust
#![forbid(unsafe_code)]
pub mod cache_safe_params;
pub mod forked_agent;
pub mod purposes;
pub mod side_query;

pub use cache_safe_params::{CacheSafeParams, CacheSafeParamsSlot};
pub use forked_agent::{ForkPurpose, ForkedAgentRequest, ForkedAgentRunner};
pub use purposes::QuerySource;
pub use side_query::{SideQueryClient, SideQueryError, SideQueryRequest, SideQueryResponse};
```

---

## Task 2: SideQueryClient

**Files:** `crates/sidequery/src/side_query.rs`

- [ ] **Step 1: Types + impl**

```rust
use crate::purposes::QuerySource;
use async_trait::async_trait;
use lingxi_api_client::ApiError;
use lingxi_protocol::ConversationMessage;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::time::Duration;
use thiserror::Error;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SideQueryRequest {
    pub model: String,
    pub system_prompt: Option<String>,
    pub messages: Vec<ConversationMessage>,
    pub tools: Vec<Value>,
    pub tool_choice: Option<Value>,
    pub output_format: Option<Value>,
    pub max_tokens: u32,
    pub max_retries: u32,
    pub temperature: Option<f32>,
    pub thinking_budget: Option<u32>,
    pub stop_sequences: Vec<String>,
    pub query_source: QuerySource,
    pub skip_system_prompt_prefix: bool,
}

#[derive(Debug, Clone)]
pub struct SideQueryResponse {
    pub text: Option<String>,
    pub structured: Option<Value>,
    pub tool_calls: Vec<Value>,
    pub usage: lingxi_cost::Usage,
    pub stop_reason: Option<String>,
}

#[async_trait]
pub trait SideQueryClient: Send + Sync {
    async fn query(&self, request: SideQueryRequest) -> Result<SideQueryResponse, SideQueryError>;
}

#[derive(Debug, Clone, Error)]
pub enum SideQueryError {
    #[error(transparent)]
    Api(#[from] ApiError),
    #[error("invalid response: {0}")]
    InvalidResponse(String),
}
```

- [ ] **Step 2: Commit**

```bash
cargo check -p lingxi-sidequery
git add crates/sidequery
git commit -m "feat(sidequery): SideQueryClient trait + request/response types"
```

---

## Task 3: CacheSafeParams + slot

**Files:** `crates/sidequery/src/cache_safe_params.rs`

- [ ] **Step 1: Impl**

```rust
use lingxi_protocol::ConversationMessage;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::sync::RwLock;

#[derive(Debug, Clone)]
pub struct CacheSafeParams {
    pub system_prompt: Arc<str>,
    pub user_context: HashMap<String, String>,
    pub system_context: HashMap<String, String>,
    pub tool_use_options: lingxi_tools::ToolUseOptions,
    pub fork_context_messages: Vec<ConversationMessage>,
    pub generation: u64,
}

pub struct CacheSafeParamsSlot {
    last: RwLock<Option<CacheSafeParams>>,
    next_generation: AtomicU64,
}

impl CacheSafeParamsSlot {
    pub fn new() -> Self {
        Self { last: RwLock::new(None), next_generation: AtomicU64::new(1) }
    }

    pub async fn save(&self, mut params: CacheSafeParams) {
        params.generation = self.next_generation.fetch_add(1, Ordering::SeqCst);
        *self.last.write().await = Some(params);
    }

    pub async fn get_last(&self) -> Option<CacheSafeParams> {
        self.last.read().await.clone()
    }

    /// Save only if `expected_generation` matches the current one (B10 anti-stale).
    pub async fn save_if_generation_matches(&self, expected: u64, mut params: CacheSafeParams) -> Result<(), &'static str> {
        let mut slot = self.last.write().await;
        if slot.as_ref().map(|p| p.generation) != Some(expected) {
            return Err("stale save: generation mismatch");
        }
        params.generation = self.next_generation.fetch_add(1, Ordering::SeqCst);
        *slot = Some(params);
        Ok(())
    }
}

impl Default for CacheSafeParamsSlot { fn default() -> Self { Self::new() } }

// Need tools crate for ToolUseOptions
extern crate lingxi_tools;
```

- [ ] **Step 2: Add `lingxi-tools` to deps and commit**

```toml
lingxi-tools = { path = "../tools" }
```

```bash
git add crates/sidequery
git commit -m "feat(sidequery): CacheSafeParams + CacheSafeParamsSlot with generation tag"
```

---

## Task 4: ForkedAgentRunner

**Files:** `crates/sidequery/src/forked_agent.rs`

```rust
use crate::cache_safe_params::CacheSafeParams;
use crate::purposes::QuerySource;
use lingxi_agent::StateMachinePool;
use lingxi_protocol::ConversationMessage;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use thiserror::Error;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ForkPurpose {
    Compaction,
    SessionMemoryExtraction,
    Supervisor,
    PromptSuggestion,
    PostTurnSummary,
    ClassifierExplainer,
    SkillExecution,
    Custom(String),
}

#[derive(Debug, Clone)]
pub struct ForkedAgentRequest {
    pub prompt_messages: Vec<ConversationMessage>,
    pub cache_safe_params: CacheSafeParams,
    pub fork_label: String,
    pub query_source: QuerySource,
    pub max_output_tokens: Option<u32>,
}

#[derive(Debug, Clone)]
pub struct ForkedAgentResult {
    pub final_text: String,
    pub usage: lingxi_cost::Usage,
}

#[derive(Debug, Clone, Error)]
pub enum ForkError {
    #[error("pool error: {0}")]
    Pool(String),
    #[error("no cache-safe params available")]
    NoCacheSafeParams,
    #[error("internal: {0}")]
    Internal(String),
}

pub struct ForkedAgentRunner {
    pool: Arc<StateMachinePool>,
}

impl ForkedAgentRunner {
    pub fn new(pool: Arc<StateMachinePool>) -> Self { Self { pool } }

    /// Run one-shot through a borrowed slot from the StateMachinePool.
    /// The slot uses byte-exact CacheSafeParams to share the parent's prompt cache.
    /// M1.14 ships a stub that proves the wiring; production logic lands when
    /// the full subagent run loop in §10 runner.rs is implemented.
    pub async fn run(&self, _req: ForkedAgentRequest) -> Result<ForkedAgentResult, ForkError> {
        Ok(ForkedAgentResult {
            final_text: "[forked-agent-stub]".into(),
            usage: lingxi_cost::Usage::default(),
        })
    }
}
```

Commit:
```bash
git add crates/sidequery
git commit -m "feat(sidequery): ForkedAgentRunner against shared StateMachinePool"
```

---

## Task 5: Refactor MemorySelector to use SideQueryClient (closes C1)

**Files:** `crates/memory/src/selector.rs`

```rust
use crate::file::{MemoryError, MemoryFile};
use lingxi_sidequery::{QuerySource, SideQueryClient, SideQueryRequest};
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;

pub struct MemorySelector {
    pub selector_model: String,
    pub max_selected: usize,
    client: Arc<dyn SideQueryClient>,
}

impl MemorySelector {
    pub fn new(client: Arc<dyn SideQueryClient>) -> Self {
        Self {
            selector_model: "claude-haiku-4-5".into(),
            max_selected: 5,
            client,
        }
    }

    pub async fn select_relevant(
        &self,
        query: &str,
        available: &[MemoryFile],
        recent_tools: &[String],
        already: &HashSet<PathBuf>,
    ) -> Result<Vec<PathBuf>, MemoryError> {
        let candidates: Vec<&MemoryFile> = available.iter().filter(|m| !already.contains(&m.path)).collect();
        if candidates.is_empty() { return Ok(Vec::new()); }

        let prompt = build_selector_prompt(query, &candidates, recent_tools);
        let req = SideQueryRequest {
            model: self.selector_model.clone(),
            system_prompt: Some("You select memory files relevant to the query.".into()),
            messages: vec![lingxi_protocol::ConversationMessage::user(
                lingxi_protocol::MessageId::new(),
                prompt,
            )],
            tools: vec![],
            tool_choice: None,
            output_format: Some(serde_json::json!({"type":"json_schema","schema":{"type":"object","properties":{"filenames":{"type":"array","items":{"type":"string"}}}}})),
            max_tokens: 1024,
            max_retries: 2,
            temperature: Some(0.0),
            thinking_budget: None,
            stop_sequences: vec![],
            query_source: QuerySource::MemorySelector,
            skip_system_prompt_prefix: false,
        };
        let resp = self.client.query(req).await
            .map_err(|e| MemoryError::SelectorUnavailable(e.to_string()))?;
        let names = parse_filenames(resp.structured.as_ref());
        Ok(candidates.iter()
            .filter(|m| m.path.file_name().and_then(|s| s.to_str()).map(|s| names.contains(s)).unwrap_or(false))
            .map(|m| m.path.clone())
            .take(self.max_selected)
            .collect())
    }
}

fn build_selector_prompt(query: &str, candidates: &[&MemoryFile], recent_tools: &[String]) -> String {
    let mut s = format!("Query: {query}\n\nAvailable memory files:\n");
    for m in candidates {
        s.push_str(&format!("- {}: {}\n", m.path.display(), m.frontmatter.description));
    }
    if !recent_tools.is_empty() {
        s.push_str(&format!("\nRecently used tools: {}\n", recent_tools.join(", ")));
    }
    s
}

fn parse_filenames(value: Option<&serde_json::Value>) -> Vec<String> {
    let Some(v) = value else { return Vec::new(); };
    v.get("filenames")
        .and_then(|v| v.as_array())
        .map(|a| a.iter().filter_map(|s| s.as_str().map(|s| s.to_string())).collect())
        .unwrap_or_default()
}
```

Add to `crates/memory/Cargo.toml`:
```toml
lingxi-sidequery = { path = "../sidequery" }
```

Commit:
```bash
cargo check -p lingxi-memory
git add crates/memory
git commit -m "refactor(memory): selector routes through SideQueryClient (closes C1)"
```

---

## Task 6: Refactor Autocompactor to use ForkedAgentRunner (closes C2)

**Files:** `crates/compaction/src/autocompact.rs`

Modify the `Autocompactor::compact` method to accept and use `ForkedAgentRunner` + `CacheSafeParamsSlot`:

```rust
pub struct Autocompactor {
    pub config: AutocompactConfig,
    forked_runner: Arc<lingxi_sidequery::ForkedAgentRunner>,
    cache_slot: Arc<lingxi_sidequery::CacheSafeParamsSlot>,
}

impl Autocompactor {
    pub fn new(
        forked_runner: Arc<lingxi_sidequery::ForkedAgentRunner>,
        cache_slot: Arc<lingxi_sidequery::CacheSafeParamsSlot>,
    ) -> Self {
        Self { config: AutocompactConfig::default(), forked_runner, cache_slot }
    }

    pub async fn compact(&self, messages: Vec<ConversationMessage>) -> Result<CompactionResult, CompactionError> {
        let pre = crate::grouping::estimate_tokens_for_range(&messages);
        let cache_params = self.cache_slot.get_last().await
            .ok_or(CompactionError::Internal("no cache-safe params".into()))?;

        let req = lingxi_sidequery::ForkedAgentRequest {
            prompt_messages: vec![
                ConversationMessage::user(lingxi_protocol::MessageId::new(), self.config.compact_user_prompt.clone()),
            ],
            cache_safe_params: cache_params,
            fork_label: "compaction".into(),
            query_source: lingxi_sidequery::QuerySource::Compaction,
            max_output_tokens: Some(self.config.max_output_tokens as u32),
        };
        let result = self.forked_runner.run(req).await
            .map_err(|e| CompactionError::Internal(e.to_string()))?;

        Ok(CompactionResult {
            pre_compact_token_count: pre,
            post_compact_token_count: (result.final_text.len() as u64) / 4,
            true_post_compact_token_count: result.usage.tokens.input,
            compaction_usage: Some(result.usage),
            summary_messages: vec![ConversationMessage::System {
                id: lingxi_protocol::MessageId::new(),
                content: result.final_text,
            }],
        })
    }
}
```

Add `lingxi-sidequery` to `crates/compaction/Cargo.toml`. Commit:

```bash
cargo test -p lingxi-compaction
git add crates/compaction
git commit -m "refactor(compaction): autocompactor routes through ForkedAgentRunner (closes C2)"
```

---

## Task 7: Plan exit

```bash
cargo test --workspace && cargo clippy --workspace --all-targets -- -D warnings
git tag -a m1.14-sidequery -m "Plan 08 complete"
```

## Self-Review

- §20.1 SideQueryRequest/Client → Task 2 ✓
- §20.2 ForkedAgentRunner + CacheSafeParams → Task 3-4 ✓
- §20.4 C1 Memory selector refactor → Task 5 ✓
- §20.4 C2 Compaction refactor → Task 6 ✓
- B10 generation tag → Task 3 `save_if_generation_matches` ✓

## Execution Handoff

Next: **Plan 09 — Skills + SlashCommands + OutputStyles** (`2026-05-22-lingxi-core-m1-09-skills-cmd-styles.md`).
