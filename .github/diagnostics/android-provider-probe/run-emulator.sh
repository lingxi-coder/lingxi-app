#!/usr/bin/env bash
set -euo pipefail
probe="$RUNNER_TEMP/provider-probe/app/build/outputs/apk"
for variant in play direct; do
  adb install -r "$probe/$variant/debug/app-$variant-debug.apk"
  adb install -r "$probe/androidTest/$variant/debug/app-$variant-debug-androidTest.apk"
done
for package in com.lingxi.code.debug com.lingxi.code.direct.debug; do
  out="$(adb shell -T am instrument -w -r -e class com.lingxi.code.project.ProviderSmokeTest "$package.test/androidx.test.runner.AndroidJUnitRunner" | tr -d '\r')"
  printf '%s\n' "$out"
  grep -Fxq 'OK (1 test)' <<< "$out"
  if grep -q 'FAILURES!!!' <<< "$out"; then exit 1; fi
done
