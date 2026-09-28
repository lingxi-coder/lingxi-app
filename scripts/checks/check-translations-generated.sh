#!/usr/bin/env bash
# Local App Phase 8 i18n freshness gate.
#
# The real freshness check is the canonical generator's `--check` mode with NO
# custom output dirs: it byte-compares the committed xcstrings/strings.xml
# outputs and fails on missing, stale, or orphaned generated files.
set -euo pipefail
cd "$(dirname "$0")/../.."

exec python3 clients/translations/generate.py --check
