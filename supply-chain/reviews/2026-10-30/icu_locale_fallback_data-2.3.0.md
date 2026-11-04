# Source review: icu_locale_fallback_data 2.3.0

Reviewer: Codex (GPT-6.1 Terra; AI-assisted source review). Independent cross-review: Codex AI-assisted.
Criterion: `safe-to-deploy`; scope: full package.
Frozen batch SHA-256: `aa0f8805f0e48d23f3eeae37a1e4097c9bdd761e30801099b4d1c3eded471b43`; frozen original Cargo.lock SHA-256: `2a60d89a9fb754475a33d8034227743a8bb5ff2e7410b064f5c122800360385d`.
This review applies to the frozen versions only; the final dependency graph can differ. Each dependency remains separately gated.

## Archive, registry and audit-chain evidence

```json
{
  "target": {
    "version": "2.3.0",
    "archive_sha256": "decf2a22ec8fa68f1a0c1129a3f8583f8f8bc24e8b9ccbe98ead99f62a4dc3a8",
    "index": [
      {
        "registry": "crates.io sparse index",
        "index_file_sha256": "508b37395603354c8c19204cc37ea5b6d01000d81e8431a0059f4bcafdc5fddd",
        "index_record_sha256": "94ba762eb1b77913bab5c81bf42f9aa37a8b5ee6428e25de4031ea33d0256dae",
        "cksum": "decf2a22ec8fa68f1a0c1129a3f8583f8f8bc24e8b9ccbe98ead99f62a4dc3a8",
        "yanked": false
      }
    ]
  },
  "base": null,
  "baseline_chain": [],
  "read_files": [
    {
      "path": ".cargo_vcs_info.json",
      "sha256": "2901ff8b55f4ac010f04fcbbbd4548bbfd19c6cb2c130f88087119d42df7fa9d",
      "bytes": 123,
      "kind": "text"
    },
    {
      "path": "Cargo.lock",
      "sha256": "9c8068c3477c00a804ceef3ebfae3655b8c99191164bb6e7fea2a7a2eb648b60",
      "bytes": 168,
      "kind": "text"
    },
    {
      "path": "Cargo.toml",
      "sha256": "9c4dbb492b573f2a753c1a1847c2973628ac22884c091f73dfabd470135e0aae",
      "bytes": 1587,
      "kind": "text"
    },
    {
      "path": "Cargo.toml.orig",
      "sha256": "5e30e603dd29a23d6cdeaeba1544635399242ee0063550daef17077840951bc8",
      "bytes": 812,
      "kind": "text"
    },
    {
      "path": "README.md",
      "sha256": "56b0ad386d87f6e13f93619613bff4d4f980ad23359ac6fa16c81e95e3105daf",
      "bytes": 520,
      "kind": "text"
    },
    {
      "path": "build.rs",
      "sha256": "c2d446772e3d766a804963dbf36e51729f910920f91f4b68c0c199fe6ca0853e",
      "bytes": 452,
      "kind": "text"
    },
    {
      "path": "src/lib.rs",
      "sha256": "9a1282933de043c6f1bbe637adb64846fe3238d662f2473dd4b6065482122347",
      "bytes": 609,
      "kind": "text"
    },
    {
      "path": "data/mod.rs",
      "sha256": "d626a3ba9b0884fe8d4d1c5c92f2646a76597c3ce082299137647c54622d5df5",
      "bytes": 1172,
      "kind": "text"
    },
    {
      "path": "data/locale_likely_subtags_language_v1.rs.data",
      "sha256": "e3cef4930a0e0363247bf9f8dc503761777186a09eb116c6be2f36e13e26877c",
      "bytes": 8169,
      "kind": "text"
    },
    {
      "path": "data/locale_parents_v1.rs.data",
      "sha256": "25f2ab821e6eacc9dafdc23a97717add132a593a2be9f93a37f57f905f8548f4",
      "bytes": 9371,
      "kind": "text"
    }
  ],
  "target_inventory_sha256": {
    ".cargo_vcs_info.json": "2901ff8b55f4ac010f04fcbbbd4548bbfd19c6cb2c130f88087119d42df7fa9d",
    "Cargo.lock": "9c8068c3477c00a804ceef3ebfae3655b8c99191164bb6e7fea2a7a2eb648b60",
    "Cargo.toml": "9c4dbb492b573f2a753c1a1847c2973628ac22884c091f73dfabd470135e0aae",
    "Cargo.toml.orig": "5e30e603dd29a23d6cdeaeba1544635399242ee0063550daef17077840951bc8",
    "LICENSE": "f367c1b8e1aa262435251e442901da4607b4650e0e63a026f5044473ecfb90f2",
    "README.md": "56b0ad386d87f6e13f93619613bff4d4f980ad23359ac6fa16c81e95e3105daf",
    "build.rs": "c2d446772e3d766a804963dbf36e51729f910920f91f4b68c0c199fe6ca0853e",
    "data/locale_likely_subtags_language_v1.rs.data": "e3cef4930a0e0363247bf9f8dc503761777186a09eb116c6be2f36e13e26877c",
    "data/locale_parents_v1.rs.data": "25f2ab821e6eacc9dafdc23a97717add132a593a2be9f93a37f57f905f8548f4",
    "data/mod.rs": "d626a3ba9b0884fe8d4d1c5c92f2646a76597c3ce082299137647c54622d5df5",
    "src/lib.rs": "9a1282933de043c6f1bbe637adb64846fe3238d662f2473dd4b6065482122347"
  },
  "base_inventory_sha256": {},
  "independent_context_files": [
    {
      "path": "validation/gates-fix-20261030/source-audits/lane-b-001/scratch/icu_locale_fallback_data-2.3.0/target/Cargo.toml",
      "sha256": "9c4dbb492b573f2a753c1a1847c2973628ac22884c091f73dfabd470135e0aae",
      "inspection": "relevant lines/byte arrays and changed regions"
    },
    {
      "path": "validation/gates-fix-20261030/source-audits/lane-b-001/scratch/icu_locale_fallback_data-2.3.0/target/Cargo.toml.orig",
      "sha256": "5e30e603dd29a23d6cdeaeba1544635399242ee0063550daef17077840951bc8",
      "inspection": "relevant lines/byte arrays and changed regions"
    },
    {
      "path": "validation/gates-fix-20261030/source-audits/lane-b-001/scratch/icu_locale_fallback_data-2.3.0/target/src/lib.rs",
      "sha256": "9a1282933de043c6f1bbe637adb64846fe3238d662f2473dd4b6065482122347",
      "inspection": "relevant lines/byte arrays and changed regions"
    },
    {
      "path": "validation/gates-fix-20261030/source-audits/lane-b-001/scratch/icu_locale_fallback_data-2.3.0/target/build.rs",
      "sha256": "c2d446772e3d766a804963dbf36e51729f910920f91f4b68c0c199fe6ca0853e",
      "inspection": "relevant lines/byte arrays and changed regions"
    },
    {
      "path": "validation/gates-fix-20261030/source-audits/lane-b-001/scratch/icu_locale_fallback_data-2.3.0/target/data/mod.rs",
      "sha256": "d626a3ba9b0884fe8d4d1c5c92f2646a76597c3ce082299137647c54622d5df5",
      "inspection": "relevant lines/byte arrays and changed regions"
    },
    {
      "path": "validation/gates-fix-20261030/source-audits/lane-b-001/scratch/icu_locale_fallback_data-2.3.0/target/data/locale_likely_subtags_language_v1.rs.data",
      "sha256": "e3cef4930a0e0363247bf9f8dc503761777186a09eb116c6be2f36e13e26877c",
      "inspection": "relevant lines/byte arrays and changed regions"
    },
    {
      "path": "validation/gates-fix-20261030/source-audits/lane-b-001/scratch/icu_locale_fallback_data-2.3.0/target/data/locale_parents_v1.rs.data",
      "sha256": "25f2ab821e6eacc9dafdc23a97717add132a593a2be9f93a37f57f905f8548f4",
      "inspection": "relevant lines/byte arrays and changed regions"
    },
    {
      "path": "validation/gates-fix-20261030/source-audits/lane-b-001/icu_locale_fallback_data-2.3.0/read-files.json",
      "sha256": "8d5d34e34f032ed4ca45cf29182703212ef56922160b3a6d5aab1047b69659c1",
      "inspection": "independently checked against archive and changed-member coverage"
    }
  ],
  "external_interface_context": [],
  "dependency_interface_contracts": {
    "purpose": "selected type/encoding contract inspection only; not a dependency audit",
    "packages": [
      {
        "package": "icu_locale_fallback-2.3.0",
        "archive_sha256": "251af8e57c9400e3eb58242fe5b8b1152b2a64fdf4cf632f923c38ccee6f2fa9",
        "files": [
          {
            "path": "crates.io/icu_locale_fallback-2.3.0/src/provider.rs",
            "sha256": "363ad5dc10d784308504d439a26d005d31e0bdb12f91c79c950a9b5c7633cb67"
          }
        ]
      },
      {
        "package": "icu_locale_core-2.3.0",
        "archive_sha256": "d56e28588da92eee5c3201a6eff33fabdd49b62269c8938d4ff050ce4d900deb",
        "files": [
          {
            "path": "crates.io/icu_locale_core-2.3.0/src/helpers.rs",
            "sha256": "51eeadbc4627baef055b4b35e9a252c302c5a7c1b0a78328f1818dba84718821"
          },
          {
            "path": "crates.io/icu_locale_core-2.3.0/src/subtags/language.rs",
            "sha256": "9c61afdd83d420602c550784ffeb5524c8a455344a32c307d6fa7938a901f811"
          },
          {
            "path": "crates.io/icu_locale_core-2.3.0/src/subtags/script.rs",
            "sha256": "8ee9c763837ce9628da359043aa56841e2f8481b73a5587578921cf1eb191049"
          },
          {
            "path": "crates.io/icu_locale_core-2.3.0/src/subtags/region.rs",
            "sha256": "f02119571e9b9fc886c2c15b6f1f031f9f0885a3f1489478d623272a06122b07"
          }
        ]
      },
      {
        "package": "tinystr-0.8.4",
        "archive_sha256": "b1e27c91459209c2986af3dcf603a5a74a4368754ce37414f59acc971167f643",
        "files": [
          {
            "path": "crates.io/tinystr-0.8.4/src/ule.rs",
            "sha256": "193b183c34269e2c6c0438c2f5e2a89edec441a7b8f5bc465f3a3b8d52af1ca8"
          },
          {
            "path": "crates.io/tinystr-0.8.4/src/unvalidated.rs",
            "sha256": "26c44e7e40d3ffaf8ba803082bc9198bb33ec051d9cecea881738d29ebffda28"
          }
        ]
      },
      {
        "package": "zerovec-0.11.8",
        "archive_sha256": "bb0464e17806c1d976d5cba29399c7f08e516e279e2ba493f63123b5fca67dd8",
        "files": [
          {
            "path": "crates.io/zerovec-0.11.8/src/varzerovec/components.rs",
            "sha256": "1b2e859ab718341471f58b2ffee05d396cbf0154ab9d521f4cbc8fa452e12f47"
          },
          {
            "path": "crates.io/zerovec-0.11.8/src/ule/option.rs",
            "sha256": "be9ea7137448530c09d7a3dea46d2bf0dc0f25f751eb90a9225faf918fc49612"
          },
          {
            "path": "crates.io/zerovec-0.11.8/src/ule/tuple.rs",
            "sha256": "6c96c1def8a1316ab2d94b40d0c59a5ac1552c7297f13bf02e50cc0967286416"
          },
          {
            "path": "crates.io/zerovec-0.11.8/src/map/map.rs",
            "sha256": "9d9bfe4085af674274b31bba75034ca4d11cfa925d3ee71af91d1870ab711d0b"
          },
          {
            "path": "crates.io/zerovec-0.11.8/src/lib.rs",
            "sha256": "59646e9221960a6706804aa16e16eb7eadfda5bb1763cab6ca3a330b97e96df8"
          }
        ]
      },
      {
        "package": "potential_utf-0.1.6",
        "archive_sha256": "d83eb9bc6d8e5cf568e7a1101d60ee05e81ed50ea106026f3d18deeb046d7661",
        "files": [
          {
            "path": "validation/gates-fix-20261030/source-audits/lane-b-001/scratch/potential_utf-0.1.6/target/src/ustr.rs",
            "sha256": "bf592b05a373e7649b8cd6e88cddde247c674e72622fe73effdc41252366a33a"
          }
        ]
      }
    ]
  },
  "full_diff_sha256": null,
  "primary_report_sha256": "a56a284bf15126d3222c748ee36afb32e9a4df0ed061a8e50c48c8d5931e35c7",
  "cross_report_sha256": "d06dadbc06d6ef08ce1028464b9e9ba8b5df27653e60140f8225cfe7933a0165"
}
```

