#!/usr/bin/env bash
set -euo pipefail
report_failure() {
  local result=$?
  trap - EXIT
  if (( result != 0 )); then
    echo "Provider diagnostic failed (exit $result); collecting existing logcat categories."
    adb logcat -d -v time -s LingxiTest:I AndroidRuntime:E System.err:W ActivityManager:W || true
  fi
  exit "$result"
}
trap report_failure EXIT
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
    exit "$status"
  fi
  if grep -q 'FAILURES!!!' "$log"; then
    echo "instrumentation reported failures: $package"
    exit 1
  fi
  if ! tr -d '\r' < "$log" | grep -Fx 'OK (1 test)' > /dev/null; then
    echo "instrumentation did not report the expected test success: $package"
    exit 1
  fi
done
