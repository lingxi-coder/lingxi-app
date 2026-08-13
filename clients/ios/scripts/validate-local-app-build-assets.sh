#!/usr/bin/env bash
# Fail closed before an iPhone Full/release build can package a bare Alpine
# rootfs or an incomplete shared Vite runtime.
set -euo pipefail

CONFIGURATION=""
PLATFORM=""
STAGED=""
ROOTFS_MANIFEST=""
while [[ $# -gt 0 ]]; do
  case "$1" in
    --configuration) CONFIGURATION="${2:-}"; shift 2 ;;
    --platform) PLATFORM="${2:-}"; shift 2 ;;
    --staged) STAGED="${2:-}"; shift 2 ;;
    --rootfs-manifest) ROOTFS_MANIFEST="${2:-}"; shift 2 ;;
    *) echo "unknown argument: $1" >&2; exit 2 ;;
  esac
done

if [[ -z "${CONFIGURATION}" || -z "${PLATFORM}" || -z "${STAGED}" || -z "${ROOTFS_MANIFEST}" ]]; then
  echo "usage: $0 --configuration <name> --platform <name> --staged <dir> --rootfs-manifest <file>" >&2
  exit 2
fi

REQUIRED=0
if [[ "${CONFIGURATION}" == *Release ]]; then
  REQUIRED=1
elif [[ "${PLATFORM}" == "iphoneos" && "${CONFIGURATION}" == Full* ]]; then
  REQUIRED=1
fi
if [[ "${REQUIRED}" != "1" ]]; then
  exit 0
fi

for required in "${STAGED}/node_modules/vite/bin/vite.js"; do
  if [[ ! -f "${required}" ]]; then
    echo "error: ${CONFIGURATION} requires the staged local-app runtime: ${required}" >&2
    echo "       Set LINGXI_LOCAL_APP_NODE_MODULES and rebuild." >&2
    exit 1
  fi
done

if [[ "${PLATFORM}" != "iphoneos" ]]; then
  exit 0
fi
if [[ ! -f "${ROOTFS_MANIFEST}" ]] || \
   ! grep -Eq '"local_app_runtime"[[:space:]]*:[[:space:]]*true([[:space:],}]|$)' "${ROOTFS_MANIFEST}"; then
  echo "error: ${CONFIGURATION} requires an iOS rootfs with local_app_runtime=true." >&2
  echo "       Rebuild with LINGXI_LOCAL_APP_RUNTIME=1 clients/ios/scripts/build-xcframework.sh." >&2
  exit 1
fi
