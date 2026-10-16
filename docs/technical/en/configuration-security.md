# Configuration, credentials and isolation

Trace settings provenance and keep model routing, authorization, credential storage and OS isolation as explicit host responsibilities.

<a id="settings-layers"></a>

## Settings sources and merge rules

The effective value stack merges defaults → user → project → local → CLI/flag settings → managed tiers → environment. Managed tiers are supplied in ascending priority. The user file is ~/.lingxi/settings.json; project and local files are .lingxi/settings.json and .lingxi/settings.local.json under the project. Explicit hosts may select a different user path and file-layer scope.

Merge semantics are field-specific: supplied scalars override, list fields concatenate/deduplicate, and hooks/sandbox objects deep-merge. Permission arrays accumulate. A malformed, oversized or unreadable file layer is skipped as a whole; later valid layers still apply. The provenance trace explains the selected source; never infer it from just the last file opened.

<a id="credential-lifecycle"></a>

## Credentials and route identity

The host resolves provider profile, account and credential source before handing request services to llm-client. Secret values belong to secure storage and request headers, not JSONL, tool arguments or public configuration exports. An empty startup API key may allow command initialization, while a model request later fails authentication; construction success is not sign-in success.

On macOS, NativePreferred requires the bundled credential broker and fails closed for a mispackaged client. NativeOrMemory is an explicitly isolated process-local mode; PlainTextFixture is an explicit test selection. Linux secure-storage availability and fallback follow its factory path. A plaintext backend uses restrictive file permissions, but those permissions are not encryption.

Do not log whole configuration or credential structs. Field-specific Debug implementations and historical comments are not an unconditional redaction guarantee; log profile IDs, source names and failure categories instead of secret contents.

<a id="authorization"></a>

## Tool authorization

PolicyGate is the dispatch enforcement seam. Allow proceeds, Deny returns a denied tool outcome, and Ask uses the injected prompting transport when required. Session visibility filters and per-invocation authorization are separate controls. The same authorization seam applies to subagents and teammates.

Wire mode names are default, plan, acceptEdits, bypassPermissions, dontAsk and auto; Bubble is internal. Mode availability and persistent-settings acceptance are not identical. Auto additionally depends on model/provider and host policy. Use mode/rule implementations for exact decisions; a mode label alone does not summarize every rule, hook or managed ceiling.

<a id="sandbox-and-watchers"></a>

## OS sandbox and live settings

Sandbox wrapping is chosen separately from permission mode and project trust. The decision considers actual runtime availability, an explicit override plus permission for unsandboxed execution, empty commands and excluded commands. Platform backends then implement the selected filesystem/network/process restrictions. A user approval does not create a missing native sandbox.

Desktop settings watchers monitor user/project/local/managed directories and fire ConfigChange. The watcher starts even without configured hooks because managed safety updates still need observation. Managed drop-ins are read in sorted order. Retain the watcher handle for the lifetime of the composition; dropping it stops its tasks.

<a id="safe-example"></a>

## Project-local example and verification

This complete .lingxi/settings.local.json example grants only a read-oriented tool. It contains no credentials. It is one input layer: managed rules and live authorization still participate, and tool enablement depends on the host. Keep personal choices in the local file rather than exporting secrets with project configuration.

### .lingxi/settings.local.json

```json
{
  "permissions": {
    "allow": ["Read"]
  }
}
```

Verification method: API excerpt

### Focused settings and sandbox checks

```bash
cargo test --locked -p core --test settings_4layer_test
cargo test --locked -p harness-runtime --features desktop --test settings_watch_spawn
cargo test --locked -p harness-runtime --features desktop --test sandbox_startup_test
```

Verification method: Environment validation

These tests validate merge, watcher and startup contracts. Packaged macOS broker identity, native sandbox enforcement, Linux secret service and mobile OS permission prompts must be checked on their actual targets.

## Sources and documentation baseline

