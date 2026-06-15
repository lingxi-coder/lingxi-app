# QuickJS Function-Call Tools Design

> **Status**: APPROVED for spec capture
> **Date**: 2026-06-13
> **Scope**: Android-first function-call tool registry that lets the LLM generate JavaScript source strings and execute them through QuickJS.
> **Refines**: `docs/superpowers/specs/2026-06-06-dynamic-function-runtime-design.md`

---

## 1. Goal

Build the first production-shaped local JavaScript execution layer for LingXi
mobile:

1. The LLM can generate JavaScript source strings.
2. The host executes those strings through QuickJS, not WebView.
3. The first release supports JavaScript only.
4. The public surface is a full function registry tool set, not a single
   compatibility runner.
5. Code can run either once or as a saved, versioned local function.
6. Saved functions have manifests, input schemas, permissions, tests, metrics,
   and explicit lifecycle states.
7. Host capabilities are exposed only through declared, authorized, audited
   `host.*` tools.
8. Function-call telemetry records lifecycle, permission, validation, runtime,
   and host-call summaries without sending raw source, input, output, logs, or
   URLs.
9. Function quality is derived from validation tests, execution counts, failure
   reasons, result quality, host-call behavior, and latency so the LLM and user
   can decide whether to keep, update, disable, or delete a function.

This design deliberately starts with the complete registry model. There is no
temporary `local_js.run` compatibility entry.

## 2. Non-Goals

- Do not execute Android native code, DEX, JNI, `.so`, shell commands, or
  downloaded binary code.
- Do not use WebView as the JavaScript execution substrate.
- Do not expose arbitrary Android APIs to generated JavaScript.
- Do not let generated JavaScript access secrets, raw files, contacts,
  clipboard write, microphone, camera, notifications, or accessibility APIs in
  this release.
- Do not implement UI rendering in the runner. `ui` remains a reserved result
  field, but v1 requires only JSON-serializable data.
- Do not optimize for provider-specific function-calling formats. Tool names
  and schemas should be provider-neutral and mappable later.

## 3. Architecture

```text
LLM function-call tools
  |
  v
JsFunctionService
  |
  +--> FunctionStore
  +--> PermissionGate
  +--> Validator
  |
  v
QuickJsRunner
  |
  v
HostToolBroker
```

### Components

- **JsFunctionService** owns tool dispatch, request validation, state
  transitions, and metrics recording.
- **FunctionStore** persists function identities, immutable versions, tests,
  metrics, and private key-value state.
- **PermissionGate** decides whether a call can execute automatically or must
  pause for user confirmation.
- **Validator** checks manifests, source shape, schemas, permissions, and test
  execution.
- **QuickJsRunner** creates an isolated QuickJS runtime/context for each
  invocation and enforces limits.
- **HostToolBroker** injects only authorized `host.*` capabilities and records
  every host call.
- **TelemetryRecorder** emits privacy-preserving `tengu_tool_*` analytics
  events for tool lifecycle, permission gates, validation, runtime failures,
  and host-call summaries.
- **QualityAggregator** turns local metrics, validation results, and user
  feedback into a bounded `QualitySummary` that can be shown in the LLM catalog
  and `js_function.inspect`.

Each JavaScript invocation gets a fresh QuickJS runtime/context. Persistent
state exists only through explicit `host.kv` storage.

## 4. LLM Tool Surface

Expose these provider-neutral tools:

```text
js_function.execute_once
js_function.create
js_function.validate
js_function.call
js_function.update
js_function.disable
js_function.delete
js_function.inspect
```

### `js_function.execute_once`

Executes generated JavaScript without storing it as a function. This is allowed
to auto-run only when requested permissions are low risk.

Input:

```json
{
  "code": "async function run(input, host) { return { \"data\": input.x + 1 }; }",
  "input": { "x": 1 },
  "permissions": {
    "host": ["time", "log"],
    "http": null,
    "location": null,
    "llm": false,
    "kv": false
  },
  "limits": {
    "timeout_ms": 1000,
    "memory_mb": 16,
    "max_output_bytes": 65536
  },
  "reason": "Compute a derived value from user-provided input"
}
```

Output:

