# Local telemetry acceptance

This is a development receiver for LingXi CLI/Desktop. It receives OTLP HTTP
(protobuf or JSON) on `127.0.0.1:4318` and OTLP gRPC on `127.0.0.1:4317`, then
writes bounded, newline-delimited OTLP JSON files. It has no cloud exporter and
does not require a Grafana, Jaeger, or database installation.

Use a fresh capture directory for each run. `traces.jsonl`, `metrics.jsonl`, and
`logs.jsonl` contain the Collector's received and filtered records. Each signal
rotates at 10 MiB with one backup. The configuration is intentionally local and
unauthenticated; it is not a production ingress configuration.

## Start the Collector

The pinned distribution is **otelcol-contrib 0.163.2**. Download the matching
platform archive from the [official release](https://github.com/open-telemetry/opentelemetry-collector-releases/releases/tag/v0.163.2)
and verify its SHA-256 against that release's checksums before extracting it.
`run-collector.sh` checks the binary version and refuses to overwrite an existing
capture. The binary is not installed or downloaded by the script.

From the repository root, in a dedicated terminal:

```sh
export OTELCOL_CONTRIB=/absolute/path/to/otelcol-contrib
bash scripts/telemetry/run-collector.sh output/telemetry/enabled --validate
bash scripts/telemetry/run-collector.sh output/telemetry/enabled
```

The runner always binds loopback. Check readiness from another terminal:

```sh
curl --fail --silent http://127.0.0.1:13133/
```

Docker is an alternative, using the same pinned release and configuration:

```sh
mkdir -p output/telemetry/enabled
export LINGXI_OTEL_CAPTURE_DIR="$(pwd)/output/telemetry/enabled"
export LINGXI_OTEL_UID="$(id -u)"
export LINGXI_OTEL_GID="$(id -g)"
docker compose -f deploy/telemetry/compose.yaml up
```

Compose binds all published ports to host loopback, sets a 384 MiB memory limit,
and mounts only the configuration and capture directory. Stop only this stack
with `docker compose -f deploy/telemetry/compose.yaml down`. No shared volume or
application data needs to be removed.

## Point a development CLI at it

Run a freshly built CLI from an isolated test project/profile:

```sh
export LINGXI_ENABLE_TELEMETRY=true
export OTEL_SERVICE_NAME=lingxi-cli
export OTEL_RESOURCE_ATTRIBUTES=deployment.environment.name=development
export OTEL_EXPORTER_OTLP_ENDPOINT=http://127.0.0.1:4318
export OTEL_EXPORTER_OTLP_PROTOCOL=http/protobuf
export OTEL_TRACES_EXPORTER=otlp
export OTEL_METRICS_EXPORTER=otlp
export OTEL_LOGS_EXPORTER=otlp
export OTEL_TRACES_SAMPLER=always_on
export OTEL_METRIC_EXPORT_INTERVAL=1000
```

The LingXi reporting switch controls all three signals. Standard privacy disable
flags take precedence. Content opt-ins cannot bypass the outbound diagnostics
schema. Do not enable HTTP gzip for this first integration; the current Rust
exporter does not support it.

For Desktop, close the app and merge these preferences into the existing
`settings.v1.json` in the Electron user-data directory (the directory returned by
`app.getPath('userData')`). Preserve the document's existing `version: 1` and
other settings:

```json
{
  "telemetryEnabled": true,
  "telemetryEndpoint": "http://127.0.0.1:4318"
}
```

Restart Desktop to apply enable/endpoint changes. With no saved boolean, a
trusted parent `LINGXI_ENABLE_TELEMETRY=true` may opt in; a saved `false` and
privacy-disable flags take precedence. The existing settings update path can
revoke reporting live with `telemetryEnabled: false`. This phase adds no new
settings UI. Workspace/project settings cannot enable reporting or redirect its
destination, and the renderer does not send telemetry directly.

## Verify an actual Agent request

The request fixture is `scripts/telemetry/run_agent_smoke.py`. It starts its own
Collector, a deterministic model server, and an OTLP recording proxy on ephemeral
loopback ports. It gives the built CLI isolated configuration and working
directories, then exercises the real Agent and a `Read` tool invocation. It uses
synthetic credentials and does not call a paid model provider.

```sh
python3 scripts/telemetry/run_agent_smoke.py \
  --cli /absolute/path/to/lingxi \
  --collector /absolute/path/to/otelcol-contrib \
  --output output/telemetry/agent-smoke
```

The default cases are `enabled`, `disabled`, `do-not-track`, `disable-telemetry`,
`provider-error`, and `collector-down`. To select a subset, append
`--cases enabled disabled`. Every case gets a new subdirectory with `result.json`,
CLI stdout/stderr, Collector logs, and a `capture/` directory. Reusing an existing
case directory fails rather than overwriting evidence. The script checks raw
OTLP requests before Collector filtering for its synthetic private-content and
credential markers. The enabled case also runs the capture validator below.

This is the executable acceptance procedure; its presence alone does not claim
that a built Agent has passed it. Collector transport probes or fabricated span
JSON are not an Agent acceptance result.

After the CLI exits and its telemetry guard flushes, allow the Collector batch
to drain (normally below one second), then validate the capture:

```sh
python3 scripts/telemetry/verify_capture.py output/telemetry/agent-smoke/enabled/capture \
  --root-span lingxi.headless.request \
  --forbid TELEMETRY_PRIVATE_CONTENT_CANARY_71029 \
  --forbid fixture-only-key
```

One-shot CLI (`-p`) uses `lingxi.headless.request` (the validator default).
Interactive TUI/REPL uses `lingxi.cli.request`; Desktop uses
`lingxi.desktop.request`. Set `--root-span` accordingly. Do not filter to one
`--service` for Desktop because its root and bridge child belong to different
services. The validator requires, within one trace:

- A parentless request root and a descendant `lingxi.orchestrator.turn*` span.
- Descendant `lingxi.llm.request` and `lingxi.tool.execute` spans with positive
  durations that start during their turn, connected by valid parent links in the
  same trace. These links describe asynchronous causality, not strict timestamp
  containment: admission can finish after dispatch, and the host can receive
  `turn_ended` before Rust finishes dropping its span guards. Child end times may
  therefore follow the turn/root end time. Desktop SDK tests separately verify
  the host request span's send-to-terminal-event lifetime.
- A log carrying a trace/span ID from that request, and received metric points.
- No prohibited identity/content attributes or supplied synthetic private marker.

The validator does not print captured content. Its tests cover broken parent
graphs, unrelated traces, orphan roots, placeholder durations, missing metric
points, missing log correlation, and private data.

```sh
python3 -m unittest discover -s scripts/telemetry -p 'test_*.py'
```

Repeat the request against a **new** capture directory with
`LINGXI_ENABLE_TELEMETRY=false`, and separately with `DO_NOT_TRACK=1` while the
master switch is enabled. Each run must finish normally and pass:

```sh
python3 scripts/telemetry/verify_capture.py output/telemetry/disabled --expect-empty
```

Also stop the Collector and repeat the request with telemetry enabled: the Agent
must still complete. Record exit status and time; exporter errors must not make
the model/tool operation fail or hang indefinitely.

## Exercise the Desktop host and live disable

With the freshly built bridge and Electron/bridge-client Node dependencies
available, run:

```sh
python3 scripts/telemetry/run_desktop_smoke.py \
  --bridge /absolute/path/to/lingxi-bridge-server \
  --collector /absolute/path/to/otelcol-contrib \
  --output output/telemetry/desktop-smoke
```

This starts the real bridge and runs the same `DesktopTelemetry` SDK and
`BridgeClient` used by the host. The local model requests only a `Read` of the
fixture-owned, read-only `telemetry-fixture.txt` containing `TELEMETRY_TOOL_OK`.
The driver denies other tool approvals and
requires the expected fixture answer. It verifies an enabled
`lingxi.desktop.request` trace, revokes both host and bridge reporting, then
performs a second real model request. A stdin checkpoint handshake freezes the
OTLP receipt count before the second turn, which must finish with no new exports,
including during bridge shutdown. Raw pre-Collector records are checked for the
synthetic private-content and credential markers.

The fixture preserves `HOME` and supplies isolated bridge/work directories.
Its configuration and session-global configuration live in a temporary directory
under `/private/tmp`, using the credential layer's existing ephemeral-root
support; this directory is cleaned up after the run. Captures and logs remain
under `--output`. It starts no Electron window; it verifies the host SDK, carrier,
bridge, Agent and exporter integration, not packaged application UI behavior.
The output directory includes process logs, Collector captures and `result.json`.

## Privacy and interpretation

The SDK enforces the outbound schema before network egress. The Collector adds
an independent attribute allowlist, clears arbitrary log bodies/status messages,
and removes span-event payloads. Test raw exporter requests separately when
checking the SDK: a filtered Collector capture alone cannot prove sensitive data
never left the application.

Metrics must use bounded diagnostic dimensions rather than user, session,
request, path, or host identifiers. One generated, ephemeral UUIDv4
`service.instance.id` is retained on the resource so cumulative metric streams
from separate CLI/Desktop bridge processes remain distinct. It is created by
the runtime, never taken from user settings, and is not copied into datapoint
attributes. The validator accepts its UUID shape only on resources and rejects
it as a metric/log/span attribute; runtime tests establish generation and
override behavior. This process identity does not identify a user or session.

The development Collector is not a product analytics system and is not an audit
ledger. Keep Crashlytics consent and crash verification separate from this
performance telemetry acceptance.

Configuration references:

- [File exporter format and rotation](https://github.com/open-telemetry/opentelemetry-collector-contrib/tree/v0.163.0/exporter/fileexporter)
- [Transform processor and OTTL contexts](https://github.com/open-telemetry/opentelemetry-collector-contrib/tree/v0.163.0/processor/transformprocessor)
- [Collector gateway deployment](https://opentelemetry.io/docs/collector/deploy/gateway/)
