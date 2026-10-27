# Product dependency source review

`cargo vet check --locked` is a hard CI gate, separate from cargo-audit and
cargo-deny. It imports official Mozilla, Google and Bytecode Alliance audit
records. The imports lock pins the evidence used for this product dependency
graph. There are no exemptions, publisher-trust rules or fabricated local reviews.

On 2026-10-26, the locked check reports 763 unvetted package versions: 754 need
`safe-to-deploy` and 9 need `safe-to-run`, for Cargo.lock SHA-256
`6ecb41acdd3711df056d58bbb2a4e992a257f82423bc4360b5066cd3c71d1dd8`.
The configured policies also require review of the crates.io baselines and
specific Git revisions of patched dependencies. Closing the gate requires
actual source reviews (including justified version deltas) or existing
trustworthy audit records. Zero known vulnerabilities does not certify source
review. Run `cargo vet regenerate imports` when intentionally updating imported
records, then `cargo vet check --locked` to verify exact version coverage.
