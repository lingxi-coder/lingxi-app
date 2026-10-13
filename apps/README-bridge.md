# Bridge transport and product development

The current protocol, complete client example and failure handling are maintained
in the bilingual [technical handbook](../docs/technical/en/bridge-client.md)
([中文](../docs/technical/zh/bridge-client.md)). Electron consumes the local
Rust Bridge through the private Node.js SDK. Android and iOS use platform-local
Rust FFI. Subscribe to events before connect/send so fast replies are not lost.

## Build the transport

From the product root:

```sh
./apps/setup.sh
cargo build --locked -p bridge-server --bin bridge-server
npm --prefix packages/bridge-client ci
npm --prefix packages/bridge-client run build
```

The SDK consumes its compiled `dist/` exports; build it before Electron. The
Bridge publishes a discovery lockfile, performs framing and client-protocol
handshakes and emits typed events. Connection/version errors are separate from
model authentication or execution failures.

## Credential delivery

The current Bridge does **not** inherit model keys from environment variables or
argv. Its host delivers credentials through `--credential-stdin` or
`--api-key-stdin`. A manually configured single-key host can use:

```sh
# PROVIDER_API_KEY is supplied by your environment; never enable shell tracing.
printf '%s' "$PROVIDER_API_KEY" | ./target/debug/bridge-server \
  --api-key-stdin --cwd /path/to/project --model your-configured-model
```

The key is sent over the pipe, not the process arguments. Provider/model settings
must match the delivered credentials. Electron uses its own credential delivery
and broker lifecycle; use the signed application for credential persistence and
real-session acceptance. `npm run dev` supports renderer development.

The old `packages/bridge-client/scripts/e2e.mjs` helper does not deliver these
stdin credentials. Its environment-key success assumptions are historical; do
not treat it as a current keyed model-turn acceptance test.

## Checks and acceptance

```sh
npm --prefix packages/bridge-client test
npm --prefix apps/electron run typecheck
npm --prefix apps/electron test
```

Transport compilation, mock events and keyless checks do not prove real provider
behavior. Verify current source identity, credentials, actual terminal events and
persistence in the appropriate host. macOS signed packaging follows
[repository instructions](../AGENTS.md) and the handbook's build/release chapter.
