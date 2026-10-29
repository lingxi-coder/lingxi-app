# GStreamer CI SDK contract

The GStreamer version and upstream SHA-256 digests used by CI live in
[`gstreamer-sdk-lock.json`](gstreamer-sdk-lock.json). The archive installer
downloads only from `gstreamer.freedesktop.org`, verifies the committed digest
before extraction or installation, and stores its cache under `RUNNER_TEMP`.
Updating a version requires refreshing every platform asset and its checksum
sidecar from the upstream download indexes, then checking the Android ABI slices,
iOS XCFramework slices, desktop package layout, and required plugin set.

CI uses the official Android universal archive, the iOS XCFramework, both macOS
universal installers, and the Windows MSVC x86_64 development installer. The
pinned Windows installer reports its package revision as `1.28.8.1`; CI checks
that exact four-part version as well as the installer's SHA-256 digest. The
lint job uses Ubuntu 26.04's archive packages for the GStreamer 1.28 API
required by Rust's `v1_28` feature. The all-features unit-test job builds the
same pinned Linux SDK on Ubuntu 22.04 with `--static-plugins` because that
feature set includes static plugin registration. This mode uses Meson's
`--default-library=both`, checks every required plugin archive and pkg-config
file, and exports both the core and plugin pkg-config directories. It removes
static core archives so the plugin probe links the same shared GStreamer core
as the Rust crates, and isolates dynamic plugin discovery to prevent duplicate
registration. The static SDK has a separate cache identity and cache path from
the shared-only package SDK.

The Desktop Linux package job stays on Ubuntu 22.04 to preserve its existing
glibc runtime baseline. It builds
GStreamer 1.28.8 from the pinned official monorepo tarball on that runner,
using Jammy's system GLib and the pinned Meson release, then stages only the
required GStreamer libraries, their non-glibc shared dependencies, nine media
plugins, and upstream license notices into a temporary SDK prefix before
packaging. The verified prefix is cached under a key derived from the source
lock, builder, SDK validator, and pinned Meson requirement; the output directory
also embeds a content identity and is revalidated on cache hits.

The Linux source build follows upstream's [Meson monorepo build flow](https://gstreamer.freedesktop.org/documentation/installing/building-from-source-using-meson.html).
Its release-tag archive includes the GStreamer modules in `subprojects/`; the
separate ORC tarball is pinned in the same lock and extracted before configuring
Meson with network downloads disabled.

The installer exports the existing build contracts:

- `LINGXI_GSTREAMER_SDK_ANDROID` points to the upstream universal root and is
  checked for both `arm64` and `x86_64` slices.
- `LINGXI_GSTREAMER_SDK_IOS` points directly to `GStreamer.xcframework`.
- `LINGXI_GSTREAMER_SDK_MACOS` points to
  `/Library/Frameworks/GStreamer.framework/Versions/1.0`.
- `LINGXI_GSTREAMER_SDK_WINDOWS` points to the MSVC SDK root; `PKG_CONFIG`,
  `PKG_CONFIG_PATH`, `PKG_CONFIG_LIBDIR`, and the plugin search paths point
  into that SDK.
- `LINGXI_GSTREAMER_SDK_LINUX_SOURCE` points to the verified Linux source-build
  prefix used for pkg-config and runtime dependency discovery.
- `LINGXI_GSTREAMER_SDK_LINUX` points to the temporary staged package SDK.

Do not make media builds optional in these jobs to avoid SDK setup. The offline
fixture suite is `python3 scripts/tests/test_gstreamer_sdk.py`.
