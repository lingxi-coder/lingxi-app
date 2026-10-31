# Reproduce the Teams T0 native SDK size probes

Run from the product repository root. These are minimal dependency probes, not
product installers. The baseline includes the product's existing Firebase Auth,
Messaging, Installations and Crashlytics products; the second build adds Firestore.
Android imports the product version catalog. iOS pins the same Firebase 12.19.2.

Keep at least 2 GiB free. Build platforms serially. These commands never install
or launch a device application and do not regenerate Rust/FFI libraries.

## Android

```sh
JAVA_HOME=/opt/homebrew/opt/openjdk@21/libexec/openjdk.jdk/Contents/Home \
ANDROID_HOME="$HOME/Library/Android/sdk" \
./apps/android/native/gradlew -p acceptance/teams-t0-20261029/probes/android \
  assembleBaselineDebug assembleFirestoreDebug --no-daemon --max-workers=2 \
  --console=plain -Dorg.gradle.jvmargs=-Xmx1536m
```

## iOS

```sh
mkdir -p output/teams-t0-ios
xcodegen generate --spec acceptance/teams-t0-20261029/probes/ios/project.yml \
  --project output/teams-t0-ios
mkdir -p output/teams-t0-ios/TeamsFirestoreProbe.xcodeproj/project.xcworkspace/xcshareddata/swiftpm
cp acceptance/teams-t0-20261029/probes/ios/Package.resolved \
  output/teams-t0-ios/TeamsFirestoreProbe.xcodeproj/project.xcworkspace/xcshareddata/swiftpm/Package.resolved
xcodebuild -project output/teams-t0-ios/TeamsFirestoreProbe.xcodeproj \
  -scheme Baseline -configuration Release -sdk iphoneos \
  -destination 'generic/platform=iOS' -derivedDataPath output/teams-t0-ios/DerivedData \
  -clonedSourcePackagesDirPath output/teams-t0-ios/SourcePackages -skipPackageUpdates \
  -disableAutomaticPackageResolution build CODE_SIGNING_ALLOWED=NO
xcodebuild -project output/teams-t0-ios/TeamsFirestoreProbe.xcodeproj \
  -scheme WithFirestore -configuration Release -sdk iphoneos \
  -destination 'generic/platform=iOS' -derivedDataPath output/teams-t0-ios/DerivedData \
  -clonedSourcePackagesDirPath output/teams-t0-ios/SourcePackages -skipPackageUpdates \
  -disableAutomaticPackageResolution build CODE_SIGNING_ALLOWED=NO
python3 acceptance/teams-t0-20261029/probes/measure.py
```

The recorded run used an APFS clone of already-cached source packages inside this
task's output directory, retaining the original cache unchanged. A fresh machine
must first resolve/download the pinned packages. Source/build outputs remain
ignored; `sizes.json` records logical bundle bytes, APK bytes and executable hashes.
