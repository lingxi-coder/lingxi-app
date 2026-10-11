# LingXi accounts / Firebase Authentication

Project: `lingxi-d6627` (display name `lingxi`, number `535003056752`).
LingXi accounts are independent of model-provider credentials and engine `auth_state`.
Account controls live in each platform's Account settings page. Existing local work
does not require signing in.

## Platform configuration

| Client | Firebase application |
| --- | --- |
| Desktop | Web app `1:535003056752:web:8e620fcba1416aff75c2b5` |
| iOS Store | `com.lingxi.code` |
| iOS Full | `com.lingxi.code.full` |
| Android Play release / debug | `com.lingxi.code` / `com.lingxi.code.debug` |
| Android Direct release / debug | `com.lingxi.code.direct` / `com.lingxi.code.direct.debug` |

The committed SDK configurations are public client identifiers, not service-account
credentials. Never place an Admin SDK private key or Apple `.p8` file in a client.

- Desktop: `apps/electron/src/shared/firebaseConfig.json`.
- Android: `apps/android/native/app/google-services.json` contains all four Android clients;
  the Google services Gradle plugin selects the matching application ID.
- iOS: `apps/ios/native/Resources/Firebase/<bundle ID>/GoogleService-Info.plist`.
  The XcodeGen build step copies exactly the selected application's file into the
  app bundle; `FIREBASE_REVERSED_CLIENT_ID` sets its Google callback scheme.
  Regenerate the project using `xcodegen generate` from `apps/ios/native`.

iOS pins Firebase 12.19.2 and GoogleSignIn 9.2.0. The Firebase 13 release notes
explicitly state that a compatible Google Sign-In Swift Package release is not
yet available. Upgrade this pair together after checking package resolution:
https://firebase.google.com/support/release-notes/ios

## Authentication flows

All platforms implement email/password registration and login, password reset,
email verification, profile refresh, sign-out, Google login, and Apple login.

Desktop uses Firebase Auth REST in the main process. Only profile/status data
crosses the account IPC channel; ID/refresh tokens never reach the renderer or
the model-provider configuration. Refresh tokens are stored with Electron
`safeStorage`, with no plaintext fallback; if an OS keystore is unavailable, the
session is memory-only. Tokens refresh on demand; terminal authentication errors
clear the session during both token refresh and authenticated account requests.
Transient network failures retain the recoverable session.

Desktop social login opens the system browser on a short-lived loopback server.
The browser loads the installed Firebase SDK, uses memory-only persistence and
Firebase's popup flow, then returns the provider credential through an
origin-checked, random-path POST callback. The main process exchanges it with
Firebase, preserving the SDK's opaque `pendingToken` when provided. Closing the
provider popup notifies the app through an origin-checked cancellation callback.
Callbacks expire after three minutes and close on cancellation.
`localhost` and `127.0.0.1` are registered Firebase authorized domains. The main
renderer retains its `connect-src 'none'` policy.

Android uses FirebaseAuth, Credential Manager for Google, and Firebase's browser
OAuth provider for Apple. It reattaches pending OAuth results after recreation.
iOS uses FirebaseAuth, GoogleSignIn, and AuthenticationServices for Apple with a
cryptographic nonce, SHA-256 request hash, and checked request state. Native SDKs
own session persistence and refresh.

## Firebase provider setup

Email/password and Google have been enabled in this project. `firebase.json`
declares their configuration; deploy just these settings with:

```sh
firebase deploy --only auth --project lingxi-d6627
```

The local Android debug keystore's SHA-1 and SHA-256 were registered for both
debug package names. Production Google login requires each distribution's actual
release signing certificate, including the Google Play App Signing certificate
when applicable. Add those public fingerprints in Firebase and download updated
Android config file. Do not reuse debug fingerprints as release credentials.

Apple **is not enabled on the server yet**, at the user's direction. The code is
present, but live Apple sign-in needs the following configuration:

1. Enable Sign in with Apple for both iOS App IDs on team `AZ4AX7J833` and refresh
   their provisioning profiles. `Account.entitlements` declares this capability
   for the containing app, separately from the widget.
2. Create an Apple Services ID for web/Android sign-in, associate it with the
   primary Apple App ID, and register `lingxi-d6627.firebaseapp.com` and the return
   URL `https://lingxi-d6627.firebaseapp.com/__/auth/handler`.
3. Create a Sign in with Apple key. In Firebase Authentication's Apple provider,
   configure Services ID, Team ID, Key ID, and private key, then enable it. Store
   the key only in the provider configuration / approved secret storage.
4. Configure Apple's private email relay for Firebase's sender if users should
   receive account emails at Apple relay addresses.

