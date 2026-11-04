# Media supply-chain verification — 2026-11-03

The checks below ran on App `4aa0f0a0e1924c25b58504a6904b15a5f56e35b5`, with Harness pinned to `0dd47853e26bc415551d1da8b677f158007cad79`. The App `Cargo.toml` SHA-256 was `d588b098ea89fc1701c17ba3004350411a74fa35ffb0e6b4f63e68477bfd6cab` and `Cargo.lock` SHA-256 was `221db470e0c34f2842b439ce0d4b2530f2093e5292871de6dde1d2f1af528b52`; both remained unchanged throughout the checks.

`cargo metadata --format-version 1 --all-features --locked --offline` resolved 1009 packages and 13 workspace members. Two independently generated metadata files matched byte-for-byte (SHA-256 `ad6283c5ee42e875a3c542bd268a319266c5162c647e74290a397ef1ee1054da`; metadata includes local paths). This all-features graph is the scope selected by `deny.toml`. The earlier default-feature graph contained 1008 packages.

The following commands were run using isolated Cargo homes; the placeholders identify the task's cached Cargo home, a separate online Cargo home, the verified cargo-audit executable, and an official RustSec advisory-db clone respectively:

```sh
CARGO_HOME="$TASK_CARGO_HOME" CARGO_NET_OFFLINE=true CARGO_TERM_COLOR=never \
  cargo deny --locked --offline check
env -u CARGO_NET_OFFLINE CARGO_HOME="$ONLINE_CARGO_HOME" CARGO_TERM_COLOR=never \
  "$CARGO_AUDIT" audit --deny warnings --db "$RUSTSEC_DB"
```

- `cargo-deny 0.19.8` passed advisories, bans, licenses and sources. Its advisory database was already updated to RustSec commit `965bf9ad083b58e6340d65829d85217b18be9290` before the offline check.
- `cargo-audit 0.22.2` fetched the official RustSec database and crates.io sparse index, scanned all 1009 lockfile packages, and exited successfully with no reported index errors. All 899 registry-sourced locked package versions were independently found in the fetched index; none was marked yanked. RustSec remained at commit `965bf9ad083b58e6340d65829d85217b18be9290`.
- The official aarch64 macOS cargo-audit 0.22.2 archive had SHA-256 `ec7ca4263769593df4d909be85b94a6b79efa2897be5d2bb8ebd516e823175af`, matching [the release metadata](https://github.com/rustsec/rustsec/releases/tag/cargo-audit%2Fv0.22.2). The extracted executable SHA-256 was `33fbe81adca1b794f4ffe98574d59b0ebe6fcfdb310976fafed98a094c795111`.

The prior lock used `pulldown-cmark 0.13.5` and both scanners reported [RUSTSEC-2026-0433](https://rustsec.org/advisories/RUSTSEC-2026-0433.html). Harness commit `a43d9811c68abdc38f28450372c606d10fbf86bb`, included in the pinned Harness commit above, updates its exact requirement to 0.13.6. After accounting for the Harness Git revision change, the App lock's only package version change was `pulldown-cmark 0.13.5` to `0.13.6`. On the verified upstream 0.13.6 source archive, 24 wikilink tests and 37 table tests passed; its library also passed a check with default features disabled. No advisory ignore was added.

These are dependency and focused upstream-source checks, not completion of cargo-vet source audits, the full CI pipeline, product builds, or device acceptance. The first separate online audit attempt for Harness logged a crates.io request timeout despite returning zero; that attempt was rejected as evidence and a subsequent clean online run verified all 714 Harness registry package versions.
