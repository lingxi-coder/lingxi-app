# Source review: futures-channel 0.3.31 -> 0.3.32

Reviewer: Codex (GPT-6.1 Terra; AI-assisted source review). Independent cross-review: Codex AI-assisted.
Criterion: `safe-to-deploy`; scope: delta.
Frozen batch SHA-256: `aa0f8805f0e48d23f3eeae37a1e4097c9bdd761e30801099b4d1c3eded471b43`; frozen original Cargo.lock SHA-256: `2a60d89a9fb754475a33d8034227743a8bb5ff2e7410b064f5c122800360385d`.
This review applies to the frozen versions only; the final dependency graph can differ. Each dependency remains separately gated.

## Archive, registry and audit-chain evidence

```json
{
  "target": {
    "version": "0.3.32",
    "archive_sha256": "07bbe89c50d7a535e539b8c17bc0b49bdb77747034daa8087407d655f3f7cc1d",
    "index": [
      {
        "registry": "crates.io sparse index",
        "index_file_sha256": "5c5b4992af3b5a87dd249330b8b5c12b7c4be1dd76dd89d71f0de66c749fc550",
        "index_record_sha256": "3c014fbe9ff427c24f4c375aece3daa3c310529217b2d8f17f2d6d4526a54296",
        "cksum": "07bbe89c50d7a535e539b8c17bc0b49bdb77747034daa8087407d655f3f7cc1d",
        "yanked": false
      },
      {
        "registry": "crates.io sparse index",
        "index_file_sha256": "5c5b4992af3b5a87dd249330b8b5c12b7c4be1dd76dd89d71f0de66c749fc550",
        "index_record_sha256": "3c014fbe9ff427c24f4c375aece3daa3c310529217b2d8f17f2d6d4526a54296",
        "cksum": "07bbe89c50d7a535e539b8c17bc0b49bdb77747034daa8087407d655f3f7cc1d",
        "yanked": false
      }
    ]
  },
  "base": {
    "version": "0.3.31",
    "archive_sha256": "2dff15bf788c671c1934e366d07e30c1814a8ef514e1af724a602e8a2fbe1b10",
    "index": [
      {
        "registry": "crates.io sparse index",
        "index_file_sha256": "5c5b4992af3b5a87dd249330b8b5c12b7c4be1dd76dd89d71f0de66c749fc550",
        "index_record_sha256": "85c18af7ba9631421f5d05147e7a33c0bbc6c9ced3af44805d5ba8a4e06d08a2",
        "cksum": "2dff15bf788c671c1934e366d07e30c1814a8ef514e1af724a602e8a2fbe1b10",
        "yanked": false
      },
      {
        "registry": "crates.io sparse index",
        "index_file_sha256": "5c5b4992af3b5a87dd249330b8b5c12b7c4be1dd76dd89d71f0de66c749fc550",
        "index_record_sha256": "85c18af7ba9631421f5d05147e7a33c0bbc6c9ced3af44805d5ba8a4e06d08a2",
        "cksum": "2dff15bf788c671c1934e366d07e30c1814a8ef514e1af724a602e8a2fbe1b10",
        "yanked": false
      }
    ]
  },
  "baseline_chain": [
    {
      "source": "bytecode-alliance",
      "url": "https://raw.githubusercontent.com/bytecodealliance/wasmtime/04f0fcf70248909cc0b6a3ec1d406c7e2d543637/supply-chain/audits.toml",
      "source_sha256": "71f9f1e8a51d25708f2bb152f39e90a6bdc3d17d91d0f5df4cbc608cef465fd2",
      "original_record": {
        "who": "Joel Dice <joel.dice@gmail.com>",
        "criteria": "safe-to-deploy",
        "version": "0.3.31"
      }
    }
  ],
  "read_files": [
    {
      "side": "base",
      "path": "src/mpsc/mod.rs",
      "sha256": "742e362fb924caf4f04054c59ec0f9983f8de44712a67d30a35a19a0f357b6c4",
      "coverage": "changed hunks and relevant source context"
    },
    {
      "side": "base",
      "path": "src/lib.rs",
      "sha256": "7f226e1dfef15df3706ecb66b75555de258d25518f692658952de54b2b17bd55",
      "coverage": "changed hunks and relevant source context"
    },
    {
      "side": "base",
      "path": "tests/mpsc.rs",
      "sha256": "ff02a212f85f92da4c1fdfad79045ff1979a28187549506ad6b73523979b2c16",
      "coverage": "changed hunks and relevant source context"
    },
    {
      "side": "base",
      "path": "tests/mpsc-close.rs",
      "sha256": "ea423e82f0dc2593b20b7d06c0fc17b7740bafdec9c63556593fbb84b536a252",
      "coverage": "changed hunks and relevant source context"
    },
    {
      "side": "base",
      "path": "tests/mpsc-size_hint.rs",
      "sha256": "50fba3495bdf4e91a84ad105b148b6cd72f73f64a85703414eeb2d07732c66b9",
      "coverage": "changed hunks and relevant source context"
    },
    {
      "side": "base",
      "path": "Cargo.toml",
      "sha256": "6b8d190d4242002abcb14a1db311a30124bdad0fc34014ec546db85289ce0f44",
      "coverage": "changed hunks and relevant source context"
    },
    {
      "side": "base",
      "path": "Cargo.toml.orig",
      "sha256": "96b8cdc514b70a914ade74c7067447960004378362e0155b9f0b4fa8f6e5d48b",
      "coverage": "changed hunks and relevant source context"
    },
    {
      "side": "target",
      "path": "src/mpsc/mod.rs",
      "sha256": "fff4ec50657e5cf0ad1a14336b621cab9076abbae4eb732dc1530df637250539",
      "coverage": "changed hunks and relevant source context"
    },
    {
      "side": "target",
      "path": "src/lib.rs",
      "sha256": "7f226e1dfef15df3706ecb66b75555de258d25518f692658952de54b2b17bd55",
      "coverage": "changed hunks and relevant source context"
    },
    {
      "side": "target",
      "path": "tests/mpsc.rs",
      "sha256": "3853d3fbda7442f5f4faa8e46dc6b7354b35b2fec0e6aeb7aace528a709a822d",
      "coverage": "changed hunks and relevant source context"
    },
    {
      "side": "target",
      "path": "tests/mpsc-close.rs",
      "sha256": "001ca3d6b3d90809e1f6ce2e96ba821504ac9ad5b74d5f0bf76de5aff478f5ff",
      "coverage": "changed hunks and relevant source context"
    },
    {
      "side": "target",
      "path": "tests/mpsc-size_hint.rs",
      "sha256": "b1a25850e60bd902d4c499e8163739a0b34ebec972202b72669238968cc8beaf",
      "coverage": "changed hunks and relevant source context"
    },
    {
      "side": "target",
      "path": "Cargo.toml",
      "sha256": "4c67ec07e8a1013cfb90e09476f780c30902f603a091e27e7e78d92bd59b20b7",
      "coverage": "changed hunks and relevant source context"
    },
    {
      "side": "target",
      "path": "Cargo.toml.orig",
      "sha256": "7714041d550ba2c30d24240f40776b646792a8bf8787cc6d50707854ecb760d1",
      "coverage": "changed hunks and relevant source context"
    }
  ],
  "target_inventory_sha256": {
    ".cargo_vcs_info.json": "945ab7d20805783ca28cba6f025f26e569aa0ccf95b3de80e46c385733dda1cb",
    "Cargo.lock": "0755f8cf5fd76793656229d1a843aeb091eda1ea37a9810d73f9eb8751ff9ca3",
    "Cargo.toml": "4c67ec07e8a1013cfb90e09476f780c30902f603a091e27e7e78d92bd59b20b7",
    "Cargo.toml.orig": "7714041d550ba2c30d24240f40776b646792a8bf8787cc6d50707854ecb760d1",
    "LICENSE-APACHE": "275c491d6d1160553c32fd6127061d7f9606c3ea25abfad6ca3f6ed088785427",
    "LICENSE-MIT": "6652c868f35dfe5e8ef636810a4e576b9d663f3a17fb0f5613ad73583e1b88fd",
    "README.md": "76d57b08b50a525e00265477b582a6bbcf82ec17bdce3a808f55f70358a83552",
    "benches/sync_mpsc.rs": "1019dd027f104f58883f396ff70efc3dd69b3a7d62df17af090e07b2b05eaf66",
    "src/lib.rs": "7f226e1dfef15df3706ecb66b75555de258d25518f692658952de54b2b17bd55",
    "src/lock.rs": "38655a797456ea4f67d132c42055cf74f18195e875c3b337fc81a12901f79292",
    "src/mpsc/mod.rs": "fff4ec50657e5cf0ad1a14336b621cab9076abbae4eb732dc1530df637250539",
    "src/mpsc/queue.rs": "22034085dc22050b708a37854e215fc7cbb16d65edc60370cb5d8f4b7faca18e",
    "src/mpsc/sink_impl.rs": "c9977b530187e82c912fcd46e08316e48ed246e77bb2419d53020e69e403d086",
    "src/oneshot.rs": "1e4e33c75d72b5d11cc23710e2a08099e04b72bf2368b68e7c1eb0beb6fc03fa",
    "tests/channel.rs": "88f4a41d82b5c1b01e153d071a2bf48e0697355908c55ca42342ed45e63fdec8",
    "tests/mpsc-close.rs": "001ca3d6b3d90809e1f6ce2e96ba821504ac9ad5b74d5f0bf76de5aff478f5ff",
    "tests/mpsc-size_hint.rs": "b1a25850e60bd902d4c499e8163739a0b34ebec972202b72669238968cc8beaf",
    "tests/mpsc.rs": "3853d3fbda7442f5f4faa8e46dc6b7354b35b2fec0e6aeb7aace528a709a822d",
    "tests/oneshot.rs": "0f97d28852a1fd1327211772f43322c93916a639be3f2581e49ad37c9f8a2f88"
  },
  "base_inventory_sha256": {
    ".cargo_vcs_info.json": "b063d686ab444d1e2bc5bbfde80b381836320f697f52341dd51d0a2fcaa7c155",
    "Cargo.toml": "6b8d190d4242002abcb14a1db311a30124bdad0fc34014ec546db85289ce0f44",
    "Cargo.toml.orig": "96b8cdc514b70a914ade74c7067447960004378362e0155b9f0b4fa8f6e5d48b",
    "LICENSE-APACHE": "275c491d6d1160553c32fd6127061d7f9606c3ea25abfad6ca3f6ed088785427",
    "LICENSE-MIT": "6652c868f35dfe5e8ef636810a4e576b9d663f3a17fb0f5613ad73583e1b88fd",
    "README.md": "faccd17503a06e7df67feb53da22dba6a8ea80ee88736ed37fae038d0d0906dd",
    "benches/sync_mpsc.rs": "1019dd027f104f58883f396ff70efc3dd69b3a7d62df17af090e07b2b05eaf66",
    "src/lib.rs": "7f226e1dfef15df3706ecb66b75555de258d25518f692658952de54b2b17bd55",
    "src/lock.rs": "38655a797456ea4f67d132c42055cf74f18195e875c3b337fc81a12901f79292",
    "src/mpsc/mod.rs": "742e362fb924caf4f04054c59ec0f9983f8de44712a67d30a35a19a0f357b6c4",
    "src/mpsc/queue.rs": "22034085dc22050b708a37854e215fc7cbb16d65edc60370cb5d8f4b7faca18e",
    "src/mpsc/sink_impl.rs": "c9977b530187e82c912fcd46e08316e48ed246e77bb2419d53020e69e403d086",
    "src/oneshot.rs": "1e4e33c75d72b5d11cc23710e2a08099e04b72bf2368b68e7c1eb0beb6fc03fa",
    "tests/channel.rs": "88f4a41d82b5c1b01e153d071a2bf48e0697355908c55ca42342ed45e63fdec8",
    "tests/mpsc-close.rs": "ea423e82f0dc2593b20b7d06c0fc17b7740bafdec9c63556593fbb84b536a252",
    "tests/mpsc-size_hint.rs": "50fba3495bdf4e91a84ad105b148b6cd72f73f64a85703414eeb2d07732c66b9",
    "tests/mpsc.rs": "ff02a212f85f92da4c1fdfad79045ff1979a28187549506ad6b73523979b2c16",
    "tests/oneshot.rs": "0f97d28852a1fd1327211772f43322c93916a639be3f2581e49ad37c9f8a2f88"
  },
  "independent_context_files": [],
  "external_interface_context": [],
  "dependency_interface_contracts": null,
  "full_diff_sha256": "c745ea62345877ed1f0a413ac10335580856d39b5dbb1a2727a5f648db0a2fa6",
  "primary_report_sha256": "0cbb0546aa7d1a55dde2b648deb8b3c5ad4875fd432f21d693b5a24ab43b567b",
  "cross_report_sha256": "3da1905e4d801141ee9e083ea0fc22c20a860f6d62d3aade0e157faff32dec29"
}
```