Provider documentation:
- https://firebase.google.com/docs/auth/android/google-signin
- https://firebase.google.com/docs/auth/android/apple
- https://firebase.google.com/docs/auth/ios/google-signin
- https://firebase.google.com/docs/auth/ios/apple

## Validation

Desktop automated tests are in `apps/electron/test/account.test.ts` and
`apps/electron/test/account-host.test.ts`. They cover
IPC input validation, profile-only responses, persistence, refresh coalescing,
revocation, sign-out versus in-flight refresh, project isolation, callback origin
validation, provider-popup cancellation, Apple pending-token handoff, and cached
ID-token rejection. Loopback tests need permission to listen
on local ephemeral ports.

The live Firebase smoke tests use disposable `example.com` accounts, never send
verification/reset emails, and delete the accounts when finished. Apple remains
code-only until the provider credentials are supplied. Google acceptance in this
run stops at the loaded account-entry page and cancellation, as requested; it
does not claim a completed Google account grant.

Recorded integration checks:
- Desktop: TypeScript checks and production build passed; 10 account/IPC tests and
  26 packaging tests passed. A follow-up regression run passed 13 account/IPC tests
  and all 26 packaging tests. Real Firebase SDK dependency packaging also passed.
- Android: full `assemblePlayDebug` and `assembleDirectDebug` passed, including
  final APK native-library and rootfs verification. Kotlin/Compose are 2.4.21
  because Firebase Auth 25.0.0 uses Kotlin 2.4 metadata.
- iOS: full StoreDebug and FullDebug simulator application builds passed against
  the actual FirebaseAuth and GoogleSignIn packages.
- Fixed Android/iOS audio adapters for the new local-app owner, preserving both
  app ID and runtime generation. Android's 10 coordinator tests and the iOS
  local-app-generation ownership regression passed.
- iOS normal-launch inspection found a synchronous Rust/MCP constructor exhausting
  a Swift cooperative worker's stack. Engine construction now uses a short-lived
  native thread with an explicit 8 MiB stack while the shared bootstrap task
  suspends; asynchronous follow-up requests retain their existing path. Four
  bootstrap regression tests passed (thread/stack boundary, coalescing, retry,
  initial session catalog), and normal Store app launch remained alive with the
  onboarding UI visible after the fix.
- Restored the Cargo-pinned MobileLinux native support artifacts from their SDK
  AAR and restaged rootfs assets against the SDK's release evidence. Manifest and
  archive digest checks remain enabled; no build gates were bypassed.
- A pre-existing Desktop host test expects audio schema v3 but the implementation
  returns v4; the same assertion fails with the unmodified HEAD host controller.

Native Auth verification passed on the Android API 37 Play-enabled emulator and
an iPhone 17 Pro / iOS 26.3 simulator: registration, wrong-password rejection,
login, logout, process-restored sessions, and the loaded Google account-entry
page followed by cancellation. The Firebase user audit after cleanup was empty.
Android also normalizes a restored empty display name to use the email fallback.

Native live tests are opt-in:
- Android: `FirebaseAccountUiTest` exercises the production account composable
  and ViewModel inside the full application/test APK. Instrumentation arguments
  are `firebaseLive=true`, `phase=register|restore|google`, `testEmail`, and
  `testPassword`. Run register then restore in separate processes; restore asserts
  the persisted SDK user before performing any new sign-in. The registration
  phase also checks logout, incorrect-password rejection, and successful login.
  The restore phase deletes the temporary user. Always provide external cleanup
  if a run is interrupted before that phase.
- iOS: `FirebaseAccountUITests` navigates the full app's Account settings. Set test
  action environment variables `LINGXI_FIREBASE_LIVE_TESTS=1` and
  `LINGXI_FIREBASE_API_KEY` to the public SDK API key. Conversation data is mocked
  by the existing UI-test launch mode; Firebase networking, native SDKs, account
  UI, Keychain restoration, and Google authorization remain real. The test uses
  synthetic-password paste to avoid the simulator's strong-password keystroke
  interception and declines the system password-save prompt.

These checks cover debug/emulator builds, not signed device distribution. Apple
provider configuration and production signing fingerprints remain required before
the corresponding production social-login flows can work.

## Build outputs

- Play debug APK: `apps/android/native/app/build/outputs/apk/play/debug/app-play-debug.apk`.
- Direct debug APK: `apps/android/native/app/build/outputs/apk/direct/debug/app-direct-debug.apk`.
- iOS simulator apps: Xcode DerivedData, `Build/Products/StoreDebug-iphonesimulator/LingxiCode.app`
  and `Build/Products/FullDebug-iphonesimulator/LingxiCode.app`.
- This validation did not create an App Store archive / signed device IPA.
