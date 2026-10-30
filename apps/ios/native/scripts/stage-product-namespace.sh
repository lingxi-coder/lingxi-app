#!/usr/bin/env bash
set -euo pipefail

export PATH="${PATH}:${HOME}/.cargo/bin:/opt/homebrew/bin:/usr/local/bin"
# Xcode's existing Python override is passed as the first argument by project.yml.
namespace_python="${1:-}"
if [[ -n "${namespace_python}" ]]; then
  if [[ "${namespace_python}" != /* || ! -x "${namespace_python}" ]] || ! "${namespace_python}" -c 'import tomllib' >/dev/null 2>&1; then
    echo "error: the configured product namespace Python must be an absolute executable with tomllib" >&2
    exit 1
  fi
else
  for candidate in python3 python3.14 python3.13 python3.12 python3.11; do
    if command -v "${candidate}" >/dev/null && "${candidate}" -c 'import tomllib' >/dev/null 2>&1; then
      namespace_python="${candidate}"
      break
    fi
  done
fi
if [[ -z "${namespace_python}" ]]; then
  echo "error: product namespace export requires Python 3.11+" >&2
  exit 1
fi
"${namespace_python}" "${SRCROOT}/../../../scripts/lib/export_product_namespace.py" \
  --output "${TARGET_BUILD_DIR}/${UNLOCALIZED_RESOURCES_FOLDER_PATH}/product-namespace.json"