Inventory hashes identify all archive members; only read-file entries and the complete reviewed delta claim review coverage. Source chain records retain original attribution; the new review is AI-assisted.

## Primary review

# futures-channel 0.3.32 source review

Target archive SHA-256 `07bbe89c50d7a535e539b8c17bc0b49bdb77747034daa8087407d655f3f7cc1d`, independently matched to frozen Cargo.lock and registry index; safe-member and full inventory in `../provenance.json`.
Required `safe-to-deploy`. Reviewer `Codex (GPT-6.1 Terra; AI-assisted source review)`.

Base `0.3.31` SHA-256 `2dff15bf788c671c1934e366d07e30c1814a8ef514e1af724a602e8a2fbe1b10`. Authentication/criteria path in `../provenance.json`.
Complete directed diff: `../diffs/futures-channel-0.3.31-to-0.3.32.diff` SHA-256 `c745ea62345877ed1f0a413ac10335580856d39b5dbb1a2727a5f648db0a2fa6`.

## Read-file list (old SHA-256 -> new SHA-256)

- `.cargo_vcs_info.json` `b063d686ab444d1e2bc5bbfde80b381836320f697f52341dd51d0a2fcaa7c155` -> `945ab7d20805783ca28cba6f025f26e569aa0ccf95b3de80e46c385733dda1cb`
- `Cargo.lock` `absent` -> `0755f8cf5fd76793656229d1a843aeb091eda1ea37a9810d73f9eb8751ff9ca3`
- `Cargo.toml` `6b8d190d4242002abcb14a1db311a30124bdad0fc34014ec546db85289ce0f44` -> `4c67ec07e8a1013cfb90e09476f780c30902f603a091e27e7e78d92bd59b20b7`
- `Cargo.toml.orig` `96b8cdc514b70a914ade74c7067447960004378362e0155b9f0b4fa8f6e5d48b` -> `7714041d550ba2c30d24240f40776b646792a8bf8787cc6d50707854ecb760d1`
- `README.md` `faccd17503a06e7df67feb53da22dba6a8ea80ee88736ed37fae038d0d0906dd` -> `76d57b08b50a525e00265477b582a6bbcf82ec17bdce3a808f55f70358a83552`
- `src/mpsc/mod.rs` `742e362fb924caf4f04054c59ec0f9983f8de44712a67d30a35a19a0f357b6c4` -> `fff4ec50657e5cf0ad1a14336b621cab9076abbae4eb732dc1530df637250539`
- `tests/mpsc-close.rs` `ea423e82f0dc2593b20b7d06c0fc17b7740bafdec9c63556593fbb84b536a252` -> `001ca3d6b3d90809e1f6ce2e96ba821504ac9ad5b74d5f0bf76de5aff478f5ff`
- `tests/mpsc-size_hint.rs` `50fba3495bdf4e91a84ad105b148b6cd72f73f64a85703414eeb2d07732c66b9` -> `b1a25850e60bd902d4c499e8163739a0b34ebec972202b72669238968cc8beaf`
- `tests/mpsc.rs` `ff02a212f85f92da4c1fdfad79045ff1979a28187549506ad6b73523979b2c16` -> `3853d3fbda7442f5f4faa8e46dc6b7354b35b2fec0e6aeb7aace528a709a822d`