```json
{
  "status": "success",
  "data": 2,
  "meta": {
    "duration_ms": 12,
    "logs": [],
    "host_calls": []
  }
}
```

### `js_function.create`

Creates a reusable function identity and version. Creating a persistent function
requires user confirmation before activation, even if permissions are low risk.

Input:

```json
{
  "name": "normalize_expense_items",
  "title": "Normalize expense items",
  "description": "Normalize messy expense text into structured line items.",
  "code": "async function run(input, host) { return { \"data\": [] }; }",
  "input_schema": {
    "type": "object",
    "properties": {
      "text": { "type": "string" }
    },
    "required": ["text"],
    "additionalProperties": false
  },
  "permissions": {
    "host": ["time", "log", "kv"],
    "http": null,
    "location": null,
    "llm": false,
    "kv": {
      "scope": "function_private",
      "max_bytes": 32768
    }
  },
  "tests": [
    {
      "name": "basic receipt text",
      "input": { "text": "coffee 30, taxi 80" },
      "expect": {
        "json_schema": { "type": "array" }
      }
    }
  ],
  "activation": {
    "requested": true,
    "reason": "User wants this reusable for future expense parsing."
  }
}
```

Output:

```json
{
  "function_id": "jsfn_abc123",
  "version": 1,
  "status": "pending_confirmation",
  "requires_user_confirmation": true,
  "validation_status": "not_run"
}
```

### `js_function.validate`

Validates a saved function version using static checks, permission checks, mock
host execution, and declared tests.

Input:

```json
{
  "function_id": "jsfn_abc123",
  "version": 1,
  "run_tests": true,
  "mock_host": {
    "time": {
      "now_iso": "2026-06-13T12:00:00+08:00"
    }
  }
}
```

Output:

```json
{
  "status": "passed",
  "checks": [
    { "name": "manifest_schema", "status": "passed" },
    { "name": "source_hash", "status": "passed" },
    { "name": "syntax", "status": "passed" },
    { "name": "permission_scope", "status": "passed" },
    { "name": "tests", "status": "passed" }
  ],
  "duration_ms": 48
}
```

### `js_function.call`

Calls an enabled saved function. It can auto-run only when the active version is
already enabled and the call does not request new permissions.

Input:

```json
{
  "function_id": "jsfn_abc123",
  "version": "active",
  "input": {
    "text": "coffee 30, taxi 80"
  },
  "call_reason": "Normalize expense text in the current user request."
}
```

Output:

```json
{
  "status": "success",
  "function_id": "jsfn_abc123",
  "version": 1,
  "data": [
    { "name": "coffee", "amount": 30 },
    { "name": "taxi", "amount": 80 }
  ],
  "meta": {
    "duration_ms": 16,
    "cache_hit": false,
    "quality_score": 0.8
  }
}
```

### `js_function.update`

Creates a new immutable version. It never overwrites existing source. Activating
the new version requires validation and user confirmation.

Input:

```json
{
  "function_id": "jsfn_abc123",
  "change_summary": "Handle currency symbols and multiline input.",
  "code": "async function run(input, host) { return { \"data\": [] }; }",
  "input_schema": {
    "type": "object",
    "properties": {
      "text": { "type": "string" }
    },
    "required": ["text"],
    "additionalProperties": false
  },
  "permissions": {
    "host": ["time", "log", "kv"],
    "kv": {
      "scope": "function_private",
      "max_bytes": 32768
    },
    "llm": false
  },
  "tests": []
}
```

Output:

```json
{
  "function_id": "jsfn_abc123",
  "version": 2,
  "status": "pending_confirmation",
  "previous_active_version": 1
}
```

### `js_function.disable`

Disables a function or version. This is allowed without user confirmation
because it reduces capability.

Input:

```json
{
  "function_id": "jsfn_abc123",
  "version": "active",
  "reason": "Repeated schema failures."
}
```

Output:

```json
{
  "status": "disabled",
  "function_id": "jsfn_abc123",
  "version": 1
}
```

### `js_function.delete`

Deletes or tombstones a function. This is a destructive maintenance operation,
so it requires user confirmation. The default behavior is a soft delete that
removes the function from catalogs and normal calls while keeping a tombstone
and anonymized aggregate quality history for audit and rollback context.

Input:

