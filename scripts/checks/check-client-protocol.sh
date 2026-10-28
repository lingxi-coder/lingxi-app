#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/../.."
python3 scripts/checks/check_client_protocol.py
python3 -m unittest scripts/checks/test_bridge_protocol_generator.py
python3 scripts/checks/generate-client-protocol.py --test
