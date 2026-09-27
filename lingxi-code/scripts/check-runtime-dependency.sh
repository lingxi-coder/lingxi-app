#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/../.."
exec python3 lingxi-code/scripts/check-runtime-dependency.py
