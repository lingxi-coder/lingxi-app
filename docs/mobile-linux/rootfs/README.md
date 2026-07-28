# Alpine 3.24.1 mobile rootfs pipeline

This directory defines the license-clean packaging contract for the future
mobile Linux rootfs. It does not ship or embed any GPL runtime code; it only
describes how a reviewed Alpine payload must be produced and verified once the
required written authorization exists.

Pinned distribution baseline:

- Alpine release: `3.24.1`
- Repository branch: `v3.24`
- Supported targets:
  - `android-proot` / `android` / `arm64`
  - `android-proot` / `android` / `x86_64`
  - `ios-ish` / `ios` / `arm64`

Fixed primary package set:

- `busybox`
- `git`
- `openssh-client`
- `python3`
- `ca-certificates`

Policy decisions:

- `apk` must be disabled in the shipped rootfs. No `apk` binary, no `pip`,
  `pip3`, `npm`, or `npx`, and no writable repository configuration may remain.
- BusyBox applets must use hardlinks, not symlinks. At minimum `/bin/sh` must
  be a hardlink to `/bin/busybox`.
- The packaged archive format is `tar.zst`.
- The rootfs manifest allowlist must hash every shipped ELF executable,
  interpreter, and shared library that remains in the release archive.
- The release evidence set must include:
  - `rootfs-manifest.json`
    - schema v2 includes a complete immutable regular-file/symlink inventory;
      writable roots are excluded and verified separately
  - `rootfs-build.lock.json`
  - `rootfs.spdx.json`
  - `executable-allowlist.json`

Recommended build flow:

1. Build or assemble an extracted Alpine 3.24.1 rootfs in a temporary workdir
   using only official Alpine `v3.24` repositories.
2. Remove/disable package-manager binaries and repository config from the final
   rootfs.
3. Materialize BusyBox applets as hardlinks instead of symlinks.
4. Run:
   `lingxi-code/scripts/mobile-linux/package-rootfs-release.sh`
5. Commit only reviewed evidence under `docs/mobile-linux/rootfs/current/` and
   `docs/mobile-linux/sbom/current/`.
6. Enable `LINGXI_MOBILE_LINUX_ENABLED=1` only after the authorization gate and
   CI policy gates pass.

Schema v1 manifests are intentionally rejected: they lack the immutable file
inventory required to detect non-ELF runtime tampering.

Tooling:

- `lingxi-code/scripts/mobile-linux/rootfs_tool.py`
  - validate extracted trees
  - validate tar archives
  - generate rootfs manifests
  - generate SPDX SBOMs
  - generate release lock files
  - snapshot executable allowlists
- `lingxi-code/scripts/mobile-linux/package-rootfs-release.sh`
  - high-level wrapper for packaging a reviewed extracted rootfs into release
    evidence
- `lingxi-code/scripts/mobile-linux/test-rootfs-tooling.sh`
  - positive/negative tests for the packaging and verification helpers
