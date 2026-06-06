# Dynamic Function Runtime Design

> **Status**: APPROVED for spec capture
> **Date**: 2026-06-06
> **Scope**: Android-first local dynamic functions for the mobile agent, with platform-neutral contracts for later `engine-mobile` ownership.
> **Design driver**: Let LingXi learn reusable local functions from successful agent traces so later requests can call those functions directly, reducing repeated reasoning, token use, latency, and variance.

---

## 1. Goal and Non-Goals

### Goal

Add a **Local Dynamic Function Runtime** for the mobile agent product:

1. The agent can expose local reusable functions to the LLM, similar to skills/tools.
2. The LLM decides whether to call a function using compact descriptions, input schemas, permissions, version, and reliability metrics.
3. A function executes locally as sandboxed JavaScript with a fixed `run(args, host)` signature.
4. A function returns `data + ui + meta`, where `ui` is UI Schema by default and restricted HTML sandbox only when explicitly permitted.
5. Successful agent traces are distilled later by Dream into candidate functions through a dedicated `create_function` skill.
6. Candidates require validation and user confirmation before activation.
7. Every function call records success/failure, failure reason, result quality, latency, cache behavior, and version.
8. Functions are versioned, auditable, rollbackable, and repairable.

### Non-Goals

- Do not execute arbitrary Android native code, DEX, JNI, or `.so` as part of this feature.
- Do not expose arbitrary Android bridge methods to generated JS.
- Do not let generated HTML/JS control the whole app shell.
- Do not automatically enable generated functions without user confirmation in v1.
- Do not move the first implementation fully into Rust/UniFFI; the first runtime is Android-first.
- Do not make function creation block the user's live conversation turn.

---

## 2. Placement in the Existing Project

The current project already has:

- `lingxi-code/apps/engine-mobile`: the mobile composition root.
- `lingxi-code/apps/android-aar`: the Android UniFFI packager.
- `lingxi-code/client-protocol`: platform-neutral `ClientCommand` / `ClientEvent` DTOs.
- `clients/android`: Android Compose shell, currently designed as a UI shell with future engine wiring.

The v1 implementation should be **Android-first but shared-contract-shaped**:

- Android owns the first `FunctionStore`, JS execution substrate, Host API bridge, UI rendering, candidate review UI, and metrics persistence.
- The data shapes for function manifest, function version, invocation, result, permissions, metrics, and UI schema must be platform-neutral JSON so `engine-mobile` can own them later.
- Future migration should move catalog/store/stats/routing into Rust while Android keeps rendering and native capability callbacks.

---

## 3. Architecture

```text
User Prompt
  |
  v
FunctionContextBuilder
  - reads enabled functions
  - shortlists relevant functions
  - injects function prompt + catalog into LLM context
  |
  v
LLM Decision
  - answer directly
  - call local function
  - ask for missing args
  - use normal agent/tools
  |
  v
FunctionRuntime
  - validates input
  - checks permissions
  - builds scoped host object
  - executes run(args, host)
  - validates result
  |
  v
UIRenderer + MetricsRecorder + TraceRecorder
```

Core components:

- **FunctionStore**: stores identities, versions, manifests, JS source, candidate states, stats, and traces.
- **FunctionContextBuilder**: builds a compact catalog for the LLM from enabled functions and metrics.
- **FunctionRuntime**: executes `run(args, host)` in a sandboxed JS environment.
- **HostAPI**: exposes only declared and authorized capabilities such as `http`, `storage`, `time`, `location`, and gated `llm`.
- **DynamicUiRenderer**: renders UI Schema to Compose, and restricted HTML only through `html_sandbox`.
- **TraceRecorder**: stores successful execution summaries for Dream.
- **DreamDistiller**: scans traces during Dream/background windows and invokes `create_function`.
- **create_function skill**: generates function manifest, JS source, tests, permissions, and review summary.
- **MetricsRecorder**: records call status, failure reason, result quality, duration, and version.

---

## 4. Function Identity, Versioning, and Manifest

Functions have two levels:

```text
FunctionIdentity
  Logical reusable ability, e.g. get_today_weather.

FunctionVersion
  One concrete implementation of that ability, e.g. get_today_weather@2.
```

Example manifest:

