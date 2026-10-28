#!/usr/bin/env bash
# Stage reviewed cross-platform fixtures into one explicitly selected debug app.
set -euo pipefail
: "${ANDROID_SERIAL:?Select the target emulator or test device with ANDROID_SERIAL}"
package="${1:?Pass the PlayDebug or DirectDebug package}"
case "$package" in
  com.lingxi.code.debug|com.lingxi.code.direct.debug) ;;
  *) echo "Refusing to stage fixtures in an unexpected package: $package" >&2; exit 1 ;;
esac
script_dir="$(cd "$(dirname "$0")" && pwd)"
fixtures="$script_dir/../../../../fixtures/local-apps/v2"
for file in static/lingxi-app.json python/server.py node/package-lock.json calorie-agent/lingxi-app.json agent/public-apple.jpg; do
  test -f "$fixtures/$file" || { echo "Missing fixture: $file" >&2; exit 1; }
done

# Never overwrite existing acceptance data on a reused device. A fresh CI
# emulator has no prior fixture directory.
adb -s "$ANDROID_SERIAL" shell run-as "$package" mkdir -p files
adb -s "$ANDROID_SERIAL" shell run-as "$package" mkdir files/local-app-acceptance-fixtures
tar -C "$fixtures" -cf - . | adb -s "$ANDROID_SERIAL" shell -T run-as "$package" \
  tar -xf - -C files/local-app-acceptance-fixtures
adb -s "$ANDROID_SERIAL" shell run-as "$package" cp \
  files/local-app-acceptance-fixtures/agent/public-apple.jpg files/local-app-acceptance-fixtures/test-photo.jpg
