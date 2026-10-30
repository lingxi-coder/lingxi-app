# Source review: home 0.5.11 -> 0.5.9

Reviewer: Codex (GPT-6.1 Terra; AI-assisted source review). Independent cross-review: Codex AI-assisted.
Criterion: `safe-to-deploy`; scope: delta.
Frozen batch SHA-256: `aa0f8805f0e48d23f3eeae37a1e4097c9bdd761e30801099b4d1c3eded471b43`; frozen original Cargo.lock SHA-256: `2a60d89a9fb754475a33d8034227743a8bb5ff2e7410b064f5c122800360385d`.
This review applies to the frozen versions only; the final dependency graph can differ. Each dependency remains separately gated.

## Archive, registry and audit-chain evidence

```json
{
  "target": {
    "version": "0.5.9",
    "archive_sha256": "e3d1354bf6b7235cb4a0576c2619fd4ed18183f689b12b006a0ee7329eeff9a5",
    "index": [
      {
        "registry": "crates.io sparse index",
        "index_file_sha256": "0676a7f6e096ee577b1793dc094623114d8bb4da705b5b3d41f34df5d42f7a64",
        "index_record_sha256": "ccd52d79de20847cfc0116247fc830af22b39878191fe3a2e217ccabb3e0201c",
        "cksum": "e3d1354bf6b7235cb4a0576c2619fd4ed18183f689b12b006a0ee7329eeff9a5",
        "yanked": false
      }
    ]
  },
  "base": {
    "version": "0.5.11",
    "archive_sha256": "589533453244b0995c858700322199b2becb13b627df2851f64a2775d024abcf",
    "index": [
      {
        "registry": "crates.io sparse index",
        "index_file_sha256": "0676a7f6e096ee577b1793dc094623114d8bb4da705b5b3d41f34df5d42f7a64",
        "index_record_sha256": "2e8cacd2b223c82bfd073b3811e5b91a1c40893e17390a0fdf4218e09109775c",
        "cksum": "589533453244b0995c858700322199b2becb13b627df2851f64a2775d024abcf",
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
        "who": "Nika Layzell <nika@thelayzells.com>",
        "criteria": "safe-to-deploy",
        "version": "0.5.3",
        "notes": "Crate with straightforward code for determining the user's HOME directory. Only\nunsafe code is used to invoke the Windows SHGetFolderPathW API to get the\nprofile directory when the USERPROFILE environment variable is unavailable.\n",
        "aggregated-from": "https://raw.githubusercontent.com/mozilla/cargo-vet/main/supply-chain/audits.toml"
      }
    },
    {
      "source": "mozilla",
      "url": "https://raw.githubusercontent.com/mozilla/supply-chain/e6648794a1deb2821ddebd846564fd99f420b432/audits.toml",
      "source_sha256": "97ac6f1002cc4e32f9a71ce5e16d2962325bdf356519d66eb21be2dc4f2ce58d",
      "original_record": {
        "who": "Nika Layzell <nika@thelayzells.com>",
        "criteria": "safe-to-deploy",
        "delta": "0.5.3 -> 0.5.11",
        "aggregated-from": "https://raw.githubusercontent.com/mozilla/cargo-vet/main/supply-chain/audits.toml"
      }
    }
  ],
  "read_files": [
    {
      "role": "baseline",
      "path": ".cargo_vcs_info.json",
      "sha256": "6971b7e7731692a90dd71554856daf500375495499ec004d8ef7320a5046c479",
      "scope": "whole changed file; base reconstructed from target plus full delta where appropriate"
    },
    {
      "role": "baseline",
      "path": "Cargo.lock",
      "sha256": "e5f8eca738cf89382f3abb414866e7adfa911d3e19d6acb94616715fe87d0aa0",
      "scope": "whole changed file; base reconstructed from target plus full delta where appropriate"
    },
    {
      "role": "baseline",
      "path": "Cargo.toml",
      "sha256": "0b55b275c612495a1c4b5aff7a58703d3b1dc1ef048dc37cb8b2dbccf2b8cc0b",
      "scope": "whole changed file; base reconstructed from target plus full delta where appropriate"
    },
    {
      "role": "baseline",
      "path": "Cargo.toml.orig",
      "sha256": "89455aefd91f57cb0688f2b9580baa316a6b263aabea631c00efe9a19fceb236",
      "scope": "whole changed file; base reconstructed from target plus full delta where appropriate"
    },
    {
      "role": "baseline",
      "path": "README.md",
      "sha256": "843c676d53b4a3bbed7284339a334b24459871ce03169d59a3c9b601efa65220",
      "scope": "whole changed file; base reconstructed from target plus full delta where appropriate"
    },
    {
      "role": "baseline",
      "path": "src/env.rs",
      "sha256": "7d4d59640e66f75f7e2fd36708ce2f893f27dddc2add8b5bbccae92d5d2d6594",
      "scope": "whole changed file; base reconstructed from target plus full delta where appropriate"
    },
    {
      "role": "baseline",
      "path": "src/lib.rs",
      "sha256": "8fe4f6400b7a63fc6f807b01d006cb323bd4643a64ca58e7de7049e91fd6d2c4",
      "scope": "whole changed file; base reconstructed from target plus full delta where appropriate"
    },
    {
      "role": "baseline",
      "path": "src/windows.rs",
      "sha256": "7111ed6c5a165a528d18b035d6a5341be774b6a4b2ddd766e3ab0d1772daa40e",
      "scope": "whole changed file; base reconstructed from target plus full delta where appropriate"
    },
    {
      "role": "target",
      "path": ".cargo_vcs_info.json",
      "sha256": "fdb253378f01705bbcf4b99fd56ed55f0a3c50ac52149d47064fd6dcabed27fb",
      "scope": "whole changed file; base reconstructed from target plus full delta where appropriate"
    },
    {
      "role": "target",
      "path": "Cargo.toml",
      "sha256": "f7861a9fdbed65ce6969d3baa0ebf300c01723d4c2f79bc30f49aad2d1b2445b",
      "scope": "whole changed file; base reconstructed from target plus full delta where appropriate"
    },
    {
      "role": "target",
      "path": "Cargo.toml.orig",
      "sha256": "2c3cc0362f66107ceeae95e6201a262975ec54ef1dcbcd2b269ddd539bcb36a6",
      "scope": "whole changed file; base reconstructed from target plus full delta where appropriate"
    },
    {
      "role": "target",
      "path": "README.md",
      "sha256": "132d9e72011322528376c46965004ffb3d1be30841eb6ff5683a4a75deb35b37",
      "scope": "whole changed file; base reconstructed from target plus full delta where appropriate"
    },
    {
      "role": "target",
      "path": "src/env.rs",
      "sha256": "de5ce25abee28dca6dc4af52257ed6dc230c2a09c587a361a2dda2ffec8f2bd9",
      "scope": "whole changed file; base reconstructed from target plus full delta where appropriate"
    },
    {
      "role": "target",
      "path": "src/lib.rs",
      "sha256": "f986d439da63d1597057be177b7ffde982c3081b5ed967f0dcaed99d3490a0b2",
      "scope": "whole changed file; base reconstructed from target plus full delta where appropriate"
    },
    {
      "role": "target",
      "path": "src/windows.rs",
      "sha256": "bfc979fb1db4bea3a13b5a9e987c8a0f19cfdf88aa1e66576a357394eefa26bd",
      "scope": "whole changed file; base reconstructed from target plus full delta where appropriate"
    }
  ],
  "target_inventory_sha256": {
    ".cargo_vcs_info.json": "fdb253378f01705bbcf4b99fd56ed55f0a3c50ac52149d47064fd6dcabed27fb",
    "Cargo.toml": "f7861a9fdbed65ce6969d3baa0ebf300c01723d4c2f79bc30f49aad2d1b2445b",
    "Cargo.toml.orig": "2c3cc0362f66107ceeae95e6201a262975ec54ef1dcbcd2b269ddd539bcb36a6",
    "LICENSE-APACHE": "8ada45cd9f843acf64e4722ae262c622a2b3b3007c7310ef36ac1061a30f6adb",
    "LICENSE-MIT": "23f18e03dc49df91622fe2a76176497404e46ced8a715d9d2b67a7446571cca3",
    "README.md": "132d9e72011322528376c46965004ffb3d1be30841eb6ff5683a4a75deb35b37",
    "src/env.rs": "de5ce25abee28dca6dc4af52257ed6dc230c2a09c587a361a2dda2ffec8f2bd9",
    "src/lib.rs": "f986d439da63d1597057be177b7ffde982c3081b5ed967f0dcaed99d3490a0b2",
    "src/windows.rs": "bfc979fb1db4bea3a13b5a9e987c8a0f19cfdf88aa1e66576a357394eefa26bd"
  },
  "base_inventory_sha256": {
    ".cargo_vcs_info.json": "6971b7e7731692a90dd71554856daf500375495499ec004d8ef7320a5046c479",
    "Cargo.lock": "e5f8eca738cf89382f3abb414866e7adfa911d3e19d6acb94616715fe87d0aa0",
    "Cargo.toml": "0b55b275c612495a1c4b5aff7a58703d3b1dc1ef048dc37cb8b2dbccf2b8cc0b",
    "Cargo.toml.orig": "89455aefd91f57cb0688f2b9580baa316a6b263aabea631c00efe9a19fceb236",
    "LICENSE-APACHE": "8ada45cd9f843acf64e4722ae262c622a2b3b3007c7310ef36ac1061a30f6adb",
    "LICENSE-MIT": "23f18e03dc49df91622fe2a76176497404e46ced8a715d9d2b67a7446571cca3",
    "README.md": "843c676d53b4a3bbed7284339a334b24459871ce03169d59a3c9b601efa65220",
    "src/env.rs": "7d4d59640e66f75f7e2fd36708ce2f893f27dddc2add8b5bbccae92d5d2d6594",
    "src/lib.rs": "8fe4f6400b7a63fc6f807b01d006cb323bd4643a64ca58e7de7049e91fd6d2c4",
    "src/windows.rs": "7111ed6c5a165a528d18b035d6a5341be774b6a4b2ddd766e3ab0d1772daa40e"
  },
  "independent_context_files": [
    {
      "role": "target",
      "path": "validation/gates-fix-20261030/source-audits/lane-a-001/home-0.5.9/scratch/target/src/windows.rs",
      "sha256": "bfc979fb1db4bea3a13b5a9e987c8a0f19cfdf88aa1e66576a357394eefa26bd"
    },
    {
      "role": "target",
      "path": "validation/gates-fix-20261030/source-audits/lane-a-001/home-0.5.9/scratch/target/src/env.rs",
      "sha256": "de5ce25abee28dca6dc4af52257ed6dc230c2a09c587a361a2dda2ffec8f2bd9"
    },
    {
      "role": "target",
      "path": "validation/gates-fix-20261030/source-audits/lane-a-001/home-0.5.9/scratch/target/src/lib.rs",
      "sha256": "f986d439da63d1597057be177b7ffde982c3081b5ed967f0dcaed99d3490a0b2"
    },
    {
      "role": "target",
      "path": "validation/gates-fix-20261030/source-audits/lane-a-001/home-0.5.9/scratch/target/Cargo.toml",
      "sha256": "f7861a9fdbed65ce6969d3baa0ebf300c01723d4c2f79bc30f49aad2d1b2445b"
    },
    {
      "role": "target",
      "path": "validation/gates-fix-20261030/source-audits/lane-a-001/home-0.5.9/scratch/target/Cargo.toml.orig",
      "sha256": "2c3cc0362f66107ceeae95e6201a262975ec54ef1dcbcd2b269ddd539bcb36a6"
    }
  ],
  "external_interface_context": [
    {
      "path": "crates.io/windows-sys-0.52.0/src/Windows/Win32/UI/Shell/mod.rs",
      "sha256": "9d1a035392107171083e6c4a22d59770868a7381e4c3346f052b231aeda08083",
      "scope": "line 725 SHGetKnownFolderPath signature; source equals archived member; archive checksum matches product lock"
    },
    {
      "path": "crates.io/windows-sys-0.52.0/src/Windows/Win32/Foundation/mod.rs",
      "sha256": "ee8378fd7a513d263aca8d5d441625965c746658f5a595f571d82e7a6cf71b36",
      "scope": "line 10235 HANDLE = isize; source matches archive"
    },
    {
      "path": "crates.io/windows-sys-0.59.0/src/Windows/Win32/UI/Shell/mod.rs",
      "sha256": "4c79366857f566ffc07df65d7ce6bc501e872c496d80662891d9761c9536ac58",
      "scope": "line 540 SHGetKnownFolderPath signature; source equals archived member; archive checksum matches lock"
    },
    {
      "path": "crates.io/windows-sys-0.59.0/src/Windows/Win32/Foundation/mod.rs",
      "sha256": "ceae2b2fcea537eb138397409ae19427258b73a77807bd38d7976d2867509ff7",
      "scope": "line 10232 HANDLE = *mut c_void; source matches archive"
    },
    {
      "path": "crates.io/windows-sys-0.52.0/src/Windows/Win32/System/Com/mod.rs",
      "sha256": "30e7c0b7ea3db8f9b9d9bafa85a3fbdd01070eb0cc43124306f210274f03cae5",
      "scope": "line 95 CoTaskMemFree signature; source matches archive"
    },
    {
      "path": "crates.io/windows-sys-0.59.0/src/Windows/Win32/System/Com/mod.rs",
      "sha256": "d317de8f81793a7a1230e0f8f8956e391092845180d5b1333a3903634e861af1",
      "scope": "line 84 CoTaskMemFree signature; source matches archive"
    }
  ],
  "dependency_interface_contracts": null,
  "full_diff_sha256": "29acbd171cf8150f3c6e5006dbfbeb3310b4623b61307c3f8802c539fff6a177",
  "primary_report_sha256": "d3e372bc25f5d63d62f0cd6d807f684ec85c9b0f0a9e1db89874013f2fcd7c8e",
  "cross_report_sha256": "4c756225d6eff9f09f0eca24e608e2518e571ddeced5db7825cd05ee6d83534a"
}
```

