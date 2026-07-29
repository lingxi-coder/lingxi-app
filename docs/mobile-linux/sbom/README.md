# SBOM and license evidence contract

When `mobile-linux` is enabled, CI must have reproducible evidence for the shipped rootfs and runtime packaging.

Required outputs for each release candidate:

- SPDX or CycloneDX SBOM for the packaged rootfs contents
- package inventory covering BusyBox, Git, OpenSSH client, Python 3, standard library, and CA certificates
- executable allowlist snapshot aligned with `rootfs-manifest.json`
- license inventory for all shipped runtime components
- corresponding-source pins from `docs/mobile-linux/mobile-linux-pins.json`
- the exact GPL/LGPL texts and `docs/mobile-linux/LICENSES/NOTICE.md`

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
  `openminis`, `proot`, `talloc`, and `alpine-rootfs`.
- `executable-allowlist.json` has `schema_version: 1` and an `entries[]` array
  byte-for-field equivalent to the rootfs manifest executable allowlist.

An Android release must fail closed when any of these artifacts is missing or
does not match the staged native/rootfs bytes.
