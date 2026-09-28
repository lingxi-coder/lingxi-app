# Mobile runtime source boundary

LingXi consumes two immutable Cargo dependencies. `runtime_source.py` resolves
Harness, which owns Local Apps templates, runtime profiles, permissions and
product execution policy. `mobile_linux_source.py` resolves the independent
mobile Linux SDK, which owns the neutral runtime, rootfs/toolchain builders,
Android native support and iOS iSH support. Both resolvers inspect locked Cargo
metadata; sibling paths and guessed Cargo cache directories are unsupported.

## Native integration

LingXi builds its own `libandroid_aar.so` and `LingxiCodeFFI.xcframework`. They
contain its single Rust runtime and preserve the product UniFFI namespaces.
The full SDK FFI library must never be linked into the same application.

Android consumes `mobile-linux-installer` and `mobile-linux-native-support`
through the local Maven repository at `clients/android/build/mobileLinuxSdk/maven`.
The build wrapper asks the fixed SDK to publish these artifacts there, together
with POM metadata and `sdk-artifacts.json`. The native-support AAR owns all six
PRoot/PTY/legacy shell helper binaries; host `jniLibs` owns only the product FFI.
The host gate verifies source revision, clean provenance, native hashes and
final APK bytes. `useLegacyPackaging=true` remains required so helpers are
extracted into `nativeLibraryDir` for execution. Play and Direct remain separate
product distributions and both use the same pinned SDK helpers.

iOS links `MobileLinuxNativeSupport.xcframework`, containing Swift/Objective-C/C
support and no Rust core. Device support is arm64; simulator slices report
unavailable. Product workspace preferences, bundled resource selection and old
call-site compatibility live in `Sources/RuntimeIntegration`. The old
`Sources/LinuxRuntimeNative` implementation is excluded from compilation.
Native framework builds and rootfs conversion are SDK tools; all generated
outputs and caches remain under the host's `clients/ios/build`.

`mobile-linux-native-pins.json` records the SDK interface and Maven version,
not duplicate native source hashes. SDK Android/iOS source pins are independent;
Android builds do not require an iOS source checkout. `local-app-native-policy.json`
keeps product network and memory policy. Its host gate checks integration and
Local Apps resource wiring, while the SDK verifies its own source and artifacts.

## Resources and provenance

Rootfs and node-module host wrappers pass explicit output/cache directories and
the locked SDK root to Harness's profile adapters. Harness selects the product
profile; the SDK receives explicit inputs and never reads Harness/LingXi files.
Source checkout directories are read-only inputs. The SDK receives local rootfs
archives; host build/download or bundling policy controls how those files arrive.

The iOS staging layout stays `build/linux-runtime/openminis` for product
compatibility, but contains SDK-generated resources and provenance, not a
source dependency on an OpenMinis checkout. Rootfs/patch resources are copied
from that staging directory. Reusing staged output verifies its SDK revision,
framework bytes and rootfs digest against the current Cargo dependency.

A passing source or fixture test is not native-device acceptance. Final release
validation separately requires the pinned rootfs archive/APK closure, complete
artifact digests, Android execution from the installed APK and iOS device
execution. The existing missing release archive evidence must not be replaced
with invented hashes or silently bypassed.
