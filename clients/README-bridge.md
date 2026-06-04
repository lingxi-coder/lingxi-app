# Running a real conversation through the bridge-server

This is the end-to-end path the native shells use:

```
Electron / iOS shell  ──ws──▶  bridge-server (loopback)  ──▶  desktop engine  ──▶  Anthropic API
        ▲                                                                              │
        └──────────────────────  streamed ClientEvents  ◀──────────────────────────────┘
```

The `bridge-server` binary (`lingxi-code/apps/bridge-server`) boots one local
conversation over a `127.0.0.1` WebSocket, publishes a discovery lockfile at
`~/.claude/bridge/<port>.lock`, and drives a real `engine_desktop` runtime. The
Node-side `@lingxi/bridge-client` SDK (`clients/shared`) connects through that
lockfile, performs the version handshake, sends prompts, and consumes the
streamed `ClientEvent`s.

**The LLM API key is read from the `ANTHROPIC_API_KEY` environment variable at
runtime.** It is never hardcoded, logged, or committed. Without a key the server
still boots (transport is testable); a live turn fails with a 401 that surfaces
as a terminal `error` ClientEvent.

---

## 1. Build the bridge-server binary

From the cargo workspace:

```sh
cd lingxi-code
cargo build -p bridge-server --bin bridge-server
# binary: lingxi-code/target/debug/bridge-server
# (add --release for an optimized build at target/release/bridge-server)
```

## 2. Build the TypeScript SDK

```sh
cd clients/shared
npm install        # first time only
npm run build      # emits dist/
```

## 3. Headless transport proof (no key needed)

`scripts/e2e.mjs` builds (or reuses) the binary, launches it as a child
inheriting your environment, connects via the lockfile, sends
`"Reply with exactly: hello from lingxi"`, prints a transcript of the streamed
events, then shuts the server down cleanly.

**Keyless** — proves the full Electron→bridge→engine→event path end to end. With
no key the engine turn 401s and a terminal `error` ClientEvent streams back:

```sh
cd clients/shared
node ./scripts/e2e.mjs
# … transcript …
# error         kind=transport message="…"
# TRANSPORT OK (keyless)         ← success, exits 0
```

## 4. A REAL conversation (keyed)

Set your key in the environment and run the SAME script. The engine now performs
a real turn: the assistant reply streams as `text_delta` events and the turn
terminates with `turn_ended`:

```sh
cd clients/shared
export ANTHROPIC_API_KEY=sk-ant-...      # your key; never committed
node ./scripts/e2e.mjs
# … transcript …
# text_delta    "hello from lingxi"
# turn_ended    outcome=end_turn …
# TURN OK (keyed)                 ← success, exits 0
```

The script reports only whether a key was *present* (a boolean) — it never reads
or prints the key value.

Useful overrides:

| Variable | Effect |
|---|---|
| `ANTHROPIC_API_KEY` | The LLM key. Present ⇒ keyed turn; absent ⇒ keyless transport proof. |
| `BRIDGE_SERVER_BIN` | Use a specific prebuilt binary instead of the workspace `target/` build. |
| `LINGXI_API_BASE_URL` | Override the API base URL (default `https://api.anthropic.com`). |
| `RUST_LOG` | Bridge-server log level (default `info`). |

## 5. Drive a conversation by hand

Run the server directly and point any `@lingxi/bridge-client` consumer (or the
Electron app) at the published lockfile:

```sh
cd lingxi-code
export ANTHROPIC_API_KEY=sk-ant-...
./target/debug/bridge-server --cwd /path/to/your/project
# logs: bridge-server: listening … port=<N> lockfile=~/.claude/bridge/<N>.lock
# stop with ctrl-c (reaps the lockfile)
```

Flags: `--cwd <DIR>` (root the engine at a directory), `--model <ID>` (default
model), `-h/--help`.

## 6. The Electron GUI

The desktop shell spawns its OWN `bridge-server` child and connects through the
lockfile that child publishes — you do NOT need to launch a server separately.
With a key in the environment, the binary built (step 1), and the SDK built
(step 2):

```sh
cd clients/electron
npm install        # first time only
npm run dev        # electron-vite dev
```

The main process resolves the `bridge-server` binary in this order:

1. `BridgeManagerOptions.serverBin` (programmatic override), else
2. the `LINGXI_BRIDGE_SERVER_BIN` environment variable, else
3. a path derived **relative to the repo**, walking up to the first existing
   `lingxi-code/target/{debug,release}/bridge-server`.

If none resolve, the app surfaces an `error` connection state telling you to
build the binary (step 1) or set `LINGXI_BRIDGE_SERVER_BIN`. No absolute paths
are hardcoded, so a fresh clone works once the binary is built.
