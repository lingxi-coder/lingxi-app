#!/usr/bin/env bash
# Create an ephemeral key for CI APKs; this is never a release credential.
set -euo pipefail

: "${RUNNER_TEMP:?RUNNER_TEMP is required}"
: "${GITHUB_ENV:?GITHUB_ENV is required}"
umask 077
signing_dir="$(mktemp -d "${RUNNER_TEMP%/}/lingxi-android-debug-signing.XXXXXX")"
keystore="$signing_dir/debug.keystore"
keytool -genkeypair -noprompt \
  -keystore "$keystore" -storetype PKCS12 -storepass android \
  -alias androiddebugkey -keypass android -keyalg RSA -keysize 2048 \
  -validity 10000 -dname 'CN=Android Debug,O=Android,C=US'
printf 'ORG_GRADLE_PROJECT_lingxiDebugKeystore=%s\n' "$keystore" >> "$GITHUB_ENV"
