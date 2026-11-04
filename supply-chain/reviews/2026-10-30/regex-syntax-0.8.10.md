# Source review: regex-syntax 0.8.5 -> 0.8.10

Reviewer: Codex (GPT-6.1 Terra; AI-assisted source review). Independent cross-review: Codex AI-assisted.
Criterion: `safe-to-deploy`; scope: delta.
Frozen batch SHA-256: `aa0f8805f0e48d23f3eeae37a1e4097c9bdd761e30801099b4d1c3eded471b43`; frozen original Cargo.lock SHA-256: `2a60d89a9fb754475a33d8034227743a8bb5ff2e7410b064f5c122800360385d`.
This review applies to the frozen versions only; the final dependency graph can differ. Each dependency remains separately gated.

## Archive, registry and audit-chain evidence

```json
{
  "target": {
    "version": "0.8.10",
    "archive_sha256": "dc897dd8d9e8bd1ed8cdad82b5966c3e0ecae09fb1907d58efaa013543185d0a",
    "index": [
      {
        "registry": "crates.io sparse index",
        "index_file_sha256": "99daa6b4b0f98d3a1070b20a95fd23ef9e0408b5a6d268fc6298f08a372530ab",
        "index_record_sha256": "de40a764afaa5f355c29c7134cc807509a404f2bf90ebbf9faa9d668dd0aa279",
        "cksum": "dc897dd8d9e8bd1ed8cdad82b5966c3e0ecae09fb1907d58efaa013543185d0a",
        "yanked": false
      },
      {
        "registry": "crates.io sparse index",
        "index_file_sha256": "99daa6b4b0f98d3a1070b20a95fd23ef9e0408b5a6d268fc6298f08a372530ab",
        "index_record_sha256": "de40a764afaa5f355c29c7134cc807509a404f2bf90ebbf9faa9d668dd0aa279",
        "cksum": "dc897dd8d9e8bd1ed8cdad82b5966c3e0ecae09fb1907d58efaa013543185d0a",
        "yanked": false
      }
    ]
  },
  "base": {
    "version": "0.8.5",
    "archive_sha256": "2b15c43186be67a4fd63bee50d0303afffcef381492ebe2c5d87f324e1b8815c",
    "index": [
      {
        "registry": "crates.io sparse index",
        "index_file_sha256": "99daa6b4b0f98d3a1070b20a95fd23ef9e0408b5a6d268fc6298f08a372530ab",
        "index_record_sha256": "dbfc7843206014793c1fbb5a92f9d0dcba8e5ed8bbf0c8bbb79f68e7622f5906",
        "cksum": "2b15c43186be67a4fd63bee50d0303afffcef381492ebe2c5d87f324e1b8815c",
        "yanked": false
      },
      {
        "registry": "crates.io sparse index",
        "index_file_sha256": "99daa6b4b0f98d3a1070b20a95fd23ef9e0408b5a6d268fc6298f08a372530ab",
        "index_record_sha256": "dbfc7843206014793c1fbb5a92f9d0dcba8e5ed8bbf0c8bbb79f68e7622f5906",
        "cksum": "2b15c43186be67a4fd63bee50d0303afffcef381492ebe2c5d87f324e1b8815c",
        "yanked": false
      }
    ]
  },
  "baseline_chain": [
    {
      "source": "google",
      "url": "https://raw.githubusercontent.com/google/rust-crate-audits/8cec794c38751495cf7a6c083220c207af1303d6/audits.toml",
      "source_sha256": "eb1c1bf1ee616471bfb56965eea325815ee41b5e3fcf5d12537dff2791160bbd",
      "original_record": {
        "who": "Manish Goregaokar <manishearth@google.com>",
        "criteria": [
          "safe-to-deploy",
          "does-not-implement-crypto",
          "ub-risk-0"
        ],
        "version": "0.8.5",
        "notes": "Contains no unsafe",
        "aggregated-from": "https://chromium.googlesource.com/chromium/src/+/main/third_party/rust/chromium_crates_io/supply-chain/audits.toml?format=TEXT"
      }
    }
  ],
  "read_files": [
    {
      "side": "base",
      "path": "src/ast/mod.rs",
      "sha256": "21cda9fe9e1810b285cb8f8a2aa5eeaff6c38e256ceed036b68c66fb6b0124d6",
      "coverage": "changed hunks and relevant source context"
    },
    {
      "side": "base",
      "path": "src/hir/translate.rs",
      "sha256": "6129a12a686a6ec8965cdeb9f889640891da6aae75995606080fa88d4dd1a602",
      "coverage": "changed hunks and relevant source context"
    },
    {
      "side": "base",
      "path": "src/lib.rs",
      "sha256": "5ae457d402e49443bdb23b71353693dd3b0d263b57a6eeb9eb5b5dae5c901bdd",
      "coverage": "changed hunks and relevant source context"
    },
    {
      "side": "base",
      "path": "Cargo.toml",
      "sha256": "362cfcf492ed93948ca374491d8dc3b688da0f90d00c2bb9a3892ec2ac27e276",
      "coverage": "changed hunks and relevant source context"
    },
    {
      "side": "base",
      "path": "Cargo.toml.orig",
      "sha256": "4c18da01da8ea5c747cb548eee8c34f935510be514b62869ff046f53aca37583",
      "coverage": "changed hunks and relevant source context"
    },
    {
      "side": "target",
      "path": "src/ast/mod.rs",
      "sha256": "38df06574a3816eae2796c757d9d268b7b42ce3fb1feee86750e7d04de7649c7",
      "coverage": "changed hunks and relevant source context"
    },
    {
      "side": "target",
      "path": "src/hir/translate.rs",
      "sha256": "8c01783e74261a8562c2694394d94ccb8cb173274932731fa77118041fab4507",
      "coverage": "changed hunks and relevant source context"
    },
    {
      "side": "target",
      "path": "src/lib.rs",
      "sha256": "c51d1e55a8b6c4608e21a278ed0ef9480f73ab5b814b6ca6127f4a049c4d5007",
      "coverage": "changed hunks and relevant source context"
    },
    {
      "side": "target",
      "path": "Cargo.toml",
      "sha256": "3ae6ebabbe2e3e56f5499bc2a3ea4d268c0eb28fe11ff9d53bde41d18aaad03a",
      "coverage": "changed hunks and relevant source context"
    },
    {
      "side": "target",
      "path": "Cargo.toml.orig",
      "sha256": "1138581b9b6d57b000926e72ed1d972776949740836913c754bb068e2372fe7a",
      "coverage": "changed hunks and relevant source context"
    }
  ],
  "target_inventory_sha256": {
    ".cargo_vcs_info.json": "47a21948a4b18c4c1b2f8f9bf0ab57906a5dc38f7912a8e1b1df4c367a0a61eb",
    "Cargo.lock": "5cf8c2048f40caa65c267c6a60c9070efddbaf2722fc10e12b084a4d41a100f4",
    "Cargo.toml": "3ae6ebabbe2e3e56f5499bc2a3ea4d268c0eb28fe11ff9d53bde41d18aaad03a",
    "Cargo.toml.orig": "1138581b9b6d57b000926e72ed1d972776949740836913c754bb068e2372fe7a",
    "LICENSE-APACHE": "a60eea817514531668d7e00765731449fe14d059d3249e0bc93b36de45f759f2",
    "LICENSE-MIT": "6485b8ed310d3f0340bf1ad1f47645069ce4069dcc6bb46c7d5c6faf41de1fdb",
    "README.md": "b2484aa7e66fb92d1378e9a7ce7605af18f77cb12c179866eaf92ba28cfec1d9",
    "benches/bench.rs": "d2b6ae5b939abd6093064f144b981b7739d7f474ec0698a1268052fc92406635",
    "src/ast/mod.rs": "38df06574a3816eae2796c757d9d268b7b42ce3fb1feee86750e7d04de7649c7",
    "src/ast/parse.rs": "af6d82e62e97379a91840e99c10be1279c0ae1298a0d8e5da429e9e7bc3ec339",
    "src/ast/print.rs": "6a681ce021a384c47dda04c71e2868b5dd61c633bb5b1a67628b9a8df16f0413",
    "src/ast/visitor.rs": "2af2efd77727803b8d15a6244af92267e96edbfc1a211bcbea2b32e1b5483918",
    "src/debug.rs": "91b2492394de05bb11ee75329115b33fb378c19e076370d1a1ae665ce1682777",
    "src/either.rs": "1758e3edd056884eccadd995708d1e374ba9aa65846bd0e13b1aae852607c560",
    "src/error.rs": "e308b3ccad0bea927f4e3957170302e9cfa743bfdf9376f3a5f4137b44ca6cfc",
    "src/hir/interval.rs": "74d75837d24ab9a3cff33b375b70694cdd3b9a4610c799137533f365755ba604",
    "src/hir/literal.rs": "61e9f54103c671694dd017c23c5c9263e032735921ef77527940e83b29ced540",
    "src/hir/mod.rs": "13ee5b65fac1f2c9780ce48a500b1e9d198cb0bc07c0d7f4a4391aab87424563",
    "src/hir/print.rs": "e1e1dfa71983c8fea64f500a0b9dfcbd258b4060e12b95d432468015a247a5cb",
    "src/hir/translate.rs": "8c01783e74261a8562c2694394d94ccb8cb173274932731fa77118041fab4507",
    "src/hir/visitor.rs": "71ca9c93aa48a5ed445399659fa6455093a1bbd9ef44b66bc7095c1b08b2ec1f",
    "src/lib.rs": "c51d1e55a8b6c4608e21a278ed0ef9480f73ab5b814b6ca6127f4a049c4d5007",
    "src/parser.rs": "6b2f4f27e3331a01a25b87c89368dd2e54396bd425dac57941f9c1ebfd238ac8",
    "src/rank.rs": "ff3d58b0cc5ffa69e2e8c56fc7d9ef41dd399d59a639a253a51551b858cb5bbd",
    "src/unicode.rs": "b2084dcbd4331501b9a895fd7e7575d93ff96eb661c6e6adbc8c66bb72685cde",
    "src/unicode_tables/LICENSE-UNICODE": "74db5baf44a41b1000312c673544b3374e4198af5605c7f9080a402cec42cfa3",
    "src/unicode_tables/age.rs": "71b7cf52acdb4aa98b44145303b8efbfa94913235493521941ef1e0092a0ffe2",
    "src/unicode_tables/case_folding_simple.rs": "7622c7f7f03ac0dc2f2bcd51c81a217d64de0cc912f62f1add5f676603a02456",
    "src/unicode_tables/general_category.rs": "9488e3721f7c2ae20e1b77fcff9a59b4ed8f22954b8645ea6d8592eac1856423",
    "src/unicode_tables/grapheme_cluster_break.rs": "0dd9d66bad598f4ec3451b6699f05c17c52079e37d463baf6385bbe51aa218f1",
    "src/unicode_tables/mod.rs": "26c837099cd934c8062e24bc9a0aaecf15fe1de03f9c6da3f3e1e5ac3ca24bee",
    "src/unicode_tables/perl_decimal.rs": "6a59143db81a0bcaf0e8d0af265e711d1a6472e1f091ee9ee4377da5d5d0cd1f",
    "src/unicode_tables/perl_space.rs": "ec9bb22ed7e99feef292249c7e6f4673ee0af9635d4d158f93923494c14cd5ed",
    "src/unicode_tables/perl_word.rs": "30f073baae28ea34c373c7778c00f20c1621c3e644404eff031f7d1cc8e9c9e2",
    "src/unicode_tables/property_bool.rs": "66cf5bd2a1438bf9694152f077a285cf014fbd50b9dd63a97233b2ea61d64962",
    "src/unicode_tables/property_names.rs": "8c93985d1bcb01735667a3c4cb92f7e260d267326bde9d7f048bc77cd7e07855",
    "src/unicode_tables/property_values.rs": "ef9131ce0a575c7327ec6d466aafd8b7c25600d80c232b5a4110bbf0a5a59136",
    "src/unicode_tables/script.rs": "41bd424f1e3a03290cf4995ced678dcf24c94b38c905c62f6819bf67e098a2ec",
    "src/unicode_tables/script_extension.rs": "a314099ddbf50a07fe350bb0835bf2fe494ed5ad278b30e171e21506eb557906",
    "src/unicode_tables/sentence_break.rs": "be84fbe8c5c67e761b16fe6c27f16664dbb145357835cd6b92bc2a4a4c52ee79",
    "src/unicode_tables/word_break.rs": "c551681ad49ec28c7ae32bab1371945821c736ca8f0de410cb89f28066ec2ecf",
    "src/utf8.rs": "193f280f3b48116ed6ca8b5fe80b9d6401b6e733dfa138caf64ec1b017b6f175"
  },
  "base_inventory_sha256": {
    ".cargo_vcs_info.json": "2f238287522901ffeae2b64fe3ad7c340c0cb73cd48dfcf7d4140f6f9965f09b",
    "Cargo.toml": "362cfcf492ed93948ca374491d8dc3b688da0f90d00c2bb9a3892ec2ac27e276",
    "Cargo.toml.orig": "4c18da01da8ea5c747cb548eee8c34f935510be514b62869ff046f53aca37583",
    "LICENSE-APACHE": "a60eea817514531668d7e00765731449fe14d059d3249e0bc93b36de45f759f2",
    "LICENSE-MIT": "6485b8ed310d3f0340bf1ad1f47645069ce4069dcc6bb46c7d5c6faf41de1fdb",
    "README.md": "b2484aa7e66fb92d1378e9a7ce7605af18f77cb12c179866eaf92ba28cfec1d9",
    "benches/bench.rs": "d2b6ae5b939abd6093064f144b981b7739d7f474ec0698a1268052fc92406635",
    "src/ast/mod.rs": "21cda9fe9e1810b285cb8f8a2aa5eeaff6c38e256ceed036b68c66fb6b0124d6",
    "src/ast/parse.rs": "89a3701a9a95fea692be925e97b7dcfc5af1ac41f20e8f054eafaeb391e8dec2",
    "src/ast/print.rs": "99cb69ece252ef31e0be177fb3364797eb30b785f936532b8dcd8106e7be0738",
    "src/ast/visitor.rs": "f0fdf758801fe70e6b299b73ab63196e814af95ef6eccad7ef4f72075743fcf6",
    "src/debug.rs": "7a16cca02be9715fdc8c26a32279465774623cd12fab1ec59ac25a6e3047817f",
    "src/either.rs": "1758e3edd056884eccadd995708d1e374ba9aa65846bd0e13b1aae852607c560",
    "src/error.rs": "01a67e3407b0d0d869119363e47a94d92158834bfe5936366c2e3f6f4ed13f36",
    "src/hir/interval.rs": "74d75837d24ab9a3cff33b375b70694cdd3b9a4610c799137533f365755ba604",
    "src/hir/literal.rs": "6a8108b8919fbfd9ab93072846124c51d2998489810fcd6e7a89fdccc45833e0",
    "src/hir/mod.rs": "599ceb4921f2345a7d01d2390188fad13f236b98efe7a38c9beb9a0ce5c4ebad",
    "src/hir/print.rs": "ad51c515c933bfd67d307ba3d7e6ac59c9c5903b4f393a9f9a4785c92b88348d",
    "src/hir/translate.rs": "6129a12a686a6ec8965cdeb9f889640891da6aae75995606080fa88d4dd1a602",
    "src/hir/visitor.rs": "71ca9c93aa48a5ed445399659fa6455093a1bbd9ef44b66bc7095c1b08b2ec1f",
    "src/lib.rs": "5ae457d402e49443bdb23b71353693dd3b0d263b57a6eeb9eb5b5dae5c901bdd",
    "src/parser.rs": "6b2f4f27e3331a01a25b87c89368dd2e54396bd425dac57941f9c1ebfd238ac8",
    "src/rank.rs": "ff3d58b0cc5ffa69e2e8c56fc7d9ef41dd399d59a639a253a51551b858cb5bbd",
    "src/unicode.rs": "b2084dcbd4331501b9a895fd7e7575d93ff96eb661c6e6adbc8c66bb72685cde",
    "src/unicode_tables/LICENSE-UNICODE": "74db5baf44a41b1000312c673544b3374e4198af5605c7f9080a402cec42cfa3",
    "src/unicode_tables/age.rs": "71b7cf52acdb4aa98b44145303b8efbfa94913235493521941ef1e0092a0ffe2",
    "src/unicode_tables/case_folding_simple.rs": "7622c7f7f03ac0dc2f2bcd51c81a217d64de0cc912f62f1add5f676603a02456",
    "src/unicode_tables/general_category.rs": "9488e3721f7c2ae20e1b77fcff9a59b4ed8f22954b8645ea6d8592eac1856423",
    "src/unicode_tables/grapheme_cluster_break.rs": "0dd9d66bad598f4ec3451b6699f05c17c52079e37d463baf6385bbe51aa218f1",
    "src/unicode_tables/mod.rs": "26c837099cd934c8062e24bc9a0aaecf15fe1de03f9c6da3f3e1e5ac3ca24bee",
    "src/unicode_tables/perl_decimal.rs": "6a59143db81a0bcaf0e8d0af265e711d1a6472e1f091ee9ee4377da5d5d0cd1f",
    "src/unicode_tables/perl_space.rs": "ec9bb22ed7e99feef292249c7e6f4673ee0af9635d4d158f93923494c14cd5ed",
    "src/unicode_tables/perl_word.rs": "30f073baae28ea34c373c7778c00f20c1621c3e644404eff031f7d1cc8e9c9e2",
    "src/unicode_tables/property_bool.rs": "66cf5bd2a1438bf9694152f077a285cf014fbd50b9dd63a97233b2ea61d64962",
    "src/unicode_tables/property_names.rs": "8c93985d1bcb01735667a3c4cb92f7e260d267326bde9d7f048bc77cd7e07855",
    "src/unicode_tables/property_values.rs": "ef9131ce0a575c7327ec6d466aafd8b7c25600d80c232b5a4110bbf0a5a59136",
    "src/unicode_tables/script.rs": "41bd424f1e3a03290cf4995ced678dcf24c94b38c905c62f6819bf67e098a2ec",
    "src/unicode_tables/script_extension.rs": "a314099ddbf50a07fe350bb0835bf2fe494ed5ad278b30e171e21506eb557906",
    "src/unicode_tables/sentence_break.rs": "be84fbe8c5c67e761b16fe6c27f16664dbb145357835cd6b92bc2a4a4c52ee79",
    "src/unicode_tables/word_break.rs": "c551681ad49ec28c7ae32bab1371945821c736ca8f0de410cb89f28066ec2ecf",
    "src/utf8.rs": "33657f668361b6648d74c92d3d59eab97e3747d785760f47e4d71c13af07bfba",
    "test": "c7de5fbc0010d9b5b758cd49956375a64b88601c068167fd366808950257f108"
  },
  "independent_context_files": [],
  "external_interface_context": [],
  "dependency_interface_contracts": null,
  "full_diff_sha256": "9ca19fdd5c93f3d5dbe96d702b7bde575bdd34307121ef9f6267beaebc6b2bd2",
  "primary_report_sha256": "b4cb00c5d96f00ac6d7afac9827cfeb26dda1cd8ec058c893c05015d7790dfc0",
  "cross_report_sha256": "3da1905e4d801141ee9e083ea0fc22c20a860f6d62d3aade0e157faff32dec29"
}
```