```json
{
  "id": "fn_weather_today",
  "name": "get_today_weather",
  "title": "获取今日天气",
  "description": "根据城市获取当天实时天气并返回天气卡片",
  "active_version": 2,
  "enabled": true,
  "versions": [
    {
      "version": 1,
      "status": "superseded",
      "source_type": "local_js",
      "source_hash": "sha256:old",
      "input_schema": {
        "type": "object",
        "properties": {
          "city": { "type": "string" }
        },
        "required": ["city"]
      },
      "permissions": {
        "host": ["http", "time"],
        "fallback_llm": false,
        "html_sandbox": false
      },
      "intent_examples": ["今天上海天气怎么样"],
      "created_from_trace_ids": ["trace_1"],
      "change_summary": "Initial generated version"
    },
    {
      "version": 2,
      "status": "enabled",
      "source_type": "local_js",
      "source_hash": "sha256:new",
      "input_schema": {
        "type": "object",
        "properties": {
          "city": { "type": "string" }
        },
        "required": ["city"]
      },
      "permissions": {
        "host": ["http", "time"],
        "optional": ["location"],
        "http": {
          "allowed_hosts": ["api.weather.example.com"],
          "methods": ["GET"],
          "timeout_ms": 8000
        },
        "location": {
          "precision": "city"
        },
        "fallback_llm": false,
        "html_sandbox": false
      },
      "cache": {
        "ttl_seconds": 1800,
        "key": ["city"]
      },
      "intent_examples": [
        "今天上海天气怎么样",
        "北京现在多少度",
        "查一下杭州今日天气"
      ],
      "tests": [
        {
          "args": { "city": "上海" },
          "expect_ui_type": "weather_card"
        }
      ],
      "created_from_trace_ids": ["trace_9", "trace_10"],
      "replaces": 1,
      "change_summary": "更换天气 API，并补充超时处理"
    }
  ]
}
```

Version rules:

- New versions never overwrite old JS source.
- Activating a new version requires validation and user confirmation.
- The previous active version becomes `superseded`, not deleted.
- Users can roll back to a prior validated version.
- Metrics are recorded per version and can also be aggregated at function identity level.
- The LLM catalog shows the active version only unless a repair/review UI needs version history.

Version states:

```text
draft
generated
validated
pending_user_confirmation
enabled
disabled
superseded
rolled_back
rejected
needs_repair
```

---

## 5. JavaScript Function Contract

Every local JS function exposes exactly one entrypoint:

```js
async function run(args, host) {
  return {
    data: {},
    ui: {},
    meta: {}
  };
}
```

Rules:

- `args` must validate against the active version's `input_schema`.
- `host` only contains declared and user-authorized capabilities.
- The return value must validate as `FunctionResult`.
- `ui.type` must be a registered UI Schema type unless `html_sandbox` is explicitly declared and authorized.
- `host.llm` is absent unless `fallback_llm` is declared and authorized.
- The runtime enforces timeout, output size, result schema, and host scope checks.

Example:

```js
async function run(args, host) {
  const city = args.city || await host.location.getCity();

  const weather = await host.http.getJson(
    "https://api.weather.example.com/today",
    {
      city,
      date: host.time.today()
    }
  );

  return {
    data: weather,
    ui: {
      type: "weather_card",
      props: {
        city,
        temperature: weather.temperature,
        condition: weather.condition,
        humidity: weather.humidity
      }
    },
    meta: {
      cacheable: true,
      confidence: 0.92
    }
  };
}
```

---

## 6. LLM-Routed Function Calling

The LLM decides whether to call a function. The system provides a compact function catalog in the prompt.

Function prompt:

```text
You have access to local reusable functions.
Prefer calling a local function when it directly matches the user's request,
has acceptable reliability, and its input schema can be satisfied.
Do not call a function if required arguments are missing; ask the user or
return a form UI. If a function result is low quality or fails, fall back to
normal agent reasoning only when fallback is allowed.
```

Catalog entry:

```text
Function: get_today_weather
Version: 2
Description: 获取指定城市今日天气，返回天气卡片。
Input schema: {"city":"string"}
Permissions: http(api.weather.example.com), time, optional location.city
Reliability: 52 success, 3 failure, avg quality 0.91, avg latency 220ms
Call when: 用户询问某城市今天/现在天气
Do not call when: 用户询问历史天气、空气质量、未来多日天气
```

