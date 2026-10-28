#!/usr/bin/env bash
set -euo pipefail
probe="$RUNNER_TEMP/provider-probe/app/build/outputs/apk"
for variant in play direct; do
  adb install -r "$probe/$variant/debug/app-$variant-debug.apk"
  adb install -r "$probe/androidTest/$variant/debug/app-$variant-debug-androidTest.apk"
done
for package in com.lingxi.code.debug com.lingxi.code.direct.debug; do
  echo "Running bounded provider smoke for $package"
  adb logcat -c
  log="$RUNNER_TEMP/provider-$package.log"
  status=0
  timeout --signal=TERM --kill-after=5s 90s adb shell -T am instrument -w -r -e class com.lingxi.code.project.ProviderSmokeTest "$package.test/androidx.test.runner.AndroidJUnitRunner" 2>&1 | tee "$log" || status=$?
  if (( status != 0 )); then
    echo "instrumentation failed or timed out: $status"
    adb logcat -d -v time -s LingxiTest:I AndroidRuntime:E System.err:W ActivityManager:W || true
    exit "$status"
  fi
  tr -d '\r' < "$log" | grep -Fxq 'OK (1 test)'
  if grep -q 'FAILURES!!!' "$log"; then exit 1; fi
done
