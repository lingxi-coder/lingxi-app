# Product dependency source review

`cargo vet check --locked` is a hard CI gate, separate from cargo-audit and
cargo-deny. It imports official Mozilla, Google and Bytecode Alliance audit
records. The imports lock pins the evidence used for this product dependency
graph. There are no exemptions, publisher-trust rules or fabricated local reviews.

On 2026-09-30, 700 locked dependencies lack safe-to-deploy review coverage. Closing the gate
requires actual source reviews (including justified version deltas) or existing
trustworthy audit records. Zero known vulnerabilities does not certify source
review. Run `cargo vet regenerate imports` when intentionally updating imported
records, then `cargo vet check --locked` to verify exact version coverage.
