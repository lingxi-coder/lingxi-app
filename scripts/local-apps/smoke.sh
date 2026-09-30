#!/usr/bin/env bash
set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo_root="$(cd "${script_dir}/../.." && pwd)"
enabled="${LINGXI_MOBILE_LINUX_ENABLED:-0}"

"${script_dir}/check-authorizations.sh"
"${script_dir}/check-store-compliance.sh"
python3 "${script_dir}/verify-local-app-host.py" --repo-root "${repo_root}"
RUNTIME_ROOT="$(python3 "${script_dir}/../lib/runtime_source.py" --root)"
SDK_ROOT="$(python3 "${script_dir}/../lib/mobile_linux_source.py" --root)"
# LingXi owns the host release policy and composes the Harness and SDK gates.
# Every gate receives sources resolved from the product's canonical Cargo pins.
"${RUNTIME_ROOT}/scripts/local-apps/check-authorizations.sh"
if [[ "${enabled}" == "1" ]]; then
  "${RUNTIME_ROOT}/scripts/local-apps/check-rootfs-manifest.sh" \
    "${MOBILE_LINUX_EVIDENCE_DIR:?enabled release requires external evidence}/rootfs-manifest.json" \
    --sdk-root "${SDK_ROOT}"
else
  bash "${SDK_ROOT}/scripts/checks/check-resource-contracts.sh"
fi
"${RUNTIME_ROOT}/scripts/local-apps/check-sbom-and-licenses.sh" --sdk-root "${SDK_ROOT}"
if [[ "${enabled}" == "1" ]]; then
  if [[ -z "${LINGXI_LOCAL_APP_APK_DIR:-}" ]]; then
    echo "LINGXI_LOCAL_APP_APK_DIR is required for an enabled local-app runtime release" >&2
    exit 1
  fi
  python3 "${RUNTIME_ROOT}/scripts/local-apps/verify-local-app-supply-chain.py" \
    --repo-root "${RUNTIME_ROOT}" --sdk-root "${SDK_ROOT}" \
    --release --apk-dir "${LINGXI_LOCAL_APP_APK_DIR}"
else
  python3 "${RUNTIME_ROOT}/scripts/local-apps/verify-local-app-supply-chain.py" \
    --repo-root "${RUNTIME_ROOT}" --sdk-root "${SDK_ROOT}"
fi

echo "mobile-linux guardrail smoke checks passed"
