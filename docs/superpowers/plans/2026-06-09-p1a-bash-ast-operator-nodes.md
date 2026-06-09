# P1a — tree-sitter bash AST: `hasActualOperatorNodes` + backslash-operator short-circuit

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a tree-sitter-bash AST layer to the `permission` crate (behind a `bash-ast` cargo feature) and use it to suppress the legacy regex false-positive in `validateBackslashEscapedOperators` — exactly as claude-code does when tree-sitter is available (`bashSecurity.ts:1702`). When the feature is off, behavior is byte-identical to today.

**Architecture:** `permission/src/bash_security.rs` is a faithful port of claude-code's *legacy* (non-tree-sitter) `bashSecurity.ts` regex battery. claude-code's real behavior, when the native tree-sitter parser is available, refines several validators via a `TreeSitterAnalysis`. The smallest, independent, high-value slice of that is `hasActualOperatorNodes`: it lets `validateBackslashEscapedOperators` skip the regex when the AST proves there are no real `;`/`&&`/`||` operator nodes (so `find . -exec rm {} \;`, where `\;` is a literal word argument, is not falsely flagged). We add a new `bash_tree_sitter` module that parses a command with `tree-sitter-bash` and reports whether the tree contains real operator nodes, thread an `Option<bool>` into the validator context, and short-circuit the validator. P1b (a separate plan) will add the full `quoteContext` swap + `validateCommentQuoteDesync` passthrough (which depends on AST-derived quote stripping to stay safe).

**Tech Stack:** Rust 1.82; `tree-sitter` + `tree-sitter-bash` (codex uses `0.25.10`/`0.25`). The verdict layer has **no coordinator-locked fixtures** — all bash-verdict assertions are in-crate `#[cfg(test)]` unit tests (`bash_security.rs`, `policy.rs`), so the only "re-bless" is updating those Rust tests.

**Reference of truth:** `claude-code/src/utils/bash/treeSitterAnalysis.ts` (`hasActualOperatorNodes`, ~L421-443) + `claude-code/src/tools/BashTool/bashSecurity.ts:1696-1721`. Rust how-to: `codex/codex-rs/shell-command/src/bash.rs`.

---

## File Structure

- **Modify** `lingxi-code/permission/Cargo.toml` — add optional `tree-sitter` + `tree-sitter-bash` behind a `bash-ast` feature.
- **Create** `lingxi-code/permission/src/bash_tree_sitter.rs` — parse a command + `has_actual_operator_nodes(command) -> Option<bool>`.
- **Modify** `lingxi-code/permission/src/lib.rs` — declare the (feature-gated) module.
- **Modify** `lingxi-code/permission/src/bash_security.rs` — add `Ctx.has_actual_operator_nodes: Option<bool>`, compute it in `bash_command_is_safe`, short-circuit `validate_backslash_escaped_operators`, and make the flipping unit test feature-aware.
- **Modify** `lingxi-code/apps/engine-desktop/Cargo.toml` — enable `bash-ast` on the `permission` dependency.

---

## Task 1: Add the `bash-ast` feature + tree-sitter deps (DE-RISK)

**Files:** Modify `lingxi-code/permission/Cargo.toml`

- [ ] **Step 1: Add the optional deps + feature**

In `[dependencies]`, after `ignore = "0.4"`, add:

```toml
# tree-sitter bash AST (claude-code's `treeSitterAnalysis`). Optional + gated
# behind `bash-ast` so the default/minimal build never compiles the grammar.
tree-sitter = { version = "0.25", optional = true }
tree-sitter-bash = { version = "0.25", optional = true }
```

Add a `[features]` section (after `[dependencies]`, before `[dev-dependencies]`):

```toml
[features]
# AST-precision layer over bash safety (claude-code's tree-sitter path). OFF by
# default; enabled by engine-desktop. Feature off ⇒ byte-identical legacy path.
bash-ast = ["dep:tree-sitter", "dep:tree-sitter-bash"]
```

- [ ] **Step 2: Verify it builds on Rust 1.82 (both feature states)**

