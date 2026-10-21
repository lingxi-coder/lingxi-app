# Native bindings generator

Run `cargo build --locked -p native-bindgen --bin uniffi-bindgen` from the
product workspace. The binary uses the same pinned UniFFI 0.32.2 format as
Android and iOS scaffolding, without depending on either platform runtime.

Both native build scripts invoke this generator. Kotlin retains the existing
error-property and cross-component audio-buffer repairs; Swift generation uses
the unchanged public UniFFI pipeline. The Android script reads its freshly
built, unstripped JNI library, then publishes matching bindings and libraries
in one rollback-protected transaction.

Validate the generator with `cargo test --locked -p native-bindgen` and
`python3 -m unittest scripts.tests.test_native_bindgen_boundary`.
