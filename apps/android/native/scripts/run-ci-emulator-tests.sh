#!/usr/bin/env bash
# Run both connected suites and the separately orchestrated acceptance checks.
set -euo pipefail
rootfs="${1:?Pass the verified rootfs evidence directory}"
: "${RUNNER_TEMP:?RUNNER_TEMP is required}"
gradle=(./gradlew --max-workers=1 -Pkotlin.daemon.jvmargs=-Xmx4g "-PlingxiRootfsEvidenceDir=$rootfs")
"${gradle[@]}" :app:installPlayDebug :app:installPlayDebugAndroidTest \
  :app:installDirectDebug :app:installDirectDebugAndroidTest

ANDROID_SERIAL="$(adb get-serialno)"
export ANDROID_SERIAL
test -n "$ANDROID_SERIAL" && test "$ANDROID_SERIAL" != unknown
for package in com.lingxi.code.debug com.lingxi.code.direct.debug; do
  bash scripts/stage-ci-android-fixtures.sh "$package"
done

run_runtime_phase() {
  local package="$1" test_class="$2" flag="$3" marker="$4" output
  output="$(adb -s "$ANDROID_SERIAL" shell -T am instrument -w -r \
    -e class "$test_class" -e "$flag" true "$package.test/androidx.test.runner.AndroidJUnitRunner" | tr -d '\r')"
  printf '%s\n' "$output"
  # An assumed/skipped test can report OK; the test emits this marker only
  # after its assertions and cleanup complete.
  grep -Fxq "INSTRUMENTATION_RESULT: $marker" <<< "$output"
  grep -Fxq 'OK (1 test)' <<< "$output"
}

for package in com.lingxi.code.debug com.lingxi.code.direct.debug; do
  run_runtime_phase "$package" com.lingxi.code.runtime.MobileLinuxProductAcceptanceTest \
    mobileLinuxProductAcceptance mobileLinuxProductResult=READY
  adb -s "$ANDROID_SERIAL" shell am force-stop "$package"
  run_runtime_phase "$package" com.lingxi.code.runtime.MobileLinuxProductColdStartAcceptanceTest \
    mobileLinuxColdStartAcceptance mobileLinuxColdStartResult=VERIFIED
done

for package in com.lingxi.code.debug com.lingxi.code.direct.debug; do
  output="$(adb -s "$ANDROID_SERIAL" shell -T am instrument -w -r \
    -e class com.lingxi.code.EngineRoundtripTest "$package.test/androidx.test.runner.AndroidJUnitRunner" | tr -d '\r')"
  printf '%s\n' "$output"
  grep -Fxq 'INSTRUMENTATION_RESULT: keylessTerminalResult=PASS' <<< "$output"
  grep -Fxq 'INSTRUMENTATION_RESULT: keylessModelListResult=PASS' <<< "$output"
  grep -Fxq 'OK (2 tests)' <<< "$output"
done

# This runner kills only its recorded Direct test host PID and validates a
# new-process recovery; am force-stop would hide orphan-process regressions.
python3 scripts/test-local-app-owner-death.py --serial "$ANDROID_SERIAL" \
  --output-dir "$RUNNER_TEMP/android-owner-death"

printf '%s\n' 'External market, provider, MCP, interactive and diagnostic cases require explicit opt-in; see scripts/README-ci-acceptance.md.'
"${gradle[@]}" :app:connectedPlayDebugAndroidTest :app:connectedDirectDebugAndroidTest