Run: `cargo check -p permission && cargo check -p permission --features bash-ast`
Expected: both succeed. `tree-sitter-bash` compiles a C grammar via `cc` (the toolchain has a C compiler).

**If `cargo check --features bash-ast` fails on an MSRV / rust-version error** (e.g. `tree-sitter` `0.25` requires a newer rustc than 1.82): do NOT thrash. Find the newest `tree-sitter`/`tree-sitter-bash` pair whose MSRV ≤ 1.82 (e.g. try `0.24`, then `0.23`), pin that exact pair, and **note the version in your report** — the AST-walking API used in Task 2 (`Parser::new`, `set_language`, `node.kind()`, `node.children`, `utf8_text`) is stable across `0.20`–`0.25`; only the `set_language`/`LANGUAGE` call may need the older spelling (see Task 2 note). If no `0.2x` builds on 1.82, report **BLOCKED** with the errors.

Then pin the exact resolved versions: `cargo tree -p permission --features bash-ast -i tree-sitter` and `... -i tree-sitter-bash`, and set `version = "=<resolved>"` for each.

- [ ] **Step 3: Commit**

```bash
git add lingxi-code/permission/Cargo.toml lingxi-code/Cargo.lock
git commit -m "build(permission): add bash-ast feature (tree-sitter + tree-sitter-bash)"
```

---

## Task 2: `bash_tree_sitter.rs` — parse + `has_actual_operator_nodes`

**Files:**
- Create `lingxi-code/permission/src/bash_tree_sitter.rs`
- Modify `lingxi-code/permission/src/lib.rs`

- [ ] **Step 1: Declare the module (feature-gated)**

In `lingxi-code/permission/src/lib.rs`, add after `pub mod bash_security;` (line 9):

```rust
#[cfg(feature = "bash-ast")]
pub mod bash_tree_sitter;
```

- [ ] **Step 2: Write the failing tests**

Create `lingxi-code/permission/src/bash_tree_sitter.rs` with ONLY the tests first:

```rust
//! tree-sitter-bash AST helpers for the bash safety layer. Ports the parts of
//! claude-code `src/utils/bash/treeSitterAnalysis.ts` that `bashSecurity.ts`
//! actually consumes. Compiled only under the `bash-ast` feature.
//!
//! Rust how-to mirrors codex `codex-rs/shell-command/src/bash.rs`.

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn real_operators_detected() {
        assert_eq!(has_actual_operator_nodes("echo hi ; ls"), Some(true));
        assert_eq!(has_actual_operator_nodes("a && b"), Some(true));
        assert_eq!(has_actual_operator_nodes("a || b"), Some(true));
    }

    #[test]
    fn escaped_semicolon_is_not_an_operator() {
        // `\;` is a literal word arg to find, NOT a `;` operator node — the
        // exact false-positive the AST suppresses (claude-code find -exec \;).
        assert_eq!(
            has_actual_operator_nodes(r"find . -exec rm {} \;"),
            Some(false)
        );
        assert_eq!(has_actual_operator_nodes("cat safe.txt \\; echo secret"), Some(false));
    }

    #[test]
    fn plain_command_has_no_operators() {
        assert_eq!(has_actual_operator_nodes("ls -la"), Some(false));
    }

    #[test]
    fn pipeline_without_logical_ops_has_no_list_operators() {
        // A bare pipe is a `pipeline`, not a `list`/`;`/`&&`/`||` — Some(false).
        assert_eq!(has_actual_operator_nodes("cat x | grep y"), Some(false));
    }
}
```

- [ ] **Step 3: Run to verify it fails**

Run: `cargo test -p permission --features bash-ast --lib bash_tree_sitter::`
Expected: FAIL — `cannot find function has_actual_operator_nodes`.

- [ ] **Step 4: Implement**

Add ABOVE the test module:

```rust
use tree_sitter::{Node, Parser, Tree};

/// Parse `command` as bash. Returns `None` if the parser can't be built or the
/// parse fails outright (catastrophic). A successful-but-error tree still
/// returns `Some` — callers treat a present tree as authoritative, matching the
/// claude-code availability gate (regex fallback only when no tree at all).
fn parse(command: &str) -> Option<Tree> {
    let mut parser = Parser::new();
    let language: tree_sitter::Language = tree_sitter_bash::LANGUAGE.into();
    parser.set_language(&language).ok()?;
    parser.parse(command, None)
}

/// Iterate ALL children of `node` (named + anonymous operator tokens), matching
/// the TS `node.children` walk. One cursor per node (tree-sitter requirement).
fn children<'a>(node: Node<'a>) -> Vec<Node<'a>> {
    let mut cursor = node.walk();
    node.children(&mut cursor).collect()
}

/// Does the AST contain a real `;` / `&&` / `||` / `list` node?  Ports
/// `treeSitterAnalysis.ts` `hasActualOperatorNodes`. `\;` parses as a `word`
/// argument (not a `;` node), so `find -exec \;` returns `Some(false)`.
///
/// `None` ⇒ no tree (parser unavailable / catastrophic parse failure) ⇒ the
/// caller keeps the legacy regex behavior.
#[must_use]
pub fn has_actual_operator_nodes(command: &str) -> Option<bool> {
    let tree = parse(command)?;
    Some(walk_has_operator(tree.root_node()))
}

/// DFS short-circuit: true if any node is `;`, `&&`, `||`, or `list`.
fn walk_has_operator(node: Node) -> bool {
    matches!(node.kind(), ";" | "&&" | "||" | "list")
        || children(node).into_iter().any(walk_has_operator)
}
```

**Note (MSRV fallback from Task 1):** if you pinned `tree-sitter` `0.2x` and `tree_sitter_bash::LANGUAGE` does not exist, use the older spelling `let language = tree_sitter_bash::language();` (returns `tree_sitter::Language` directly — drop the `.into()`). Everything else is unchanged.

- [ ] **Step 5: Run to verify it passes**

Run: `cargo test -p permission --features bash-ast --lib bash_tree_sitter::`
Expected: PASS (4 tests). If `escaped_semicolon_is_not_an_operator` fails (the AST *does* report an operator for `\;`), inspect the tree: run a quick scratch test printing `tree.root_node().to_sexp()` for `r"find . -exec rm {} \;"` and confirm there is no `;`/`list` node; adjust the kind set only if tree-sitter-bash names the escaped token differently (it should be a `word`).

- [ ] **Step 6: Commit**

```bash
git add lingxi-code/permission/src/bash_tree_sitter.rs lingxi-code/permission/src/lib.rs
git commit -m "feat(permission): bash_tree_sitter has_actual_operator_nodes (AST)"
```

---

## Task 3: thread `has_actual_operator_nodes` into `Ctx`

**Files:** Modify `lingxi-code/permission/src/bash_security.rs`

- [ ] **Step 1: Add the field to `Ctx`**

In the `struct Ctx { ... }` (starts at line 79), add a final field (after `unquoted_keep_quote_chars`):

```rust
    /// AST verdict: whether real `;`/`&&`/`||`/`list` operator nodes exist.
    /// `Some(false)` lets `validate_backslash_escaped_operators` short-circuit
    /// (claude-code `bashSecurity.ts:1702`). `None` ⇒ no AST (legacy behavior:
    /// the `bash-ast` feature is off, or the parser had no tree).
    has_actual_operator_nodes: Option<bool>,
```

- [ ] **Step 2: Compute it where `Ctx` is built**

In `bash_command_is_safe` (line ~1605), the `Ctx { ... }` literal (line ~1628) currently ends at `unquoted_keep_quote_chars,`. Add the new field. Compute it just before the `Ctx {` literal:

```rust
    // AST operator-node detection (claude-code's tree-sitter precision layer).
    // Feature off ⇒ None ⇒ every validator keeps the legacy regex path.
    #[cfg(feature = "bash-ast")]
    let has_actual_operator_nodes = crate::bash_tree_sitter::has_actual_operator_nodes(command);
    #[cfg(not(feature = "bash-ast"))]
    let has_actual_operator_nodes: Option<bool> = None;
```

and add `has_actual_operator_nodes,` as the last field of the `Ctx { ... }` literal.

