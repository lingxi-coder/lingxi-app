# Include all third-party forks in source review

Codex AI-assisted, 2026-11-03.

Eleven vendored/Git third-party package identities were previously omitted from the formal cargo-vet dependency review as if they were first-party packages. Explicit audit policies now include them. This adds no trust, exemption, wildcard, or equivalence assertion between different sources. Cargo-vet 0.10.3 reports their exact Git-qualified versions as missing; a registry audit for the unqualified version does not satisfy these entries.

With the same imports and 41 local records, actual frozen checking changed from 754 missing entries to 765. The extra eleven are the previously omitted forks; ten newly reviewed registry versions still close ten gaps relative to the earlier graph. The remaining five pre-existing Git policies and eleven new policies yield sixteen Git-qualified failures in total. All remain failures until exact-source evidence is reviewed and registered.

The policy for the Git nix package names its exact Git-qualified version to distinguish the registry package of the same name. Changing the Harness pin requires updating that policy and rechecking the graph.

Validation used the current App lock and unchanged imports, with all features (cargo-vet default), locked and frozen, no registry suggestions and no exemption minimization. The actual result is a vetting failure, not a CI pass. Machine-readable evidence remains in the local integration/vet-resume-20261103/forks-included.json receipt.
