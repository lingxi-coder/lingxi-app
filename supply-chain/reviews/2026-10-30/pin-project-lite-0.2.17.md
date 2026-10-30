# Source review: pin-project-lite 0.2.16 -> 0.2.17

Reviewer: Codex (GPT-6.1 Terra; AI-assisted source review). Independent cross-review: Codex AI-assisted.
Criterion: `safe-to-deploy`; scope: delta.
Frozen batch SHA-256: `aa0f8805f0e48d23f3eeae37a1e4097c9bdd761e30801099b4d1c3eded471b43`; frozen original Cargo.lock SHA-256: `2a60d89a9fb754475a33d8034227743a8bb5ff2e7410b064f5c122800360385d`.
This review applies to the frozen versions only; the final dependency graph can differ. Each dependency remains separately gated.

## Archive, registry and audit-chain evidence

```json
{
  "target": {
    "version": "0.2.17",
    "archive_sha256": "a89322df9ebe1c1578d689c92318e070967d1042b512afbe49518723f4e6d5cd",
    "index": [
      {
        "registry": "crates.io sparse index",
        "index_file_sha256": "602412dec1578068348640111fab9dc4b9f47e1f947463d6aa7bcd03f5b19020",
        "index_record_sha256": "9d6ec417aa7d919cdef13709faadc5d813b9fdafe8ff24f87fde0629423a3124",
        "cksum": "a89322df9ebe1c1578d689c92318e070967d1042b512afbe49518723f4e6d5cd",
        "yanked": false
      },
      {
        "registry": "crates.io sparse index",
        "index_file_sha256": "602412dec1578068348640111fab9dc4b9f47e1f947463d6aa7bcd03f5b19020",
        "index_record_sha256": "9d6ec417aa7d919cdef13709faadc5d813b9fdafe8ff24f87fde0629423a3124",
        "cksum": "a89322df9ebe1c1578d689c92318e070967d1042b512afbe49518723f4e6d5cd",
        "yanked": false
      }
    ]
  },
  "base": {
    "version": "0.2.16",
    "archive_sha256": "3b3cff922bd51709b605d9ead9aa71031d81447142d828eb4a6eba76fe619f9b",
    "index": [
      {
        "registry": "crates.io sparse index",
        "index_file_sha256": "602412dec1578068348640111fab9dc4b9f47e1f947463d6aa7bcd03f5b19020",
        "index_record_sha256": "c669a70079785ea273a731fe57e63f84bb3795572b3321327e64a9deec2eacac",
        "cksum": "3b3cff922bd51709b605d9ead9aa71031d81447142d828eb4a6eba76fe619f9b",
        "yanked": false
      },
      {
        "registry": "crates.io sparse index",
        "index_file_sha256": "602412dec1578068348640111fab9dc4b9f47e1f947463d6aa7bcd03f5b19020",
        "index_record_sha256": "c669a70079785ea273a731fe57e63f84bb3795572b3321327e64a9deec2eacac",
        "cksum": "3b3cff922bd51709b605d9ead9aa71031d81447142d828eb4a6eba76fe619f9b",
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
        "who": "David Koloski <dkoloski@google.com>",
        "criteria": [
          "ub-risk-1",
          "safe-to-deploy"
        ],
        "version": "0.2.9",
        "notes": "Reviewed on https://fxrev.dev/824504",
        "aggregated-from": "https://fuchsia.googlesource.com/fuchsia/+/refs/heads/main/third_party/rust_crates/supply-chain/audits.toml?format=TEXT"
      }
    },
    {
      "source": "google",
      "url": "https://raw.githubusercontent.com/google/rust-crate-audits/8cec794c38751495cf7a6c083220c207af1303d6/audits.toml",
      "source_sha256": "eb1c1bf1ee616471bfb56965eea325815ee41b5e3fcf5d12537dff2791160bbd",
      "original_record": {
        "who": "David Koloski <dkoloski@google.com>",
        "criteria": [
          "safe-to-deploy",
          "ub-risk-2"
        ],
        "delta": "0.2.9 -> 0.2.13",
        "notes": "Audited at https://fxrev.dev/946396",
        "aggregated-from": "https://fuchsia.googlesource.com/fuchsia/+/refs/heads/main/third_party/rust_crates/supply-chain/audits.toml?format=TEXT"
      }
    },
    {
      "source": "bytecode-alliance",
      "url": "https://raw.githubusercontent.com/bytecodealliance/wasmtime/04f0fcf70248909cc0b6a3ec1d406c7e2d543637/supply-chain/audits.toml",
      "source_sha256": "71f9f1e8a51d25708f2bb152f39e90a6bdc3d17d91d0f5df4cbc608cef465fd2",
      "original_record": {
        "who": "Alex Crichton <alex@alexcrichton.com>",
        "criteria": "safe-to-deploy",
        "delta": "0.2.13 -> 0.2.14",
        "notes": "No substantive changes in this update"
      }
    },
    {
      "source": "mozilla",
      "url": "https://raw.githubusercontent.com/mozilla/supply-chain/e6648794a1deb2821ddebd846564fd99f420b432/audits.toml",
      "source_sha256": "97ac6f1002cc4e32f9a71ce5e16d2962325bdf356519d66eb21be2dc4f2ce58d",
      "original_record": {
        "who": "Nika Layzell <nika@thelayzells.com>",
        "criteria": "safe-to-deploy",
        "delta": "0.2.14 -> 0.2.16",
        "notes": "Only functional change is to work around a bug in the negative_impls feature\n(https://github.com/taiki-e/pin-project/issues/340#issuecomment-2432146009)\n",
        "aggregated-from": "https://raw.githubusercontent.com/mozilla/cargo-vet/main/supply-chain/audits.toml"
      }
    }
  ],
  "read_files": [
    {
      "side": "base",
      "coverage": "all changed lines and context in full.diff",
      "path": ".cargo_vcs_info.json",
      "sha256": "83708687ea7d2dd0d5d7a1f95bf99e13297ba21e2de4347d8264b6e4bb8eddd3"
    },
    {
      "side": "target",
      "coverage": "all changed lines and context in full.diff",
      "path": ".cargo_vcs_info.json",
      "sha256": "8963794f9697aaefa78e18e84868db11ccb35ccf181bee63080b4b8288aca713"
    },
    {
      "side": "base",
      "coverage": "all changed lines and context in full.diff",
      "path": "CHANGELOG.md",
      "sha256": "ed6040767b0917706393024e79dec1461c1fe9984329915e416226c3701f2a57"
    },
    {
      "side": "target",
      "coverage": "all changed lines and context in full.diff",
      "path": "CHANGELOG.md",
      "sha256": "bc8ee542db08cd5c877ca45fb7266131d7da2be12271147d3596becec3825002"
    },
    {
      "side": "target",
      "coverage": "all changed lines and context in full.diff",
      "path": "Cargo.lock",
      "sha256": "da0e15b444ac71828ac931113ff6737f4d583ddad879a90a0171b211fb32840f"
    },
    {
      "side": "base",
      "coverage": "all changed lines and context in full.diff",
      "path": "Cargo.toml",
      "sha256": "21af3f4e9564a78293aa3e71bd146510a31c818c9cb78619b789c51dc45fa638"
    },
    {
      "side": "target",
      "coverage": "all changed lines and context in full.diff",
      "path": "Cargo.toml",
      "sha256": "473f9cfe9738986d0d1c46e306bd11ad3e36bca8f1eab5bbe9f7297313fb33f4"
    },
    {
      "side": "base",
      "coverage": "all changed lines and context in full.diff",
      "path": "Cargo.toml.orig",
      "sha256": "779f68957a09f40302f1d2a5a4c200773f871a882762ee6dd85325db5e9d2106"
    },
    {
      "side": "target",
      "coverage": "all changed lines and context in full.diff",
      "path": "Cargo.toml.orig",
      "sha256": "db145fe035b37a7965b03b7df78b5b4635a1bad60b13e76f87ec541667c3d143"
    },
    {
      "side": "base",
      "coverage": "all changed lines and context in full.diff",
      "path": "README.md",
      "sha256": "11841d1d294b58cb7c77b561fa5c3de1e71f32a347b73637e36f69ff32adf55b"
    },
    {
      "side": "target",
      "coverage": "all changed lines and context in full.diff",
      "path": "README.md",
      "sha256": "a6221fce50cc09c13b03a7973fb1d4dbf59ce7f46ca06830bbd46d201ed3b2d8"
    },
    {
      "side": "base",
      "coverage": "all changed lines and context in full.diff",
      "path": "src/lib.rs",
      "sha256": "56accfe9d4ea6f09162478aabebb055f75a94714bc5163c467a59cd18335845b"
    },
    {
      "side": "target",
      "coverage": "all changed lines and context in full.diff",
      "path": "src/lib.rs",
      "sha256": "09a84ff233021dabb75200ef71ebcb1c5a47c94484ce294ef5dcf43797873ba4"
    },
    {
      "side": "base",
      "coverage": "all changed lines and context in full.diff",
      "path": "tests/test.rs",
      "sha256": "37faf4f378b43c58f7aad09b05bcc520dcfd058be85c3bd0da93623139d48e01"
    },
    {
      "side": "target",
      "coverage": "all changed lines and context in full.diff",
      "path": "tests/test.rs",
      "sha256": "3bfff987a820770bd0126656a77871d5609f21c9c3b6825bd1606561aa37b572"
    },
    {
      "side": "base",
      "coverage": "all changed lines and context in full.diff",
      "path": "tests/ui/not_unpin/conflict-unpin.stderr",
      "sha256": "e6a6584c1958bc9e805188d207b39fe7d0cce27a7bdcfe2b96310f459cc9e874"
    },
    {
      "side": "target",
      "coverage": "all changed lines and context in full.diff",
      "path": "tests/ui/not_unpin/conflict-unpin.stderr",
      "sha256": "af2198329d947c0ce17478f84e2f9930eec2fbf7724fe3d498cddc649882afc6"
    },
    {
      "side": "base",
      "coverage": "all changed lines and context in full.diff",
      "path": "tests/ui/not_unpin/negative_impls_stable.stderr",
      "sha256": "c25e91a02bff728cebd90af12eb17c3ae9d269e767eaf88634bcb2bedf8c4cb3"
    },
    {
      "side": "target",
      "coverage": "all changed lines and context in full.diff",
      "path": "tests/ui/not_unpin/negative_impls_stable.stderr",
      "sha256": "3753d61c5ac847522b778a89000ed023ce69b5f9bf1078184e6f54a071f9854b"
    },
    {
      "side": "base",
      "coverage": "all changed lines and context in full.diff",
      "path": "tests/ui/pin_project/conflict-drop.stderr",
      "sha256": "3af45f260f413b2643d9a9ceec122babb61a6f488a79df697bed93c27aee0c06"
    },
    {
      "side": "target",
      "coverage": "all changed lines and context in full.diff",
      "path": "tests/ui/pin_project/conflict-drop.stderr",
      "sha256": "623613517ebc409a6b9f85c70f392610f7f04600719be3fe2393e3a7161c9c3a"
    },
    {
      "side": "base",
      "coverage": "all changed lines and context in full.diff",
      "path": "tests/ui/pin_project/conflict-unpin.stderr",
      "sha256": "ae6f740fc0330a0c69646c8715acea160a5ca5437970f7c606d5830313db6ed9"
    },
    {
      "side": "target",
      "coverage": "all changed lines and context in full.diff",
      "path": "tests/ui/pin_project/conflict-unpin.stderr",
      "sha256": "c927a3fb51a27faea9a622ac4389023a7bd2d12d91579977c42688efa3823f8b"
    },
    {
      "side": "base",
      "coverage": "all changed lines and context in full.diff",
      "path": "tests/ui/pin_project/invalid-bounds.stderr",
      "sha256": "f9e5e40a51afb62034246e214ac7655df2e5e45e44392ab171c2dee2151b9c05"
    },
    {
      "side": "target",
      "coverage": "all changed lines and context in full.diff",
      "path": "tests/ui/pin_project/invalid-bounds.stderr",
      "sha256": "2a666eda02fbd995399a7f835e97d46512d6d473585f6a65045a1ba245820d15"
    },
    {
      "side": "base",
      "coverage": "all changed lines and context in full.diff",
      "path": "tests/ui/pin_project/invalid.stderr",
      "sha256": "ea3fb689d43e5bde62a9620093acc2d17b67a63547638525ea17cc4b9f180bef"
    },
    {
      "side": "target",
      "coverage": "all changed lines and context in full.diff",
      "path": "tests/ui/pin_project/invalid.stderr",
      "sha256": "b6da005d80c9cd83075b767e03b2b7783c6ce1e7d91af2d2c8dc8b147e1984f9"
    },
    {
      "side": "base",
      "coverage": "all changed lines and context in full.diff",
      "path": "tests/ui/pin_project/negative_impls_stable.stderr",
      "sha256": "1834433559cb74b5158cefb27bee7b01fb9ba6eb57d82fbb3a2541c6205a7034"
    },
    {
      "side": "target",
      "coverage": "all changed lines and context in full.diff",
      "path": "tests/ui/pin_project/negative_impls_stable.stderr",
      "sha256": "ec2685a5d7b574a4b433c2f20f36097a14a080ad3965fe13aad461ba76584f8a"
    },
    {
      "side": "base",
      "coverage": "all changed lines and context in full.diff",
      "path": "tests/ui/pin_project/overlapping_lifetime_names.stderr",
      "sha256": "1df727ff5129e07c4c213352bc18dec9390a562726186596c07d4cc38a2f114a"
    },
    {
      "side": "target",
      "coverage": "all changed lines and context in full.diff",
      "path": "tests/ui/pin_project/overlapping_lifetime_names.stderr",
      "sha256": "a957ca87b4f6fceb65ffd53eb306ce0bcab49e8268a51a979477b9051d7c9c83"
    },
    {
      "side": "base",
      "coverage": "all changed lines and context in full.diff",
      "path": "tests/ui/pin_project/overlapping_unpin_struct.stderr",
      "sha256": "c0f2f178f779c9f0788fcf7ea697662f93fade1080cf7c014540d78aba01e100"
    },
    {
      "side": "target",
      "coverage": "all changed lines and context in full.diff",
      "path": "tests/ui/pin_project/overlapping_unpin_struct.stderr",
      "sha256": "5008d2ab8d761f307bf82c97555b5c25ade7a1fa3a9d2cb84b9840e3c8eff544"
    },
    {
      "side": "base",
      "coverage": "all changed lines and context in full.diff",
      "path": "tests/ui/pin_project/packed.stderr",
      "sha256": "bee0ce6ee7943d2e6e71f7e3b9bf2602516cfbf9b03d4c77d8140803f95639ff"
    },
    {
      "side": "target",
      "coverage": "all changed lines and context in full.diff",
      "path": "tests/ui/pin_project/packed.stderr",
      "sha256": "87b287980cc26fe0f086d3f6a733339c96da4fa6cec904e3005d16345c2fb08b"
    },
    {
      "side": "base",
      "coverage": "all changed lines and context in full.diff",
      "path": "tests/ui/pin_project/unpin_sneaky.stderr",
      "sha256": "d2b305d2ee117a4734390fbd6d9b9c6214443b61cdc15bef44ef6789bc66352a"
    },
    {
      "side": "target",
      "coverage": "all changed lines and context in full.diff",
      "path": "tests/ui/pin_project/unpin_sneaky.stderr",
      "sha256": "cc0b8a2b832d309e3ac73bdbdc369fbbb738ed20898ccff89b1f6fb2bd4fa301"
    },
    {
      "side": "base",
      "coverage": "all changed lines and context in full.diff",
      "path": "tests/ui/pinned_drop/call-drop-inner.stderr",
      "sha256": "d4a998b7f6a0378d179a9d32ecf45c5e20da5f25bce559a26e603bb785c6b9fb"
    },
    {
      "side": "target",
      "coverage": "all changed lines and context in full.diff",
      "path": "tests/ui/pinned_drop/call-drop-inner.stderr",
      "sha256": "ce3f7849c00ccfd31cfba66eda040402948c543e19bbae00e46509fe8ed3398f"
    },
    {
      "side": "base",
      "coverage": "all changed lines and context in full.diff",
      "path": "tests/ui/pinned_drop/conditional-drop-impl.stderr",
      "sha256": "0236b5e4bc14c550409c62cc4d5213467e53783e44ae5253bbafcbc9a1729d2b"
    },
    {
      "side": "target",
      "coverage": "all changed lines and context in full.diff",
      "path": "tests/ui/pinned_drop/conditional-drop-impl.stderr",
      "sha256": "fefd8098eacd9776ef3996c34caa39c0b77c47f216fb5b186f123580f8203f65"
    }
  ],
  "target_inventory_sha256": {
    ".cargo_vcs_info.json": "8963794f9697aaefa78e18e84868db11ccb35ccf181bee63080b4b8288aca713",
    "CHANGELOG.md": "bc8ee542db08cd5c877ca45fb7266131d7da2be12271147d3596becec3825002",
    "Cargo.lock": "da0e15b444ac71828ac931113ff6737f4d583ddad879a90a0171b211fb32840f",
    "Cargo.toml": "473f9cfe9738986d0d1c46e306bd11ad3e36bca8f1eab5bbe9f7297313fb33f4",
    "Cargo.toml.orig": "db145fe035b37a7965b03b7df78b5b4635a1bad60b13e76f87ec541667c3d143",
    "LICENSE-APACHE": "0d542e0c8804e39aa7f37eb00da5a762149dc682d7829451287e11b938e94594",
    "LICENSE-MIT": "23f18e03dc49df91622fe2a76176497404e46ced8a715d9d2b67a7446571cca3",
    "README.md": "a6221fce50cc09c13b03a7973fb1d4dbf59ce7f46ca06830bbd46d201ed3b2d8",
    "src/lib.rs": "09a84ff233021dabb75200ef71ebcb1c5a47c94484ce294ef5dcf43797873ba4",
    "tests/auxiliary/mod.rs": "52a78b77be05c67737c56865fbce4477eaacc6482d6377af2a5822f05a2ce061",
    "tests/compiletest.rs": "48cfd92c7271345d57a6c222a7a61e00449f38e6ba379130813c45b31a5589c9",
    "tests/drop_order.rs": "9b13b6461e44be1c1beca6ec47387de83efcb988326e4ff832183b8fad17a3f4",
    "tests/expand/.rustfmt.toml": "3025e466e14d8ac54e5371397434f79be4976b5bf3faf64e36c5e941980a0cf2",
    "tests/expand/default/enum.expanded.rs": "a0e001a28bd060f5091a977141eddfe169b2e1ebffe4b180e3af20b6837b4f02",
    "tests/expand/default/enum.rs": "ebe6b6403e35459420f6fe979243c382972d2d582376d69f6c630ab5ba2599b5",
    "tests/expand/default/struct.expanded.rs": "437332ba78e77ddf56431f897f2032994654a8d0a958bc97fb155072282c91ae",
    "tests/expand/default/struct.rs": "05767cd68ec5cddc481cff5ad677b5146f7d6fd1effcf1ff131d24c334d85032",
    "tests/expand/multifields/enum.expanded.rs": "f6ed5344acfb0ebcd55cbe0e9a70bedad50815e2f540729f1ca59bb33e77b463",
    "tests/expand/multifields/enum.rs": "d38eb301c86307edbf5cb07ed434b64a10a68aaef32dfb8b15029b5cc5586b0b",
    "tests/expand/multifields/struct.expanded.rs": "7257c732191d5dd697ddf7939d24f5600bda10cc72f80c92c1217b3209832dff",
    "tests/expand/multifields/struct.rs": "1fe2b911e8782eaac5952d9e0914473d46d82db45b1da859bdc6b8dc3978e549",
    "tests/expand/naming/enum-all.expanded.rs": "a0e001a28bd060f5091a977141eddfe169b2e1ebffe4b180e3af20b6837b4f02",
    "tests/expand/naming/enum-all.rs": "ebe6b6403e35459420f6fe979243c382972d2d582376d69f6c630ab5ba2599b5",
    "tests/expand/naming/enum-mut.expanded.rs": "72712b60b635d79ff6d53dcefac952335fc7b2c1befd245f122921b23a7bfa9e",
    "tests/expand/naming/enum-mut.rs": "bf215d055b29a7f9fdb84cabf34576d9dec8e87c33cebe2b553d1fe5e5cb0882",
    "tests/expand/naming/enum-none.expanded.rs": "6c00c1ab0d340fe72ea6c2fe502073ad74c6801250a6261d7cd3612a315ae798",
    "tests/expand/naming/enum-none.rs": "115968816e1a1a6b3634b8594dde4815d4641d817d69ebdd90879b96a8a2808e",
    "tests/expand/naming/enum-ref.expanded.rs": "fa708877f09333c49c6fc5c101ff0d5c70a1372f7ee6d4830939ce5ff197ac48",
    "tests/expand/naming/enum-ref.rs": "e09ce2ca812b9fcf7dc59b486e5b50b84db08860a2ceda63ba094d7351bb391f",
    "tests/expand/naming/struct-all.expanded.rs": "fdfdb4d9c696ff9103cdbdea350dbe0dd2b40d5680e19295747bb167f3791b94",
    "tests/expand/naming/struct-all.rs": "0d6b64bc687fbc19329a5558461329e3d2cfa367ff8adb7be76efe2d3160d33e",
    "tests/expand/naming/struct-mut.expanded.rs": "d7f0c80f787de540c97faed814c51efd66d08b0e5bb8db3185a923f1bfdbf19f",
    "tests/expand/naming/struct-mut.rs": "36edd8db816c122c80ccb139b4305cad8edfdf9b7ee4aebacafa1180613a5075",
    "tests/expand/naming/struct-none.expanded.rs": "437332ba78e77ddf56431f897f2032994654a8d0a958bc97fb155072282c91ae",
    "tests/expand/naming/struct-none.rs": "05767cd68ec5cddc481cff5ad677b5146f7d6fd1effcf1ff131d24c334d85032",
    "tests/expand/naming/struct-ref.expanded.rs": "8c5d5efc6f18f08c84c977d8b4e8f2e3505a75a198c418cf0c60cd7b51d2024b",
    "tests/expand/naming/struct-ref.rs": "dd249875f962d16dd9ec821acf0c2b5c1ed0ed05569f6e874ccd5caca2759acf",
    "tests/expand/not_unpin/enum.expanded.rs": "1c2dde8ff53bff0716a490ca6dc2fed351d4db1ef7bbd4cf40d48b8f7d305839",
    "tests/expand/not_unpin/enum.rs": "c65aac4d2083e83d2dfc3bec8ce243e3e9169ef5f986f077f0a8b4226d39f7c3",
    "tests/expand/not_unpin/struct.expanded.rs": "3ba32743c7578aad45354b59e1ad2d9dcb64a753f34ad25431c529b436260e15",
    "tests/expand/not_unpin/struct.rs": "99d77727393e4a050d2ceb99710172dfdd13dfcb6e6416f63de39e5df3a9f03f",
    "tests/expand/pinned_drop/enum.expanded.rs": "8776e664d0af9102c6af6f41dfb3e6cd8c69a5cb4fb3fd8ed8fb0aaca95dc711",
    "tests/expand/pinned_drop/enum.rs": "8f590791cb4693bb077c562f0d93128f9452e6a5f60c631b5fd624a221883ebd",
    "tests/expand/pinned_drop/struct.expanded.rs": "9643f7d71b94675a71c7c0ff8d8a3b1e3f716bb1f7c0efdea5c6c74b76274d2b",
    "tests/expand/pinned_drop/struct.rs": "8da271d7d3182b33be297d985a1e3b57d6067406db21eca8ac2c448ef1a0869a",
    "tests/expand/pub/enum.expanded.rs": "c52cfaa064ffbf04821386180d3ddf8a57a54012ece5114114f4ae4c7215d93f",
    "tests/expand/pub/enum.rs": "fdbd806e92cfad4e67737d1f782544aa4e3116bae702161c43b5c22bfbf3a208",
    "tests/expand/pub/struct.expanded.rs": "9e84c170b1e6505ffed33d66c976c44338cffa35838ddaef52c3598dec7cfe55",
    "tests/expand/pub/struct.rs": "05cb3f03ef9801fecf0a7c86bbd236c97218b34ac01ee507cb7de9a78efaeffa",
    "tests/expandtest.rs": "a4336ab879d79824ab3144ea385a7a00ba77604c3b6639ae53b6283850b421df",
    "tests/include/basic.rs": "8c06c765c92d696284dde5b4762488100ce0ee1661ead307023a85d5c82d8ddc",
    "tests/proper_unpin.rs": "83790eb38ae2fd5aed97fc67da79d91c1071f08d1e5aaef0c71e65d8b5767403",
    "tests/test.rs": "3bfff987a820770bd0126656a77871d5609f21c9c3b6825bd1606561aa37b572",
    "tests/ui/not_unpin/conflict-unpin.rs": "23c7e081d86f2000838c32844dce1eee98d1b118eb1a29daf65c705aad5c68fb",
    "tests/ui/not_unpin/conflict-unpin.stderr": "af2198329d947c0ce17478f84e2f9930eec2fbf7724fe3d498cddc649882afc6",
    "tests/ui/not_unpin/negative_impls_stable.rs": "f014d5398d505ca5f482905cb9597a2d5fced563b9350229468c3751dd7f92ae",
    "tests/ui/not_unpin/negative_impls_stable.stderr": "3753d61c5ac847522b778a89000ed023ce69b5f9bf1078184e6f54a071f9854b",
    "tests/ui/pin_project/conflict-drop.rs": "a1964fa136080cc354479ff55b8428e5fa79305681fb2475cc37d6071cd64112",
    "tests/ui/pin_project/conflict-drop.stderr": "623613517ebc409a6b9f85c70f392610f7f04600719be3fe2393e3a7161c9c3a",
    "tests/ui/pin_project/conflict-unpin.rs": "37e7b2f9ab1be70d0521c9dbbc954067f22a2321511e008f0f72e13d0edb4afe",
    "tests/ui/pin_project/conflict-unpin.stderr": "c927a3fb51a27faea9a622ac4389023a7bd2d12d91579977c42688efa3823f8b",
    "tests/ui/pin_project/invalid-bounds.rs": "4068e272a4f5d44851e0901dd6cfa806ead000c2e1a507a2b42be51d26d17056",
    "tests/ui/pin_project/invalid-bounds.stderr": "2a666eda02fbd995399a7f835e97d46512d6d473585f6a65045a1ba245820d15",
    "tests/ui/pin_project/invalid.rs": "1c84cc089623dd31f483011ed6543f169af341b028a212fcf00146bc96dd792f",
    "tests/ui/pin_project/invalid.stderr": "b6da005d80c9cd83075b767e03b2b7783c6ce1e7d91af2d2c8dc8b147e1984f9",
    "tests/ui/pin_project/negative_impls_stable.rs": "1e70055fbec4bc0a53454fd93a0663f8785440d37dcf359ab29958d97fd51e9d",
    "tests/ui/pin_project/negative_impls_stable.stderr": "ec2685a5d7b574a4b433c2f20f36097a14a080ad3965fe13aad461ba76584f8a",
    "tests/ui/pin_project/overlapping_lifetime_names.rs": "ac9c29b389e8e067e9e384cf04be69c0e5bb862cf373de0e3b4296fe2007809b",
    "tests/ui/pin_project/overlapping_lifetime_names.stderr": "a957ca87b4f6fceb65ffd53eb306ce0bcab49e8268a51a979477b9051d7c9c83",
    "tests/ui/pin_project/overlapping_unpin_struct.rs": "cb3d6d4087b8b4c917bc71e55764775f6bb018a77cda5c6b726afacd66789859",
    "tests/ui/pin_project/overlapping_unpin_struct.stderr": "5008d2ab8d761f307bf82c97555b5c25ade7a1fa3a9d2cb84b9840e3c8eff544",
    "tests/ui/pin_project/packed.rs": "6a24ac14e61e5d3e3b495f49d226318ec500e5aa46f34acf8e4f3e0827bd9604",
    "tests/ui/pin_project/packed.stderr": "87b287980cc26fe0f086d3f6a733339c96da4fa6cec904e3005d16345c2fb08b",
    "tests/ui/pin_project/unpin_sneaky.rs": "6e6385e067679c3bf5233163b3fa99ff2fc28bcb93fe594a5c987c0db7c0eaa6",
    "tests/ui/pin_project/unpin_sneaky.stderr": "cc0b8a2b832d309e3ac73bdbdc369fbbb738ed20898ccff89b1f6fb2bd4fa301",
    "tests/ui/pin_project/unsupported.rs": "3ad5f584f9e56be3573133fba026a7bb378595d0a4b7b1bcbb6b78e37cef20a7",
    "tests/ui/pin_project/unsupported.stderr": "93433fdb5c0542036a5a44f25da885d0c1ae78ad84094b924debe7af4cd8b73b",
    "tests/ui/pinned_drop/call-drop-inner.rs": "0618b60a450c99b5953b302e26c68df1abe7632af3d9baf3639eb60091e4ff4e",
    "tests/ui/pinned_drop/call-drop-inner.stderr": "ce3f7849c00ccfd31cfba66eda040402948c543e19bbae00e46509fe8ed3398f",
    "tests/ui/pinned_drop/conditional-drop-impl.rs": "df03f38b2a331f339af2734fdd4ad2f637b2e2189d9928599adebe04ebee517f",
    "tests/ui/pinned_drop/conditional-drop-impl.stderr": "fefd8098eacd9776ef3996c34caa39c0b77c47f216fb5b186f123580f8203f65"
  },
  "base_inventory_sha256": {
    ".cargo_vcs_info.json": "83708687ea7d2dd0d5d7a1f95bf99e13297ba21e2de4347d8264b6e4bb8eddd3",
    "CHANGELOG.md": "ed6040767b0917706393024e79dec1461c1fe9984329915e416226c3701f2a57",
    "Cargo.toml": "21af3f4e9564a78293aa3e71bd146510a31c818c9cb78619b789c51dc45fa638",
    "Cargo.toml.orig": "779f68957a09f40302f1d2a5a4c200773f871a882762ee6dd85325db5e9d2106",
    "LICENSE-APACHE": "0d542e0c8804e39aa7f37eb00da5a762149dc682d7829451287e11b938e94594",
    "LICENSE-MIT": "23f18e03dc49df91622fe2a76176497404e46ced8a715d9d2b67a7446571cca3",
    "README.md": "11841d1d294b58cb7c77b561fa5c3de1e71f32a347b73637e36f69ff32adf55b",
    "src/lib.rs": "56accfe9d4ea6f09162478aabebb055f75a94714bc5163c467a59cd18335845b",
    "tests/auxiliary/mod.rs": "52a78b77be05c67737c56865fbce4477eaacc6482d6377af2a5822f05a2ce061",
    "tests/compiletest.rs": "48cfd92c7271345d57a6c222a7a61e00449f38e6ba379130813c45b31a5589c9",
    "tests/drop_order.rs": "9b13b6461e44be1c1beca6ec47387de83efcb988326e4ff832183b8fad17a3f4",
    "tests/expand/.rustfmt.toml": "3025e466e14d8ac54e5371397434f79be4976b5bf3faf64e36c5e941980a0cf2",
    "tests/expand/default/enum.expanded.rs": "a0e001a28bd060f5091a977141eddfe169b2e1ebffe4b180e3af20b6837b4f02",
    "tests/expand/default/enum.rs": "ebe6b6403e35459420f6fe979243c382972d2d582376d69f6c630ab5ba2599b5",
    "tests/expand/default/struct.expanded.rs": "437332ba78e77ddf56431f897f2032994654a8d0a958bc97fb155072282c91ae",
    "tests/expand/default/struct.rs": "05767cd68ec5cddc481cff5ad677b5146f7d6fd1effcf1ff131d24c334d85032",
    "tests/expand/multifields/enum.expanded.rs": "f6ed5344acfb0ebcd55cbe0e9a70bedad50815e2f540729f1ca59bb33e77b463",
    "tests/expand/multifields/enum.rs": "d38eb301c86307edbf5cb07ed434b64a10a68aaef32dfb8b15029b5cc5586b0b",
    "tests/expand/multifields/struct.expanded.rs": "7257c732191d5dd697ddf7939d24f5600bda10cc72f80c92c1217b3209832dff",
    "tests/expand/multifields/struct.rs": "1fe2b911e8782eaac5952d9e0914473d46d82db45b1da859bdc6b8dc3978e549",
    "tests/expand/naming/enum-all.expanded.rs": "a0e001a28bd060f5091a977141eddfe169b2e1ebffe4b180e3af20b6837b4f02",
    "tests/expand/naming/enum-all.rs": "ebe6b6403e35459420f6fe979243c382972d2d582376d69f6c630ab5ba2599b5",
    "tests/expand/naming/enum-mut.expanded.rs": "72712b60b635d79ff6d53dcefac952335fc7b2c1befd245f122921b23a7bfa9e",
    "tests/expand/naming/enum-mut.rs": "bf215d055b29a7f9fdb84cabf34576d9dec8e87c33cebe2b553d1fe5e5cb0882",
    "tests/expand/naming/enum-none.expanded.rs": "6c00c1ab0d340fe72ea6c2fe502073ad74c6801250a6261d7cd3612a315ae798",
    "tests/expand/naming/enum-none.rs": "115968816e1a1a6b3634b8594dde4815d4641d817d69ebdd90879b96a8a2808e",
    "tests/expand/naming/enum-ref.expanded.rs": "fa708877f09333c49c6fc5c101ff0d5c70a1372f7ee6d4830939ce5ff197ac48",
    "tests/expand/naming/enum-ref.rs": "e09ce2ca812b9fcf7dc59b486e5b50b84db08860a2ceda63ba094d7351bb391f",
    "tests/expand/naming/struct-all.expanded.rs": "fdfdb4d9c696ff9103cdbdea350dbe0dd2b40d5680e19295747bb167f3791b94",
    "tests/expand/naming/struct-all.rs": "0d6b64bc687fbc19329a5558461329e3d2cfa367ff8adb7be76efe2d3160d33e",
    "tests/expand/naming/struct-mut.expanded.rs": "d7f0c80f787de540c97faed814c51efd66d08b0e5bb8db3185a923f1bfdbf19f",
    "tests/expand/naming/struct-mut.rs": "36edd8db816c122c80ccb139b4305cad8edfdf9b7ee4aebacafa1180613a5075",
    "tests/expand/naming/struct-none.expanded.rs": "437332ba78e77ddf56431f897f2032994654a8d0a958bc97fb155072282c91ae",
    "tests/expand/naming/struct-none.rs": "05767cd68ec5cddc481cff5ad677b5146f7d6fd1effcf1ff131d24c334d85032",
    "tests/expand/naming/struct-ref.expanded.rs": "8c5d5efc6f18f08c84c977d8b4e8f2e3505a75a198c418cf0c60cd7b51d2024b",
    "tests/expand/naming/struct-ref.rs": "dd249875f962d16dd9ec821acf0c2b5c1ed0ed05569f6e874ccd5caca2759acf",
    "tests/expand/not_unpin/enum.expanded.rs": "1c2dde8ff53bff0716a490ca6dc2fed351d4db1ef7bbd4cf40d48b8f7d305839",
    "tests/expand/not_unpin/enum.rs": "c65aac4d2083e83d2dfc3bec8ce243e3e9169ef5f986f077f0a8b4226d39f7c3",
    "tests/expand/not_unpin/struct.expanded.rs": "3ba32743c7578aad45354b59e1ad2d9dcb64a753f34ad25431c529b436260e15",
    "tests/expand/not_unpin/struct.rs": "99d77727393e4a050d2ceb99710172dfdd13dfcb6e6416f63de39e5df3a9f03f",
    "tests/expand/pinned_drop/enum.expanded.rs": "8776e664d0af9102c6af6f41dfb3e6cd8c69a5cb4fb3fd8ed8fb0aaca95dc711",
    "tests/expand/pinned_drop/enum.rs": "8f590791cb4693bb077c562f0d93128f9452e6a5f60c631b5fd624a221883ebd",
    "tests/expand/pinned_drop/struct.expanded.rs": "9643f7d71b94675a71c7c0ff8d8a3b1e3f716bb1f7c0efdea5c6c74b76274d2b",
    "tests/expand/pinned_drop/struct.rs": "8da271d7d3182b33be297d985a1e3b57d6067406db21eca8ac2c448ef1a0869a",
    "tests/expand/pub/enum.expanded.rs": "c52cfaa064ffbf04821386180d3ddf8a57a54012ece5114114f4ae4c7215d93f",
    "tests/expand/pub/enum.rs": "fdbd806e92cfad4e67737d1f782544aa4e3116bae702161c43b5c22bfbf3a208",
    "tests/expand/pub/struct.expanded.rs": "9e84c170b1e6505ffed33d66c976c44338cffa35838ddaef52c3598dec7cfe55",
    "tests/expand/pub/struct.rs": "05cb3f03ef9801fecf0a7c86bbd236c97218b34ac01ee507cb7de9a78efaeffa",
    "tests/expandtest.rs": "a4336ab879d79824ab3144ea385a7a00ba77604c3b6639ae53b6283850b421df",
    "tests/include/basic.rs": "8c06c765c92d696284dde5b4762488100ce0ee1661ead307023a85d5c82d8ddc",
    "tests/proper_unpin.rs": "83790eb38ae2fd5aed97fc67da79d91c1071f08d1e5aaef0c71e65d8b5767403",
    "tests/test.rs": "37faf4f378b43c58f7aad09b05bcc520dcfd058be85c3bd0da93623139d48e01",
    "tests/ui/not_unpin/conflict-unpin.rs": "23c7e081d86f2000838c32844dce1eee98d1b118eb1a29daf65c705aad5c68fb",
    "tests/ui/not_unpin/conflict-unpin.stderr": "e6a6584c1958bc9e805188d207b39fe7d0cce27a7bdcfe2b96310f459cc9e874",
    "tests/ui/not_unpin/negative_impls_stable.rs": "f014d5398d505ca5f482905cb9597a2d5fced563b9350229468c3751dd7f92ae",
    "tests/ui/not_unpin/negative_impls_stable.stderr": "c25e91a02bff728cebd90af12eb17c3ae9d269e767eaf88634bcb2bedf8c4cb3",
    "tests/ui/pin_project/conflict-drop.rs": "a1964fa136080cc354479ff55b8428e5fa79305681fb2475cc37d6071cd64112",
    "tests/ui/pin_project/conflict-drop.stderr": "3af45f260f413b2643d9a9ceec122babb61a6f488a79df697bed93c27aee0c06",
    "tests/ui/pin_project/conflict-unpin.rs": "37e7b2f9ab1be70d0521c9dbbc954067f22a2321511e008f0f72e13d0edb4afe",
    "tests/ui/pin_project/conflict-unpin.stderr": "ae6f740fc0330a0c69646c8715acea160a5ca5437970f7c606d5830313db6ed9",
    "tests/ui/pin_project/invalid-bounds.rs": "4068e272a4f5d44851e0901dd6cfa806ead000c2e1a507a2b42be51d26d17056",
    "tests/ui/pin_project/invalid-bounds.stderr": "f9e5e40a51afb62034246e214ac7655df2e5e45e44392ab171c2dee2151b9c05",
    "tests/ui/pin_project/invalid.rs": "1c84cc089623dd31f483011ed6543f169af341b028a212fcf00146bc96dd792f",
    "tests/ui/pin_project/invalid.stderr": "ea3fb689d43e5bde62a9620093acc2d17b67a63547638525ea17cc4b9f180bef",
    "tests/ui/pin_project/negative_impls_stable.rs": "1e70055fbec4bc0a53454fd93a0663f8785440d37dcf359ab29958d97fd51e9d",
    "tests/ui/pin_project/negative_impls_stable.stderr": "1834433559cb74b5158cefb27bee7b01fb9ba6eb57d82fbb3a2541c6205a7034",
    "tests/ui/pin_project/overlapping_lifetime_names.rs": "ac9c29b389e8e067e9e384cf04be69c0e5bb862cf373de0e3b4296fe2007809b",
    "tests/ui/pin_project/overlapping_lifetime_names.stderr": "1df727ff5129e07c4c213352bc18dec9390a562726186596c07d4cc38a2f114a",
    "tests/ui/pin_project/overlapping_unpin_struct.rs": "cb3d6d4087b8b4c917bc71e55764775f6bb018a77cda5c6b726afacd66789859",
    "tests/ui/pin_project/overlapping_unpin_struct.stderr": "c0f2f178f779c9f0788fcf7ea697662f93fade1080cf7c014540d78aba01e100",
    "tests/ui/pin_project/packed.rs": "6a24ac14e61e5d3e3b495f49d226318ec500e5aa46f34acf8e4f3e0827bd9604",
    "tests/ui/pin_project/packed.stderr": "bee0ce6ee7943d2e6e71f7e3b9bf2602516cfbf9b03d4c77d8140803f95639ff",
    "tests/ui/pin_project/unpin_sneaky.rs": "6e6385e067679c3bf5233163b3fa99ff2fc28bcb93fe594a5c987c0db7c0eaa6",
    "tests/ui/pin_project/unpin_sneaky.stderr": "d2b305d2ee117a4734390fbd6d9b9c6214443b61cdc15bef44ef6789bc66352a",
    "tests/ui/pin_project/unsupported.rs": "3ad5f584f9e56be3573133fba026a7bb378595d0a4b7b1bcbb6b78e37cef20a7",
    "tests/ui/pin_project/unsupported.stderr": "93433fdb5c0542036a5a44f25da885d0c1ae78ad84094b924debe7af4cd8b73b",
    "tests/ui/pinned_drop/call-drop-inner.rs": "0618b60a450c99b5953b302e26c68df1abe7632af3d9baf3639eb60091e4ff4e",
    "tests/ui/pinned_drop/call-drop-inner.stderr": "d4a998b7f6a0378d179a9d32ecf45c5e20da5f25bce559a26e603bb785c6b9fb",
    "tests/ui/pinned_drop/conditional-drop-impl.rs": "df03f38b2a331f339af2734fdd4ad2f637b2e2189d9928599adebe04ebee517f",
    "tests/ui/pinned_drop/conditional-drop-impl.stderr": "0236b5e4bc14c550409c62cc4d5213467e53783e44ae5253bbafcbc9a1729d2b"
  },
  "independent_context_files": [
    {
      "path": "validation/gates-fix-20261030/source-audits/lane-b-001/scratch/pin-project-lite-0.2.17/target/Cargo.toml",
      "sha256": "473f9cfe9738986d0d1c46e306bd11ad3e36bca8f1eab5bbe9f7297313fb33f4",
      "inspection": "relevant lines/byte arrays and changed regions"
    },
    {
      "path": "validation/gates-fix-20261030/source-audits/lane-b-001/scratch/pin-project-lite-0.2.17/target/Cargo.toml.orig",
      "sha256": "db145fe035b37a7965b03b7df78b5b4635a1bad60b13e76f87ec541667c3d143",
      "inspection": "relevant lines/byte arrays and changed regions"
    },
    {
      "path": "validation/gates-fix-20261030/source-audits/lane-b-001/scratch/pin-project-lite-0.2.17/target/src/lib.rs",
      "sha256": "09a84ff233021dabb75200ef71ebcb1c5a47c94484ce294ef5dcf43797873ba4",
      "inspection": "changed prefix inspected; remaining macro implementation compared byte-for-byte against certified base"
    },
    {
      "path": "validation/gates-fix-20261030/source-audits/lane-b-001/pin-project-lite-0.2.17/full.diff",
      "sha256": "b91ae3934e45723c7a8e91e73b19d792ed2b008ef3f20d0360b6b566cc7dd882",
      "inspection": "all changed hunks and resources; independently reconstructed against archive bytes"
    },
    {
      "path": "validation/gates-fix-20261030/source-audits/lane-b-001/pin-project-lite-0.2.17/read-files.json",
      "sha256": "3170a8c48069aa3ab2e55a038dd0ba654fb50769be8cd935d1cc1d38e3f66834",
      "inspection": "independently checked against archive and changed-member coverage"
    }
  ],
  "external_interface_context": [],
  "dependency_interface_contracts": null,
  "full_diff_sha256": "b91ae3934e45723c7a8e91e73b19d792ed2b008ef3f20d0360b6b566cc7dd882",
  "primary_report_sha256": "4574d8406476b6901c090a0a024a06656916a149c617573a28e1c3b4e82fb31b",
  "cross_report_sha256": "d06dadbc06d6ef08ce1028464b9e9ba8b5df27653e60140f8225cfe7933a0165"
}
```

