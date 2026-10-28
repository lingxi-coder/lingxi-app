# Bridge protocol generation

`python3 scripts/checks/generate-client-protocol.py` resolves the Harness source
using the product's Cargo dependency and local development patch, then uses the
small, independently locked `syn` tool in this directory to generate
`packages/bridge-client/src/protocol.generated.ts`. Run the same command with
`--check` to detect drift without editing files; the client-protocol check runs
it too. Python 3.11 or newer and the product Rust toolchain are required.

Cargo runs the independent tool from a temporary directory with the product
toolchain, so a local Harness Cargo patch cannot alter the tool's lockfile. The
check also runs the standalone Rust and Python regression suites.

The generated scope is `ClientCommand`, `ClientEvent`, `TaskRowDto`,
`CoordinatorWorkerDto`, `OpenAiOAuthSessionDto`, `UiSurfaceDto`, their variant
inventories, and basic event field checks. The generator parses the actual
Rust macro invocations and their Serde wrappers, rejects unsupported or conditional
wire constructs, verifies the transparent credential wrapper and manually decoded
command branches against those declarations, and handles the currently used field rename, default,
skip, nullable option and flatten forms. The checked-in JSON snapshots remain
independent behavior evidence, not a schema source. Unknown future event variants
remain accepted; checks on known events cover top-level presence and primitive
shapes only. Specialized validation still handles nested DTOs and semantics.

Command types describe accepted input, including `null` for `Option` fields;
event types describe what Rust serializes, where `skip_serializing_if` removes
`None`. `ClientTextDto` and other custom Serde types have explicit wire mappings:
text is an unchanged JavaScript string (including lone UTF-16 surrogates),
answer pairs are an object of strings, credential secrets are strings, and
fields carrying raw JSON remain strings. The custom command/event decoders, exact-text carrier, answer carrier, and
credential wrapper are fingerprinted in the generated output. Changes require
regeneration and manual behavior review; fingerprints do not prove equivalent
behavior. Other nested DTOs, bridge frames, versions, semantic bounds, and
renderer sender/origin/permission checks remain hand-maintained; this tool is
not a full generated protocol or authorization validator.

Run the syntax and drift regressions with:

```sh
python3 scripts/checks/generate-client-protocol.py --test
python3 -m unittest scripts/checks/test_bridge_protocol_generator.py
```
