# Source review: url 2.5.7 -> 2.5.8

Reviewer: Codex (GPT-6.1 Terra; AI-assisted source review). Independent cross-review: Codex AI-assisted.
Criterion: `safe-to-deploy`; scope: delta.
Frozen batch SHA-256: `aa0f8805f0e48d23f3eeae37a1e4097c9bdd761e30801099b4d1c3eded471b43`; frozen original Cargo.lock SHA-256: `2a60d89a9fb754475a33d8034227743a8bb5ff2e7410b064f5c122800360385d`.
This review applies to the frozen versions only; the final dependency graph can differ. Each dependency remains separately gated.

## Archive, registry and audit-chain evidence

```json
{
  "target": {
    "version": "2.5.8",
    "archive_sha256": "ff67a8a4397373c3ef660812acab3268222035010ab8680ec4215f38ba3d0eed",
    "index": [
      {
        "registry": "crates.io sparse index",
        "index_file_sha256": "9e63addfab0fa6b5533bf9a28a599544de94a058f3ca7a18128d146c9db0ee81",
        "index_record_sha256": "7b87a5ba131ef93c865552ef03e0e6c133dccabb24a5e922bdcd4fd7f89b9235",
        "cksum": "ff67a8a4397373c3ef660812acab3268222035010ab8680ec4215f38ba3d0eed",
        "yanked": false
      },
      {
        "registry": "crates.io sparse index",
        "index_file_sha256": "9e63addfab0fa6b5533bf9a28a599544de94a058f3ca7a18128d146c9db0ee81",
        "index_record_sha256": "7b87a5ba131ef93c865552ef03e0e6c133dccabb24a5e922bdcd4fd7f89b9235",
        "cksum": "ff67a8a4397373c3ef660812acab3268222035010ab8680ec4215f38ba3d0eed",
        "yanked": false
      }
    ]
  },
  "base": {
    "version": "2.5.7",
    "archive_sha256": "08bc136a29a3d1758e07a9cca267be308aeebf5cfd5a10f3f67ab2097683ef5b",
    "index": [
      {
        "registry": "crates.io sparse index",
        "index_file_sha256": "9e63addfab0fa6b5533bf9a28a599544de94a058f3ca7a18128d146c9db0ee81",
        "index_record_sha256": "dab3b9e1b71e03948657d729ad1e9ef380709517630cebd48b75fc8cf3283357",
        "cksum": "08bc136a29a3d1758e07a9cca267be308aeebf5cfd5a10f3f67ab2097683ef5b",
        "yanked": false
      },
      {
        "registry": "crates.io sparse index",
        "index_file_sha256": "9e63addfab0fa6b5533bf9a28a599544de94a058f3ca7a18128d146c9db0ee81",
        "index_record_sha256": "dab3b9e1b71e03948657d729ad1e9ef380709517630cebd48b75fc8cf3283357",
        "cksum": "08bc136a29a3d1758e07a9cca267be308aeebf5cfd5a10f3f67ab2097683ef5b",
        "yanked": false
      }
    ]
  },
  "baseline_chain": [
    {
      "source": "mozilla",
      "url": "https://raw.githubusercontent.com/mozilla/supply-chain/e6648794a1deb2821ddebd846564fd99f420b432/audits.toml",
      "source_sha256": "97ac6f1002cc4e32f9a71ce5e16d2962325bdf356519d66eb21be2dc4f2ce58d",
      "original_record": {
        "who": "Valentin Gosu <valentin.gosu@gmail.com>",
        "criteria": "safe-to-deploy",
        "version": "2.4.0",
        "aggregated-from": "https://raw.githubusercontent.com/mozilla-firefox/firefox/main/supply-chain/audits.toml"
      }
    },
    {
      "source": "mozilla",
      "url": "https://raw.githubusercontent.com/mozilla/supply-chain/e6648794a1deb2821ddebd846564fd99f420b432/audits.toml",
      "source_sha256": "97ac6f1002cc4e32f9a71ce5e16d2962325bdf356519d66eb21be2dc4f2ce58d",
      "original_record": {
        "who": "Valentin Gosu <valentin.gosu@gmail.com>",
        "criteria": "safe-to-deploy",
        "delta": "2.4.0 -> 2.4.1",
        "aggregated-from": "https://raw.githubusercontent.com/mozilla-firefox/firefox/main/supply-chain/audits.toml"
      }
    },
    {
      "source": "mozilla",
      "url": "https://raw.githubusercontent.com/mozilla/supply-chain/e6648794a1deb2821ddebd846564fd99f420b432/audits.toml",
      "source_sha256": "97ac6f1002cc4e32f9a71ce5e16d2962325bdf356519d66eb21be2dc4f2ce58d",
      "original_record": {
        "who": "Valentin Gosu <valentin.gosu@gmail.com>",
        "criteria": "safe-to-deploy",
        "delta": "2.4.1 -> 2.5.0",
        "aggregated-from": "https://raw.githubusercontent.com/mozilla-firefox/firefox/main/supply-chain/audits.toml"
      }
    },
    {
      "source": "mozilla",
      "url": "https://raw.githubusercontent.com/mozilla/supply-chain/e6648794a1deb2821ddebd846564fd99f420b432/audits.toml",
      "source_sha256": "97ac6f1002cc4e32f9a71ce5e16d2962325bdf356519d66eb21be2dc4f2ce58d",
      "original_record": {
        "who": "Henri Sivonen <hsivonen@hsivonen.fi>",
        "criteria": "safe-to-deploy",
        "delta": "2.5.0 -> 2.5.1",
        "aggregated-from": "https://raw.githubusercontent.com/mozilla-firefox/firefox/main/supply-chain/audits.toml"
      }
    },
    {
      "source": "mozilla",
      "url": "https://raw.githubusercontent.com/mozilla/supply-chain/e6648794a1deb2821ddebd846564fd99f420b432/audits.toml",
      "source_sha256": "97ac6f1002cc4e32f9a71ce5e16d2962325bdf356519d66eb21be2dc4f2ce58d",
      "original_record": {
        "who": "Valentin Gosu <valentin.gosu@gmail.com>",
        "criteria": "safe-to-deploy",
        "delta": "2.5.1 -> 2.5.4",
        "aggregated-from": "https://raw.githubusercontent.com/mozilla-firefox/firefox/main/supply-chain/audits.toml"
      }
    },
    {
      "source": "mozilla",
      "url": "https://raw.githubusercontent.com/mozilla/supply-chain/e6648794a1deb2821ddebd846564fd99f420b432/audits.toml",
      "source_sha256": "97ac6f1002cc4e32f9a71ce5e16d2962325bdf356519d66eb21be2dc4f2ce58d",
      "original_record": {
        "who": "edgul <ed.guloien@gmail.com>",
        "criteria": "safe-to-deploy",
        "delta": "2.5.4 -> 2.5.7",
        "aggregated-from": "https://raw.githubusercontent.com/mozilla-firefox/firefox/main/supply-chain/audits.toml"
      }
    }
  ],
  "read_files": [
    {
      "side": "base",
      "path": "src/host.rs",
      "sha256": "7dcf34a6759f19153d2820683c714e7aafbf35d225e942d3756098c5aa95a3db",
      "coverage": "changed hunks and relevant source context"
    },
    {
      "side": "base",
      "path": "src/parser.rs",
      "sha256": "c39489cb77906d2632876ef6773bc138bdcb6cf04196be3592e3e9c38437f5ce",
      "coverage": "changed hunks and relevant source context"
    },
    {
      "side": "base",
      "path": "src/lib.rs",
      "sha256": "b055bb8ca7bae16a29ca8f214888504bff47836524494842f11a312055e2a1b2",
      "coverage": "changed hunks and relevant source context"
    },
    {
      "side": "base",
      "path": "tests/unit.rs",
      "sha256": "7139f9dcbee4f12ba934f5d37c96a8697cead904537ea28315bbdad6b8fbca0a",
      "coverage": "changed hunks and relevant source context"
    },
    {
      "side": "base",
      "path": "tests/wpt.rs",
      "sha256": "53b8da195ba33fe7c42de24b292d8e058daad2a10a366d97daf6bd1e81ad0113",
      "coverage": "changed hunks and relevant source context"
    },
    {
      "side": "base",
      "path": "Cargo.toml",
      "sha256": "8e9c4b2aff26efd7cfa11becdf7f4b4ee04adecab8ba70e48a4128833d344845",
      "coverage": "changed hunks and relevant source context"
    },
    {
      "side": "base",
      "path": "Cargo.toml.orig",
      "sha256": "7b0c0d904605179de24415c605d6b9d303ac3f41f6ae5e90555c275a24b32b5c",
      "coverage": "changed hunks and relevant source context"
    },
    {
      "side": "target",
      "path": "src/host.rs",
      "sha256": "24bba45b14cb843ea0fe5c857284d458097a13a84847f5c6a15a14c9e1334335",
      "coverage": "changed hunks and relevant source context"
    },
    {
      "side": "target",
      "path": "src/parser.rs",
      "sha256": "2dd4210e1773fc5d3fdc15ce506ecff30dbf8eb358a79bbbc2835cc30d513c3a",
      "coverage": "changed hunks and relevant source context"
    },
    {
      "side": "target",
      "path": "src/lib.rs",
      "sha256": "51f47a93654528f3b8cb92c94fc26ad25775860d84197fff0c3124a687f8c5e3",
      "coverage": "changed hunks and relevant source context"
    },
    {
      "side": "target",
      "path": "tests/unit.rs",
      "sha256": "7eb96c1194fba03fd7f469c44cbbe28699dae14399be72e4c5c675a90fe08dc7",
      "coverage": "changed hunks and relevant source context"
    },
    {
      "side": "target",
      "path": "tests/wpt.rs",
      "sha256": "2d083ab72ad8cc9b116dd164e63440dd120f07977ac306377633cfe214f34a62",
      "coverage": "changed hunks and relevant source context"
    },
    {
      "side": "target",
      "path": "Cargo.toml",
      "sha256": "7003e5562b5ab8454b9d78376303151af79011f5dd684c1891e0f46bc9112016",
      "coverage": "changed hunks and relevant source context"
    },
    {
      "side": "target",
      "path": "Cargo.toml.orig",
      "sha256": "71ca556493d5a7be3df490bee1cfe11da6174f5d1bd6e346efd76e682b26696b",
      "coverage": "changed hunks and relevant source context"
    }
  ],
  "target_inventory_sha256": {
    ".cargo_vcs_info.json": "79d39e19f84f35830fef9637c62027c992e1107f6d850abce545f8277bd99aed",
    "Cargo.lock": "f219c87b25034c1046eeffa7e75288646b719131dd49cf27e1efe64141c65ba7",
    "Cargo.toml": "7003e5562b5ab8454b9d78376303151af79011f5dd684c1891e0f46bc9112016",
    "Cargo.toml.orig": "71ca556493d5a7be3df490bee1cfe11da6174f5d1bd6e346efd76e682b26696b",
    "LICENSE-APACHE": "a60eea817514531668d7e00765731449fe14d059d3249e0bc93b36de45f759f2",
    "LICENSE-MIT": "b38f11f6096706e6de553dabe2a7ed142d59b6fa8c97e290c67496154745cdd5",
    "README.md": "141354cfa79fe3a7e739834bf6e001ff1293169e1809cf9eb6d5c8af0b7a191f",
    "src/host.rs": "24bba45b14cb843ea0fe5c857284d458097a13a84847f5c6a15a14c9e1334335",
    "src/lib.rs": "51f47a93654528f3b8cb92c94fc26ad25775860d84197fff0c3124a687f8c5e3",
    "src/origin.rs": "e8ffaf759aa78c92574c6ae49f76999a751064621c399059b3d4eb819cb0d821",
    "src/parser.rs": "2dd4210e1773fc5d3fdc15ce506ecff30dbf8eb358a79bbbc2835cc30d513c3a",
    "src/path_segments.rs": "28c8b6ffa880c7da76dcf43af8dcae2c46cd74389472e6e43efdf2973026a462",
    "src/quirks.rs": "c3c8380f9dbecf65924272e904cad8876bda0c62e0e48a3c6b1344fac258102f",
    "src/slicing.rs": "b59bf2fa4cbfd31619b860766268d4e99b46537d3b9702b73f6d0a979a7de24a",
    "tests/expected_failures.txt": "dd016d320be3fe6ad3810c22302f8eabd755d9effc937b2193dbf222855c4b7f",
    "tests/setters_tests.json": "a3a4cbd7b798bc2c4d9656dc50be7397a5a5ed1f0b52daa1da1ad654d38c1dcd",
    "tests/unit.rs": "7eb96c1194fba03fd7f469c44cbbe28699dae14399be72e4c5c675a90fe08dc7",
    "tests/urltestdata.json": "58d67bea710d5f46324fe6841df5fd82090fe4ec2d882bc0fc7c1784d4771884",
    "tests/wpt.rs": "2d083ab72ad8cc9b116dd164e63440dd120f07977ac306377633cfe214f34a62"
  },
  "base_inventory_sha256": {
    ".cargo_vcs_info.json": "d734bc8911752ace6f07fd0bac9cb3a9af04f3657fb545b20bf63f06d5141b98",
    "Cargo.lock": "0e5c4536b6d6ef1a30cb3b93d43c8994c0dd91d84c41634b01ec057abf61b431",
    "Cargo.toml": "8e9c4b2aff26efd7cfa11becdf7f4b4ee04adecab8ba70e48a4128833d344845",
    "Cargo.toml.orig": "7b0c0d904605179de24415c605d6b9d303ac3f41f6ae5e90555c275a24b32b5c",
    "LICENSE-APACHE": "a60eea817514531668d7e00765731449fe14d059d3249e0bc93b36de45f759f2",
    "LICENSE-MIT": "b38f11f6096706e6de553dabe2a7ed142d59b6fa8c97e290c67496154745cdd5",
    "README.md": "141354cfa79fe3a7e739834bf6e001ff1293169e1809cf9eb6d5c8af0b7a191f",
    "src/host.rs": "7dcf34a6759f19153d2820683c714e7aafbf35d225e942d3756098c5aa95a3db",
    "src/lib.rs": "b055bb8ca7bae16a29ca8f214888504bff47836524494842f11a312055e2a1b2",
    "src/origin.rs": "e8ffaf759aa78c92574c6ae49f76999a751064621c399059b3d4eb819cb0d821",
    "src/parser.rs": "c39489cb77906d2632876ef6773bc138bdcb6cf04196be3592e3e9c38437f5ce",
    "src/path_segments.rs": "28c8b6ffa880c7da76dcf43af8dcae2c46cd74389472e6e43efdf2973026a462",
    "src/quirks.rs": "c3c8380f9dbecf65924272e904cad8876bda0c62e0e48a3c6b1344fac258102f",
    "src/slicing.rs": "b59bf2fa4cbfd31619b860766268d4e99b46537d3b9702b73f6d0a979a7de24a",
    "tests/expected_failures.txt": "dd016d320be3fe6ad3810c22302f8eabd755d9effc937b2193dbf222855c4b7f",
    "tests/setters_tests.json": "a3a4cbd7b798bc2c4d9656dc50be7397a5a5ed1f0b52daa1da1ad654d38c1dcd",
    "tests/unit.rs": "7139f9dcbee4f12ba934f5d37c96a8697cead904537ea28315bbdad6b8fbca0a",
    "tests/urltestdata.json": "58d67bea710d5f46324fe6841df5fd82090fe4ec2d882bc0fc7c1784d4771884",
    "tests/wpt.rs": "53b8da195ba33fe7c42de24b292d8e058daad2a10a366d97daf6bd1e81ad0113"
  },
  "independent_context_files": [],
  "external_interface_context": [],
  "dependency_interface_contracts": null,
  "full_diff_sha256": "866bb529fffa434664c2a3d3da52633beb5fb8bfee64ae7d8e8d631db82c85fd",
  "primary_report_sha256": "8ff0d73d9d7ee210554405c562e7a2663858f1e129bfbfb2c56aef736dc7abf2",
  "cross_report_sha256": "3da1905e4d801141ee9e083ea0fc22c20a860f6d62d3aade0e157faff32dec29"
}
```

