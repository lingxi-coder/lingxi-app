# Desktop Decision

Decision is a host-global service independent from chat sessions. Open Settings → Model & Services → Decision to choose OpenAI, Jev, or a self-hosted service. Save the configuration before using the test panel. Test results do not execute tools or approve actions.

## Services

- OpenAI: `https://api.openai.com/v1/decisions`, initially `gpt-6-luna`. Use an enabled first-party OpenAI API-key account or a dedicated key. ChatGPT subscription credentials are not admitted.
- Jev: `https://api.typesafe.ai/v1/systemone`, initially `jev-1.14.0`, with a dedicated TypeSafe API key.
- Self-hosted: enter the complete resource URL, protocol and model. Examples: `http://127.0.0.1:8000/v1/systemone` or `https://decision.example.net/v1/decisions`. Authentication is optional. A Chat Completions endpoint is not a Decision endpoint. Start and manage the server yourself; Desktop neither installs nor runs model weights.

The default is unconfigured. One explicitly saved service handles all Decision requests on this Desktop. There is no automatic provider fallback or replay. Each operation has a 30-second total budget and can be cancelled. Requests already in flight retain their captured configuration.

## Test panel

Provide text or JSON evidence, optionally attach PNG/JPEG images, and add uniquely named questions:

- Condition probability: instructions defining a yes/no proposition.
- Choice: one `ID=description` option per line.
- Score: two to ten descriptive levels, one per line from lowest to highest.

The result includes answers, available probability/confidence information, observed token usage, actual model when reported, and latency. Missing statistics remain unknown. Image support depends on the selected server; Jev's image feature is experimental upstream. Unsupported requests surface the service error and do not change backend.

## Architecture and storage

`decision-api` owns the shared contracts; `decision-runtime` converts those contracts to provider-native operations and validates replies. `llm-client` owns HTTP wire execution and provider resources. Its former Structured Outputs `client.decisions()` facade is removed; ordinary Structured Outputs remains available through Chat.

Electron Main owns configuration, account resolution, cancellation and subprocess lifecycle. The session-independent `bridge-server --decision-service-json` mode receives one bounded private envelope through stdin, evaluates it and returns one bounded JSON response. Credentials are excluded from argv, environment, settings snapshots and diagnostic output.

Configuration lives in the existing Desktop user-data directory as `decision.json`. It uses atomic replacement and revision checks. API keys use the signed credential broker on macOS and Electron system encryption on Windows/Linux. Linux's `basic_text` fallback is rejected. Keys are bound to the selected endpoint; changing the endpoint does not forward a saved key.

The feature is exposed only to trusted Desktop windows. Mobile clients and existing Computer use/classifier callers are not wired to it in this release.

## Development

This change spans `llm-client`, `harness-runtime`, and `lingxi-app`. Workspace dependencies pin published upstream Git commits, including the Decision API/runtime crates; no sibling checkout or local Cargo patch is required.

Primary wire references: [OpenAI Decisions](https://developers.openai.com/api/docs/guides/decisions), [TypeSafe System One](https://docs.typesafe.ai/api/system-one), [TypeSafe image assets](https://docs.typesafe.ai/images).

## Agent tool and Skill

Desktop exposes `Decision` as an ordinary foreground tool. The schema accepts only evidence (`state`), named `questions`, and optional inline `images`. The normal tool dispatcher owns its call ID, status, history, permission rules and cancellation. No new TaskRegistry task type or background/recovery queue is created.

The connection-scoped Rust proxy forwards one request over the existing authenticated bridge to Electron Main. Main uses the same global DecisionHost as the settings test panel. Cancelling the tool, dropping its future, disconnecting or closing the host terminates the owned request and discards late results. One request is never automatically replayed. Provider endpoints, credentials and model binding remain host-owned.

The `decision` Skill is injected only when a service is configured. Official services need a saved account or dedicated key reference; unauthenticated custom services need only their saved configuration. Capability discovery does not resolve credentials or probe provider health. Disabling removes the Skill from subsequent listings, direct loads, Agent preloads, pending discovery results and compaction/projection restoration. Previously recorded conversation messages remain unchanged.

The tool definition remains static across configuration changes. Skill names, descriptions and source order remain static; endpoint, key, model, revision and elapsed time never appear in capability metadata. Unchanged availability emits no update. Enabling appends only the new Skill listing; disabling appends a short withdrawal notice; enabling again restores only that listing. Compaction creates a fresh bounded listing. Skill bodies load on demand through the existing Skill pipeline.

This follows [Claude Code's progressive disclosure and invoked-skill lifecycle](https://code.claude.com/docs/en/skills#skill-content-lifecycle) and the snapshot/delta approach in [Codex's public skill catalog](https://github.com/openai/codex/blob/main/codex-rs/ext/skills/src/catalog_delta.rs). Preserving prior messages and stable tool definitions supports [prompt-prefix cache reuse](https://developers.openai.com/api/docs/guides/prompt-caching). Behavioral tests establish stable metadata and minimal deltas; they do not measure live provider cache-hit billing.