Inventory hashes identify all archive members; only read-file entries and the complete reviewed delta claim review coverage. Source chain records retain original attribution; the new review is AI-assisted.

## Primary review

# home 0.5.9: source delta review

**Disposition:** review complete; candidate `safe-to-deploy` delta for root review. Auditor: Codex (GPT-6.1 Terra; AI-assisted source review). No record has been registered.

## Identity and evidence

- Frozen batch `aa0f8805f0e48d23f3eeae37a1e4097c9bdd761e30801099b4d1c3eded471b43` and product Cargo.lock `2a60d89a9fb754475a33d8034227743a8bb5ff2e7410b064f5c122800360385d` are verified. Target archive `e3d1354bf6b7235cb4a0576c2619fd4ed18183f689b12b006a0ee7329eeff9a5` matches the product-lock and registry-index checksums; base `0.5.11` archive `589533453244b0995c858700322199b2becb13b627df2851f64a2775d024abcf` matches its registry index. Manifest identities agree.
- `verification.json` records archive/index paths, member hashes and types, and original records of the trusted safe-to-deploy baseline links. Source snapshots match their fixed SHA-256 and Git blob IDs. No links, special entries, path escape, case/Unicode filename collisions, NUL-containing binary, or other non-UTF-8 member was found. Extra checks are recorded in `../member-safety-extra.json`.
- `full.diff` is the complete package diff with SHA-256 `29acbd171cf8150f3c6e5006dbfbeb3310b4623b61307c3f8802c539fff6a177`; its changed-file set matches the frozen queue. `read-files.json` lists reviewed changed files, context and externally read dependency-interface source with hashes. Unchanged files are compared by bytes; no independent full audit of the certified base is claimed.