Inventory hashes identify all archive members; only read-file entries and the complete reviewed delta claim review coverage. Source chain records retain original attribution; the new review is AI-assisted.

## Primary review

# url 2.5.8 source review

Target archive SHA-256 `ff67a8a4397373c3ef660812acab3268222035010ab8680ec4215f38ba3d0eed`, independently matched to frozen Cargo.lock and registry index; safe-member and full inventory in `../provenance.json`.
Required `safe-to-deploy`. Reviewer `Codex (GPT-6.1 Terra; AI-assisted source review)`.

Base `2.5.7` SHA-256 `08bc136a29a3d1758e07a9cca267be308aeebf5cfd5a10f3f67ab2097683ef5b`. Authentication/criteria path in `../provenance.json`.
Complete directed diff: `../diffs/url-2.5.7-to-2.5.8.diff` SHA-256 `866bb529fffa434664c2a3d3da52633beb5fb8bfee64ae7d8e8d631db82c85fd`.

## Read-file list (old SHA-256 -> new SHA-256)

- `.cargo_vcs_info.json` `d734bc8911752ace6f07fd0bac9cb3a9af04f3657fb545b20bf63f06d5141b98` -> `79d39e19f84f35830fef9637c62027c992e1107f6d850abce545f8277bd99aed`
- `Cargo.lock` `0e5c4536b6d6ef1a30cb3b93d43c8994c0dd91d84c41634b01ec057abf61b431` -> `f219c87b25034c1046eeffa7e75288646b719131dd49cf27e1efe64141c65ba7`
- `Cargo.toml` `8e9c4b2aff26efd7cfa11becdf7f4b4ee04adecab8ba70e48a4128833d344845` -> `7003e5562b5ab8454b9d78376303151af79011f5dd684c1891e0f46bc9112016`
- `Cargo.toml.orig` `7b0c0d904605179de24415c605d6b9d303ac3f41f6ae5e90555c275a24b32b5c` -> `71ca556493d5a7be3df490bee1cfe11da6174f5d1bd6e346efd76e682b26696b`
- `src/host.rs` `7dcf34a6759f19153d2820683c714e7aafbf35d225e942d3756098c5aa95a3db` -> `24bba45b14cb843ea0fe5c857284d458097a13a84847f5c6a15a14c9e1334335`
- `src/lib.rs` `b055bb8ca7bae16a29ca8f214888504bff47836524494842f11a312055e2a1b2` -> `51f47a93654528f3b8cb92c94fc26ad25775860d84197fff0c3124a687f8c5e3`
- `src/parser.rs` `c39489cb77906d2632876ef6773bc138bdcb6cf04196be3592e3e9c38437f5ce` -> `2dd4210e1773fc5d3fdc15ce506ecff30dbf8eb358a79bbbc2835cc30d513c3a`
- `tests/unit.rs` `7139f9dcbee4f12ba934f5d37c96a8697cead904537ea28315bbdad6b8fbca0a` -> `7eb96c1194fba03fd7f469c44cbbe28699dae14399be72e4c5c675a90fe08dc7`
- `tests/wpt.rs` `53b8da195ba33fe7c42de24b292d8e058daad2a10a366d97daf6bd1e81ad0113` -> `2d083ab72ad8cc9b116dd164e63440dd120f07977ac306377633cfe214f34a62`

