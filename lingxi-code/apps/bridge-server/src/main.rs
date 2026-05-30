//! `bridge-server` (M8-P13) — hello-world remote-engine entrypoint.
//!
//! M8 ships an inert binary that initializes tracing and proves the `bridge`
//! wire types link. M9 grows this into the real remote-drive server: a
//! WebSocket listener, the [`bridge::ClientHello`]/[`bridge::ServerHello`]
//! handshake, the JWT auth flow, and `engine-desktop` wiring so a remote
//! (mobile) client can drive a desktop engine.

#![forbid(unsafe_code)]

use bridge::{Capabilities, ServerHello, BRIDGE_PROTOCOL_VERSION};

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();

    // Prove the bridge wire types load (M9 sends this over a real socket).
    let hello = ServerHello {
        protocol_version: BRIDGE_PROTOCOL_VERSION.to_string(),
        server_name: concat!("lingxi-bridge-server/", env!("CARGO_PKG_VERSION")).to_string(),
        capabilities: Capabilities::default(),
    };

    tracing::info!(
        protocol_version = %hello.protocol_version,
        server_name = %hello.server_name,
        "bridge-server hello-world (M9 will grow this into the remote-drive server)"
    );

    // M9: bind a WebSocket listener, run the handshake + auth, wire engine-desktop.
    Ok(())
}