Inventory hashes identify all archive members; only read-file entries and the complete reviewed delta claim review coverage. Source chain records retain original attribution; the new review is AI-assisted.

## Primary review

# icu_locale_fallback_data 2.3.0

Reviewer: Codex (GPT-6.1 Terra; AI-assisted source review)

Full package-source and packaged generated-data review; propose package-local `safe-to-deploy` for independent review. The static argument below is tied to the frozen dependency contracts and is not a formal proof or a recursive dependency audit.

- `Cargo.toml:12-29,61-65` sets edition 2024/MSRV 1.88, an explicit build script, no Rust dependencies and the data include paths. `build.rs:5-10` tests whether `ICU4X_DATA_DIR` exists, emitting the cfg and rerun directive. With it set, `src/lib.rs:14-17` includes `<ICU4X_DATA_DIR>/mod.rs` as Rust source, so that directory is trusted build input and its contents require separate review; without it, the packaged `data/mod.rs:1-3,16-40` includes and exports the two generated macro sets. The include mechanism does not establish safety of arbitrary custom data.
- `data/locale_likely_subtags_language_v1.rs.data:18-50` creates three singleton maps using unchecked ZeroVec byte constructors. The locked `icu_locale_fallback` field definitions and `tinystr`/`zerovec` ULE contracts give fixed key/value widths of 7/3, 6/4 and 3/7 bytes. `probe_data.py` decodes every literal, establishes equal key/value counts of 21, 46 and 161 respectively, confirms strictly increasing keys using the fixed-width byte order of `UnvalidatedTinyAsciiStr`, and checks normalized Language, Script and Region byte representations. It validates every generated region and script value against the locked locale subtag constraints; their tuple ULE uses packed, field-by-field validation.
- `data/locale_parents_v1.rs.data:17-40` builds the fourth map from a VarZeroVec16 of PotentialUtf8 keys and a ZeroVec of `(Language, Option<Script>, Option<Region>)`. The locked `zerovec` format encodes a 16-bit little-endian length and 151 offsets for the 152 keys. The probe validates monotonic in-range offsets, all 152 strictly sorted byte strings, and 152 twelve-byte values, including normalized language/script/region fields, boolean option tags 0/1 and all-zero inactive option payloads. PotentialUtf8 accepts all byte sequences; the stronger nonempty UTF-8 and locale syntax checks also pass. `und` is built by the compile-time `language!`, `script!`, and `region!` macros from `en`, `Latn`, `US`.
- Both generated provider implementations return the immutable singleton for the unknown locale and `InvalidRequest` otherwise; the `DRY` and `ITER` arms return corresponding metadata and singleton ID. No local network, command, FFI or mutable concurrency path is present. The macros assume the separately audited ICU/zerovec/locale contracts and are not an audit of those dependencies.
- `dependency-contract.json` identifies relevant frozen-lock archive hashes and source file SHA-256, including provider types, tuple/option layouts, unvalidated tiny strings, VarZeroVec16 format, and subtag validators. `structural-probe.json` records the bounded independent reconstruction of all packaged byte literals. A small host-only Rust program in `data-probe/validate.rs` also ran the corresponding safe Rust ULE/VarZeroVec parsers over all eight extracted byte arrays and obtained the same counts and strict Rust key ordering; `data-probe/evidence.json` records the binary and pre-existing library hashes. Those libraries were identified as matching versions by their depfiles, but were not rebuilt and their artifact provenance was not independently proven. Neither probe proves all consumers safe, verifies custom included source, establishes locale-data linguistic correctness, or replaces platform tests.