Inventory hashes identify all archive members; only read-file entries and the complete reviewed delta claim review coverage. Source chain records retain original attribution; the new review is AI-assisted.

## Primary review

# regex-syntax 0.8.10 source review

Target archive SHA-256 `dc897dd8d9e8bd1ed8cdad82b5966c3e0ecae09fb1907d58efaa013543185d0a`, checked against frozen Cargo.lock and registry index. See `../provenance.json` for member safety, checksum inputs and complete inventory.
Required `safe-to-deploy`. Reviewer `Codex (GPT-6.1 Terra; AI-assisted source review)`.

Base `0.8.5` SHA-256 `2b15c43186be67a4fd63bee50d0303afffcef381492ebe2c5d87f324e1b8815c`; directed baseline certificate chain and source hashes in `../provenance.json`.
Complete reviewed diff: `../diffs/regex-syntax-0.8.5-to-0.8.10.diff` SHA-256 `9ca19fdd5c93f3d5dbe96d702b7bde575bdd34307121ef9f6267beaebc6b2bd2`.

## Read-file list (old SHA-256 -> new SHA-256)

- `.cargo_vcs_info.json` `2f238287522901ffeae2b64fe3ad7c340c0cb73cd48dfcf7d4140f6f9965f09b` -> `47a21948a4b18c4c1b2f8f9bf0ab57906a5dc38f7912a8e1b1df4c367a0a61eb`
- `Cargo.lock` `absent` -> `5cf8c2048f40caa65c267c6a60c9070efddbaf2722fc10e12b084a4d41a100f4`
- `Cargo.toml` `362cfcf492ed93948ca374491d8dc3b688da0f90d00c2bb9a3892ec2ac27e276` -> `3ae6ebabbe2e3e56f5499bc2a3ea4d268c0eb28fe11ff9d53bde41d18aaad03a`
- `Cargo.toml.orig` `4c18da01da8ea5c747cb548eee8c34f935510be514b62869ff046f53aca37583` -> `1138581b9b6d57b000926e72ed1d972776949740836913c754bb068e2372fe7a`
- `src/ast/mod.rs` `21cda9fe9e1810b285cb8f8a2aa5eeaff6c38e256ceed036b68c66fb6b0124d6` -> `38df06574a3816eae2796c757d9d268b7b42ce3fb1feee86750e7d04de7649c7`
- `src/ast/parse.rs` `89a3701a9a95fea692be925e97b7dcfc5af1ac41f20e8f054eafaeb391e8dec2` -> `af6d82e62e97379a91840e99c10be1279c0ae1298a0d8e5da429e9e7bc3ec339`
- `src/ast/print.rs` `99cb69ece252ef31e0be177fb3364797eb30b785f936532b8dcd8106e7be0738` -> `6a681ce021a384c47dda04c71e2868b5dd61c633bb5b1a67628b9a8df16f0413`
- `src/ast/visitor.rs` `f0fdf758801fe70e6b299b73ab63196e814af95ef6eccad7ef4f72075743fcf6` -> `2af2efd77727803b8d15a6244af92267e96edbfc1a211bcbea2b32e1b5483918`
- `src/debug.rs` `7a16cca02be9715fdc8c26a32279465774623cd12fab1ec59ac25a6e3047817f` -> `91b2492394de05bb11ee75329115b33fb378c19e076370d1a1ae665ce1682777`
- `src/error.rs` `01a67e3407b0d0d869119363e47a94d92158834bfe5936366c2e3f6f4ed13f36` -> `e308b3ccad0bea927f4e3957170302e9cfa743bfdf9376f3a5f4137b44ca6cfc`
- `src/hir/literal.rs` `6a8108b8919fbfd9ab93072846124c51d2998489810fcd6e7a89fdccc45833e0` -> `61e9f54103c671694dd017c23c5c9263e032735921ef77527940e83b29ced540`
- `src/hir/mod.rs` `599ceb4921f2345a7d01d2390188fad13f236b98efe7a38c9beb9a0ce5c4ebad` -> `13ee5b65fac1f2c9780ce48a500b1e9d198cb0bc07c0d7f4a4391aab87424563`
- `src/hir/print.rs` `ad51c515c933bfd67d307ba3d7e6ac59c9c5903b4f393a9f9a4785c92b88348d` -> `e1e1dfa71983c8fea64f500a0b9dfcbd258b4060e12b95d432468015a247a5cb`
- `src/hir/translate.rs` `6129a12a686a6ec8965cdeb9f889640891da6aae75995606080fa88d4dd1a602` -> `8c01783e74261a8562c2694394d94ccb8cb173274932731fa77118041fab4507`
- `src/lib.rs` `5ae457d402e49443bdb23b71353693dd3b0d263b57a6eeb9eb5b5dae5c901bdd` -> `c51d1e55a8b6c4608e21a278ed0ef9480f73ab5b814b6ca6127f4a049c4d5007`
- `src/utf8.rs` `33657f668361b6648d74c92d3d59eab97e3747d785760f47e4d71c13af07bfba` -> `193f280f3b48116ed6ca8b5fe80b9d6401b6e733dfa138caf64ec1b017b6f175`
- `test` `c7de5fbc0010d9b5b758cd49956375a64b88601c068167fd366808950257f108` -> `absent`

