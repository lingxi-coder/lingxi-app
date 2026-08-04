# Alpine 3.21.3 Android rootfs pipeline

This directory defines the reproducible evidence contract for the Android
MobileLinux rootfs. Release archives come from the official Alpine mirrors and
must match `docs/mobile-linux/mobile-linux-pins.json`.

Pinned distribution baseline:

- Alpine release: `3.21.3`
- Repository branch: `v3.21`
- Supported targets:
  - `android-proot` / `android` / `arm64`
  - `android-proot` / `android` / `x86_64`

Fixed primary package set:

- `apk-tools`
- `busybox`
- `git`
- `nodejs`
- `openssh-client`
- `python3`
- `ca-certificates`

Local-app runtime pins:

- `nodejs 22.23.0-r0`
- `git 2.47.3-r0`
- `npm`, `npx`, `corepack`, `yarn`, and `pnpm` are excluded from the image.
- `node_modules` is built off-device from the committed lockfile and mounted
  read-only; neither generated code nor MCP jobs may install dependencies.

`docs/mobile-linux/local-app-runtime-pins.json` records the exact APK and npm
pins. The structural verifier accepts an explicitly recorded upstream gap, but
the `--release` gate fails until both ABIs have a complete hashed APK closure.
This distinction prevents development checks from inventing a digest while
ensuring a release can never ship an unverified substitute.

`docs/mobile-linux/local-app-runtime-policy.json` is the executable contract:
the host invokes Next through `/usr/bin/node` directly, mounts the committed
`node_modules` bundle read-only, binds production servers to loopback, and
enforces the build/start timeout and 800 MiB process-tree limit. npm scripts are
developer conveniences only and are never part of the device execution path.

Policy decisions:

- The interactive, user-opened terminal retains `apk` and networking so the
  user can explicitly install packages.
- Agent-initiated networking, package installation, and external-mount writes
  remain controlled by the host permission gate; PRoot is not a security
  boundary.
- The source archive format is Alpine's official `tar.gz`; its digest is a
  source pin. The deterministic package-augmented release archive has a
  separate manifest digest and Gradle stores it without recompression.
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

1. Download both official Alpine 3.21.3 minirootfs archives named in the pin
   manifest and verify SHA-256 before extraction or modification.
2. Produce the complete package/license inventory and immutable content digest.
3. Generate `rootfs-manifest.json` and `rootfs.spdx.json` for each ABI.
4. Stage only through `clients/android/scripts/stage-mobile-linux-assets.sh`.
5. Publish the archives, evidence, and corresponding source together.

Schema v1 manifests are intentionally rejected because they lack the immutable
file inventory required to detect runtime tampering.

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
- `clients/android/scripts/verify-local-app-supply-chain.sh`
- `clients/ios/scripts/verify-local-app-supply-chain.sh`
  - validate the shared Node/Next/Git pins, template lockfile, source policy,
    and SPDX inventory
