# Audio within the media service

Audio is part of the app-scoped media service. `AppMediaHost` composes native
audio/camera adapters, a bounded resource store, the shared route selector and an
injected product connector. Agent tools, native UI and Remote Control use that
same service and native device coordinator. See [Media service](media-service.md)
for connection and lifecycle ownership.

Recognition, speech synthesis and native realtime conversation have independent
provider/model bindings in audio configuration v4. `follow_session` uses the
current session's exact provider profile and its credential account. An explicit
profile overrides only that audio operation; a chat model is never reused as an
audio model. A request freezes its binding and configuration before capture or
network dispatch. Session changes invalidate the prepared request.

`automatic` chooses a ready system service, then an installed compatible offline
backend before dispatch. It never enables cloud audio. Explicit choices do not
retry through another backend when unavailable or after an operation fails.
Local capture, playback and settings previews do not create an Agent engine.

The shared SDK owns provider capabilities, protocol, transport and audio model
defaults. The product's `audio-provider` crate resolves host credentials and
provides bounded transcription, PCM synthesis and native realtime sessions to
Electron main and mobile FFI. Native device adapters own permissions, audio
resource leases, PCM conversion and actual playback completion. Harness tools
use the same native device service as their product UI. Product hosts map the
saved v4 configuration into the common route selector. For a configured local
route, the native host resolves System/Offline and installed models from one
snapshot. A local-only execution constraint prevents a concurrent settings
change from dispatching that operation to a provider. The common runtime
resolves cloud operations against the exact SDK/profile/account binding and
checks a saved provider voice against the resolved profile and effective model.
Automatic System/Offline voice pins remain local.

Only audio configuration v4 is accepted. Missing or older stored configurations
use fresh v4 defaults; old keys, voice strings and configuration migrations are
not read or executed.

Native realtime attaches to the current Harness `ConversationOrchestrator`,
imports its history and invokes tools through its normal approval and execution
path. Output transcripts enter history only after native playback is acknowledged.
Cancellation and interruption discard output that has not been acknowledged.
Providers lacking the required Agent realtime contract remain unavailable even
when they expose a lower-level audio API. Turn-based capture is the default.
Interruptible conversation additionally requires provider audio truncation and
verified device echo cancellation and an actual speech interruption detector.
Current product controllers expose turn-based realtime; SDK truncation support
alone does not enable natural spoken interruption.

Capability support and readiness are separate. Unsupported use controls are
hidden; configuration, permission and temporary availability failures remain
recoverable through settings or an actionable operation error. Settings remain
accessible. Availability is per operation, rather than a single audio-enabled flag.

## Dependency ownership

The SDK owns provider audio protocols; Harness owns Agent execution, and the
shared `media-api` crate owns media operation and native port contracts. Product dependencies
use immutable published Git revisions for release builds. Cross-repository
development uses an explicit local Harness patch, validated by
`scripts/lib/runtime_source.py`, until the new Harness revision is published and
the product pins are updated. A local patch does not establish release
provenance.

Unit, contract and native compilation checks do not establish microphone,
speaker, echo cancellation or live provider behavior on a physical device.
