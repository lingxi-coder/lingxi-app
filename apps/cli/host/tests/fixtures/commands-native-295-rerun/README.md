# Native 2.1.295 command replay fields

`cases.json` and `inputs/` reuse the fixed inputs from Harness
`docs/parity/current-alignment/fixtures/commands/`; `source/native-receipt-293.json`
preserves the original 2.1.293 input-fixture provenance. `native-rerun-receipt.json`
and `native-baseline/` are a separate 2.1.295 capture using binary SHA
`0116ee2e0a513900b633d9951367f18747686478e2b462805b8c31609f047f70` and a
127.0.0.1-only Messages stub. The 2.1.293 fixture directory remains untouched.

The receipt records hashes and lengths for full Native requests and session
transcripts. Full files remain in the local ignored capture to avoid vendoring
Native-injected account/global instructions. `native-session-user-rows/` keeps
the exact raw JSONL bytes for only the fixed user rows. `native-fixed-fields.json`
records hashes, exact UTF-16 content units, fixed request text blocks, and
stream-json event fields. The unknown-command reminder's explanatory text and
the original slash input are compared to this Native receipt; its available
command preview is generated from LingXi's live command registry, so the test
validates its syntax and placement separately. Temporary project roots are replaced by
`<PROJECT_ROOT>` only in the comparison projection; dynamic cwd, UUID,
timestamps, duration, cost, and usage remain separately validated.