## Analysis

Read the complete 0.3.31 -> 0.3.32 diff for all 9 changed files, including tests, Cargo.lock, both manifests, README and source. Bytecode Alliance fixed-commit full audit for 0.3.31 covers safe-to-deploy; actual record and archive/index hashes are in provenance.json. The version raises MSRV 1.56 -> 1.71 and updates futures-core and optional futures-sink to 0.3.32; `std`, `alloc`, `sink`, target-atomic and unstable feature meanings are unchanged; build remains false. Neither new native code nor build script/proc-macro/binary appears in this delta.

src/mpsc/mod.rs:169-192 adds `TryRecvError::{Empty,Closed}`, RecvError and the borrowing Recv future. For bounded/unbounded receivers, `try_recv` maps the existing `next_message()` outcomes to value / closed / empty (1050-1057,1260-1267); deprecated try_next retains its original distinctions. Existing unsafe queue pop remains in `next_message`, not changed by the delta: an empty queue checks atomically whether it is fully closed, and a sender that reserved a count but has not enqueued results in Pending rather than premature closure. Receiver poll_next registers the waker then rechecks queue (1126-1145,1320-1338) to avoid losing a concurrent send. Recv::poll borrows the stream uniquely and delegates to poll_next, so cancellation drops only the future/borrow and does not drop/duplicate messages; it maps Some/None/Pending to Ok/Err/Pending (1158-1181). FusedFuture delegates to receiver stream termination and is not a once-only completion flag; repeated polls can read more messages while the stream is active. try_recv does not register a waker and must not be used as an async notification source by itself. Changed tests cover bounded/unbounded buffered drain, empty and closure.

