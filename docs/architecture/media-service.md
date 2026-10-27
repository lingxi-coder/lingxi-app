# App media integration

The product injects platform implementations and service connection policy into
Harness. `AppMediaHost` is the app-scoped composition point for the shared
`MediaService`, native device ports, resource store, provider registry and product
connector. Agent engines borrow this host; engine shutdown ends that owner's
resources rather than the entire app's media service.

## Dependency direction

`media-api` defines portable operations and frames. `media-runtime` manages
resources and backend selection. The built-in `tool-media` package and bundled
`media` skill expose available operations to the Agent. Native adapters implement
capture, playback, permissions and device leases. The product's Remote Control
adapter supplies authenticated signaling and per-session ICE configuration.

Firebase configuration, approved-viewer routing, pairing identity and Remote
Control message formats belong to this repository. They are not runtime
configuration defaults in Harness. `RemoteMediaConfigProvider` is injected into
the product connector and resolves one frozen connection configuration for each
session.

## Platform seams

| Platform | Host boundary |
| --- | --- |
| Desktop | Rust app media service; authenticated bridge to Electron main for native device and approved peer operations |
| Android | Direct UniFFI callbacks to native audio/camera/device adapters |
| iOS | Direct UniFFI callbacks to native audio/camera/device adapters |
| Web | Existing remote viewer; media traverses the negotiated RTC connection |
| CLI | No device service injected; unsupported media tools are omitted |

Native request/response DTOs are correlated, bounded host control traffic. They
do not become model messages, AG-UI events, durable turn events or remote
conversation broadcasts. The bridge verifies authenticated route and attached
session ownership before forwarding media operations.

Device ports declare their supported session modes separately from legacy
AudioService operations. Desktop obtains that declaration from its native helper
and forwards it on the authenticated handshake and capability-update lane.
Mobile hosts obtain the modes directly from their injected native callback.
Permission and device-busy results remain transient readiness conditions.

The app's logical media epoch is independent of the native audio helper's epoch.
Routing translates an operation to its selected backend's epoch and records that
identity for cancellation. A helper restart can refresh device readiness without
making every app-level audio request stale.

## Remote sessions

Remote Control's data connection carries approved control and signaling. A
separate RTC media connection carries audio/video. Firebase is not a channel for
raw media frames. A single aggregate connector resolves opaque source references
to the exact approved viewer route; it never chooses an arbitrary connected peer.
Route close or revocation removes that route and stops its related media sessions.

The connector owns one RTC pump and opens the native device ports once. A
conversation uses the same session's decoded input and generated-output audio
channel. Desktop conversation output returns through the selected peer without
opening the host's microphone or speaker. Physical mobile capture uses the
existing native resource coordinator and permissions.

Observation sessions expose bounded frames/events without starting another
Agent. Conversation sessions attach the current Agent's realtime driver after
the media-start tool result is committed. Tools, approvals, history and
cancellation remain owned by the current Harness conversation.

Stopping a remote session waits for the selected peer to acknowledge native
device cleanup. Failed cleanup remains retryable. Playback receipts use the
caller's PCM sample rate; interruptions preempt a pending flush and prevent it
from acknowledging the interrupted audio again.

## Settings and rollout

Existing audio configuration v4, credential/account ownership and device pairing
remain authoritative. This change adds no legacy settings decoder or business
data migration. The old public `voice`, `speech`, `camera` tools and `mobile-media`
skill are replaced by the unified media interface.

For development across isolated repositories, use the explicit local Cargo patch
validated by `scripts/lib/runtime_source.py`. A local patch is development
provenance; release builds require publishing Harness and updating immutable
product pins. Do not identify a development patch as a published upstream revision.

Validation must distinguish contract tests, native compilation, fresh sidecar /
JNI / XCFramework generation and physical device acceptance. Production Firebase
deployment and formal release publication are separate actions.