```json
{
  "function_id": "jsfn_abc123",
  "delete_mode": "soft",
  "reason_code": "obsolete",
  "reason": "The user no longer wants this generated expense parser."
}
```

Output:

```json
{
  "status": "deleted",
  "function_id": "jsfn_abc123",
  "delete_mode": "soft",
  "retained_aggregate_metrics": true
}
```

### `js_function.inspect`

Returns manifest, state, permissions, tests, metrics, quality summary,
maintenance recommendations, and optionally source. Source access should default
to false to reduce prompt exposure.

Input:

```json
{
  "function_id": "jsfn_abc123",
  "include_source": false,
  "include_metrics": true,
  "include_tests": true
}
```

Output:

```json
{
  "function_id": "jsfn_abc123",
  "name": "normalize_expense_items",
  "active_version": 1,
  "status": "enabled",
  "permissions": {
    "host": ["time", "log", "kv"]
  },
  "metrics": {
    "success_count": 8,
    "failure_count": 1,
    "avg_duration_ms": 14
  },
  "quality": {
    "status": "healthy",
    "score": 0.86,
    "validation_status": "passed",
    "test_pass_count": 4,
    "test_fail_count": 0,
    "success_rate_recent": 0.89,
    "failure_reasons_recent": {
      "schema_error": 1
    },
    "result_quality_avg": 0.82,
    "avg_duration_ms": 14,
    "p95_duration_ms": 31,
    "consecutive_failures": 0,
    "host_scope_violations": 0,
    "maintenance_recommendation": "keep"
  },
  "maintenance_actions": ["call", "update", "disable", "delete"]
}
```

## 5. JavaScript Contract

The runner accepts full source that defines exactly one entrypoint:

```js
async function run(input, host) {
  return {
    data: {},
    meta: {}
  };
}
```

Rules:

- `input` must validate against the version's `input_schema`.
- `host` contains only declared and authorized capabilities.
- Return values must be JSON-serializable.
- The required result shape is `{ data, meta? }`.
- `ui` is reserved but optional and not interpreted by this runner release.
- Source cannot rely on browser or Node globals.

Disallowed in this release:

```text
eval
Function constructor
import/module loading
native bindings
raw file paths
unbounded host calls
unbounded logs
```

`Date` and `Math.random` should be policy-controlled. Validation tests should
prefer deterministic `host.time` and seeded inputs.

## 6. QuickJS Runner

Execution lifecycle:

```text
create runtime
set memory, stack, timeout, and output limits
create context
inject input
inject scoped host object
evaluate source
call run(input, host)
drain pending jobs for async host bridge support
serialize result
destroy context/runtime
record metrics
```

Default limits:

```json
{
  "timeout_ms": 1000,
  "memory_mb": 16,
  "max_stack_bytes": 1048576,
  "max_output_bytes": 65536,
  "max_log_entries": 50,
  "max_host_calls": 32
}
```

Execution states:

```text
success
syntax_error
runtime_error
timeout
memory_limit
output_too_large
schema_error
permission_denied
host_scope_violation
host_call_failed
cancelled
```

## 7. Host Object API

The JavaScript environment sees only authorized `host.*` capabilities.

```js
host.time.nowIso()
host.time.today()
host.time.timezone()

host.log.debug(message, data)
host.log.info(message, data)
host.log.warn(message, data)

host.kv.get(key)
host.kv.set(key, value)
host.kv.delete(key)
host.kv.list(prefix)

host.http.request(request)
host.location.getCity()
host.llm.complete(request)
```

Implementation priority:

```text
P0: time, log
P1: function_private kv
P2: http with allowed_hosts
P3: location, llm fallback
```

Permission manifest:

```json
{
  "permissions": {
    "host": ["time", "log", "kv", "http"],
    "kv": {
      "scope": "function_private",
      "max_bytes": 32768
    },
    "http": {
      "allowed_hosts": ["api.example.com"],
      "methods": ["GET"],
      "timeout_ms": 5000,
      "max_response_bytes": 65536
    },
    "location": null,
    "llm": false
  }
}
```

Every host call records an audit envelope:

```json
{
  "call_id": "hc_123",
  "tool": "http.request",
  "input": {},
  "started_at": "2026-06-13T12:00:00+08:00",
  "duration_ms": 42,
  "status": "success",
  "redacted_input": {},
  "redacted_output": {}
}
```

