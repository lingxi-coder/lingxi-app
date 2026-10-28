# Android instrumentation and acceptance

The `clients` emulator job builds and installs both PlayDebug and DirectDebug,
stages the checked-in `fixtures/local-apps/v2` for each app, and runs both
unfiltered `connected*DebugAndroidTest` suites. It separately runs the two
keyless `EngineRoundtripTest` methods, the Mobile Linux product and cold-start
phases for each flavor, and the Direct owner-death runner. The targeted runs
require success markers as well as a JUnit success count: an assumed or skipped
test is not accepted as a pass. The ordinary suites still run the keyless tests.

The Python/Node Local App execution, failure-matrix and rootfs-repair cases
require the Direct flavor; Play reports those individual tests as skipped and
the same connected run executes them on Direct. The ordinary connected suite
also includes `GitToolTest`, which needs working network access to its public
Git clone host and `self-signed.badssl.com`; its negative TLS case requires a
certificate rejection and does not treat a network failure as success.

`stage-ci-android-fixtures.sh <package>` takes `ANDROID_SERIAL` and accepts
only `com.lingxi.code.debug` or `com.lingxi.code.direct.debug`. It copies the
reviewed fixtures and the separately named `test-photo.jpg`. It deliberately
fails if the destination already exists, instead of overwriting a prior device
acceptance directory. The CI uses a fresh emulator. Prepare a dedicated test
installation before invoking it on a device; never clear existing app data
merely to make the script succeed.

The following cases are skipped in ordinary connected runs because the runner
cannot create their independent live service, model, user interaction or
diagnostic context. Each remains an explicitly selectable instrumentation
test. Set only the indicated flag for a selected method; an enabled test fails
if a required argument or fixture is missing. Explicit flag values other than
`true` or `false` fail instead of silently skipping. A command that explicitly
selects an acceptance class or method, or supplies its prior fixture or phase
arguments, fails with the required flag if the flag is missing. An unrelated
class or method selection does not opt in other tests.

| Test | Flag and additional prerequisites |
| --- | --- |
| `UnifiedLocalAppsAcceptanceTest#marketInstallUpdateRemixReviewAndTakedown` | `localAppMarketAcceptance=true`, Direct, an isolated HTTPS market at `localAppMarketRoot`, and its `market-ca.pem` in the fixture directory. The server must provide the `__test` state, advance, executable-update and takedown endpoints. |
| `LocalAppAgentAcceptanceTest#externalBrowserApprovedSkillRunsInBackgroundAndRecoversThroughNativeWakeLink` | `localAppAgentSkillFixture=true`, synthetic `fixtureProviderUrl` and `fixtureProviderKey`, a configured host default model, staged fixtures, and a real installed system browser. |
| `LocalAppAgentAcceptanceTest#exactMcpGrantRequiresNativeApprovalAndRevokeFencesTask` | `localAppAgentMcpFixture=true`, `fixtureNonce` from the test server receipt, `adb reverse tcp:18867 tcp:18867`, a configured provider and the MCP acceptance fixture. |
| Agent browser file/chat, native question and gateway tests | `localAppAgentLive=true`, staged fixtures, a configured suitable provider/model and, for browser cases, a real installed system browser. |
| `LocalAppAgentAcceptanceTest#stageCalorieAppForInteractiveAcceptance` | `localAppAgentInteractive=true`, staged fixtures and a configured model; this prepares an app for manual interaction. |
| `LocalAppAgentAcceptanceTest#cleanupInterruptedSkillFixtureProvider` | `localAppAgentCleanup=true` and the exact `fixtureCleanupProviderId`; the test additionally restricts cleanup to the synthetic fixture namespace. |
| `LocalAppExitDiagnosticsTest#exportOwnAnr` | `localAppExitDiagnostics=true`; exports only the target package's own ANR trace if one exists. |

The pre-existing live Firebase authentication, push, Crashlytics and
authenticated Git tests retain their own explicit test-account or credential
guards; neither connected suite should be reported as live-service acceptance.

For example, after preparing an isolated Direct test installation and the
fixture service, select the market case explicitly:

```sh
adb -s "$ANDROID_SERIAL" shell am instrument -w -r \
  -e class 'com.lingxi.code.apps.UnifiedLocalAppsAcceptanceTest#marketInstallUpdateRemixReviewAndTakedown' \
  -e localAppMarketAcceptance true -e localAppMarketRoot "$MARKET_ROOT" \
  com.lingxi.code.direct.debug.test/androidx.test.runner.AndroidJUnitRunner
```

The isolated skill fixture uses the same form with its method name,
`-e localAppAgentSkillFixture true`, `-e fixtureProviderUrl "$FIXTURE_URL"`
and `-e fixtureProviderKey "$FIXTURE_KEY"`. The MCP case uses
`-e localAppAgentMcpFixture true -e fixtureNonce "$FIXTURE_NONCE"`. The CI does
not provision these external services and does not claim they passed.

The SIGKILL acceptance is orchestrated with
`python3 scripts/test-local-app-owner-death.py --serial "$ANDROID_SERIAL" --output-dir <evidence-dir>`
after installing the matching Direct APKs, staging the fixtures and preparing
the runtime. Its runner passes `ownerDeathAcceptance=true` and the correct
`ownerDeathPhase` for both phases. Use its `--recover-only` mode for an existing
receipt; do not force-stop the app during this test, because that would hide
orphaned child processes.

The product runtime phase and cold phase can also be selected separately with
`mobileLinuxProductAcceptance=true` and `mobileLinuxColdStartAcceptance=true`.
Run the product phase first, then force-stop that test package and run cold in
a new process. The CI runner implements this order for both flavors. A report
from fixture staging or a mocked shell check is not a device acceptance result.