## Analysis

- This is a reverse delta from 0.5.11. `Cargo.toml:12-37` lowers the declared MSRV from 1.81 to 1.70 and changes Windows-only windows-sys 0.59 to 0.52 while retaining Foundation/Shell/Com features. The target has no `build=false` or explicit library, but the verified archive has no build.rs or implicit other target and Cargo discovers the same `src/lib.rs`. Its package-local Cargo.lock is absent rather than 0.5.11's lock; the frozen product lock includes windows-sys 0.52.0 and separately requires its audit.
- The only executable source difference is `src/windows.rs:19-35`: null token is expressed as `0` rather than `ptr::null_mut()`. Verified windows-sys declarations use `HANDLE = isize` in 0.52 versus `*mut c_void` in 0.59, so each passes a null token appropriate to its binding. On success the path is read as a UTF-16 NUL-terminated buffer and freed; failure frees any allocated pointer, with null free harmless. This matches Microsoft's [SHGetKnownFolderPath contract](https://learn.microsoft.com/en-us/windows/win32/api/shlobj_core/nf-shlobj_core-shgetknownfolderpath) and [CoTaskMemFree contract](https://learn.microsoft.com/en-us/windows/win32/api/combaseapi/nf-combaseapi-cotaskmemfree). The assumptions about OS-provided pointer validity and successful termination are inherited, not independently tested.
- `src/env.rs:31-105` and `src/lib.rs:31-72,101-145` use environment home paths/current directory and return paths without opening or writing them; their changes are documentation only. README removes newer Rust version advice and maintainer warning, and manifest lints/VCS metadata differ. Untrusted environment values can influence paths used later by callers, as before.

