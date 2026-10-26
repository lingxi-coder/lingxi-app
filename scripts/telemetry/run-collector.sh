#!/usr/bin/env bash
set -euo pipefail

# Use the same pinned Collector/config on a workstation without Docker.
# Downloading/installing binaries is deliberately separate from this runner.
if [[ $# -lt 1 || $# -gt 2 || ( $# -eq 2 && "$2" != "--validate" ) ]]; then
  echo "Usage: OTELCOL_CONTRIB=/path/to/otelcol-contrib $0 /new/capture/directory [--validate]" >&2
  exit 2
fi
collector="${OTELCOL_CONTRIB:-otelcol-contrib}"
collector_version="$("$collector" --version)"
if [[ "$collector_version" != *"0.163.2"* ]]; then
  echo "Expected otelcol-contrib 0.163.2; found: $collector_version" >&2
  exit 2
fi
repo_root="$(cd "$(dirname "$0")/../.." && pwd)"
mkdir -p "$1"
export LINGXI_OTEL_OUTPUT_DIR="$(cd "$1" && pwd)"
export LINGXI_OTEL_BIND_ADDRESS=127.0.0.1
if [[ "${2:-}" == "--validate" ]]; then
  exec "$collector" validate --config="$repo_root/deploy/telemetry/collector.yaml"
fi
for signal in traces metrics logs; do
  if compgen -G "$LINGXI_OTEL_OUTPUT_DIR/$signal*.jsonl" > /dev/null; then
    echo "Capture files already exist; select a new directory to preserve evidence." >&2
    exit 2
  fi
done
umask 077
exec "$collector" --config="$repo_root/deploy/telemetry/collector.yaml"
