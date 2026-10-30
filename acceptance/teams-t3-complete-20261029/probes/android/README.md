# Android T3 feature acceptance harness

This separate demo APK compiles **all eight actual production Teams Kotlin files**
and their actual Compose screens, plus the product Team unit tests. It uses the
product version catalog, Firebase BoM, Auth and Firestore Android SDKs. It does
not include the unrelated product JNI/engine/MobileLinux/rootfs/voice modules.
Successful execution is feature-harness acceptance, not a full signed product,
physical-device, release, production-account or real-model result.

`prepare.py` copies only existing `teams_*` strings from the product English XML
into generated `output/teams-t3-android/resources/values/strings.xml`, verifies
both production model constructor defaults remain `https://lingxios.net`, and
records source/resource/catalog/harness hashes in `provenance.json`. Kotlin
sources are compiled directly, without copies or patches. Build products and
project caches belong in `output/teams-t3-android`, not the acceptance source.
The unchanged `TeamInvitationTest` also compiles the exact canonical production
`DeepLink.kt`, `DeepLinkInbox.kt` and `PushRoute.kt` into the JVM test variant.
These direct file references and hashes retain invite/inbox/push coverage without
adding unrelated product navigation, push services or native dependencies.

The three small product DI changes preserve default construction: private final
model API roots default to HTTPS and the messages screen defaults to its existing
ViewModel. Only this separate probe's trusted Activity factory supplies a local
root. The production app has no Intent, environment or deep-link endpoint switch.
The probe APK alone allows cleartext traffic. No global URL/TLS interception is
used.

## Local fixture contract

- Installed AVD: `Pixel_10_Pro` (parent controls boot/serial scheduling).
- Project: `demo-lingxi-teams-t0`; Auth `10.0.2.2:9098`; Firestore `10.0.2.2:8085`.
- Router: `http://10.0.2.2:18783` by default. Auth bootstrap is
  `GET /_acceptance/bootstrap?uid=android-human` returning
  `{customToken, detail: {team,members,invites}}`.
- The router serves the real authenticated `/v1/teams` and `/v1/teams/messages`
  routes. Tokens are obtained through real SDK `signInWithCustomToken`, not fake
  HTTP user headers. Firestore runs with its real emulator rules.
- Seed at least one enabled/permitted Bot for the fixture human, a conversation
  they can read, and a **fresh running original-human question** with enough
  remaining lease for the UI journey. Start/seed after APK compilation so the
  five-minute run lease has not expired during compilation. The test discovers
  this question using `list/sync/botState`; it fails instead of skipping if absent.
- Test identity and router port are overridable only in the probe through
  `fixture_uid` and `fixture_port`. All real Team model HTTP calls use the injected
  local root. The probe's Firebase SDK options always name the demo project.

## Serialized commands (do not run alongside another heavy build)

Requirements verified during preparation: cached Gradle **9.6.1**, AGP **9.4.1**,
Kotlin **2.4.21**, Firebase BoM **35.0.2**, Compose BoM **2026.09.00**, UI test
**1.12.1**, Android test runner **1.7.0**, installed SDK `platforms;android-37.0`.
Use Java 21 at `/opt/homebrew/opt/openjdk@21/libexec/openjdk.jdk/Contents/Home`.
Caches are under `/Users/luolingfeng/.gradle/caches/modules-2/files-2.1` and the
Gradle wrapper distribution is already under
`/Users/luolingfeng/.gradle/wrapper/dists/gradle-9.6.1-bin`.

From the product repository root, prepare only:

```sh
python3 acceptance/teams-t3-complete-20261029/probes/android/prepare.py
```

After the parent reserves the build slot:

```sh
JAVA_HOME=/opt/homebrew/opt/openjdk@21/libexec/openjdk.jdk/Contents/Home \
ANDROID_HOME=/Users/luolingfeng/Library/Android/sdk \
./apps/android/native/gradlew \
  -p acceptance/teams-t3-complete-20261029/probes/android \
  --project-cache-dir "$PWD/output/teams-t3-android/gradle-project-cache" \
  assembleDebug assembleDebugAndroidTest testDebugUnitTest \
  --offline --no-daemon --max-workers=1 --console=plain
```

After the parent starts the dedicated fixture/emulators and selects the AVD:

```sh
JAVA_HOME=/opt/homebrew/opt/openjdk@21/libexec/openjdk.jdk/Contents/Home \
ANDROID_HOME=/Users/luolingfeng/Library/Android/sdk \
./apps/android/native/gradlew \
  -p acceptance/teams-t3-complete-20261029/probes/android \
  --project-cache-dir "$PWD/output/teams-t3-android/gradle-project-cache" \
  connectedDebugAndroidTest --offline --no-daemon --max-workers=1 --console=plain \
  -Pandroid.testInstrumentationRunnerArguments.fixture_uid=android-human \
  -Pandroid.testInstrumentationRunnerArguments.fixture_port=18783
```

Alternatively install the two generated debug APKs to the parent's selected
`adb -s <serial>` device and execute:

```sh
adb -s <serial> shell am instrument -w \
  -e fixture_uid android-human -e fixture_port 18783 \
  com.lingxi.teamst3.probe.test/androidx.test.runner.AndroidJUnitRunner
adb -s <serial> pull \
  /sdcard/Android/data/com.lingxi.teamst3.probe/files/acceptance \
  output/teams-t3-android/device-evidence
```

`ProbeActivity` also supports manual launch extras `fixture_uid`, `fixture_port`,
`conversation_id`, and `page=home`; all are exclusive to the demo probe APK.

The instrumentation journey verifies SDK custom-token sign-in, a rules-authorized
Firestore server membership read, actual Bot cards, an original-human question
answer, Bot DM creation, group Bot participation, plain-text `@mention` remaining
untargeted, and an explicitly targeted message. It saves three screenshots and
`result.json` without tokens in its own external-files directory. The other
human/owner, offline/restart, revocation, lease expiry, RC and runtime-containment
acceptance cases remain part of the broader parent matrix; this one journey does
not establish them.

Preparation has not booted, installed, built or run an emulator. Record fresh
Gradle/installation/UI results before claiming these gates passed. Missing cached
artifacts are a blocker to report, not a reason to change product versions or
fetch unrelated builds.
