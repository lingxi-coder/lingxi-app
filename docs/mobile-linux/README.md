# MobileLinux release contract

LingXi's Android or iOS combined distribution is GPLv3 when it includes the
OpenMinis-derived shell, PRoot, PTY bridge, or Alpine rootfs integration.
Original MIT and Apache-2.0 components retain their notices.

The source baseline is immutable and machine-readable in
`mobile-linux-pins.json`:

- OpenMinis `9cf3a855fecd27bb5735b84cacbd56852a3ab8dd`
- OpenMinis PRoot fork `8cf13e997cdc9472997aae19df8050c073c9a86c`
- talloc 2.4.2
- Alpine 3.21.3 for `arm64-v8a` and `x86_64`

Android keeps one distribution dimension:

- `play` maps to **Store** and compiles out policy-sensitive high-risk offloads.
- `direct` maps to **Full** and compiles the complete permission-gated surface.

Both distributions can contain MobileLinux. A MobileLinux failure must be
reported explicitly and must never silently switch to the Legacy runtime.

## iOS (iSH ARM64)

iOS device builds use the pinned OpenMinis iSH ARM64 source and Alpine aarch64
fakefs rootfs. `clients/ios/scripts/build-linux-runtime.sh` reconstructs all
native archives and the rootfs locally; simulator builds never link iSH.

The runtime is in-process Linux userspace emulation, not a Linux kernel or a
security boundary. Isolation remains the iOS app sandbox plus explicit fakefs
bind mounts. Release artifacts must include iSH's GPLv3 text, `LICENSE.IOS`,
the Alpine package notices/SBOM, and corresponding source for the exact pins.

Build and verification entrypoints:

```text
clients/android/scripts/verify-mobile-linux-pins.sh
clients/android/scripts/build-mobile-linux-native.sh --variant play
clients/android/scripts/build-mobile-linux-native.sh --variant direct
clients/android/scripts/verify-mobile-linux-native.sh --variant <play|direct>
clients/android/scripts/stage-mobile-linux-assets.sh --variant <play|direct> --input <evidence>
```

Native and rootfs outputs are generated under gitignored build/output
directories. Reference-tree `.so`, loader, and rootfs binaries are never copied
into a product artifact. `build-mobile-linux-native.sh` reconstructs PRoot,
its unbundled read-only loader, the PTY bridge, the UniFFI library, mksh, and
toybox from pinned source for both supported ABIs.

A release evidence directory contains, per ABI:

```text
<abi>/alpine-minirootfs-3.21.3-<alpine-arch>.tar.gz
<abi>/rootfs-manifest.json
<abi>/rootfs.spdx.json
```

Staging verifies the official archive digest and manifest digest before
copying assets under `app/build/generated/mobileLinux/<play|direct>/assets`.
Gradle stores `.gz` and `.zst` assets without recompression, preserving the
manifest hash.

Release deliverables must also include:

- `LICENSES/NOTICE.md` and the exact GPL/LGPL texts;
- `sbom/mobile-linux.spdx.json` plus the generated per-rootfs SBOM;
- complete corresponding source or a durable corresponding-source offer;
- all Android/Gradle dependency notices and the Alpine package-license list.

The former authorization documents remain as historical migration records.
The distribution choice is now GPLv3; they are no longer a substitute for the
source, notice, and SBOM release gates described above.