Evidence: `inventory.json` records the target archive/index/App-lock checksums and all member hashes. `read-files.json` records source and metadata actually read; license text identified by filename/hash only, not legally reviewed. This queue had no trusted upstream base. No binary archive member is present.

Proposed record for independent reviewer (not registered):

```toml
[[audits.icu_locale_fallback_data]]
who = "Codex (GPT-6.1 Terra; AI-assisted source review)"
criteria = "safe-to-deploy"
version = "2.3.0"
```


## Independent cross-review

Generated data/locale_likely_subtags_language_v1.rs.data:25-38 uses unchecked zero-map/zero-vec constructors; parents data:24-27 also uses unchecked VarZeroVec16. Independently decoded all eight embedded byte arrays and checked locked provider, tinystr, zerovec, locale-core and potential_utf contracts: maps have 21/46/161/152 entries, equal key/value counts and strict correct ordering; parent offsets are monotone/in-bounds, and all Language/Script/Region values, Option tags and inactive payloads are valid. build.rs:5-10 and src/lib.rs:14-17 can include ICU4X_DATA_DIR/mod.rs: that external directory is trusted build input requiring its own review. Packaged data only; this does not establish linguistic correctness, arbitrary external-data safety, dependency certification or runtime acceptance.

Static evidence is not a formal proof, exhaustive platform or concurrency execution, or a recursive certification of dependencies.
