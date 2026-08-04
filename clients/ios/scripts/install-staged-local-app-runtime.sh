#!/usr/bin/env bash
# Install a staged local-app runtime tree into an app bundle's resources.
#
# This lives in a script rather than inline in the Xcode build phase so the
# removal-and-copy is reachable from lingxi-code/scripts/mobile-linux/
# test-local-app-supply-chain.sh. The phase body it replaces was not runnable
# by any test, so the chmod below shipped unverified.
set -euo pipefail

STAGED=""
DESTINATION=""
while [[ $# -gt 0 ]]; do
  case "$1" in
    --staged) STAGED="${2:-}"; shift 2 ;;
    --destination) DESTINATION="${2:-}"; shift 2 ;;
    *) echo "unknown argument: $1" >&2; exit 2 ;;
  esac
done
if [[ -z "${STAGED}" || -z "${DESTINATION}" ]]; then
  echo "usage: $0 --staged <dir> --destination <dir>" >&2
  exit 2
fi
if [[ ! -d "${STAGED}" ]]; then
  echo "staged local-app runtime is missing: ${STAGED}" >&2
  exit 1
fi

# Staging locks the tree to 0555/0444, so rm cannot descend into a previous
# build's copy. Without the chmod, rm -rf fails and cp -R nests the new runtime
# inside the stale one, shipping build #1's node_modules.
chmod -R u+w "${DESTINATION}" 2>/dev/null || true
rm -rf "${DESTINATION}"
mkdir -p "$(dirname "${DESTINATION}")"
cp -R "${STAGED}" "${DESTINATION}"
