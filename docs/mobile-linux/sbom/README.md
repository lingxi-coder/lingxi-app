# SBOM and license evidence contract

When `mobile-linux` is enabled, CI must have reproducible evidence for the shipped rootfs and runtime packaging.

Required outputs for each release candidate:

- SPDX or CycloneDX SBOM for the packaged rootfs contents
- package inventory covering BusyBox, Git, OpenSSH client, Python 3, standard library, and CA certificates
- executable allowlist snapshot aligned with `rootfs-manifest.json`
- license inventory for all shipped runtime components
- written authorization files referenced by `docs/mobile-linux/authorization/AUTHORIZATION_MANIFEST.json`

Suggested artifact layout once real release assets exist:

- `docs/mobile-linux/rootfs/current/rootfs-build.lock.json`
- `docs/mobile-linux/sbom/current/rootfs.spdx.json`
- `docs/mobile-linux/sbom/current/licenses.json`
- `docs/mobile-linux/sbom/current/executable-allowlist.json`

Evidence shape enforced by CI:

- `rootfs.spdx.json` is an SPDX 2.x document whose `packages[]` covers every
  package in the release rootfs manifest.
- `licenses.json` has `schema_version: 1`, `status: "approved"`, and
  `components[]` entries with non-empty `id` and `license` values for at least
  `proot`, `ish`, and `alpine-rootfs`.
- `executable-allowlist.json` has `schema_version: 1` and an `entries[]` array
  byte-for-field equivalent to the rootfs manifest executable allowlist.

Until those artifacts exist, `LINGXI_MOBILE_LINUX_ENABLED=1` must fail in CI.