## Analysis

Full 0.8.5 -> 0.8.10 diff read including all 17 changed files, Cargo.lock, normalized/original manifests and removed test shell script. Google fixed-commit full 0.8.5 record explicitly contains safe-to-deploy; archive checksum/index and exact chain are in provenance.json. Rust `forbid(unsafe_code)` stays in place; no new build script, C/asm, proc-macro, binary, network, process or ambient filesystem operations are introduced by this delta. New lock records dev/optional arbitrary dependencies; manifest keeps existing optional arbitrary/derive and features, MSRV 1.65 and build=false; package include/autotarget and rustdoc cfg changes were inspected. The deleted shell test invoked Cargo and is not a build script.

Important behavior change: src/hir/translate.rs:1024-1054 replaces raw `ast_class.negated` with `is_negated()`. src/ast/mod.rs:887-917 shows `!=` flips the outer `\p`/`\P` sense. Thus `\p{gc!=Separator}` negates the queried class while `\P{gc!=Separator}` applies double negation; folding still precedes class negation at translate.rs:1154-1172. Updated tests at 2452-2494 exercise both; other changed Rust hunks are comments, error formatting or equivalent `format!` captures (including repetition and escape printing), not further parser boundary changes. Changes to utf8 and debug are likewise format-only. Source remains capable of input-proportional allocation/parsing inherited from base; this delta does not add network, filesystem or unsafe paths.

