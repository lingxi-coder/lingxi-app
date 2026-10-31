# iOS Teams T3 feature harness

This target compiles **all eight actual native Teams Swift source files**, the
actual canonical deep-link parser, the actual `TeamBotTests` and `TeamsTests`,
and the product localization catalog. Sources are file references, not copies.
`Probe.swift` supplies only demo Firebase bootstrap and the containing app. It
links Firebase Auth/Firestore **12.19.2**, matching the product pin. No Rust/FFI,
MobileLinux, provider credentials, owner tools or production Google plist is
linked. This is a Teams feature harness, not the full signed LingXi application.

Preparation, compilation, XCTest, simulator UI and physical-device acceptance
are separate evidence. Generating the project is not compilation or acceptance.
This work prepared source and generated a project; it did not boot a simulator,
resolve/download Swift packages, build, install or run the app.

## API injection

The product model has an internal `init(apiRoot:)` with a private immutable URL.
Its default remains `https://lingxios.net/v1/teams/messages`. The product page has
an internal optional model argument; existing callers create the same default
model. There is no production environment, user-setting or deep-link override.
The trusted probe alone passes `http://127.0.0.1:<port>/v1/teams/messages`.
No URLProtocol interception or Foundation swizzling is used.

All actual Teams files compile, but this executable opens the actual
`TeamMessagesPage(uid:detail:onClose:model:)`, not `TeamsPage`; its membership
model is not instantiated and retains its unchanged production URL.

## Local fixture contract

`TEAM_TEST_BACKEND` and `TEAM_TEST_UID` are required. The bootstrap parser accepts
only an explicit `http://127.0.0.1:<port>` origin, with no credentials, path,
query or fragment. The probe makes:

```text
GET /_acceptance/bootstrap?uid=<URL-encoded synthetic UID>
-> {"customToken":"<demo emulator token>","detail":<actual TeamDetail JSON>}
```

The detail contains `team {id,name,memberCount,role,generation}`, `members
[{uid,displayName,role,generation}]`, and `invites []`. It must include the
requested UID at the same generation as `team.generation`. The probe signs in
through the real Firebase Auth SDK using that custom token and verifies the UID
before constructing the real messaging model.

The Firebase project is fixed to `demo-lingxi-teams-t0`. Auth uses loopback port
`TEAM_TEST_AUTH_PORT` (default `9098`); Firestore uses loopback port
`TEAM_TEST_FIRESTORE_PORT` (default `8085`) with SSL disabled and an in-memory SDK
cache. Auth/Firestore SDK traffic remains separate from HTTP message API traffic.
The app's ATS exception permits local networking only; arbitrary loads remain
disabled. Bootstrap redirects are rejected. Tokens and SDK session material are
never printed, captured in fixtures or stored in this project.

The HTTP fixture must implement the real `/v1/teams/messages` protocol, verify the
Firebase demo ID token, and seed a current usable Bot. For repeat fixture resets,
use a new team ID or a fresh membership generation; do not reset server cursors
under the same durable native projection namespace. The separate probe bundle
has its own app data and never opens full-product data.

## Prepare and inspect

From the product repository root:

```sh
python3 acceptance/teams-t3-complete-20261029/probes/ios/prepare.py
```

Only `output/teams-t3-ios` is generated; `output/` is ignored. The workspace
`Package.resolved` is staged from the existing T0 pin file, including Firebase
revision `8c29ca981990a32e626c8617544fa65d22b1834f` / version `12.19.2`.
`preparation-inventory.json` records source and lockfile hashes.

The previous expected cache at `output/teams-t0-ios/SourcePackages` was absent
at preparation time. Existing canonical product frameworks were inventoried but
are not used, so their FFI provenance does not establish or block this harness.
Do not mutate another task's SPM cache. Package resolution must be separately
serialized by the parent, using this probe's own output directory. If an already
validated cache is available, copy/materialize it into this owned directory first
and verify the locked package revisions before using it.

## Parent-serialized build and unit tests

The following commands are instructions, not recorded results. Do not run in
parallel with another native build. The simulator SDK Auth flow uses Keychain
persistence, so its runtime build uses ad-hoc signing and the probe-only
`Simulator.entitlements`. This does not require a signing certificate or device
provisioning. The Flare team remains `AZ4AX7J833`; its namespace is restricted to
`com.lingxi.teams.t3.probe`. No product entitlement or account is changed.

The project still defaults to `CODE_SIGNING_ALLOWED=NO` for compile-only probes.
For simulator Auth acceptance, the explicit flags below override that default:
`CODE_SIGNING_ALLOWED=YES CODE_SIGN_IDENTITY=- DEVELOPMENT_TEAM=AZ4AX7J833`.
Regenerate with `prepare.py` before the next parent build so the simulator-only
entitlement path is present in the generated project.

If this probe has no owned package cache, the parent first authorizes/performs
resolution (may require network):