## Analysis

Read complete 2.5.7 -> 2.5.8 diff, including all 9 changed files and the full 950+ line lockfile delta. Mozilla fixed-commit chain full 2.4.0 plus successive deltas to 2.5.7 satisfies safe-to-deploy; exact records/attribution/checksums are in provenance.json. Package Cargo.lock changes idna_adapter from 1.2.0 with ICU to 1.1.0 with unicode mapping, plus test/tooling and transitive version changes; that package lock does not replace the frozen application Cargo.lock. Both manifests retain MSRV 1.63, idna 1.1.0/alloc/compiled_data, form_urlencoded and percent-encoding dependencies, and build=false. `serde` and `serde_derive` become separately optional but jointly activated by explicit serde feature; `serde?/std` avoids enabling serde merely because std is enabled. Test derives switch to serde_derive. Dependencies/proc macros remain independent audits.

In src/host.rs:123-175, opaque host parsing retains existing bracketed IPv6 and forbidden host character rejection before encoding. The behavioral fix is lines 162-173: percent encoder can return Cow::Borrowed for either original unchanged text or an encoded single item from a static table. The old code always reused input for Borrowed, losing a single encoded control; target compares content and owns the encoded static value when different. As a result one control byte is serialized percent encoded instead of passed through, while unchanged text is reused and multi-piece owned output is retained. Parser special-host path remains the IDNA+IPv4 path, not touched by this change; non-special host path reaches this helper. The changed unit test parses `l://\x01:` then reparses serialized form and compares equality. Other source changes are comments; no new unsafe, FFI, network, filesystem, subprocess or secret access.

