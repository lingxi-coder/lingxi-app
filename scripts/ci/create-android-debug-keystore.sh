#!/usr/bin/env bash
set -euo pipefail

if [[ $# -ne 1 ]]; then
  echo "usage: $0 <debug-keystore-path>" >&2
  exit 2
fi

keystore="$1"
if [[ -e "$keystore" || -L "$keystore" ]]; then
  echo "refusing to replace existing debug keystore: $keystore" >&2
  exit 1
fi

# This disposable key is used only by fresh CI jobs. Its credentials are the
# Android debug defaults; it is not the local or release signing identity.
umask 077
mkdir -p "$(dirname "$keystore")"
keytool -genkeypair -keystore "$keystore" -storetype JKS \
  -alias androiddebugkey -keyalg RSA -keysize 2048 -validity 10000 \
  -storepass android -keypass android -dname 'CN=Android Debug,O=Android,C=US' \
  -noprompt