Inventory hashes identify all archive members; only read-file entries and the complete reviewed delta claim review coverage. Source chain records retain original attribution; the new review is AI-assisted.

## Primary review

# pin-project-lite-0.2.17: delta 0.2.16 → 0.2.17

Reviewer: Codex (GPT-6.1 Terra; AI-assisted source review)

Base chain: google:0.2.9 → google:0.2.9 -> 0.2.13 → bytecode-alliance:0.2.13 -> 0.2.14 → mozilla:0.2.14 -> 0.2.16. `inventory.json` preserves every raw original record, source, and archive/index checksum; `preflight.json` records source commit, blob, SHA-256 and frozen lock. Each record explicitly contains safe-to-deploy and the path is contiguous.

- `src/lib.rs:1-384` changes comments, rustdoc code fences, and doctest lint attributes; the entire suffix from the first `#[macro_export]` at target line 385 to end is byte-identical to the base (base line 381), so macro token definitions including unsafe pinned projections are unchanged. This relies on the independently verified upstream base audit, not a fresh full proof of the ~5,600-line macro.
- `tests/test.rs:5-7,447-454` changes imports from core to std for test and removes redundant trait-object parentheses. The 14 changed UI `.stderr` files are diagnostic wording/spacing/locations; `invalid.stderr` removes the secondary missing-attribute diagnostic but preserves the primary parse failure. They are test expectations, not compiled production code. Full diff preserves each hunk, including every test resource; no new binary.
- `Cargo.toml:12-26` retains edition 2018, MSRV 1.37 and build=false; other manifest changes are docs/lints and version. Added package Cargo.lock contains dev dependencies, not a new production dependency declaration. No changed manifest/build path makes runtime code or executable macro expansion differ. No C/asm, FFI or system command is added by this delta.