Independent `../probes/url_host.rs` (SHA-256 `f4b1134d61603b5df20ea13715748df3c28b44eed84dc8c3bbd0abe37ce87034`) compiled lock/index-verified percent-encoding 2.3.2 and exercised the exact changed branch for a one-byte control, unchanged host and multi-byte composition; output matched `%01`, unchanged, `a%01b`. It copies the branch and is not an end-to-end Url parser test. No serious vulnerability identified in this delta conditional on base; no exhaustive host/IDNA/URL correctness or runtime integration claim.

## Proposed audit (not registered)

```toml
[[audits.url]]
who = "Codex (GPT-6.1 Terra; AI-assisted source review)"
criteria = "safe-to-deploy"
delta = "2.5.7 -> 2.5.8"
```


## Independent cross-review

src/host.rs:123-175 first rejects the listed forbidden opaque-host characters and handles bracketed IPv6. utf8_percent_encode may return a Cow::Borrowed reference to its static percent table, so the changed arm compares the encoded content with input: unchanged input can be reused, otherwise the encoded bytes become owned. This preserves %XX when a one-character input requires encoding. src/parser.rs:1038-1040 calls this for non-special hosts; special host/IDNA parsing is a separate path. Manifest std now forwards serde?/std without enabling optional serde by itself. Read all nine changed paths, including complete lock and tests. Mozilla's fixed full 2.4.0 plus consecutive safe-to-deploy deltas anchor 2.5.7; proposal is 2.5.7 -> 2.5.8. This does not certify IDNA dependencies or exhaustive URL conformance.

Static evidence is not a formal proof, exhaustive platform or concurrency execution, or a recursive certification of dependencies.
