# Reproduce the bounded Rust 1.94.0 observations

Run on a host with the pinned toolchain installed, at least 4 GiB free, and permission to execute a local test binary. The scripts only read the supplied cached crate source and create small files in a fresh temporary directory; they do not require network access or a full Cargo build.

```sh
G=/Users/luolingfeng/lingxi/worktrees/media-architecture/validation/gates-fix-20261030
S="$G/audit-finding-fixes/cargo-home/registry/src/index.crates.io-1949cf8c6b5b557f"
P="$G/source-audits/round3-20261103/lane-a/probe"
D=$(mktemp -d "${TMPDIR:-/tmp}/lane-a-audit.XXXXXXXX")
R=/Users/luolingfeng/.cargo/bin/rustc

"$R" +1.94.0 --edition 2021 --crate-name version_compare --crate-type lib \
  "$S/version-compare-0.2.1/src/lib.rs" --out-dir "$D"
"$R" +1.94.0 --edition 2021 "$P/version_compare_probe.rs" \
  --extern "version_compare=$D/libversion_compare.rlib" -o "$D/version_compare_probe"
"$D/version_compare_probe"
"$R" +1.94.0 --edition 2021 --test --crate-name version_compare \
  "$S/version-compare-0.2.1/src/lib.rs" -o "$D/version_compare_tests"
"$D/version_compare_tests" --test-threads=1

"$R" +1.94.0 --edition 2015 --crate-name version_check --crate-type lib \
  "$S/version_check-0.9.5/src/lib.rs" --out-dir "$D"
"$R" +1.94.0 --edition 2015 "$P/version_check_probe.rs" \
  --extern "version_check=$D/libversion_check.rlib" -o "$D/version_check_probe"
"$D/version_check_probe" "$P/nonzero-rustc"
env CARGO_MANIFEST_DIR="$S/version_check-0.9.5" "$R" +1.94.0 --edition 2015 \
  --test --crate-name version_check "$S/version_check-0.9.5/src/lib.rs" -o "$D/version_check_tests"
env -u FORCE_STATIC -u KNOWN_CHANNEL RUSTC=/Users/luolingfeng/.rustup/toolchains/1.94.0-aarch64-apple-darwin/bin/rustc \
  "$D/version_check_tests" --test-threads=1

"$R" +1.94.0 --edition 2018 --crate-name untrusted --crate-type lib \
  "$S/untrusted-0.9.0/src/lib.rs" --out-dir "$D"
"$R" +1.94.0 --edition 2018 --test \
  "$G/source-audits/round3-20261103/lane-b/untrusted-invariant-probe.rs" \
  --extern "untrusted=$D/libuntrusted.rlib" -o "$D/untrusted_invariant_tests"
"$D/untrusted_invariant_tests" --test-threads=1 --nocapture
```

The final test intentionally triggers and catches a panic. The historical compiler-output static-fixture test returns early because the published version_check archive excludes those fixtures. The exact toolchain path in `RUSTC=` is specific to the aarch64 macOS host; use the equivalent Rust 1.94.0 compiler executable on another host. Remove only the newly created `$D` directory when finished.
