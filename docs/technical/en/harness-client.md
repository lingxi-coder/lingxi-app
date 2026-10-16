# Client protocol & adapters

Share commands, events, presentation, and runtime adapters across clients.

<a id="modules"></a>

## Three client modules

The companion Rust package is client. protocol provides transport-independent commands, events, DTOs, and a protocol version. presentation contains shared tool summaries, structured diffs, text rendering, and styles. adapter translates runtime output and interactive requests into client DTOs.

The default feature set exposes protocol only. presentation enables rendering helpers; adapter includes presentation and runtime integration. uniffi adds native metadata for the enabled modules. test-support enables adapter and MockSink and belongs on development dependencies.

### Cargo.toml · same local source as the runtime

```toml
[dependencies]
client = { path = "../harness-runtime/crates/client", features = ["adapter"] }
```

Verification method: API excerpt

<a id="wire-contract"></a>

## Commands & events

ClientCommand and ClientEvent use internally tagged snake_case JSON with a type field and are non-exhaustive Rust enums. Live commands target the session mounted on the connection. ResumeSession names a target session; normal prompt submission does not carry session_id.

SendPrompt carries text, optional prompt_mode and turn_id, and an images vector. Cancel accepts an optional turn_id. The event feed includes TextDelta, ToolUseStarted, ToolUseResult, MessageComplete, TurnStarted, TurnEnded, session lifecycle events, and structured errors.

### ClientCommand JSON · SendPrompt

```json
{
  "type": "send_prompt",
  "text": "Explain this project",
  "images": [],
  "turn_id": 1
}
```

### ClientCommand JSON · Cancel

```json
{
  "type": "cancel",
  "turn_id": 1
}
```

### CLIENT_PROTOCOL_VERSION

```text
pub const CLIENT_PROTOCOL_VERSION: &str = "25.0.0";
```

Protocol version declared in this inspected source tree. Native clients must version-lock generated binding layouts with the host they actually build.

These JSON examples show the command payload, not a transport envelope. Bridge frames and native binding entrypoints are defined by their respective hosts.

<a id="output-adapter"></a>

## Connect an event destination

ClientEventSink is the shared outbound seam. AdapterOutputStream implements the runtime OutputStream and lowers its callbacks into ClientEvent values. The sink can enqueue events for a WebSocket transport or forward them through a mobile ListenerSink; adapter code remains independent of the transport.

### Rust · host-supplied event sink

```rust
use std::sync::Arc;
use client::adapter::{AdapterOutputStream, ClientEventSink};

fn make_output(sink: Arc<dyn ClientEventSink>) -> AdapterOutputStream {
    AdapterOutputStream::new(sink)
}
```

Verification method: Compile check (harness-output-adapter)

### ClientEventSink::emit

```text
async fn emit(&self, event: client::protocol::events::ClientEvent)
```

Forward one lowered event. Keep delivery cheap so transport work does not stall execution.

### AdapterOutputStream::new

```text
pub fn new(sink: Arc<dyn ClientEventSink>) -> Self
```

Create a connection-scoped output adapter around the shared sink.

### ClientEventListener::on_event

```text
async fn on_event(&self, event: client::protocol::events::ClientEvent)
```

Mobile host callback for each client event; exposed as a callback interface when uniffi is enabled.

<a id="turn-events"></a>

## Turn events & interactive requests

TurnEventEmitter emits turn-start and completion/error events around a result supplied by the caller. It does not execute a turn. AdapterPermissionGate, AskUserQuestionBroker, and ComputerAccessBroker provide connection-scoped interactive request handling; hosts still own execution and response routing.

### TurnEventEmitter::new

```text
pub fn new(sink: Arc<dyn ClientEventSink>) -> Self
```

Use the same event destination as the output and permission adapters.

### TurnEventEmitter::emit_turn_started

```text
pub async fn emit_turn_started(&self, turn_id: Option<u64>)
```

Emit a correlated start event for a host-admitted turn.