[Source repository](https://github.com/lingxi-coder/harness-runtime)

- [harness-runtime/crates/core/src/settings/mod.rs](https://github.com/lingxi-coder/harness-runtime/blob/4b099cf0f4eb2ab676e17dae806066d096ba39af/crates/core/src/settings/mod.rs) — Committed source (4b099cf0f4eb2ab676e17dae806066d096ba39af)
- [harness-runtime/crates/core/src/settings/loader.rs](https://github.com/lingxi-coder/harness-runtime/blob/4b099cf0f4eb2ab676e17dae806066d096ba39af/crates/core/src/settings/loader.rs) — Committed source (4b099cf0f4eb2ab676e17dae806066d096ba39af)
- [harness-runtime/crates/core/src/settings/merger.rs](https://github.com/lingxi-coder/harness-runtime/blob/4b099cf0f4eb2ab676e17dae806066d096ba39af/crates/core/src/settings/merger.rs) — Committed source (4b099cf0f4eb2ab676e17dae806066d096ba39af)
- [harness-runtime/crates/core/src/settings/schema.rs](https://github.com/lingxi-coder/harness-runtime/blob/4b099cf0f4eb2ab676e17dae806066d096ba39af/crates/core/src/settings/schema.rs) — Committed source (4b099cf0f4eb2ab676e17dae806066d096ba39af)
- [harness-runtime/crates/runtime/src/desktop/settings_watch.rs](https://github.com/lingxi-coder/harness-runtime/blob/4b099cf0f4eb2ab676e17dae806066d096ba39af/crates/runtime/src/desktop/settings_watch.rs) — Committed source (4b099cf0f4eb2ab676e17dae806066d096ba39af)
- [harness-runtime/crates/runtime/src/desktop/credentials.rs](https://github.com/lingxi-coder/harness-runtime/blob/4b099cf0f4eb2ab676e17dae806066d096ba39af/crates/runtime/src/desktop/credentials.rs) — Committed source (4b099cf0f4eb2ab676e17dae806066d096ba39af)
- [harness-runtime/crates/platforms/posix/src/secure_storage/factory.rs](https://github.com/lingxi-coder/harness-runtime/blob/4b099cf0f4eb2ab676e17dae806066d096ba39af/crates/platforms/posix/src/secure_storage/factory.rs) — Committed source (4b099cf0f4eb2ab676e17dae806066d096ba39af)
- [harness-runtime/crates/platforms/posix/src/secure_storage/plaintext.rs](https://github.com/lingxi-coder/harness-runtime/blob/4b099cf0f4eb2ab676e17dae806066d096ba39af/crates/platforms/posix/src/secure_storage/plaintext.rs) — Committed source (4b099cf0f4eb2ab676e17dae806066d096ba39af)
- [harness-runtime/crates/permission/src/mode.rs](https://github.com/lingxi-coder/harness-runtime/blob/4b099cf0f4eb2ab676e17dae806066d096ba39af/crates/permission/src/mode.rs) — Committed source (4b099cf0f4eb2ab676e17dae806066d096ba39af)
- [harness-runtime/crates/permission/src/policy_gate.rs](https://github.com/lingxi-coder/harness-runtime/blob/4b099cf0f4eb2ab676e17dae806066d096ba39af/crates/permission/src/policy_gate.rs) — Committed source (4b099cf0f4eb2ab676e17dae806066d096ba39af)
- [harness-runtime/crates/sandbox/src/decision.rs](https://github.com/lingxi-coder/harness-runtime/blob/4b099cf0f4eb2ab676e17dae806066d096ba39af/crates/sandbox/src/decision.rs) — Committed source (4b099cf0f4eb2ab676e17dae806066d096ba39af)

- harness-runtime: HEAD 4b099cf0f4eb2ab676e17dae806066d096ba39af; Referenced input fingerprint sha256:2d439c83e9c12bcb5aacfcf0a5da70a9d8beba68e9eba861d817bcf67c9bb51b; Product pin 4b099cf0f4eb2ab676e17dae806066d096ba39af

<!-- Generated from apps/web/src/data/docs. Do not edit this file directly. -->