```sh
xcodebuild -resolvePackageDependencies \
  -project output/teams-t3-ios/TeamsT3Probe.xcodeproj \
  -scheme TeamsT3Probe \
  -clonedSourcePackagesDirPath output/teams-t3-ios/SourcePackages \
  -onlyUsePackageVersionsFromResolvedFile
```

Set `TEAM_TEST_SIMULATOR_UDID` to the inspected iPhone 18 Pro / iOS 27 simulator
UDID. The parent handles boot and app installation/launch. Build all three targets
for testing, then run the actual native unit suites:

```sh
xcodebuild -project output/teams-t3-ios/TeamsT3Probe.xcodeproj \
  -scheme TeamsT3Probe -configuration Debug -sdk iphonesimulator \
  -destination "platform=iOS Simulator,id=$TEAM_TEST_SIMULATOR_UDID" \
  -derivedDataPath output/teams-t3-ios/DerivedData \
  -clonedSourcePackagesDirPath output/teams-t3-ios/SourcePackages \
  -skipPackageUpdates -disableAutomaticPackageResolution -jobs 2 \
  build-for-testing CODE_SIGNING_ALLOWED=YES CODE_SIGN_IDENTITY=- DEVELOPMENT_TEAM=AZ4AX7J833

xcodebuild -project output/teams-t3-ios/TeamsT3Probe.xcodeproj \
  -scheme TeamsT3Probe -configuration Debug \
  -destination "platform=iOS Simulator,id=$TEAM_TEST_SIMULATOR_UDID" \
  -derivedDataPath output/teams-t3-ios/DerivedData \
  -clonedSourcePackagesDirPath output/teams-t3-ios/SourcePackages \
  -skipPackageUpdates -disableAutomaticPackageResolution -parallel-testing-enabled NO \
  -only-testing:TeamsT3ProbeTests \
  -resultBundlePath output/teams-t3-ios/UnitTests.xcresult \
  test-without-building CODE_SIGNING_ALLOWED=YES CODE_SIGN_IDENTITY=- DEVELOPMENT_TEAM=AZ4AX7J833
```

Use a new result-bundle path for every run; retain earlier evidence. App bootstrap
is intentionally unavailable without the fixture env; pure unit tests still run.

## Parent-serialized UI acceptance

The UI suite runs the real SwiftUI page, real Firebase SDK sign-in/listeners and
real authenticated HTTP API calls. `TEAM_TEST_BOT_NAME` defaults to
`Acceptance Bot`; seed it as permitted to the chosen synthetic user. The Bot may
be offline: a new explicit group request must show queued status to the original
human without requiring an executing Desktop.

Pass launch values as **xcodebuild build settings**, so the scheme expands them
into the UI-test runner environment. The runner forwards only `TEAM_TEST_*` to
the app. For example, with the parent's local fixture on port `9100`:

```sh
xcodebuild -project output/teams-t3-ios/TeamsT3Probe.xcodeproj \
  -scheme TeamsT3Probe -configuration Debug \
  -destination "platform=iOS Simulator,id=$TEAM_TEST_SIMULATOR_UDID" \
  -derivedDataPath output/teams-t3-ios/DerivedData \
  -clonedSourcePackagesDirPath output/teams-t3-ios/SourcePackages \
  -skipPackageUpdates -disableAutomaticPackageResolution -parallel-testing-enabled NO \
  -only-testing:TeamsT3ProbeUITests \
  -resultBundlePath output/teams-t3-ios/MemberUI.xcresult \
  TEAM_TEST_BACKEND=http://127.0.0.1:9100 TEAM_TEST_UID=acceptance_member \
  TEAM_TEST_AUTH_PORT=9098 TEAM_TEST_FIRESTORE_PORT=8085 \
  'TEAM_TEST_BOT_NAME=Acceptance Bot' \
  -jobs 2 test CODE_SIGNING_ALLOWED=YES CODE_SIGN_IDENTITY=- DEVELOPMENT_TEAM=AZ4AX7J833
```

This UI invocation uses incremental `test` so launch build-setting values are
written into a fresh test descriptor, rather than relying on an older
`build-for-testing` descriptor that may contain empty launch values. It remains a
parent-serialized native command.

The prepared UI cases cover Bot card/private DM creation; group creation with Bot
participation; explicit target opt-in (initially off); sending and original-human
queued status; and seeded question routing. UI tests attach screenshots to the
result bundle. Actual simulator inspection established that conversation button
labels include an unread suffix (for example `iOS live Bot question, 3`), and a
SwiftUI Toggle exposes a labelled outer row plus its interactive nested thumb.
The tests use an anchored title/count selector and tap that thumb while checking
the outer value changes from 0 to 1. They dismiss the title keyboard with Return
and scroll in the List gutter to avoid text-cursor gestures. The scroll range is
bounded by the navigation bar and the observed keyboard frame, so a drag cannot
begin among keyboard keys. The exact system onboarding text about sliding across
letters gates its Continue action. After sending, the test scrolls upward to the
exact committed message before checking its status. In the r3 attempt, backend
evidence confirmed the group send committed; the screenshot showed the viewport
below the transcript and the keyboard onboarding sheet covering its lower area.
The corrected formal UI suite still requires a fresh parent run. A later
conversations-r4 attempt reached the exact human message, then scrolled upward
past the transcript to the Bot cards while looking for status. Backend evidence
confirmed one explicit trigger and three messages: the latest status belonged to
a later Bot question row below the human message. The final status search now
scrolls downward from that human message and retains the status assertion.