HTTP constraints:

- URL host must match `allowed_hosts`.
- Method must match declared `methods`.
- `file://`, `content://`, and localhost are blocked by default.
- Sensitive headers are blocked until a separate secret-binding design exists.
- Responses are limited by `max_response_bytes`.
- Timeouts use the lower of request timeout and manifest timeout.

## 8. Permission and Confirmation Policy

Allowed to auto-run:

```text
execute_once with pure computation
execute_once with time/log
execute_once with function-private kv
call of an enabled function with no permission expansion
validate
inspect
disable
```

Requires user confirmation:

```text
create persistent function
activate new version
expand permissions
http
location
llm
recover from needs_repair to enabled
delete function or purge tombstone
```

Manifest declaration is necessary but not sufficient. The runtime also needs an
active grant for any high-risk capability. Unauthorized capabilities are absent
from `host`, and attempted use returns `permission_denied` or
`host_scope_violation`.

## 9. Storage and Versioning

Data model:

```text
JsFunction
  id
  name
  title
  description
  status
  active_version
  created_at
  updated_at

JsFunctionVersion
  function_id
  version
  source_hash
  code
  input_schema
  permissions
  limits
  tests
  status
  change_summary
  created_at
```

State machine:

```text
draft -> validated -> pending_confirmation -> enabled -> disabled
enabled -> disabled
enabled -> superseded
enabled -> needs_repair
disabled -> deleted
validated -> rejected
pending_confirmation -> rejected
```

Version rules:

- `update` always creates a new version.
- Existing version source is immutable.
- `active_version` points only to an enabled version.
- Permission expansion requires a new confirmation.
- Rollback activates a previously validated version.

Android file layout:

```text
files/lingxi-js-functions/
  registry.json
  functions/
    jsfn_abc123/
      manifest.json
      versions/
        1.js
        1.meta.json
        2.js
        2.meta.json
      validation-runs.jsonl
      metrics.jsonl
      hook-runs.jsonl
      quality.json
      kv.json
      tombstone.json
```

Storage responsibilities:

```text
registry.json
  Function list, active_version pointers, coarse status, and catalog indexes.

manifest.json
  Function identity, latest display metadata, permission grant state, and
  tombstone pointer when soft-deleted.

versions/<n>.js
  Immutable JavaScript source for one version.

versions/<n>.meta.json
  Source hash, input schema, permissions, limits, declared tests, validation
  status, activation status, and change summary for one version.

validation-runs.jsonl
  Append-only validation and test execution results, including pass/fail counts,
  failed check names, failure codes, duration, mock-host use, and tested version.

metrics.jsonl
  Append-only runtime invocation records for calls and execute-once executions
  promoted into function history. This is the source of success/failure counts,
  failure reasons, result quality, host-call counts, and latency distribution.

hook-runs.jsonl
  Append-only redacted hook outcomes for function lifecycle hooks. Stores hook
  alias, hook id, decision, reason_code, outcome, duration, and whether the hook
  changed input or recommended maintenance. It does not store raw hook payloads,
  raw source, raw input, raw output, logs, URLs, or stack traces.

quality.json
  Cached QualitySummary derived from version metadata, validation-runs.jsonl,
  metrics.jsonl, hook-runs.jsonl, user feedback, and host-scope incidents. This
  is what js_function.inspect and the LLM catalog read by default.

kv.json
  Function-private key-value state used only through host.kv.

tombstone.json
  Soft-delete record with reason_code, deleted_at, deleted_by, last active
  version, and optional anonymized aggregate quality summary.
```

Telemetry is not a storage dependency for function quality. Analytics events may
help product-level observation, but LLM routing, maintenance recommendations,
and update/delete decisions must use local FunctionStore metrics and
QualitySummary.

## 10. Validation

`js_function.validate` runs:

```text
manifest_schema
name_format
input_schema_valid
permission_schema_valid
limits_within_policy
source_hash
syntax_check
entrypoint_exists
forbidden_globals_check
mock_execution
result_schema
tests
```

Test case shape:

```json
{
  "name": "basic case",
  "input": { "text": "coffee 30" },
  "mock_host": {
    "time": {
      "now_iso": "2026-06-13T12:00:00+08:00"
    },
    "http": {
      "responses": [
        {
          "match": {
            "method": "GET",
            "url": "https://api.example.com/items"
          },
          "status": 200,
          "body": { "ok": true }
        }
      ]
    }
  },
  "expect": {
    "json_schema": {
      "type": "object"
    }
  }
}
```

## 11. Error Handling

Failure envelope:

```json
{
  "status": "failed",
  "error": {
    "code": "timeout",
    "message": "JS execution exceeded 1000ms",
    "recoverable": true,
    "details": {}
  },
  "meta": {
    "duration_ms": 1000,
    "host_calls": []
  }
}
```

Error codes:

```text
invalid_request
function_not_found
version_not_found
function_disabled
confirmation_required
validation_failed
syntax_error
runtime_error
timeout
memory_limit
output_too_large
schema_error
permission_denied
host_scope_violation
host_call_failed
rate_limited
storage_error
internal_error
```

Automatic responses:

- Three consecutive `timeout`, `runtime_error`, or `schema_error` failures mark
  the version `needs_repair`.
- Any `host_scope_violation` disables the active version pending review.
- `memory_limit` and `output_too_large` record failures but do not auto-disable.
- `permission_denied` returns `confirmation_required` when a user grant could
  resolve it, otherwise a normal failure.

## 12. Metrics

Metrics are durable local function-quality state. They are used for ranking,
repair, and debugging inside the app. They are separate from telemetry, which is
covered in the next section.

Each execution appends a local JSONL record:

```json
{
  "call_id": "jscall_123",
  "function_id": "jsfn_abc123",
  "version": 1,
  "mode": "stored_function",
  "started_at": "2026-06-13T12:00:00+08:00",
  "duration_ms": 18,
  "status": "success",
  "error_code": null,
  "input_hash": "sha256:...",
  "output_hash": "sha256:...",
  "host_calls": 0,
  "quality_score": 0.8
}
```

Aggregates:

```text
success_count
failure_count
failure_reasons
avg_duration_ms
p95_duration_ms
last_success_at
last_failure_at
consecutive_failures
last_error_code
```

## 13. Quality Summary and Maintenance

`QualitySummary` is the LLM-visible and user-visible maintenance view derived
from local FunctionStore data. It is read from `quality.json`, refreshed after
validation runs, function calls, user feedback, and state transitions, and may
be recomputed from `validation-runs.jsonl` plus `metrics.jsonl` if the cache is
missing.

The LLM should use `QualitySummary`, not raw metrics or raw telemetry, when
deciding whether to call, update, disable, or delete a function.

Shape:

```json
{
  "status": "healthy",
  "score": 0.86,
  "validation_status": "passed",
  "test_pass_count": 4,
  "test_fail_count": 0,
  "test_last_run_at": "2026-06-13T12:00:00+08:00",
  "call_count_total": 52,
  "success_count": 48,
  "failure_count": 4,
  "success_rate_recent": 0.92,
  "failure_reasons_recent": {
    "schema_error": 1,
    "timeout": 1,
    "host_call_failed": 2
  },
  "result_quality_avg": 0.82,
  "result_quality_recent": 0.79,
  "avg_duration_ms": 18,
  "p95_duration_ms": 44,
  "last_success_at": "2026-06-13T12:05:00+08:00",
  "last_failure_at": null,
  "consecutive_failures": 0,
  "host_call_count_avg": 1.2,
  "host_scope_violations": 0,
  "permission_risk": "low",
  "user_feedback": {
    "thumbs_up": 12,
    "thumbs_down": 1,
    "disable_requests": 0
  },
  "maintenance_recommendation": "keep",
  "maintenance_reasons": [
    "recent success rate is high",
    "validation tests pass"
  ]
}
```

Status values:

```text
healthy
degraded
needs_repair
unsafe
obsolete
disabled
deleted
```

Maintenance recommendations:

```text
keep
update
disable
delete
ask_user
```

Recommendation rules:

- `keep`: validation passes, recent success rate is acceptable, result quality
  is acceptable, and there are no host-scope violations.
- `update`: failures are concentrated in recoverable causes such as
  `schema_error`, stale output shape, or API response changes.
