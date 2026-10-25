# CLI, TUI and headless hosts

Use the product command line, distinguish terminal and print modes, and integrate deterministic output without bypassing runtime admission.

<a id="cli-assembly"></a>

## Host and runtime boundary

The cli package builds lingxi-cli. apps/cli/host owns argv/environment resolution, startup, stdout/stderr, signals and terminal handoff; apps/cli/tui renders the interactive terminal and tui-core shares UI state. Initialization produces Harness DesktopConfig and delegates agent assembly to the desktop facade. CLI and Electron are different hosts around the same runtime layers.

A reusable application should depend on the Harness public facade rather than import the product CLI crate. Product headless execution still receives a DesktopHostServices implementation for OAuth descriptors, environment updates, launch identity and runtime-ready hooks. Output selection does not replace provider credentials, project validation, permission policy or persisted session setup.

<a id="cli-mode-selection"></a>

## Mode selection and output contracts

decide_mode selects Print for a nonempty prompt; otherwise --no-tui or non-terminal stdin/stdout selects StdioRepl; a full terminal selects Tui. Startup and argv gates further establish valid combinations. Use -p explicitly for automated one-shot runs. Print-only controls include output/input format, budget/turn limits, structured schema and fallback model; interactive behavior should not be inferred from those flags.

text prints the human-facing result; json returns a single structured result; stream-json is newline-delimited realtime input/output for programmatic consumers. Preserve record framing and separate diagnostic stderr from protocol stdout. The stream-json shape belongs to the Harness headless protocol, whereas Bridge uses Frame over WebSocket and native clients use UniFFI; consumers must choose one transport contract.

| Mode | Use | Output |
| --- | --- | --- |
| Tui | Interactive terminal with full stdin/stdout TTY. | Terminal UI and direct interaction. |
| StdioRepl | --no-tui or non-TTY fallback. | Line-oriented conversation. |
| Print | One-shot prompt, automation and scripts. | text / json / stream-json. |

<a id="cli-permission-and-session"></a>

## Permission, session and cancellation behavior

Runtime initialization loads permitted configuration sources and resolves model/provider state before execution. Explicit --model overrides environment and persisted selection. Print mode enables deny_unresolved_ask so unattended execution cannot leave a user permission prompt parked; --permission-prompts none applies the same no-dialog policy. Preapproved rules still act through the runtime permission machinery.

Resume restores persisted session state through the shared session lifecycle; UI row state alone is insufficient. Ctrl+C cancels owned turn work and uses the product signal/cleanup path. TUI queued prompts and slash operations carry submission order and source generation so cancellation or session reset cannot publish an old result into the replacement session. Background session attach is a separate product lifecycle from shell process output.

### CLI consumer examples from the product root

```bash
cargo build --locked -p cli --bin lingxi-cli
./target/debug/lingxi-cli --help
./target/debug/lingxi-cli -p "Explain the project structure" --output-format json
./target/debug/lingxi-cli -p "Summarize the selected files" --output-format stream-json --verbose
./target/debug/lingxi-cli --no-tui
```

Verification method: Environment validation

Prompt runs require your configured provider and may call a real model. --help validates the CLI surface without a model call. These examples do not enable a permission bypass.

<a id="cli-diagnostics"></a>

## Exit status and verification

The product exit constants are success 0, runtime/argv error 1 and SIGINT 130; reserved not-implemented paths use 64. Automation must inspect both process status and the final structured record, because argument validation failure can produce no turn output. A nonzero provider result is not resolved by re-parsing it as a successful JSON response.

Run CLI mode/argv and host tests to establish dispatch and headless behavior, then use deterministic fake-provider checks for serialization. Real-provider cancellation, terminal restoration across shells and operating-system process-tree cleanup require their own runtime acceptance. Do not call a parser/unit suite full terminal or model parity evidence.

### Focused CLI checks

```bash
cargo test --locked -p cli --lib
cargo check --locked -p tui -p tui-core
```

Verification method: Environment validation