Shortlist inputs:

- `name`, `title`, `description`, and `intent_examples` keyword match.
- Recent use.
- Success rate and failure reasons.
- Average and p95 latency.
- Average quality score.
- Permission risk.
- Whether the user previously rejected or downvoted the function.

The first version should inject at most 5-10 functions per turn.

Canonical call block:

```json
{
  "type": "function_use",
  "id": "fu_123",
  "function_id": "fn_weather_today",
  "version": 2,
  "name": "get_today_weather",
  "input": {
    "city": "上海"
  }
}
```

Canonical result block:

```json
{
  "type": "function_result",
  "function_use_id": "fu_123",
  "function_id": "fn_weather_today",
  "version": 2,
  "status": "success",
  "data": {},
  "ui": {},
  "meta": {
    "duration_ms": 184,
    "quality_score": 0.91
  }
}
```

Compatibility path:

- If provider codecs do not yet support `function_use`, expose one tool named `call_local_function`.
- `call_local_function({ function_id, version, args })` dispatches to the Android-local runtime in v1.
- Later, `engine-mobile` can own this dispatch natively without changing the manifest/result contract.

---

## 7. Host API and Permissions

The JS function cannot access Android directly. It receives a scoped `host` object derived from manifest permissions and user grants.

V1 Host API:

```text
host.http
host.storage
host.time
host.location
host.llm
host.log
```

### `host.http`

- Requires `host: ["http"]`.
- Must declare `allowed_hosts`, allowed methods, and timeout.
- Defaults to `GET` only.
- Blocks arbitrary URLs and undeclared hosts.
- Redacts sensitive headers and response fields in traces.
- Enforces response size and timeout limits.

### `host.storage`

- Scoped to `function_id`.
- No raw file paths.
- Supports small key-value state such as last-used args, lightweight cache, and user preferences.
- Does not store API keys, tokens, or large private payloads.

### `host.time`

- Pure low-risk capability.
- Provides `today()`, `nowIso()`, timezone-aware helpers, and duration utilities.

### `host.location`

- Requires user authorization.
- V1 should expose city-level helpers first: `getCity()` and `getRegion()`.
- Coordinate-level access must be separately declared as `precision: "coordinates"`.

### `host.llm`

- Exists only when `fallback_llm: true` and the user has authorized fallback.
- Offers a restricted fallback API, not arbitrary chat.
- Records token and cost.
- Returned fallback output still validates as `FunctionResult`.

### `host.log`

- Records scoped diagnostic logs.
- Redacts sensitive data and length-limits entries.

Permission rules:

- Manifest declaration is necessary but not sufficient; user authorization is also required.
- Host calls perform scope checks at call time.
- Unauthorized host capabilities are absent or throw `permission_denied`.
- Permission widening requires a new confirmation.
- High-risk future permissions such as camera, microphone, contacts, clipboard write, notification send, and calendar write are out of v1.

---

## 8. UI Output

Function output uses UI Schema by default.

```json
{
  "type": "weather_card",
  "props": {
    "city": "上海",
    "temperature": "24°C",
    "condition": "多云",
    "humidity": "72%",
    "updated_at": "2026-06-06T10:30:00+08:00"
  },
  "actions": [
    {
      "type": "function_call",
      "label": "查看未来 7 天",
      "function": "get_weather_forecast",
      "args": {
        "city": "上海",
        "days": 7
      }
    }
  ]
}
```

V1 UI types:

```text
text_block
info_card
result_list
table
form
weather_card
chart_basic
action_group
error_panel
html_sandbox
```

Rules:

- `ui.type` must be registered.
- `props` must validate against that type's schema.
- `actions` can only be registered safe actions.
- UI Schema cannot contain arbitrary JS.
- Compose renders UI Schema.
- Restricted WebView renders `html_sandbox`.

Parameter completion UI:

```json
{
  "type": "form",
  "props": {
    "title": "需要城市",
    "fields": [
      {
        "name": "city",
        "label": "城市",
        "type": "text",
        "required": true
      }
    ],
    "submit": {
      "type": "function_call",
      "function": "get_today_weather"
    }
  }
}
```

