#!/usr/bin/env bash
set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
enabled="${LINGXI_MOBILE_LINUX_ENABLED:-0}"

"${script_dir}/check-authorizations.sh"
"${script_dir}/check-store-compliance.sh"
RUNTIME_ROOT="$(python3 "${script_dir}/../lib/runtime_source.py" --root)"
SDK_ROOT="$(python3 "${script_dir}/../lib/mobile_linux_source.py" --root)"
# LingXi owns the host release policy and composes the Harness and SDK gates.
# Every gate receives sources resolved from the product's canonical Cargo pins.
"${RUNTIME_ROOT}/scripts/mobile-linux/check-authorizations.sh"
if [[ "${enabled}" == "1" ]]; then
  "${RUNTIME_ROOT}/scripts/mobile-linux/check-rootfs-manifest.sh" \
    "${MOBILE_LINUX_EVIDENCE_DIR:?enabled release requires external evidence}/rootfs-manifest.json" \
    --sdk-root "${SDK_ROOT}"
else
  bash "${SDK_ROOT}/scripts/checks/check-resource-contracts.sh"
fi
"${RUNTIME_ROOT}/scripts/mobile-linux/check-sbom-and-licenses.sh" --sdk-root "${SDK_ROOT}"

echo "mobile-linux guardrail smoke checks passed"
