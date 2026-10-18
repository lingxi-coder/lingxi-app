# Product CLI HTTP boundary replay

This test fixture is copied byte-for-byte from
`harness-runtime/docs/parity/current-alignment/fixtures/http-layout-295`.
The original fixture README is preserved at `SOURCE-README.md`, and
`native-baseline/2.1.295/artifact-sha256.json` pins every retained capture.

`apps/cli/host/tests/provider_error_cli.rs` invokes the Product CLI with the
same fixed model, prompt, system prompt, tools, permission mode, and session ID
as the Native 2.1.295 headless print-text capture. It feeds the captured raw
HTTP response over an isolated loopback listener and compares the actual
Product request line and headers as bytes, allowing only the dynamic loopback
`Host` value. The API key is loaded from the captured synthetic fixture header
so its bytes remain exact. The complete request body is byte-compared except
the documented 64-byte `metadata.user_id.device_id` range, which is checked
for Native's lowercase-hex format. The test also checks the exact fixed stdout
and empty stderr.

The comparison does not deserialize and reserialize the request body, alter
the captured prompt, or claim the request body is identical when its device ID
differs. It does not launch Native; it uses only the retained 2.1.295 capture.
