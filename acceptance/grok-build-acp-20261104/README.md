# Grok Build / ACP integration investigation

Date: 2026-11-04. Product baseline: `13247fe5f65d92b0dc646c7c02602da7985a2a01`.

## Result

The official Grok CLI can be embedded as an external agent over ACP, including
its account authorization flow. This is a viable integration direction, but it
is not a drop-in OAuth option for LingXi's existing xAI model provider. No
production login option has been enabled by this investigation.

The existing xAI provider and all of its API Key accounts are unchanged. An ACP
initialization smoke does not establish successful account authorization,
subscription eligibility, inference, tool approval, or mobile support.

## Evidence

- Official [Grok Build overview](https://docs.x.ai/build/overview) explicitly
  describes ACP integration in other apps.
- Official [headless/ACP documentation](https://docs.x.ai/build/cli/headless-scripting)
  starts the agent with `grok agent stdio`. `session/prompt` completes with
  metadata; assistant text arrives through `session/update` notifications.
- Official [enterprise authentication documentation](https://docs.x.ai/build/enterprise)
  documents browser OIDC, device authorization, refresh, and API Key login.
  Session inference uses `cli-chat-proxy.grok.com`; the direct API Key path uses
  `api.x.ai`. These are distinct execution routes.
- Official [settings documentation](https://docs.x.ai/build/settings) documents
  `GROK_HOME` as the config-directory override.
- Official [ACP implementation guide](https://github.com/xai-org/grok-build/blob/main/crates/codegen/xai-grok-pager/docs/user-guide/15-agent-mode.md)
  describes sessions, reasoning, tools, permission requests, and extensions.
- Official [auth extension implementation](https://github.com/xai-org/grok-build/blob/main/crates/codegen/xai-grok-shell/src/extensions/auth.rs)
  includes `x.ai/auth/get_url`, `submit_code`, `cancel`, `info`,
  `check_subscription`, and `getBearerToken`. The CLI owns its auth store.
  Cancellation can bind `request_seq` to avoid cancelling a newer attempt.
- Official [ACP agent implementation](https://github.com/xai-org/grok-build/blob/main/crates/codegen/xai-grok-shell/src/agent/mvp_agent/acp_agent.rs)
  drives interactive authentication in `authenticate`. A successful login must
  still pass the agent's access/subscription gate before task execution.
- Official [auth-method implementation](https://github.com/xai-org/grok-build/blob/main/crates/codegen/xai-grok-shell/src/agent/auth_method.rs)
  explicitly describes auth as process-global: one user and one AuthManager.
  Multiple LingXi accounts therefore need distinct homes AND distinct processes;
  merely switching an account name on one shared CLI process is insufficient.
- Official [login config](https://github.com/xai-org/grok-build/blob/main/crates/codegen/xai-grok-login/src/config.rs)
  defines CLI OAuth scopes and its own client ID. This is not evidence that
  LingXi has a separately registered third-party OAuth client. Token-export
  capability alone does not verify that using that token as a normal xAI API
  credential is supported.

Local executable: `grok 1.0.70 (f50904eaee6c)`.
Upstream source-tree SHA observed during research:
`2bdd1d6a6369de0e8c68132ea4539e9abd9e14a8` (documentation/source links above
track main and may subsequently change).

## Local compatibility probe

Run from the product repository:

```sh
python3 acceptance/grok-build-acp-20261104/probe.py \
  --binary /absolute/path/to/grok
```

The probe sends only ACP `initialize`, using an isolated temporary HOME,
GROK_HOME, cwd, a credential-free allowlisted environment, and
`--no-auto-update agent --no-leader stdio`. It does not send a prompt, create a
session, invoke `authenticate`, read an existing Grok auth store, or print raw
protocol/auth data. It terminates only its owned process group and removes its
own scratch directory.

Observed: protocol version 1, interactive auth method `grok.com` / `Grok`,
load/list/resume/close session capabilities, image prompts, MCP HTTP/SSE, and
Grok hook capabilities. No auth.json was created. This confirms local protocol
compatibility only; it is not end-to-end acceptance.

## Current LingXi constraints

- `apps/electron/src/main/provider-accounts.ts` accepts OAuth only for logical
  OpenAI accounts bound to `openai-chatgpt`; its stored OAuth shape and refresh
  authority use the Codex implementation.
- Harness `crates/runtime/src/provider_accounts.rs` also gates its browser and
  device coordinators on `openai-chatgpt`. It cannot safely authorize Grok by
  changing a renderer capability flag.
- The current model request path is owned by llm-client, with the LingXi Harness
  executing tools. ACP instead invokes the Grok agent and Grok's tools.
  No existing ACP external-agent execution route was found in these runtime
  sources. Treating an ACP session as a model bearer token would lose those
  execution semantics.
- `packages/mobile-ffi/src/remote.rs` explicitly withholds remote OAuth actions
  until the execution Host has browser authorization authority. A desktop ACP
  login must not imply local Android/iOS login works.

## Integration contract if the external-agent route is selected

Keep one logical xAI provider with clearly named connections: current API Key
model connection, and Grok Build account/agent connection. The latter must be
explicit about which runtime executes the task. Never silently change the
runtime of an existing chat or switch from subscription access to paid API Key
usage on authorization failure.

Each Grok account requires its own protected CLI home, process, login-attempt
identity and lifecycle. Use the current account UUID/generation/revision as the
authority for add, reconnect, cancel, remove and selection. Disable shared-leader
reuse, remove ambient xAI keys from account-login processes, and preserve the
user's global Grok home. Prevent old callbacks/refreshes from recreating a
removed account or overwriting another account's authorization. The existing
Credential Broker contract must be reconciled with CLI-owned token persistence
before enabling production login.

Add ACP request correlation, bounded frames/timeouts, cancellation, stream and
error normalization, durable session mapping, and explicit tool approval. Use
Grok's advertised methods/capabilities; do not start it with `--always-approve`.
Do not advertise fs/terminal capabilities until implemented. Tool execution and
permissions must be connected before a prompt is allowed, because the agent
has built-in tools independently of client-advertised capabilities.

For mobile, a Desktop execution Host is a possible route; it requires an
authenticated Host login/callback protocol and honest capability reporting.
No local Android/iOS CLI integration has been established by this investigation.

Required acceptance before enabling: real login and subscription check, two
independent accounts, cancel/reconnect/remove races, token refresh and expiry,
streamed reply, tool allow/deny, task cancellation, session restart/resume,
unchanged API Key behavior, explicit billing fallback, signed Desktop package,
and affected mobile Host workflows.

## User decision

The user requires the existing LingXi model-call and tool-execution loop and
declined the external Grok Build agent connection. The proposed ACP integration
contract above is therefore not selected for implementation.

Keep the existing xAI API Key connection and its multiple accounts. Do not add
an ACP connection or an account-login button backed by the Grok agent. A future
account authorization option must have a verified model-inference route that
preserves LingXi's existing execution engine, account isolation and API Key
support. This investigation has not established such a supported route; it
does not prove that no such route can exist.