- `disable`: consecutive failures, low recent success rate, or low result
  quality make the function unreliable but it may still be repairable.
- `delete`: the function is obsolete, user-requested deletion exists, or the
  function has stayed disabled/unused past the retention window.
- `ask_user`: maintenance action is destructive or confidence is low.

LLM catalog entries should expose only the compact subset needed for routing:

```json
{
  "quality": {
    "status": "healthy",
    "score": 0.86,
    "success_rate_recent": 0.92,
    "failure_count": 4,
    "failure_reasons_recent": ["schema_error", "timeout", "host_call_failed"],
    "result_quality_recent": 0.79,
    "avg_duration_ms": 18,
    "p95_duration_ms": 44,
    "maintenance_recommendation": "keep"
  }
}
```

## 14. Telemetry

Telemetry follows the existing `tengu_tool_*_{started,completed,failed}` shape
used by LingXi tools. The JS function registry emits both the generic tool
lifecycle events and JS-function-specific event names so product analytics can
distinguish create, validate, call, and temporary execution paths.

Event names:

```text
tengu_tool_js_function_execute_once_started
tengu_tool_js_function_execute_once_completed
tengu_tool_js_function_execute_once_failed

tengu_tool_js_function_create_started
tengu_tool_js_function_create_completed
tengu_tool_js_function_create_failed

tengu_tool_js_function_validate_started
tengu_tool_js_function_validate_completed
tengu_tool_js_function_validate_failed

tengu_tool_js_function_call_started
tengu_tool_js_function_call_completed
tengu_tool_js_function_call_failed

tengu_tool_js_function_update_started
tengu_tool_js_function_update_completed
tengu_tool_js_function_update_failed

tengu_tool_js_function_disable_started
tengu_tool_js_function_disable_completed
tengu_tool_js_function_disable_failed

tengu_tool_js_function_delete_started
tengu_tool_js_function_delete_completed
tengu_tool_js_function_delete_failed

tengu_tool_js_function_inspect_started
tengu_tool_js_function_inspect_completed
tengu_tool_js_function_inspect_failed
```

Permission confirmation uses the existing generic permission telemetry events:

```text
tengu_tool_permission_requested
tengu_tool_permission_granted
tengu_tool_permission_denied
tengu_tool_permission_remembered
```

### Common telemetry fields

All JS-function-specific events should include only safe, typed metadata:

```json
{
  "tool_name": "js_function.call",
  "tool_call_id": "tc_123",
  "js_call_id": "jscall_123",
  "operation": "call",
  "mode": "stored_function",
  "function_id_hash": "sha256:...",
  "version": 1,
  "source_hash": "sha256:...",
  "source_bytes": 842,
  "input_schema_hash": "sha256:...",
  "input_hash": "sha256:...",
  "output_hash": "sha256:...",
  "permissions_declared": ["time", "log", "kv"],
  "permissions_granted": ["time", "log", "kv"],
  "high_risk_permissions": [],
  "confirmation_state": "not_required",
  "timeout_ms": 1000,
  "memory_mb": 16,
  "max_output_bytes": 65536,
  "host_call_count": 0,
  "host_tools_used": [],
  "duration_ms": 18,
  "status": "success",
  "error_code": null
}
```

Field rules:

- `tool_call_id` is the outer function-call/tool invocation id.
- `js_call_id` is the runner invocation id used by local metrics.
- `function_id_hash`, `input_hash`, and `output_hash` are hashes, not raw user
  content.
- `source_hash` and `source_bytes` may be emitted; raw code must never be sent.
- `permissions_declared`, `permissions_granted`, and `host_tools_used` use
  bounded enum values, not arbitrary strings.
- `duration_ms`, `timeout_ms`, `memory_mb`, and `max_output_bytes` are numeric
  diagnostics.
- `error_code` uses the closed error-code vocabulary from this spec.

### Operation-specific fields

`execute_once` adds:

```json
{
  "ephemeral": true,
  "persisted": false
}
```

`create` and `update` add:

```json
{
  "persisted": true,
  "requested_activation": true,
  "validation_status": "not_run",
  "test_count": 2,
  "permission_expansion": false
}
```

`validate` adds:

```json
{
  "check_count": 10,
  "checks_failed": [],
  "tests_run": 2,
  "tests_failed": 0,
  "used_mock_host": true
}
```

`call` adds:

```json
{
  "active_version": true,
  "cache_hit": false,
  "quality_score": 0.8
}
```

`disable` adds:

```json
{
  "disable_target": "active_version",
  "reason_code": "repeated_schema_failures"
}
```

`delete` adds:

```json
{
  "delete_mode": "soft",
  "reason_code": "obsolete",
  "retained_aggregate_metrics": true
}
```

`inspect` adds:

```json
{
  "include_source": false,
  "include_metrics": true,
  "include_tests": true
}
```

### Host-call telemetry

Host calls are not emitted as one event per call by default. The terminal
function event includes a summary:

```json
{
  "host_call_count": 3,
  "host_call_counts_by_tool": {
    "time": 1,
    "kv": 1,
    "http": 1
  },
  "host_call_failures": 0,
  "http_allowed_host_hashes": ["sha256:..."],
  "http_status_classes": ["2xx"],
  "http_response_bytes": 1204
}
```

Privacy rules:

- Do not emit raw JavaScript source.
- Do not emit raw input, raw output, logs, stack traces, HTTP URLs, headers,
  request bodies, response bodies, city/location values, or LLM fallback text.
- If hostnames are needed, emit salted hashes or counts. Raw hostnames are
  treated as PII unless a future schema explicitly marks a field as safe.
- Stack traces stay local in metrics/debug logs unless explicitly user-exported.
- `host.log.*` entries remain local and length-limited; telemetry records only
  `log_entry_count` and `log_level_counts`.

### Correlation and sampling

- Every event includes `tool_call_id`; stored-function execution also includes
  `js_call_id`.
- Telemetry should correlate to conversation/session only through existing
  approved hashed identifiers.
- Failed, timed-out, permission-denied, and host-scope-violation events are not
  sampled.
- Successful `inspect` and high-frequency successful `call` events may be
  sampled later, but local metrics remain complete.
- Telemetry emission must respect the platform's analytics opt-in/opt-out
  setting.

## 15. Hook Integration

Function-call tools participate in the existing hook runtime. The implementation
should first map function tools onto the generic tool hooks that already exist:

```text
PreToolUse          -> before function operation starts
PostToolUse         -> after function operation succeeds
PostToolUseFailure  -> after function operation fails
PermissionRequest   -> before high-risk grants or destructive maintenance
PermissionDenied    -> after denied grants or destructive maintenance
```

For product and hook-author clarity, expose semantic aliases in docs and hook
payloads:

```text
before_function_call
after_function_call
function_call_error
before_function_validate
after_function_validate
function_validate_error
before_function_maintenance
after_function_maintenance
function_maintenance_error
```

Alias mapping:

```text
before_function_call        PreToolUse for js_function.call or js_function.execute_once
after_function_call         PostToolUse for js_function.call or js_function.execute_once
function_call_error         PostToolUseFailure for js_function.call or js_function.execute_once

before_function_validate    PreToolUse for js_function.validate
after_function_validate     PostToolUse for js_function.validate
function_validate_error     PostToolUseFailure for js_function.validate

before_function_maintenance PreToolUse for js_function.create/update/disable/delete
after_function_maintenance  PostToolUse for js_function.create/update/disable/delete
function_maintenance_error  PostToolUseFailure for js_function.create/update/disable/delete
```

`before_function_call` fires after basic request schema validation and before
QuickJS execution. It can:

- block execution;
- require user confirmation;
- reduce limits such as timeout or output size;
- add system messages;
- mutate `input` if the result still validates against `input_schema`.

It cannot:

- grant high-risk permissions by itself;
- increase permissions without user confirmation;
- modify saved source code;
- bypass validation or confirmation;
- override a disabled/deleted function state.

`after_function_call` fires after a successful runner result is validated and
before the result is returned to the agent. It can add system messages,
attachments, or maintenance hints. It cannot rewrite `data` in this release.

`function_call_error` fires after a failed runner result is classified. It can
add maintenance hints such as `update`, `disable`, or `ask_user`. It cannot
suppress `host_scope_violation`, `permission_denied`, or security failures.

Hook payloads must be redacted and quality-oriented:

