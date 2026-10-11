# MobileLinux release contract

LingXi's Android or iOS combined distribution is GPLv3 when it includes the
OpenMinis-derived shell, PRoot, PTY bridge, or Alpine rootfs integration.
Original MIT and Apache-2.0 components retain their notices.

Native sources and toolchains belong to the Cargo-locked mobile Linux SDK. See
[RUNTIME-SOURCE-CONTRACT.md](RUNTIME-SOURCE-CONTRACT.md) for its resolver,
artifact ownership and validation. The inherited component baseline includes:

- OpenMinis `9cf3a855fecd27bb5735b84cacbd56852a3ab8dd`
- OpenMinis PRoot fork `8cf13e997cdc9472997aae19df8050c073c9a86c`
- talloc 2.4.2
- Base MobileLinux rootfs: Alpine 3.21.3 for `arm64-v8a` and `x86_64`

Android keeps one distribution dimension:

- `play` maps to **Store** and compiles out policy-sensitive high-risk offloads.
- `direct` maps to **Full** and compiles the complete permission-gated surface.

Both Android distributions use the PRoot MobileLinux runtime for Agent shell
commands and terminal sessions. A missing rootfs or unavailable runtime is
reported explicitly; there is no Android host-shell fallback.

## iOS (iSH ARM64)

iOS device builds use the pinned OpenMinis iSH ARM64 source and Alpine aarch64
fakefs rootfs. `apps/ios/native/scripts/build-linux-runtime.sh` reconstructs all
native-support framework and rootfs through the locked SDK; simulator slices
compile the same API and report unavailable without linking the iSH kernel.
The runtime is in-process Linux userspace emulation, not a Linux kernel or a
security boundary. Isolation remains the iOS app sandbox plus explicit fakefs
bind mounts. Release artifacts must include iSH's GPLv3 text, `LICENSE.IOS`,
the Alpine package notices/SBOM, and corresponding source for the exact pins.

Build and verification entrypoints:

```text
apps/android/native/scripts/verify-mobile-linux-pins.sh
apps/android/native/scripts/build-mobile-linux-native.sh --variant play
apps/android/native/scripts/build-mobile-linux-native.sh --variant direct
apps/android/native/scripts/verify-mobile-linux-native.sh --variant <play|direct>
apps/android/native/scripts/stage-mobile-linux-assets.sh --variant <play|direct> --input <evidence> --apk-dir <closure>
```

Native and rootfs outputs are generated under gitignored build/output
directories. Reference-tree `.so`, loader, and rootfs binaries are never copied
into a product artifact. `build-mobile-linux-native.sh` reconstructs PRoot,
its unbundled loader and network-policy launcher from pinned SDK source, then
builds the product UniFFI library for both supported ABIs.

A release evidence directory contains, per ABI:

```text
<abi>/<rootfs archive named by rootfs-manifest.json>
<abi>/rootfs-manifest.json
<abi>/rootfs-build.lock.json
<abi>/rootfs.spdx.json
<abi>/executable-allowlist.json
```

The official minirootfs digest in `rootfs.archives` remains the immutable
source pin. The shipped archive is the deterministic, package-augmented
rootfs, so it needs a committed digest of its own in
`rootfs.release_archives.<abi>.sha256`; staging refuses any ABI that has no
such pin rather than trust the hash the evidence directory issues for itself.

### KNOWN UNANCHORED STEP: no release archive digest is committed

`mobile-linux-pins.json` carries **no `rootfs.release_archives` key**, for
either ABI. Minting one requires a reproducible package-augmented rootfs: the
generic MobileLinux release archive still needs its reproducible packaging run
and reviewed repository digest. These pins must not be invented from a
single local build or revised merely to route around the remaining gate.

So the shipped rootfs bytes have **no repository-committed anchor at all**. The
only guarantee today is a refusal: `stage-mobile-linux-assets.sh` aborts on its
first ABI with `no committed release rootfs digest for arm64-v8a`. That is a
documented gap, not coverage. Two things must not be misread as closing it:

- `verify-mobile-linux-pins.sh` succeeding. It validates the source pins and
  never opens a rootfs archive.
- Synthetic `verify-release-archive` fixtures. They prove the comparator
  works, not that a real digest exists.

Whoever unblocks the APK closure must, in the same change: build the archive
with `package-rootfs-release.sh`; confirm the two-pass determinism check in
`test-rootfs-tooling.sh` reproduces the digest byte-for-byte; cross-check it
against `rootfs-manifest.json`'s `archive.sha256`; commit it under
`rootfs.release_archives.<abi>.sha256`.

Once a pin exists, staging verifies the archive bytes against it, against its
own manifest and lock, and against the archive policy before copying assets
under `app/build/generated/mobileLinux/<play|direct>/assets`. Gradle stores
`.gz` and `.zst` assets without recompression, preserving the release hash.

Release deliverables must also include:

- `LICENSES/NOTICE.md` and the exact GPL/LGPL texts;
- `sbom/mobile-linux.spdx.json` plus the generated per-rootfs SBOM;
- complete corresponding source or a durable corresponding-source offer;
- all Android/Gradle dependency notices and the Alpine package-license list.

The former authorization documents remain as historical migration records.
The distribution choice is now GPLv3; they are no longer a substitute for the
source, notice, and SBOM release gates described above.
