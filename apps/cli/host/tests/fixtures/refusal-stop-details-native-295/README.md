# Native 2.1.295 refusal stop-details baseline

This Product test fixture is copied byte-for-byte from
`harness-runtime/docs/parity/current-alignment/fixtures/stop-details-295`.
The source fixture README is retained at `SOURCE-README.md`; its
`SHA256SUMS` manifest pins the response, original public stdout, Native
receipts, and captured stop-details row artifacts.

`apps/cli/host/tests/provider_error_cli.rs` replays the two captured SSE
responses through the public Product `--print --output-format json`
entrypoint, using a dummy key and a `127.0.0.1`-only stub. The test compares
the raw JSON `result` value and deterministic top-level field slices without
reserializing stdout. It separately constrains generated duration and UUID
fields, then checks the persisted refusal type, category presence, and exact
explanation UTF-16 units. This test does not claim whole-stdout byte equality.

The companion `http-layout-native-295` fixture supports a pinned Product
headless print-text request replay. Its request-body comparison leaves only
the documented 64-byte `metadata.user_id.device_id` range dynamic; header
comparison permits only the loopback `Host` and dummy API key values.

`native-baseline/<case>/` contains only the captured response bytes, stdout,
stderr, extracted stop-details rows, and receipt needed to replay and audit
these cases. The temporary HOME, complete session JSONL, and other run state
are intentionally not included. `SHA256SUMS` pins every checked-in case and
capture artifact to the captured bytes.

The test parses the original stdout bytes through `Utf16JsonProjection` and
compares the exact `/result` UTF-16 units. It does not reserialize or compare
the entire stdout JSON, whose unrelated fields can vary. It separately checks
the fixed response/session identity, model/stream request shape, terminal
refusal marker, and persisted explanation units. Native 2.1.295 captured three
stop-details rows per case; the Harness assertion checks the explanation and
category semantics on its own transcript rows.

The 2.1.295 acceptance comparison records the Native result and response bytes
equal to the 2.1.293 case, with the same three-row explanation persistence.
This is a Native baseline only and does not itself establish Harness parity.