- [ ] **Step 3: Verify it still compiles both feature states**

Run: `cargo build -p permission && cargo build -p permission --features bash-ast`
Expected: both compile. (No behavior change yet — the field is unused; that's the next task. A `dead_code`/`unused` warning on the field is acceptable here and resolved in Task 4 once the validator reads it; if `-D warnings` in the crate's `[lints]` makes the unused field fail the build, proceed straight to Task 4 in the same commit.)

- [ ] **Step 4: Commit**

```bash
git add lingxi-code/permission/src/bash_security.rs
git commit -m "feat(permission): carry has_actual_operator_nodes on bash-safety Ctx"
```

---

## Task 4: short-circuit `validate_backslash_escaped_operators` + tests

**Files:** Modify `lingxi-code/permission/src/bash_security.rs`

- [ ] **Step 1: Write/adjust the failing tests**

The existing test (line ~1908) asserts the legacy Ask. Replace it with a feature-aware pair. Find:

```rust
    fn backslash_escaped_operator_asks() {
        assert!(message(r"cat safe.txt \; echo secret")
            .unwrap()
            .contains("backslash before a shell operator"));
    }
```

Replace with:

```rust
    // Feature OFF (legacy regex): a backslash before any operator char asks.
    #[cfg(not(feature = "bash-ast"))]
    #[test]
    fn backslash_escaped_operator_asks() {
        assert!(message(r"cat safe.txt \; echo secret")
            .unwrap()
            .contains("backslash before a shell operator"));
    }

    // Feature ON (AST): `\;` is a literal word arg, not a `;` operator node, so
    // the AST suppresses the false-positive — matching claude-code's tree-sitter
    // path (bashSecurity.ts:1702). The command is Safe.
    #[cfg(feature = "bash-ast")]
    #[test]
    fn backslash_escaped_operator_suppressed_when_no_ast_operators() {
        assert!(!asks(r"cat safe.txt \; echo secret"));
    }

    // Feature ON: when REAL operator nodes exist, the AST does NOT relax the
    // check — a backslash-escaped operator alongside a real `;` still asks
    // (the short-circuit only fires when has_actual_operator_nodes == Some(false)).
    #[cfg(feature = "bash-ast")]
    #[test]
    fn backslash_escaped_operator_still_asks_with_real_operators() {
        // `a ; b \> c` — the unquoted `;` is a real operator node, so the
        // backslash-before-`>` regex still runs and asks.
        assert!(message(r"a ; b \> c")
            .unwrap()
            .contains("backslash before a shell operator"));
    }
```

- [ ] **Step 2: Run feature-ON tests to verify the new one fails**