HTML sandbox is allowed only when `html_sandbox: true`:

```json
{
  "type": "html_sandbox",
  "props": {
    "html": "<div id=\"app\"></div>",
    "css": ".temp { font-size: 24px; }",
    "js": "document.getElementById('app').textContent = '24°C';",
    "height": "auto"
  }
}
```

HTML sandbox restrictions:

- No Android bridge.
- No function `host`.
- No remote JS.
- No `file://` or `content://`.
- Isolated cookie and storage.
- Network resources off by default.
- HTML is a display layer, not the function execution layer.

Supported UI actions:

```text
function_call
open_url
copy_text
submit_form
dismiss
```

Every result UI should support feedback hooks:

```text
thumb_up
thumb_down
run_with_agent
disable_function
```

---

## 9. Dream Distillation and `create_function`

Real-time turns record reusable trace summaries. Dream later distills them.

Trace summary:

```json
{
  "trace_id": "trace_123",
  "session_id": "session_456",
  "user_intent": "查询上海今日天气",
  "tool_calls": [
    {
      "tool": "web_search",
      "input_json": "{}",
      "result_summary": "Found weather source"
    }
  ],
  "host_calls": [],
  "final_data": {},
  "final_ui": {},
  "user_feedback": "positive",
  "duration_ms": 3210,
  "created_at": "2026-06-06T00:00:00Z"
}
```

Dream creates candidates when:

- Similar intents repeat.
- Execution path is stable.
- Inputs can be schema-modeled.
- Output can be represented as UI Schema or approved HTML sandbox.
- User feedback is positive or neutral.
- The function does not rely on one-off conversation context.

Dream must not create candidates when:

- The path depends on open-ended research or complex judgment.
- Required permissions are high risk.
- Input/output cannot be made stable.
- Recent user feedback is negative.
- The trace cannot be reproduced with mocks.

`create_function` skill input:

```json
{
  "source": "trace",
  "trace_ids": ["trace_1", "trace_2"],
  "goal": "沉淀获取今日天气的本地 function",
  "constraints": {
    "source_type": "local_js",
    "default_ui": "schema",
    "host_permissions": ["http", "time", "location.city"],
    "fallback_llm": false,
    "require_tests": true
  }
}
```

`create_function` output:

```text
function manifest
JS source
test cases
permission declaration
review summary
```

Candidate lifecycle:

```text
draft
  -> generated
  -> validated
  -> pending_user_confirmation
  -> enabled
```

Failure states:

```text
rejected
disabled
superseded
needs_repair
```

User confirmation UI shows ability, permissions, expected benefits, risks, tests, and optional source view. The user can enable, reject, edit permissions, run tests, or inspect source.

---

## 10. Call Metrics and Quality

Each invocation records a call event:

```json
{
  "function_id": "fn_weather_today",
  "version": 2,
  "call_id": "fc_123",
  "started_at": "2026-06-06T00:00:00Z",
  "duration_ms": 184,
  "status": "success",
  "failure_reason": null,
  "args_hash": "sha256:args",
  "cache_hit": false,
  "result_quality": {
    "score": 0.91,
    "source": "heuristic",
    "notes": "天气卡片完整，城市参数正确"
  }
}
```

Aggregates:

```text
success_count
failure_count
failure_reasons
avg_duration_ms
p95_duration_ms
avg_quality_score
last_used_at
last_failed_at
```

Quality sources:

- **Heuristic**: schema pass, UI completeness, timeout, missing fields, and result shape.
- **User feedback**: thumbs up/down, run with agent, disable.
- **LLM judge**: Dream-time low-frequency review of trace quality.

Initial scoring:

```text
schema pass = 0.60
successful execution without error = +0.20
thumb_up = +0.20
thumb_down caps score at 0.30
```

Auto response:

- 3 consecutive `runtime_error` or `schema_error`: mark `needs_repair`, stop recommending.
- Recent 10-call success rate below 60%: downrank in catalog.
- Average quality below 0.60: downrank in catalog.
- Any `host_scope_violation`: immediately disable active version pending review.

---

## 11. Storage

Android v1 storage layout:

```text
files/
  lingxi-functions/
    functions.json
    sources/
      fn_weather_today/
        1.js
        2.js
    stats/
      fn_weather_today.json
    traces/
      trace_20260606_001.json
```

