# Mobile Linux migration guardrails

This directory exists to keep the OpenMinis-style mobile Linux migration in a legally and store-compliant state before any GPL-derived runtime code is introduced into product builds.

Current status on July 28, 2026:

- `mobile-linux` product builds must stay disabled until written redistribution/linking authorization is present for every GPL-governed runtime component that would ship in the app.
- No GPL runtime source may be copied from `docs/superpowers/references/OpenMinis/` into product paths while authorization is missing.
- The guardrail scripts under `lingxi-code/scripts/mobile-linux/` are fail-closed when `LINGXI_MOBILE_LINUX_ENABLED=1`.
- The license-clean phase-1 implementation is present: shared runtime contracts, Android/iOS bridge configuration, unavailable/blocked runtime behavior, rootfs manifest verification and atomic activation helpers, mobile settings UI, and CI policy checks.
- Android remains on the legacy Minijail-backed execution path. iOS remains on its unavailable shell path. Neither product target links PRoot/iSH or embeds an Alpine archive in the current authorization state.

After written authorization is approved, activation still requires a reviewed
change that:

1. pins the authorization-manifest digest at build time;
2. bundles the approved runtime source/binaries and reproducible rootfs assets;
3. implements the prepared Android PRoot and iOS iSH runtime traits;
4. passes the command, PTY, rootfs, process-tree, network-denial, device, and
   staged-rollout tests from the migration plan.

Possessing an authorization file alone never enables execution: the current
bridges intentionally report `Unsupported` after a valid grant is detected
until a separately reviewed native runtime is actually linked.

An enabled release must provide its generated manifest at
`rootfs/current/rootfs-manifest.json`. The sample manifest is never accepted by
the enabled CI path.

Directory layout:

- `authorization/` — required grant-file contract and placeholders
- `rootfs/` — rootfs manifest schema and sample
- `sbom/` — SBOM and license evidence expectations

Recommended CI entrypoints:

- `lingxi-code/scripts/mobile-linux/check-authorizations.sh`
- `lingxi-code/scripts/mobile-linux/check-store-compliance.sh`
- `lingxi-code/scripts/mobile-linux/check-rootfs-manifest.sh`
- `lingxi-code/scripts/mobile-linux/check-sbom-and-licenses.sh`
- `lingxi-code/scripts/mobile-linux/package-rootfs-release.sh`
- `lingxi-code/scripts/mobile-linux/test-rootfs-tooling.sh`
- `lingxi-code/scripts/mobile-linux/smoke.sh`
