# Shared mobile bindings implementation

This directory is the canonical source for the behavior used by the iOS and
Android Rust FFI adapters. `remote.rs` owns connection generations, ordered
event delivery, session projections, transfers, and shutdown. `audio_provider.rs`
owns provider and realtime-session lifetimes and callback delivery.
`callbacks.rs` implements the identical share and secure-storage conversions.

The platform FFI crates compile these files as ordinary Rust modules via
`#[path]`. They are intentionally not a separate UniFFI component: keeping the
exported records and errors in each existing component preserves their Swift
and Kotlin binding names. The small platform wrappers define the native callback
and object names and choose the platform identity and secure-storage backend.
They must not duplicate the shared behavior.

The Remote and callback tests live beside these sources and are compiled by
both platform crates. Run the `ios-framework` and `android-aar` Rust tests after
changes. Generate fresh bindings from matching rebuilt libraries before checking
native consumers; previously generated bindings must not be reused for this
validation. `cargo fmt --all -- --check` traverses the referenced modules; the
shared files can also be checked directly with `rustfmt --edition 2021 --check`.
