# Product RTC 0.21.1 integration evidence

This checkpoint uses the actual, published Harness review-branch commit in
`identity.json`; the App must be repinned and revalidated after Harness main is
published. No source audit, exemption or trust record is added by this work.
The compressed files are unmodified raw output; `identity.json` records hashes
of both the compressed and original bytes and of all relevant inputs.

* `cargo metadata --offline --locked --all-features --format-version 1`
  resolved 959 packages. All 84 Harness packages use one Git revision; there is
  one rtc/webrtc 0.21.1 stack, no crypto-bigint or zbus family, and clipboard-win
  is 5.4.3. The local xcap source is a Git-sourced package, not a registry or
  untracked path package.
* With the captured official RustSec database commit and a temporary config
  differing from `deny.toml` only in `db-path`, `cargo deny --frozen --format
  json check` exited 0. It emitted 67 duplicate-version and two unused-license
  warnings; its advisory check reported no errors or warnings. No ignore was
  added.
* `cargo +1.94.0 vet check --locked --frozen --no-minimize-exemptions
  --no-registry-suggestions` exited 255 with 725 missing review entries.
  `vet-delta.json` compares the 815-entry baseline whose five inputs exactly
  match App main 9b0ad700. The candidate removes 116 and adds 26 entries;
  the new RTC versions, xcap Git delta, clipboard version, and Git-source
  identities remain unreviewed. This reduction comes from a changed graph and
  does not establish that newly added sources have been reviewed.
* `python3.13 scripts/check-curated-audits.py` and the associated 16 guard tests
  passed; the xcap policy forces Cargo Vet to require its exact Git source.
* `cargo test --locked --offline -p remote-control-client --features native-media
  --test remote_media_loopback -- --test-threads=1` passed six actual local
  product media tests with the declared GStreamer plugins and loopback sockets.
  The existing native peer binary DataChannel test passed separately.

The separate Harness evidence records 28 core tests, two real Coturn tests,
53 media tests, three mobile tests, and the test-server/probe flow. These are
local runtime checks, not physical-device, production-relay, or full product CI
acceptance. Cargo Audit was not run locally because its executable was absent.