- [Harness integration](harness.md)
- [Desktop host](desktop-host.md)

## Sources and documentation baseline

[Source repository](https://github.com/lingxi-coder/lingxi-app/tree/main/apps/cli/host/src/entrypoint.rs)

- [lingxi-app/apps/cli/host/Cargo.toml](https://github.com/lingxi-coder/lingxi-app/blob/caf679253971034bc202c566a81c10d4c5af31c4/apps/cli/host/Cargo.toml) — Committed source (caf679253971034bc202c566a81c10d4c5af31c4)
- [lingxi-app/apps/cli/host/src/entrypoint.rs](https://github.com/lingxi-coder/lingxi-app/blob/caf679253971034bc202c566a81c10d4c5af31c4/apps/cli/host/src/entrypoint.rs) — Committed source (caf679253971034bc202c566a81c10d4c5af31c4)
- [lingxi-app/apps/cli/host/src/argv.rs](https://github.com/lingxi-coder/lingxi-app/blob/caf679253971034bc202c566a81c10d4c5af31c4/apps/cli/host/src/argv.rs) — Committed source (caf679253971034bc202c566a81c10d4c5af31c4)
- [lingxi-app/apps/cli/host/src/init.rs](https://github.com/lingxi-coder/lingxi-app/blob/caf679253971034bc202c566a81c10d4c5af31c4/apps/cli/host/src/init.rs) — Committed source (caf679253971034bc202c566a81c10d4c5af31c4)
- [lingxi-app/apps/cli/host/src/mode.rs](https://github.com/lingxi-coder/lingxi-app/blob/caf679253971034bc202c566a81c10d4c5af31c4/apps/cli/host/src/mode.rs) — Committed source (caf679253971034bc202c566a81c10d4c5af31c4)
- [lingxi-app/apps/cli/host/src/headless_host.rs](https://github.com/lingxi-coder/lingxi-app/blob/caf679253971034bc202c566a81c10d4c5af31c4/apps/cli/host/src/headless_host.rs) — Committed source (caf679253971034bc202c566a81c10d4c5af31c4)
- [lingxi-app/apps/cli/host/src/sigint.rs](https://github.com/lingxi-coder/lingxi-app/blob/caf679253971034bc202c566a81c10d4c5af31c4/apps/cli/host/src/sigint.rs) — Committed source (caf679253971034bc202c566a81c10d4c5af31c4)
- [lingxi-app/apps/cli/host/src/exit_codes.rs](https://github.com/lingxi-coder/lingxi-app/blob/caf679253971034bc202c566a81c10d4c5af31c4/apps/cli/host/src/exit_codes.rs) — Committed source (caf679253971034bc202c566a81c10d4c5af31c4)
- [lingxi-app/apps/cli/tui/src/lib.rs](https://github.com/lingxi-coder/lingxi-app/blob/caf679253971034bc202c566a81c10d4c5af31c4/apps/cli/tui/src/lib.rs) — Committed source (caf679253971034bc202c566a81c10d4c5af31c4)
- [harness-runtime/crates/runtime/src/headless/mod.rs](https://github.com/lingxi-coder/harness-runtime/blob/f975ab5804bb3af0e72beb3974e5c911ece44d53/crates/runtime/src/headless/mod.rs) — Committed source (f975ab5804bb3af0e72beb3974e5c911ece44d53)

- lingxi-app: HEAD caf679253971034bc202c566a81c10d4c5af31c4; Referenced input fingerprint sha256:5172e409e969106eb5149a400160fbc916294195f79f186ce9f63e32444c54b5
- harness-runtime: HEAD f975ab5804bb3af0e72beb3974e5c911ece44d53; Referenced input fingerprint sha256:92c2108dc63215bc69ea9c11ea081795da883fdcb1c9c28b060aa9221a1b95a2; Product pin e04c1837508014d68ea042d5c9fe3c8c15af5aa0

<!-- Generated from apps/web/src/data/docs. Do not edit this file directly. -->
