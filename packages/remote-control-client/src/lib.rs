//! Viewer-side client of LingXi Remote Control (`lingxi-rc/1`).
//!
//! The crate lets a phone (through UniFFI, later) list the user's computers, dial
//! one over Firebase RTDB signaling, bring up a WebRTC DataChannel or the
//! end-to-end encrypted RTDB tunnel, pair with the desktop and speak the RPC
//! protocol. `packages/bridge-client/src/remote` (TypeScript) is the reference
//! implementation; the fixtures in `packages/bridge-client/test/fixtures/remote`
//! are the shared contract.

#![forbid(unsafe_code)]

pub mod clock;
pub mod connection;
pub mod decode;
pub mod device;
mod direct;
pub mod encoding;
pub mod error;
pub mod files;
pub mod framing;
#[cfg(feature = "native-media")]
pub mod host_media;
pub mod keys;
pub mod local_apps;
pub mod media;
pub mod messages;
#[cfg(feature = "native-media")]
pub mod native_media;
#[cfg(feature = "webrtc")]
pub mod native_peer;
pub mod pairing;
pub mod peer;
pub mod rpc;
pub mod rtdb;
pub mod session;
pub mod signaling;
pub mod testing;
pub mod text;
pub mod transport;
pub mod tunnel;
pub mod tunnel_crypto;
pub mod version;
