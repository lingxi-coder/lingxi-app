# Product dependency source review

`cargo vet check --locked` is a hard CI gate, separate from cargo-audit and
cargo-deny. It imports published audit records from Mozilla, Google, Bytecode
Alliance, ISRG, Zcash and Embark Studios. The source URLs pin specific upstream
commits, and the imports lock pins the evidence used for this dependency graph.
Imported records retain the original auditor and notes, including named wildcard
audits that attest to a publisher's release process for a specified period.
There are no exemptions, publisher-trust rules or claimed local reviews.

On 2026-10-27, the locked check reports 688 unvetted package versions: 679 need
`safe-to-deploy` and 9 need `safe-to-run`, for Cargo.lock SHA-256
`12ea25be126eb8b277eb5b823fa79f82f19d37fa019f722aae55e13b8eaa35b1`.
The configured policies also require review of the crates.io baselines and
specific Git revisions of patched dependencies. Closing the gate requires
actual source reviews (including justified version deltas) or existing
trustworthy audit records. Zero known vulnerabilities does not certify source
review. To update a source, review and change its pinned commit first, then run
`cargo vet regenerate imports` and `cargo vet check --locked` to verify exact
version coverage.