Run: `cargo test -p permission --features bash-ast --lib backslash_escaped_operator`
Expected: `backslash_escaped_operator_suppressed_when_no_ast_operators` FAILS (the command currently still asks — the short-circuit isn't wired).

- [ ] **Step 3: Add the short-circuit**

In `validate_backslash_escaped_operators` (line ~1082), add the short-circuit at the top and update the stale comment:

```rust
/// TS `validateBackslashEscapedOperators` (`bashSecurity.ts:1696-1721`),
/// including the tree-sitter short-circuit (`:1702-1704`): when the AST proves
/// there are no real operator nodes, a backslash-escaped operator is a literal
/// argument (e.g. `find -exec \;`), not hidden structure — passthrough.
fn validate_backslash_escaped_operators(ctx: &Ctx) -> Option<String> {
    if ctx.has_actual_operator_nodes == Some(false) {
        return None;
    }
    if has_backslash_escaped_operator(&ctx.original) {
        return Some(
            "Command contains a backslash before a shell operator (;, |, &, <, >) which can hide command structure"
                .to_string(),
        );
    }
    None
}
```

- [ ] **Step 4: Run all three feature states' tests**

Run: `cargo test -p permission --features bash-ast --lib backslash_escaped_operator`
Expected: `backslash_escaped_operator_suppressed_when_no_ast_operators` and `backslash_escaped_operator_still_asks_with_real_operators` PASS.
Run: `cargo test -p permission --lib backslash_escaped_operator`
Expected: `backslash_escaped_operator_asks` (the feature-off legacy test) PASS.

- [ ] **Step 5: Run the FULL bash_security + policy suites both feature states (catch other flips)**

Run: `cargo test -p permission --lib`
Expected: all pass (legacy path unchanged).
Run: `cargo test -p permission --features bash-ast --lib`
Expected: all pass. **If another test flips** (a command whose legacy Ask becomes Safe under the AST — e.g. a `policy.rs` `bash_safety_*` case using a `\;` command), apply the same `#[cfg(feature = "bash-ast")]` split to that test with a comment explaining the AST suppression. Only the backslash-operator family should be affected by *this* task (the only validator wired to the AST so far); if anything else changes, STOP and report it — it may indicate the short-circuit is too broad.

- [ ] **Step 6: Commit**

```bash
git add lingxi-code/permission/src/bash_security.rs
git commit -m "feat(permission): AST short-circuits validate_backslash_escaped_operators"
```

---

## Task 5: enable `bash-ast` on the desktop engine

**Files:** Modify `lingxi-code/apps/engine-desktop/Cargo.toml`

- [ ] **Step 1: Find the permission dependency line**

Run: `grep -n 'permission' lingxi-code/apps/engine-desktop/Cargo.toml`
You'll find `permission = { path = "../../permission" }` (or `permission.workspace = ...`).

- [ ] **Step 2: Enable the feature**

Change that line to add the feature (preserve the existing path/form):

```toml
permission = { path = "../../permission", features = ["bash-ast"] }
```

Do NOT change `engine-mobile`'s permission dependency (mobile stays on the legacy path — it never registers shell tools anyway).

- [ ] **Step 3: Build both engines**

Run: `cargo build -p engine-desktop && cargo build -p engine-mobile`
Expected: both PASS. Confirm desktop pulls tree-sitter: `cargo tree -p engine-desktop -i tree-sitter-bash` prints the crate; `cargo tree -p engine-mobile 2>/dev/null | grep -c tree-sitter-bash` prints `0`.

- [ ] **Step 4: Commit**

```bash
git add lingxi-code/apps/engine-desktop/Cargo.toml lingxi-code/Cargo.lock
git commit -m "feat(engine-desktop): enable permission bash-ast feature"
```

---

## Task 6: full gate

- [ ] **Step 1: Struct-trap**

Run: `cargo test --workspace --no-run`
Expected: builds, no errors.

- [ ] **Step 2: Targeted tests both feature states**

Run: `cargo test -p permission && cargo test -p permission --features bash-ast`
Expected: all pass.

- [ ] **Step 3: Clippy both feature states**

Run: `cargo clippy -p permission --all-targets --no-deps -- -D warnings && cargo clippy -p permission --all-targets --features bash-ast --no-deps -- -D warnings`
Expected: clean. (Fix only issues in the new code. The `bash_tree_sitter` module's items are used under the feature; no `dead_code` allow should be needed since the module only compiles under the feature.)

- [ ] **Step 4: Confirm the legacy path is byte-identical**

Run: `cargo test -p permission --lib` and confirm the 45 `bash_security` tests + the `policy` `bash_safety_*` tests pass unchanged (feature off = today's behavior).

---

## Notes & follow-ups (out of scope — P1b)

- **P1b (separate plan):** port `quoteContext` (the 3 quote variants via `collectQuoteSpans`/`removeSpans`/`replaceSpansKeepQuotes`) and swap it into `Ctx`'s quote-stripped views, plus `extractDangerousPatterns`, plus the `validateCommentQuoteDesync` passthrough (`bashSecurity.ts:1998`). **B depends on the quoteContext swap**: disabling the comment-quote-desync defense is only safe once the *other* validators read AST-accurate quote stripping (otherwise the desync attack reopens). That is why it is deferred to its own plan.
- `compoundStructure` from `treeSitterAnalysis.ts` is **not ported** — `bashSecurity.ts` never reads it (verified). YAGNI.
- No `test-harness` fixtures change — bash verdicts have no coordinator-locked JSON/snap; all assertions are in-crate unit tests.