Storage rules:

- App private storage only.
- JS source files are immutable per version.
- `source_hash` validates source integrity.
- Traces and metrics redact secrets and private payloads.
- Deleting a function can delete source and stats, with an optional user-visible choice to keep anonymized aggregate stats.

---

## 12. Protocol Shapes

Android v1 can implement these locally, but names and payloads should match future `client-protocol` additions.

Suggested events:

```text
FunctionCatalogUpdated
FunctionCandidateCreated
FunctionCallStarted
FunctionCallCompleted
FunctionVersionActivated
FunctionPermissionRequested
FunctionQualityFeedbackRecorded
```

Suggested commands:

```text
ListFunctions
CallFunction
EnableFunctionVersion
DisableFunction
ApproveFunctionCandidate
RejectFunctionCandidate
RecordFunctionFeedback
CreateFunctionFromTrace
```

Example event:

```json
{
  "type": "function_call_completed",
  "function_id": "fn_weather_today",
  "version": 2,
  "status": "success",
  "duration_ms": 184,
  "ui_json": "{}"
}
```

---

## 13. Error Handling

Failure reasons:

```text
intent_mismatch
missing_args
permission_denied
network_error
timeout
schema_error
runtime_error
host_scope_violation
quality_rejected
llm_fallback_failed
cache_error
```

Failure result:

```json
{
  "status": "failed",
  "failure_reason": "network_error",
  "message": "天气服务请求超时",
  "recoverable": true,
  "suggested_next": "fallback_agent"
}
```

Recovery:

- `missing_args`: render a form UI.
- `permission_denied`: show permission request or fail clearly.
- `timeout` / `network_error`: return recoverable error and optionally allow agent fallback if policy allows.
- `schema_error` / `runtime_error`: mark metric failure and consider `needs_repair`.
- `host_scope_violation`: disable active version pending review.

---

## 14. Verification Strategy

Candidate validation before enablement:

```text
manifest schema validation
version transition validation
JS syntax validation
input schema validation
permission scope validation
mock host execution
FunctionResult schema validation
UI schema validation
timeout test
metrics write test
```

Runtime checks:

```text
active version exists and is enabled
args validate against active input_schema
permission grants match manifest
host calls stay inside declared scope
result validates as FunctionResult
ui validates against UI schema
call metrics are written on success and failure
```

Regression coverage:

```text
old versions remain loadable
active_version can roll back
superseded versions do not enter LLM catalog
disabled functions do not enter prompt
low-quality functions downrank
fallback_llm absent unless authorized
html_sandbox rejected unless declared
HTML sandbox cannot access host
unauthorized host APIs fail
```

Android/WebView checks when WebView is the v1 JS substrate:

```text
JS timeout works
host bridge is scoped per function invocation
HTML sandbox has no Android bridge
HTML sandbox has isolated storage and cookies
remote JS is blocked
file:// and content:// are blocked
```

---

## 15. V1 Success Criteria

The first deliverable is complete when:

1. The LLM can see a compact local function catalog and choose `call_local_function`.
2. Android can execute an enabled `get_today_weather`-style local JS function.
3. Function calls record version, success/failure, failure reason, quality, latency, and cache hit.
4. Function output can render as UI Schema.
5. Restricted HTML sandbox rendering works only for declared functions.
6. Dream can generate a candidate through `create_function` from a successful trace.
7. Candidate validation runs before enablement.
8. User confirmation activates a specific function version.
9. A new version can supersede an old version.
10. A user can roll back to a prior version.
11. Bad functions downrank, disable, or enter `needs_repair`.

---

## 16. Key Decisions

- Use **Android-first implementation with shared contracts**, not a Rust-first rewrite.
- Use **LLM-routed calls**, not fully local automatic intent routing.
- Use **local JS functions** with `run(args, host)`.
- Use **manifest-scoped Host API**, not arbitrary Android bridge access.
- Use **UI Schema by default** and `html_sandbox` only as an explicit permission.
- Use **Dream + create_function** for background distillation.
- Use **user confirmation** before enabling generated functions.
- Use **versioned immutable sources** with rollback.
- Use **metrics-driven catalog ranking** so bad functions become less visible to the LLM.
