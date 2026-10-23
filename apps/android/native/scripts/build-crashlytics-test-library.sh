#!/bin/bash
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
SDK="${ANDROID_SDK_ROOT:-${ANDROID_HOME:-$HOME/Library/Android/sdk}}"
NDK="${ANDROID_NDK_HOME:-$SDK/ndk/29.0.14206865}"
CLANG="$NDK/toolchains/llvm/prebuilt/darwin-x86_64/bin/aarch64-linux-android29-clang"
SYMBOLS="$ROOT/app/build/crashlytics-verification/arm64-v8a"
ASSETS="$ROOT/app/src/androidTest/assets/crashlytics/arm64-v8a"
test -x "$CLANG"
mkdir -p "$SYMBOLS" "$ASSETS"
"$CLANG" -shared -fPIC -g -O0 -fno-omit-frame-pointer \
  -Wl,--build-id=sha1 -Wl,-z,max-page-size=16384 \
  "$ROOT/app/src/androidTest/native/crashlytics_verification.c" \
  -o "$SYMBOLS/libcrashlytics_verification.so"
cp "$SYMBOLS/libcrashlytics_verification.so" "$ASSETS/libcrashlytics_verification.so"
echo "Test-only ARM64 asset and matching unstripped symbols prepared. No upload performed."
