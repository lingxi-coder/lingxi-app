# Provider resources & hosted tools

Bind native resources explicitly and separate provider-side execution from Harness tools and background work.

<a id="native-binding"></a>

## Bind one provider and exact connection

client.provider::<T>(profile) validates provider identity and profile scope. Live provider handles capture a fresh immutable configuration at each operation; ClientSnapshot-derived handles use that retained revision. A resource reference records the account/endpoint/profile needed for reuse. Configuration replacement can invalidate a binding instead of rerouting it to another account.

| Typed client | Implemented resource families |
| --- | --- |
| OpenAiClient | files, embeddings, images, retrieval, batches, background, containers, audio, realtime/live |
| AnthropicClient | files, batch, provider Skills, Messages-native hosted tools and hosted wrappers |
| GoogleClient | files, embeddings, file_search, batch, context_cache, interactions, speech/voices, Vertex speech, Live |
| QwenClient | files, embeddings, images, batch, rerank, knowledge, ASR/TTS/AudioGen and typed realtime services |
| XaiClient | files(), collections(config), batch(scope), audio(config), deferred(), images(); native realtime connectors |
| OpenRouterClient | files(), embeddings(), images(), audio(), batch(scope), rerank(scope); typed hosted server tools |
| MiniMaxClient | files(), images(), audio(endpoint), tts(config), async_tts(config), voices(config); incremental speech connectors |
| ZhipuClient | files(), embeddings(), images(), batch(scope), knowledge(), async_tasks(config), cloud_audio(scope), asr(route, scope); native realtime |
| KimiClient | files(), batch(scope), chat(); file extraction remains distinct from general model-file input |
| DeepSeekClient / GithubCopilotClient | Bound chat and account resources; no inferred file/image/speech resource from chat compatibility |

Resource families describe code entry points, not universal upstream support. Check the operation’s typed scope, concrete route and authentication restrictions. Use Rustdoc for complete method signatures and provider payload types.

<a id="tool-execution-choice"></a>

## Choose who executes the tool

| Mechanism | Executes on | Application integration |
| --- | --- | --- |
| ChatRequest.tools | Your host | Validate completed tool calls, apply permission policy, run registered code and append ToolResult |
| ChatRequest.hosted_tools | Provider infrastructure | Enable an admitted typed tool and retain provider-native results, citations and usage |
| Hosted remote MCP | Remote MCP service via the provider | Supply request-local server authorization; handle any provider approval/continuation outcome |
| Harness local tools / MCP | Harness-managed host execution | Use Harness registration, permissions, lifecycle and durable session integration |