### TurnEventEmitter::complete

```text
pub async fn complete(&self, result: Result<orchestrator::streaming_loop::PumpedTurn, orchestrator::OrchestratorError>)
```

Translate a pumped turn result into message completion or error events.

Client DTO compatibility does not establish device or transport correctness. Validate generated bindings, event routing, permissions, and shutdown in the target host.

<a id="resume-presentation"></a>

## Cold resume and compatibility failures

MessageComplete and resumed history use the same ordered MessageDto block schema. MessageDto.sent_at_ms carries the original persisted Unix time; source transcript UUID mapping supplies historical timestamps instead of matching by text or message position. Inline images and visualization references likewise retain their own identity.

Rust enums are non-exhaustive; hosts handle unknown future variants without inventing successes. Native enum and record layouts require matching generated bindings and the protocol contract gate. A JSON payload that deserializes does not establish a compatible UniFFI binary layout or correct transport handshake.

### Shared client contracts

```bash
cargo test --locked -p client --features test-support
```

Verification method: Environment validation

The current working-tree protocol constant is 21.0.0. Always read the baseline and the host pin before connecting a separate client. Device callbacks, ordering under disconnect and replay presentation remain target-host checks.

## Sources and documentation baseline

[Source repository](https://github.com/lingxi-coder/harness-runtime)

- [harness-runtime/crates/client/src/lib.rs](https://github.com/lingxi-coder/harness-runtime/blob/6a26289330604ca621d3d1c46e5e71e3369f9955/crates/client/src/lib.rs) — Committed source (6a26289330604ca621d3d1c46e5e71e3369f9955)
- [harness-runtime/crates/client/Cargo.toml](https://github.com/lingxi-coder/harness-runtime/blob/6a26289330604ca621d3d1c46e5e71e3369f9955/crates/client/Cargo.toml) — Committed source (6a26289330604ca621d3d1c46e5e71e3369f9955)
- [harness-runtime/crates/client/src/protocol/version.rs](https://github.com/lingxi-coder/harness-runtime/blob/6a26289330604ca621d3d1c46e5e71e3369f9955/crates/client/src/protocol/version.rs) — Committed source (6a26289330604ca621d3d1c46e5e71e3369f9955)
- [harness-runtime/crates/client/src/protocol/commands.rs](https://github.com/lingxi-coder/harness-runtime/blob/6a26289330604ca621d3d1c46e5e71e3369f9955/crates/client/src/protocol/commands.rs) — Committed source (6a26289330604ca621d3d1c46e5e71e3369f9955)
- [harness-runtime/crates/client/src/protocol/events.rs](https://github.com/lingxi-coder/harness-runtime/blob/6a26289330604ca621d3d1c46e5e71e3369f9955/crates/client/src/protocol/events.rs) — Committed source (6a26289330604ca621d3d1c46e5e71e3369f9955)
- [harness-runtime/crates/client/src/adapter/output_stream.rs](https://github.com/lingxi-coder/harness-runtime/blob/6a26289330604ca621d3d1c46e5e71e3369f9955/crates/client/src/adapter/output_stream.rs) — Committed source (6a26289330604ca621d3d1c46e5e71e3369f9955)
- [harness-runtime/crates/client/src/protocol/message.rs](https://github.com/lingxi-coder/harness-runtime/blob/6a26289330604ca621d3d1c46e5e71e3369f9955/crates/client/src/protocol/message.rs) — Committed source (6a26289330604ca621d3d1c46e5e71e3369f9955)

- harness-runtime: HEAD 6a26289330604ca621d3d1c46e5e71e3369f9955; Referenced input fingerprint sha256:50eb3dc51cb2a5ee927dcf0f48b20b7e956398fe44c92c11c0f1ee8b4b25cc83; Product pin 6a26289330604ca621d3d1c46e5e71e3369f9955

<!-- Generated from apps/web/src/data/docs. Do not edit this file directly. -->