All changed hunks of every added/deleted/changed text file were read in `full.diff` (SHA-256 `b91ae3934e45723c7a8e91e73b19d792ed2b008ef3f20d0360b6b566cc7dd882`), with six lines of context; `read-files.json` lists both versions and their hashes. Unchanged files rely on the cited base. The target and base archives have no binary members. No formal proof, platform execution or recursive dependency audit is asserted.

Proposed record for independent review (not registered):

```toml
[[audits.pin-project-lite]]
who = "Codex (GPT-6.1 Terra; AI-assisted source review)"
criteria = "safe-to-deploy"
delta = "0.2.16 -> 0.2.17"
```


## Independent cross-review

The generated projection implementation beginning at the first #[macro_export] in target src/lib.rs:385 is byte-for-byte identical to base 0.2.16 (suffix SHA-256 d8224073fa1bf38c7bd3c9434fabe7d8d9e45366d2d70138e02c88d4fece00a3). Earlier changes are documentation and crate/test lint settings; Cargo.toml retains edition 2018, MSRV 1.37, no runtime dependency or build script. tests/test.rs and stderr snapshots change test imports, syntax and compiler diagnostic text, including one removed secondary error. The contiguous google → Bytecode Alliance → Mozilla safe-to-deploy chain ends at 0.2.16; it supplies the unsafe projection baseline.

Static evidence is not a formal proof, exhaustive platform or concurrency execution, or a recursive certification of dependencies.
