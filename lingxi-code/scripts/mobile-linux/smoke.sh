#!/usr/bin/env bash
set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo_root="$(cd "${script_dir}/../../.." && pwd)"
enabled="${LINGXI_MOBILE_LINUX_ENABLED:-0}"

"${script_dir}/check-authorizations.sh"
"${script_dir}/check-store-compliance.sh"
if [[ "${enabled}" == "1" ]]; then
  "${script_dir}/check-rootfs-manifest.sh" \
    "${repo_root}/docs/mobile-linux/rootfs/current/rootfs-manifest.json"
else
  "${script_dir}/check-rootfs-manifest.sh"
fi
"${script_dir}/check-sbom-and-licenses.sh"

echo "mobile-linux guardrail smoke checks passed"
