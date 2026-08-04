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
- Local apps: Node `22.23.0-r0`, Git `2.47.3-r0`, Next `16.2.11`, and
  React/ReactDOM `19.2.8`

Android keeps one distribution dimension:

- `play` maps to **Store** and compiles out policy-sensitive high-risk offloads.
- `direct` maps to **Full** and compiles the complete permission-gated surface.

Both distributions can contain MobileLinux. A MobileLinux failure must be
reported explicitly and must never silently switch to the Legacy runtime.

## iOS (iSH ARM64)

iOS device builds use the pinned OpenMinis iSH ARM64 source and Alpine aarch64
fakefs rootfs. `clients/ios/scripts/build-linux-runtime.sh` reconstructs all
native archives and the rootfs locally; simulator builds never link iSH.
Full local-app release builds pass `--local-app-runtime --apk-dir <closure>`;
that path validates the exact Node/Git closure before doing the expensive iSH
build and verifies the resulting rootfs package database before staging.

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
clients/android/scripts/stage-mobile-linux-assets.sh --variant <play|direct> --input <evidence> --apk-dir <closure>
clients/android/scripts/verify-local-app-supply-chain.sh [--release --apk-dir <closure>]
clients/ios/scripts/verify-local-app-supply-chain.sh [--release --apk-dir <closure>]
clients/android/scripts/stage-local-app-runtime.sh --variant <play|direct> --node-modules <dir>
clients/ios/scripts/stage-local-app-runtime.sh --variant <store|full> --node-modules <dir>
```

The local-app release check is fail-closed. At present the pin manifest records
that Alpine's v3.21 aarch64 repository no longer serves the requested
`nodejs-22.23.0-r0` artifact. Structural verification succeeds only because the
missing digest is explicitly represented as unavailable; release verification
continues to fail until the exact signed artifact and full transitive APK
closure are supplied or the product pin is deliberately revised.

The staging scripts accept only a host-prepared `node_modules` tree containing
the exact Linux musl SWC bindings for ARM64 and x86_64. They reject package
manager binaries and escaping symlinks, emit a per-file SHA-256 manifest, and
stage immutable files for the runtime's read-only mount.

Native and rootfs outputs are generated under gitignored build/output
directories. Reference-tree `.so`, loader, and rootfs binaries are never copied
into a product artifact. `build-mobile-linux-native.sh` reconstructs PRoot,
its unbundled read-only loader, the PTY bridge, the UniFFI library, mksh, and
toybox from pinned source for both supported ABIs.

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
either ABI. Minting one requires a reproducible package-augmented rootfs, and
`local-app-runtime-pins.json` records `release_ready: false` with
`closure_status: "blocked"` for both ABIs — `arm64-v8a` because Alpine v3.21 no
longer serves the pinned `nodejs-22.23.0-r0`, `x86_64` because no signed
recursive APK closure has been captured. The pinned versions are a product
decision and must not be revised to route around this.

So the shipped rootfs bytes have **no repository-committed anchor at all**. The
only guarantee today is a refusal: `stage-mobile-linux-assets.sh` aborts on its
first ABI with `no committed release rootfs digest for arm64-v8a`. That is a
documented gap, not coverage. Two things must not be misread as closing it:

- `verify-mobile-linux-pins.sh` succeeding. It validates the source pins and
  never opens a rootfs archive.
- The `verify-release-archive` cases in `test-local-app-supply-chain.sh`. All
  but the last drive **synthetic** pin fixtures; they prove the comparator
  works, not that a real digest exists. The last block is a known-gap anchor
  that asserts the committed pins file still has no digest.

Whoever unblocks the APK closure must, in the same change: build the archive
with `package-rootfs-release.sh`; confirm the two-pass determinism check in
`test-rootfs-tooling.sh` reproduces the digest byte-for-byte; cross-check it
against `rootfs-manifest.json`'s `archive.sha256`; commit it under
`rootfs.release_archives.<abi>.sha256`; and convert the known-gap anchor at the
end of `test-local-app-supply-chain.sh` into a positive match assertion. That
anchor is written to fail the moment a digest appears, so this section cannot
silently go stale.

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