Small independent rustc 1.98.1 probe in `../probes/regex.rs` built source directly with std/unicode-gencat/unicode-case and confirmed equality of both double/single negations, inequality between them, and a case-insensitive double-negation case; output `unicode property negation cases passed`. This tests only these cases. Criteria: no serious security vulnerability identified in this delta, conditional on the authentic upstream base audit. Dependencies and full parser/source base are not re-audited; no exhaustive regex semantics, platform behavior or formal guarantee is claimed.

## Proposed delta audit (not registered)

```toml
[[audits.regex-syntax]]
who = "Codex (GPT-6.1 Terra; AI-assisted source review)"
criteria = "safe-to-deploy"
delta = "0.8.5 -> 0.8.10"
```


## Independent cross-review

src/ast/mod.rs:887-917 distinguishes syntactic \P from semantic negation: NotEqual flips that flag. src/hir/translate.rs:1024-1054 now passes is_negated to unicode_fold_and_negate, fixing \p{property!=value} and the double-negative \P case; :1154-1172 case-folds before complementing. The crate retains forbid(unsafe_code). Read all 17 changed file hunks, including manifests, lock and removed shell test. The fixed Google source records a full safe-to-deploy audit at 0.8.5; this proposal covers only 0.8.5 -> 0.8.10. Hostile regex resource exhaustion and whole-parser semantic correctness are not proven by the delta.

Static evidence is not a formal proof, exhaustive platform or concurrency execution, or a recursive certification of dependencies.