## Criteria and boundaries

Every authenticated baseline link explicitly carries safe-to-deploy, which includes safe-to-run ([cargo-vet criteria](https://mozilla.github.io/cargo-vet/built-in-criteria.html)). In this reviewer’s judgment the delta preserves those criteria. No Windows binary was built or run. Older Windows binding and its linking artifacts must be reviewed separately; environment-supplied path selection and OS pointer guarantees remain caller/platform trust boundaries. Dependent crates must satisfy their own audits. This static review is neither formal verification nor cross-platform runtime acceptance; no product, lock, registry or audit store was changed.

## Proposed record, for root review only

```toml
[[audits.home]]
who = "Codex (GPT-6.1 Terra; AI-assisted source review)"
criteria = "safe-to-deploy"
delta = "0.5.11 -> 0.5.9"
notes = "Reverse delta reviewed including Windows null HANDLE bindings and CoTaskMemFree paths; windows-sys audited separately; Windows runtime not tested."
```


## Independent cross-review

Reverse 0.5.11 -> 0.5.9: read complete package delta and src/windows.rs. Source differences are documentation, plus hToken in SHGetKnownFolderPath changing from null_mut() to 0. Independently inspected frozen-lock-matched windows-sys archives: 0.59.0 Foundation HANDLE is *mut c_void (archive SHA 1e38bc4d79ed67fd075bcc251a1c39b32a1776bbe92e5bef1f0bf1f8c531853b); 0.52.0 HANDLE is isize (SHA 282be5f36a8ce781fad8c8ae18fa3f9beff57ec1b52cb3de0789201425d9a33d), and its Shell signature takes that HANDLE. Zero remains the null current-user token; frozen home 0.5.9 resolves windows-sys 0.52.0. Existing unsafe success path converts a null-terminated OS result to OsString then frees it; error path also frees output. Microsoft documents the null token, terminated output and CoTaskMemFree obligation at https://learn.microsoft.com/en-us/windows/win32/api/shlobj_core/nf-shlobj_core-shgetknownfolderpath. USERPROFILE is read first and UWP fallback returns None. The reverse manifest restores auto discovery, but no target build.rs or extra source target exists in the verified archive; MSRV changes 1.81 -> 1.70, Windows dependency changes 0.59 -> 0.52 and standalone lock disappears. Windows ABI/OS contract and dependency implementations remain separate; no Windows runtime test or all-platform proof. Mozilla full 0.5.3 -> 0.5.11 is the authenticated safe-to-deploy baseline for this reverse edge.

Static evidence is not a formal proof, exhaustive platform or concurrency execution, or a recursive certification of dependencies.
