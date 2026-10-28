#!/usr/bin/env bash
set -euo pipefail

: "${GITHUB_WORKSPACE:?}"
: "${RUNNER_TEMP:?}"
task_evidence="$RUNNER_TEMP/lingxi-desktop-test-evidence"
task_script="$GITHUB_WORKSPACE/scripts/ci/test_desktop_with_rtdb.sh"
task_verify="$GITHUB_WORKSPACE/scripts/ci/verify_desktop_test_log.py"

if [[ "${1:-}" == --run-tests ]]; then
  # The CLI starts the official RTDB emulator before invoking this branch and
  # supplies its address. The jar hash is from the pinned CLI's metadata.
  test "${FIREBASE_DATABASE_EMULATOR_HOST:-}" = '127.0.0.1:19261'
  printf '%s  %s\n' \
    b70d99344caf17c98b6f910fa8f6edf32a7c016cb1035e8915f70d38901eb97f \
    "$FIREBASE_EMULATORS_PATH/firebase-database-emulator-v4.11.2.jar" | sha256sum --check | tee "$task_evidence/emulator-sha256-check.txt"

  cd "$GITHUB_WORKSPACE/packages/bridge-client"
  npm test 2>&1 | tee "$task_evidence/sdk-tests.log"
  python3 "$task_verify" sdk "$task_evidence/sdk-tests.log"

  cd "$GITHUB_WORKSPACE/apps/electron"
  xvfb-run --auto-servernum npm test 2>&1 | tee "$task_evidence/electron-tests.log"
  python3 "$task_verify" electron "$task_evidence/electron-tests.log"
  exit 0
fi

if [[ $# -ne 0 ]]; then
  printf 'Unexpected argument: %s\n' "$1" >&2
  exit 2
fi

task_firebase="$RUNNER_TEMP/firebase-cli/node_modules/.bin/firebase"
task_bridge="$GITHUB_WORKSPACE/target/release/bridge-server"
task_terminal_prepare="$GITHUB_WORKSPACE/scripts/ci/prepare_terminal_test_broker.py"
task_terminal_tests="$GITHUB_WORKSPACE/scripts/tests/test_terminal_test_broker.py"
test -x "$task_firebase"
test -x "$task_bridge"
command -v xvfb-run >/dev/null
command -v vim >/dev/null
command -v rg >/dev/null

task_config="$(mktemp -d "$RUNNER_TEMP/lingxi-desktop-rtdb.XXXXXX")"
mkdir -p "$task_evidence" "$task_config/config" "$task_config/emulators"
python3 -B "$task_terminal_tests"
task_terminal_wrapper="$task_config/terminal-test-bridge"
python3 -B "$task_terminal_prepare" \
  --workspace "$GITHUB_WORKSPACE" --runner-temp "$RUNNER_TEMP" \
  --bridge "$task_bridge" --wrapper "$task_terminal_wrapper" --evidence "$task_evidence"
cp "$GITHUB_WORKSPACE/infra/firebase/database.rules.json" "$task_config/database.rules.json"
cat > "$task_config/firebase.json" <<'JSON'
{"database":{"rules":"database.rules.json"},"emulators":{"database":{"host":"127.0.0.1","port":19261},"hub":{"host":"127.0.0.1","port":19401},"ui":{"enabled":false},"singleProjectMode":true}}
JSON

export XDG_CONFIG_HOME="$task_config/config"
export FIREBASE_EMULATORS_PATH="$task_config/emulators"
export LINGXI_BRIDGE_SERVER_BIN="$task_bridge"
export LINGXI_LOCAL_APP_CORE_BIN="$task_bridge"
export LINGXI_TERMINAL_TEST_BIN="$task_terminal_wrapper"
task_bun="$(command -v bun)"
export LINGXI_MOD_BUN_EXECUTABLE="$task_bun"
export NO_COLOR=1

test "$("$task_firebase" --version)" = '15.34.0'
{
  date -u '+%Y-%m-%dT%H:%M:%SZ'
  git -C "$GITHUB_WORKSPACE" rev-parse HEAD
  node --version
  npm --version
  java -version 2>&1
  "$task_firebase" --version
  "$task_bun" --version
  df -h "$GITHUB_WORKSPACE"
} > "$task_evidence/provenance.txt"
if [[ "${GITHUB_EVENT_NAME:-}" == pull_request ]]; then
  python3 -c 'import json, sys; event = json.load(open(sys.argv[1], encoding="utf-8")); print("pull_request_head=" + event["pull_request"]["head"]["sha"])' \
    "$GITHUB_EVENT_PATH" >> "$task_evidence/provenance.txt"
fi
python3 "$GITHUB_WORKSPACE/scripts/lib/runtime_source.py" --json > "$task_evidence/runtime-source.json"
python3 -c 'import json, sys; assert not json.load(open(sys.argv[1], encoding="utf-8"))["development"], "Harness must come from the locked source"' \
  "$task_evidence/runtime-source.json"
sha256sum "$task_bridge" "$GITHUB_WORKSPACE/Cargo.toml" "$GITHUB_WORKSPACE/Cargo.lock" \
  "$task_terminal_prepare" "$task_terminal_tests" "$task_terminal_wrapper" \
  "$task_evidence/terminal-broker-provenance.json" \
  "$GITHUB_WORKSPACE/packages/bridge-client/package-lock.json" \
  "$GITHUB_WORKSPACE/apps/electron/package-lock.json" \
  "$GITHUB_WORKSPACE/infra/firebase/database.rules.json" "$task_config/database.rules.json" \
  > "$task_evidence/inputs.sha256"
git -C "$GITHUB_WORKSPACE" diff --binary | sha256sum > "$task_evidence/tracked-before.sha256"
git -C "$GITHUB_WORKSPACE" diff --cached --binary | sha256sum > "$task_evidence/index-before.sha256"
(cd "$GITHUB_WORKSPACE" && find packages/bridge-client/dist -type f -print0 | sort -z | xargs -0 -r sha256sum) \
  > "$task_evidence/sdk-dist-before.sha256"
test -s "$task_evidence/sdk-dist-before.sha256"

# Run in the temporary directory so firebase-debug.log is not placed in the
# checkout or uploaded: the CLI's debug log can contain process environment.
printf -v task_command 'bash %q --run-tests' "$task_script"
(
  cd "$task_config"
  "$task_firebase" emulators:exec --only database --project demo-lingxi-backend \
    --config "$task_config/firebase.json" "$task_command"
)
sha256sum --check "$task_evidence/inputs.sha256"
python3 -B "$task_terminal_prepare" --verify "$task_evidence/terminal-broker-provenance.json"
git -C "$GITHUB_WORKSPACE" diff --binary | sha256sum > "$task_evidence/tracked-after.sha256"
git -C "$GITHUB_WORKSPACE" diff --cached --binary | sha256sum > "$task_evidence/index-after.sha256"
(cd "$GITHUB_WORKSPACE" && find packages/bridge-client/dist -type f -print0 | sort -z | xargs -0 -r sha256sum) \
  > "$task_evidence/sdk-dist-after.sha256"
cmp "$task_evidence/tracked-before.sha256" "$task_evidence/tracked-after.sha256"
cmp "$task_evidence/index-before.sha256" "$task_evidence/index-after.sha256"
cmp "$task_evidence/sdk-dist-before.sha256" "$task_evidence/sdk-dist-after.sha256"