```json
{
  "hook_alias": "before_function_call",
  "tool_name": "js_function.call",
  "tool_use_id": "tc_123",
  "js_call_id": "jscall_123",
  "function_id_hash": "sha256:...",
  "version": 2,
  "source_hash": "sha256:...",
  "input_hash": "sha256:...",
  "input_schema_hash": "sha256:...",
  "permissions_declared": ["time", "log", "kv"],
  "permissions_granted": ["time", "log", "kv"],
  "high_risk_permissions": [],
  "limits": {
    "timeout_ms": 1000,
    "memory_mb": 16,
    "max_output_bytes": 65536
  },
  "quality": {
    "status": "healthy",
    "score": 0.86,
    "success_rate_recent": 0.92,
    "failure_reasons_recent": ["schema_error"],
    "maintenance_recommendation": "keep"
  }
}
```

Post-call hook payloads additionally include result summaries:

```json
{
  "status": "success",
  "duration_ms": 18,
  "output_hash": "sha256:...",
  "output_bytes": 1420,
  "result_quality_score": 0.82,
  "host_call_count": 1,
  "host_tools_used": ["time"]
}
```

Failure hook payloads additionally include:

```json
{
  "status": "failed",
  "error_code": "schema_error",
  "recoverable": true,
  "consecutive_failures": 2,
  "maintenance_recommendation": "update"
}
```

Hook privacy rules match telemetry privacy rules. Hooks do not receive raw source
code, raw input, raw output, logs, URLs, headers, stack traces, or location
values by default. A future explicit local-debug mode may expose raw values only
to trusted local hooks after user confirmation.

Hook outcomes that affect function execution or maintenance are persisted as
redacted `hook-runs.jsonl` entries and then folded into `quality.json` when they
change call reliability or maintenance recommendations. Non-blocking observer
hooks may be sampled or skipped from local storage unless they return a
maintenance signal.

## 16. Verification Strategy

Design-time verification:

- Placeholder scan: no unresolved placeholder markers or unfinished sections.
- Consistency check against the earlier dynamic-function runtime spec.
- Confirmation that the chosen approach is the complete registry model, not a
  compatibility runner.
- Confirmation that telemetry contains no raw source, input, output, logs, URLs,
  headers, stack traces, or location values.
- Confirmation that hooks receive redacted payloads and cannot bypass
  permission, validation, disabled, or deleted states.

Implementation verification later:

- Unit tests for tool request validation and schema failures.
- Unit tests for state transitions and version immutability.
- QuickJS syntax/runtime/timeout/memory/output-limit tests.
- Host permission tests for missing, allowed, denied, and expanded capability
  cases.
- Mock host tests for `time`, `log`, `kv`, and `http`.
- Metrics write tests for success, failure, and auto-`needs_repair`.
- QualitySummary aggregation tests for call counts, validation test results,
  failure reasons, result quality, latency, user feedback, and maintenance
  recommendations.
- Telemetry tests for every `js_function.*` started/completed/failed event.
- Telemetry privacy tests proving raw code/input/output/logs/URLs are absent.
- Permission telemetry tests for requested/granted/denied/remembered decisions.
- Hook integration tests for `before_function_call`, `after_function_call`,
  `function_call_error`, validation hooks, and maintenance hooks.
- Hook safety tests proving hooks cannot grant permissions, mutate source, or
  suppress security failures.
- Android integration test proving execution does not require WebView.

## 17. Decisions

- Use QuickJS, not WebView.
- Support JavaScript only in the first release.
- Use the complete function registry tool set from the start.
- Keep `execute_once` for temporary generated code, but do not make it the
  compatibility layer.
- Require persistent functions to be versioned, validated, and confirmed before
  activation.
- Auto-run only low-risk operations.
- Require confirmation for network, location, LLM fallback, function creation,
  version activation, permission expansion, and repair recovery.
- Keep UI rendering outside the runner release; return JSON data first.
- Keep local metrics and analytics telemetry separate: metrics can support
  function repair/ranking, while telemetry uses privacy-preserving summaries.
- Store quality facts locally in FunctionStore; expose only bounded
  QualitySummary to the LLM catalog and hooks.
- Reuse generic tool hooks for compatibility, with semantic function-call hook
  aliases for author clarity.
