# Reproduce T1 native source checks

Run at the repository root with at least 2 GiB free. These probes compile the
actual Team page/model and canonical invite adapter from the product, rather than
copies. They do not replace full application, signing or physical-device gates.

## Android

The probe stages current product sources and Team string resources at build time.
It imports the product version catalog/Firebase BoM and includes the new Teams
tests plus the existing Android deep-link/inbox suites.

```sh
JAVA_HOME=/opt/homebrew/opt/openjdk@21/libexec/openjdk.jdk/Contents/Home \
ANDROID_HOME="$HOME/Library/Android/sdk" \
./apps/android/native/gradlew -p acceptance/teams-t1-20261029/probes/android \
  assembleDebug testDebugUnitTest --no-daemon --max-workers=1 \
  --console=plain -Dorg.gradle.jvmargs=-Xmx1024m
```

## iOS

This reuses the T0 worktree-owned SDK checkout/cache. The pinned package resolution
is `acceptance/teams-t0-20261029/probes/ios/Package.resolved`. No foreign build
directory or source checkout is modified.

```sh
mkdir -p output/teams-t1-ios
xcodegen generate --spec acceptance/teams-t1-20261029/probes/ios/project.yml \
  --project output/teams-t1-ios
mkdir -p output/teams-t1-ios/TeamsMembersProbe.xcodeproj/project.xcworkspace/xcshareddata/swiftpm
cp acceptance/teams-t0-20261029/probes/ios/Package.resolved \
  output/teams-t1-ios/TeamsMembersProbe.xcodeproj/project.xcworkspace/xcshareddata/swiftpm/Package.resolved
xcodebuild -project output/teams-t1-ios/TeamsMembersProbe.xcodeproj \
  -scheme TeamsMembersProbe -configuration Release -sdk iphoneos \
  -destination 'generic/platform=iOS' -derivedDataPath output/teams-t0-ios/DerivedData \
  -clonedSourcePackagesDirPath output/teams-t0-ios/SourcePackages -skipPackageUpdates \
  -disableAutomaticPackageResolution -jobs 2 build CODE_SIGNING_ALLOWED=NO
xcrun swiftc -module-cache-path /private/tmp/lingxi-teams-t1-swift-cache \
  apps/ios/native/Sources/Teams/TeamInvitation.swift \
  apps/ios/native/Sources/App/CanonicalDeepLink.swift \
  acceptance/teams-t1-20261029/probes/ios/check-invitations.swift \
  -o output/teams-t1-ios/check-invitations
./output/teams-t1-ios/check-invitations
```

Full Android product compilation in this worktree currently lacks the pinned
MobileLinux Maven artifacts and sherpa AAR. Full iOS product frameworks, signing
and on-device validation remain a separate gate. None of these probes installs
an application, changes an existing device account or regenerates Rust/FFI.