Independent native rustc 1.98.1 probe `../probes/futures_channel.rs` (SHA-256 `c56ff5eef6fa20f90324ffcbba7c252613c6cec740117b1d5c0dd7fe9ad0cefb`) compiled target source against lock/index-verified futures-core 0.3.32, observed Pending -> other-thread send -> wake -> value, and bounded buffered drain -> Closed; passed. This is not a concurrency stress, cancellation, all-features or formal memory-ordering proof. No new unsafe/FFI/files/network/process/secret handling in delta; unchanged base and dependencies are not recursively certified. No serious vulnerability identified in this delta conditional on base.

## Proposed audit (not registered)

```toml
[[audits.futures-channel]]
who = "Codex (GPT-6.1 Terra; AI-assisted source review)"
criteria = "safe-to-deploy"
delta = "0.3.31 -> 0.3.32"
```


## Independent cross-review

src/mpsc/mod.rs:1050-1094,1260-1301 maps Ready(Some) to a message, Ready(None) to Closed, and Pending to Empty. Existing queue pop uses unsafe; state.is_closed at :1436-1439 requires closed AND zero reserved messages, so a sender that reserved a count before enqueue does not cause premature end. Sender pushes and wakes after reservation; poll_next :1126-1145,1320-1339 registers a waker then checks again. Recv borrows the receiver mutably, and dropping it does not itself pop a message. :1158-1180 delegates polling and FusedFuture::is_terminated to the stream: a ready Recv is not necessarily terminated and repolling it can consume a later item. The independently checked futures-util 0.3.32 Next wrapper has the same design; futures-core describes termination after Ready as usual, not unconditional. Callers must account for this behavior. Read all nine changed paths, including tests and feature manifests. Bytecode Alliance full safe-to-deploy at 0.3.31 anchors the proposed 0.3.31 -> 0.3.32 delta; this is static reasoning, not a proof of all concurrency schedules.

Static evidence is not a formal proof, exhaustive platform or concurrency execution, or a recursive certification of dependencies.