For question routing, additionally pass `TEAM_TEST_CONVERSATION_TITLE`,
`TEAM_TEST_QUESTION_ID`, `TEAM_TEST_QUESTION_TEXT` (the shared message body), and
`TEAM_TEST_EXPECT_QUESTION=visible` for the original human. Run the same seeded
thread with an owner/other member and `TEAM_TEST_EXPECT_QUESTION=hidden` to assert
no answer UI. The question test skips explicitly if these fixture names are
absent; a skip is not question acceptance. The fixture must provide an unexpired,
unanswered question with matching original-human generation, and must authorize
each tested user to read that conversation. The negative case first asserts the
shared message body loaded, so absence of the answer form alone is insufficient.
Actual R9 inspection showed SwiftUI propagates the parent message row identifier
to the question's children, overriding the nested form identifier. The test derives
the immutable `teams.message.bot_...` identifier from the exact question body,
then selects only that row's text fields/text views with placeholder `Your answer`
and its button labelled `Send answers`. Both positive and negative checks stay
scoped to the seeded question row; no global form lookup is used. The parent
validates the accepted answer against the backend question ID after the run.

For manual UI inspection after the parent installs/launches the app, use
`SIMCTL_CHILD_TEAM_TEST_BACKEND`, `SIMCTL_CHILD_TEAM_TEST_UID`, and matching
`SIMCTL_CHILD_TEAM_TEST_*` emulator ports with `simctl launch`. The bundle ID is
`com.lingxi.teams.t3.probe`. This is still feature-harness acceptance, not full
product, signing, physical-device, real-account or production-service acceptance.

## Keychain failure diagnostics

The first simulator UI attempt stopped at `FIRAuthErrorDomain / 17995`, before
Teams UI acceptance. In the verified Firebase checkout
`8c29ca981990a32e626c8617544fa65d22b1834f`, `AuthErrors.swift` defines 17995 as a
Keychain error. `AuthKeychainServices.swift` wraps Security-framework errors from
`SecItemCopyMatching`, `SecItemAdd`, `SecItemUpdate`, and `SecItemDelete`.
`AuthErrorUtils.swift` stores the original OSStatus only in a fixed failure-reason
format, not an underlying NSError. Therefore 17995 alone does **not** prove a
missing entitlement: a locked/unavailable Keychain can produce that code too.

The probe now emits only NSError domains/numeric codes, including any underlying
NSError chain. For this pinned Firebase error only, it extracts the numeric
OSStatus from the known fixed prefix without printing the failure reason,
localized description, token, userInfo dictionary, URL or account/session data.
For example, `NSOSStatusErrorDomain / -34018` means the Security framework's
`errSecMissingEntitlement`; `-25308` is a different condition and must not be
reported as a signing failure. The subsequent actual unsigned simulator run displayed exactly
`Demo bootstrap failed: FIRAuthErrorDomain / 17995 -> NSOSStatusErrorDomain / -34018`,
confirming the missing entitlement. After the parent rebuilt with ad-hoc signing
and the probe-only simulator entitlements, actual Firebase SDK sign-in reached
the ready UI for both `fixture-owner` and `ios-human`. This establishes demo SDK
bootstrap; complete formal Teams UI acceptance remains a separate gate.

For an iOS Simulator executable, `codesign -d --entitlements -` may legitimately
print `{}`: Xcode materializes the simulated capabilities in the main Mach-O's
`__TEXT,__entitlements` section using `*-Simulated.xcent`. The parent verified
those embedded entitlements and `codesign --verify --strict` on the actual app.
Do not mistake the empty code-signature entitlement dictionary for missing
simulated capabilities. Inspect the executable section as well:

```sh
codesign --verify --strict \
  output/teams-t3-ios/DerivedData/Build/Products/Debug-iphonesimulator/TeamsT3Probe.app
xcrun otool -s __TEXT __entitlements \
  output/teams-t3-ios/DerivedData/Build/Products/Debug-iphonesimulator/TeamsT3Probe.app/TeamsT3Probe
```

Decode the arm64 `otool` output by reversing bytes within each displayed
32-bit word, concatenating the words, and parsing the resulting plist. The
materialized `application-identifier` and `keychain-access-groups` must name
`AZ4AX7J833.com.lingxi.teams.t3.probe`; do not add product/shared Keychain groups.
Firebase persistence remains the real SDK implementation. No in-memory Auth
substitute or persistence bypass is permitted by this harness.
