#!/usr/bin/env bash
set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo_root="$(cd "${script_dir}/../../.." && pwd)"
enabled="${LINGXI_MOBILE_LINUX_ENABLED:-0}"

"${script_dir}/check-authorizations.sh"
"${script_dir}/check-store-compliance.sh"
python3 "${script_dir}/verify-local-app-host.py" --repo-root "${repo_root}"
RUNTIME_ROOT="$(python3 "${script_dir}/../runtime_source.py" --root)"
exec bash "${RUNTIME_ROOT}/scripts/mobile-linux/smoke.sh"