- [Complete a local function round trip](llm-requests.md#function-tool-roundtrip)

<a id="cloud-job-recovery"></a>

## Persist cloud job identity before polling

1. Persist the returned resource reference with the application job record immediately after a successful submit. Keep profile, account, endpoint and resource scope together.
2. After a restart, query the saved reference with fresh credentials for the same account. Polling is a read operation on the existing job, not another submit.
3. Distinguish cancel from delete. Cancelling asks the provider to stop work; deleting a stored result is a separate resource-lifecycle action where supported.
4. If submit/cancel acknowledgement is lost, retain the unknown-outcome state and reconcile against supported provider APIs. Do not turn a network error into a second side effect.

<a id="tools-boundaries"></a>

## Local functions and provider-side tools

ChatRequest.tools declares functions the host executes. ToolUse events carry data back to Harness; permissions, sandboxing, tool-result persistence and interaction prompts belong to Harness. ChatRequest.hosted_tools declares work the provider executes. Native hosted output survives in ProviderContent and native metadata; it must not be dispatched as a local function call.

- Web search uses hosted provider search only. chat().web_search_in adds WebSearchConfig and preserves citations/metadata; unsupported profile/model/search options fail explicitly. There is no internal search backend fallback.
- File search, code execution, shell, computer use, tool search, URL/Maps grounding, remote MCP and provider Skills are route-specific typed extensions. A protocol label alone does not admit them. Anthropic provider Skills differ from Harness local SKILL.md discovery.
- Hosted remote MCP authorizations are fresh secrets in RequestOptions.mcp_authorizations, indexed by declared server label/name. They are not serialized into ChatRequest or history. Provider approval output can require host interaction and explicit continuation.
- Execution-safety admission pins side-effecting hosted requests and suppresses unsafe automatic resends/failover. Token estimates exclude provider tool/container charges. Preserve both usage and server-tool observations for reconciliation.

<a id="native-jobs"></a>

## Native jobs and durable polling

Persist the returned native reference in the host, then call get once per chosen polling interval with the same account. OpenAI background has distinct submit/get/cancel/delete and resumable event APIs; delete removes a stored response and does not cancel inference. SubmitOutcomeUnknown and CancelOutcomeUnknown are reconciliation states, not invitations to resubmit.

Provider Batch services accept their own typed request records or JSONL files; output identities/statuses/usage stay provider-native. xAI deferred, GLM async, MiniMax async TTS and image tasks retain their own reference types and terminal states. These are cloud jobs, distinct from Harness task execution, Cron and Workflow scheduling.

### Submit one background response and return a durable reference

```rust
use lingxi_llm_client::{LlmClient, RequestOptions};
use lingxi_llm_client::protocol::ChatRequest;
use lingxi_llm_client::providers::{OpenAiClient, openai::background::BackgroundJobRef};

pub async fn submit_background(
    client: &LlmClient,
    request: &ChatRequest,
    options: &RequestOptions,
) -> Result<BackgroundJobRef, Box<dyn std::error::Error>> {
    // Supply the selected account's credential and stable account_scope.
    let provider = client.provider::<OpenAiClient>("openai")?;
    let job = provider.background().submit(request, options).await?;
    Ok(job.reference)
}
```

Verification method: Compile check (llm-background-submit)

<a id="native-failure"></a>

## Scope, errors and acceptance evidence

- Carry exact account/resource/workspace scopes through file, container, cache, job and retrieval references. A profile rename, changed endpoint or wrong credential scope must fail instead of silently rebuilding a resource elsewhere.
- Mutations with uncertain dispatch retain typed unknown-outcome errors. Query an existing reference where possible. Hosts choose polling, retry, cancellation and durable cleanup; the SDK does not become a task scheduler.
- Reference validation, provider binding and mock wire tests establish local admission and encoding behavior. Actual provider endpoints, tool entitlements, remote MCP authorization and server-side billing require a separately recorded live-account run.

- [Execution and per-attempt accounting](llm-execution-accounting.md)
- [Scoped files and upload recovery](llm-files.md)

## Sources and documentation baseline

[Source repository](https://github.com/lingxi-coder/llm-client)

- llm-client/src/providers/binding.rs — Unpublished source (ad8c60f7e4ffcbe4c0c0fedc20586df873254c0b)
- llm-client/src/providers/openai/client.rs — Unpublished source (ad8c60f7e4ffcbe4c0c0fedc20586df873254c0b)
- llm-client/src/providers/google/client.rs — Unpublished source (ad8c60f7e4ffcbe4c0c0fedc20586df873254c0b)
- llm-client/src/providers/anthropic/client.rs — Unpublished source (ad8c60f7e4ffcbe4c0c0fedc20586df873254c0b)
- llm-client/src/providers/qwen/client.rs — Unpublished source (ad8c60f7e4ffcbe4c0c0fedc20586df873254c0b)
- llm-client/src/providers/xai/client.rs — Unpublished source (ad8c60f7e4ffcbe4c0c0fedc20586df873254c0b)
- llm-client/src/providers/minimax/client.rs — Unpublished source (ad8c60f7e4ffcbe4c0c0fedc20586df873254c0b)
- llm-client/src/providers/zhipu/client.rs — Unpublished source (ad8c60f7e4ffcbe4c0c0fedc20586df873254c0b)
- llm-client/src/providers/openrouter/client.rs — Unpublished source (ad8c60f7e4ffcbe4c0c0fedc20586df873254c0b)
- llm-client/src/providers/kimi/client.rs — Unpublished source (ad8c60f7e4ffcbe4c0c0fedc20586df873254c0b)
- llm-client/src/providers/deepseek/client.rs — Unpublished source (ad8c60f7e4ffcbe4c0c0fedc20586df873254c0b)
- llm-client/src/providers/github_copilot/client.rs — Unpublished source (ad8c60f7e4ffcbe4c0c0fedc20586df873254c0b)
- llm-client/src/providers/file_resources.rs — Unpublished source (ad8c60f7e4ffcbe4c0c0fedc20586df873254c0b)
- llm-client/src/providers/account_resources.rs — Unpublished source (ad8c60f7e4ffcbe4c0c0fedc20586df873254c0b)
- llm-client/src/providers/openai/background/mod.rs — Unpublished source (ad8c60f7e4ffcbe4c0c0fedc20586df873254c0b)
- llm-client/src/providers/dispatch.rs — Unpublished source (ad8c60f7e4ffcbe4c0c0fedc20586df873254c0b)
- llm-client/src/execution_safety.rs — Unpublished source (ad8c60f7e4ffcbe4c0c0fedc20586df873254c0b)
- llm-client/src/hosted_search.rs — Unpublished source (ad8c60f7e4ffcbe4c0c0fedc20586df873254c0b)
- llm-client/tests/code_interpreter.rs — Unpublished source (ad8c60f7e4ffcbe4c0c0fedc20586df873254c0b)
- llm-client/tests/provider_clients.rs — Unpublished source (ad8c60f7e4ffcbe4c0c0fedc20586df873254c0b)
- llm-client/src/protocol/llm.rs — Unpublished source (ad8c60f7e4ffcbe4c0c0fedc20586df873254c0b)
- llm-client/src/client/options.rs — Unpublished source (ad8c60f7e4ffcbe4c0c0fedc20586df873254c0b)

- llm-client: HEAD ad8c60f7e4ffcbe4c0c0fedc20586df873254c0b; Referenced input fingerprint sha256:c68d002b005c15c21681b54d3f7efe358c4c1be6ec7cb07501009d1d11938e38; Product pin ff71b2ec1139bc67cc8719d78b1735aca967bbe8

<!-- Generated from apps/web/src/data/docs. Do not edit this file directly. -->
