# Native 2.1.293 command replay fields

`cases.json` and `source/native-receipt.json` are copied from Harness
`docs/parity/current-alignment/fixtures/commands/`. `native-rerun-receipt.json`
and `native-baseline/` were captured in an independent rerun using the pinned
Native binary SHA recorded in the receipt and a 127.0.0.1-only Messages stub.
The rerun did not modify the source fixture.

The receipt records the SHA and byte length of every full Native request and
session transcript. Those full files remain in the local ignored capture so
the checkout does not vendor Native-injected account/global instructions.
`native-session-user-rows/` holds the original JSONL bytes for only the fixed
user rows. `native-fixed-fields.json` records their hashes and exact UTF-16
content units, plus fixed request text blocks; temporary project roots are
replaced by `<PROJECT_ROOT>` for comparison. It also records the fixed event
sequence and fields from raw stream-json stdout. UUIDs, cwd paths, timestamps,
duration, cost, and usage remain dynamic and are asserted by shape or
normalized separately.
