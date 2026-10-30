#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/../.."
python3 scripts/checks/check_client_protocol.py

# Test runners print timings and temporary fixture paths. Keep successful gate
# output deterministic so the trigger check can compare real executions exactly.
test_log="$(mktemp)"
trap 'rm -f "$test_log"' EXIT
run_tests() {
    local label="$1"
    shift
    if "$@" >"$test_log" 2>&1; then
        printf 'CLIENT-PROTOCOL OK: %s\n' "$label"
    else
        local status=$?
        cat "$test_log" >&2
        return "$status"
    fi
}

run_tests 'Python generator regression tests passed' \
    python3 -m unittest scripts/checks/test_bridge_protocol_generator.py
run_tests 'locked Rust generator regression tests passed' \
    python3 scripts/checks/generate-client-protocol.py --test
