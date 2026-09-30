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
through the local Maven repository at `apps/android/native/build/mobileLinuxSdk/maven`.
The build wrapper asks the fixed SDK to publish these artifacts there, together
with POM metadata and `sdk-artifacts.json`. The native-support AAR owns three
PRoot helpers per ABI (PRoot, loader and network-policy launcher); host `jniLibs`
owns only the product FFI. The previous JNI PTY, mksh, toybox and Minijail
dependencies are absent from the Android build.
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
outputs and caches remain under the host's `apps/ios/native/build`.

`mobile-linux-native-pins.json` records the SDK interface and Maven version,
not duplicate native source hashes. SDK Android/iOS source pins are independent;
Android builds do not require an iOS source checkout. `local-app-native-policy.json`
keeps product network and memory policy. Its host gate checks integration and
Local Apps resource wiring, while the SDK verifies its own source and artifacts.

## Resources and provenance

Rootfs and node-module host wrappers pass explicit output/cache directories and
the locked SDK root to Harness's profile adapters. Harness selects the product
profile; the SDK receives explicit inputs and never reads Harness/LingXi files.
LingXi owns the host release policy. Its product policy smoke composes host
authorization, store compliance and integration checks with the pinned Harness
authorization, SBOM/license and supply-chain gates and the SDK resource gates.
Both upstream roots are resolved from the product's canonical Cargo pins before
being passed to the gates. SDK resource contracts live at
`scripts/checks/check-resource-contracts.sh`; rootfs tooling tests live at
`scripts/rootfs/test-rootfs-tooling.sh`.

The supply-chain wrapper passes the locked SDK root explicitly, including on
macOS Bash 3.2. `test_local_app_policy.py` proves that host policy preserves gate
ordering and failure propagation, requires release evidence and APK inputs, and
rejects writable-root expansion, removal of host-managed helpers and disabled
forbidden-feature gates in every locked runtime profile. Enabled releases still
require authorization, real rootfs archive/evidence, approved licenses and APK
validation.

The structural Android/iOS store gates load identities through
`scripts/lib/local_app_branding.py`, rooted at the gate's own Host checkout,
independently of the scanned `--repo-root`. Android's reviewed package and
constant paths use `namespace`/`applicationId` from
`apps/android/native/app/build.gradle.kts`; they must agree. iOS task IDs use
only the main application target's `settings.base.PRODUCT_BUNDLE_IDENTIFIER`
in `apps/ios/native/project.yml`. The helper reads the existing literal,
indented XcodeGen/Gradle definitions and fails on missing, ambiguous or
unsupported forms. Manifest/plist values and flavor, widget or test bundle IDs
do not define the expected identity. Reviewed task suffixes, service classes,
subtypes, declaration scope and registration/expiration/completion/audio
cleanup checks remain enforced.

The accessibility service name uses `branding::PRODUCT_NAME`; runtime profile
policy paths use `branding::DOT_DIR`, read from the branding package located by
the canonical locked Cargo resolver. Smoke tests obtain the enabled env key
from the Host authorization script and the APK env key from the Host smoke
script. Fixtures copy the canonical native configs and invoke the real Host
gates against their separate mutable source trees, including coordinated
config/manifest/plist/source identity attacks.

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
