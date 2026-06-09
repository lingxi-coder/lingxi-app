# P1b — tree-sitter bash AST: quoteContext swap + comment-quote-desync passthrough

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Complete the tree-sitter bash AST precision layer (the P1a follow-on): replace the legacy regex quote-stripping that feeds the bash-safety validator context with claude-code's AST-derived `quoteContext`, and make `validate_comment_quote_desync` a passthrough when a tree is present — exactly as claude-code does when tree-sitter is available (`bashSecurity.ts:1998`). Behind the existing `bash-ast` feature (on for engine-desktop). Feature off ⇒ byte-identical legacy behavior.

**Architecture:** P1a added `permission/src/bash_tree_sitter.rs` (parse + `has_actual_operator_nodes`) and a `Ctx.has_actual_operator_nodes` field. P1b adds the `quoteContext` half: a single-pass `collect_quote_spans` DFS + span helpers + `extract_quote_context` producing the three quote views (`with_double_quotes`, `fully_unquoted`, `unquoted_keep_quote_chars`). In `bash_command_is_safe`, when the feature is on and the command parses, those three views replace the regex `extract_quoted_content` output that builds the `Ctx`'s quote-stripped fields (the crate-side `strip_safe_redirections` + pre/post dual-store are unchanged). A new `Ctx.tree_sitter_present` bool (true iff the AST quoteContext was obtained) gates the `validate_comment_quote_desync` passthrough — safe now precisely because the OTHER validators read AST-accurate quote stripping (the reason this was deferred from P1a). **`is_jq` is intentionally NOT threaded into the AST path:** claude-code's `extractQuoteContext` is structural with no jq special-case (the jq quirk lives only in the legacy regex), so `jq "a;b"` becomes Safe under the feature — matching claude-code's real tree-sitter behavior (the `;` is quoted jq syntax, not shell separation).

**Tech Stack:** Rust 1.82; tree-sitter `=0.25.10` / tree-sitter-bash `=0.25.1` (already added in P1a). Verdicts are in-crate `#[cfg(test)]` tests — no coordinator-locked fixtures.

**Reference of truth:** `claude-code/src/utils/bash/treeSitterAnalysis.ts` (`extractQuoteContext` + `collectQuoteSpans` + span helpers) and `bashSecurity.ts:1998` (the desync passthrough) + `:2468-2488` (the quoteContext swap into the validation context).

---

## File Structure

- **Modify** `lingxi-code/permission/src/bash_tree_sitter.rs` — append `QuoteSpans`, `collect_quote_spans`, `build_position_set`, `drop_contained_spans`, `remove_spans`, `replace_spans_keep_quotes`, `QuoteContext`, `extract_quote_context` + tests.
- **Modify** `lingxi-code/permission/src/bash_security.rs` — add `Ctx.tree_sitter_present: bool`; in `bash_command_is_safe`, source the quote views from the AST quoteContext (feature-gated, with legacy fallback); add the `validate_comment_quote_desync` passthrough; make flipped in-crate tests feature-aware.

(No Cargo/engine changes — P1a already added the deps + enabled `bash-ast` on engine-desktop.)

---

## Task 1: port the quoteContext functions into `bash_tree_sitter.rs`

**Files:** Modify `lingxi-code/permission/src/bash_tree_sitter.rs`

- [ ] **Step 1: Write the failing tests**

Append this test module at the end of `bash_tree_sitter.rs`:

```rust
#[cfg(test)]
mod quote_context_tests {
    use super::*;

    fn ctx(cmd: &str) -> QuoteContext {
        extract_quote_context(cmd).expect("parseable command")
    }

    #[test]
    fn plain_command_unchanged() {
        let c = ctx("echo hello world");
        assert_eq!(c.with_double_quotes, "echo hello world");
        assert_eq!(c.fully_unquoted, "echo hello world");
        assert_eq!(c.unquoted_keep_quote_chars, "echo hello world");
    }

    #[test]
    fn single_quotes_removed_double_path_and_fully() {
        let c = ctx("echo 'a;b'");
        assert_eq!(c.with_double_quotes, "echo ");
        assert_eq!(c.fully_unquoted, "echo ");
        assert_eq!(c.unquoted_keep_quote_chars, "echo ''");
    }

    #[test]
    fn double_quotes_content_kept_for_with_double_delims_dropped() {
        let c = ctx(r#"echo "a;b""#);
        assert_eq!(c.with_double_quotes, "echo a;b");
        assert_eq!(c.fully_unquoted, "echo ");
        assert_eq!(c.unquoted_keep_quote_chars, r#"echo """#);
    }

    #[test]
    fn nested_single_inside_double() {
        let cmd = r#"echo "$(echo 'hi')""#;
        let c = ctx(cmd);
        // withDoubleQuotes uses the raw position-set path (NOT drop_contained):
        // inner 'hi' positions stripped, outer " delimiters stripped, rest kept.
        assert_eq!(c.with_double_quotes, "echo $(echo )");
        // fully/keep use drop_contained: the outer double span wins.
        assert_eq!(c.fully_unquoted, "echo ");
        assert_eq!(c.unquoted_keep_quote_chars, r#"echo """#);
    }

    #[test]
    fn ansi_c_string_includes_leading_dollar() {
        let c = ctx("echo $'x'");
        assert_eq!(c.with_double_quotes, "echo ");
        assert_eq!(c.fully_unquoted, "echo ");
        assert_eq!(c.unquoted_keep_quote_chars, "echo $''");
    }
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p permission --features bash-ast --lib quote_context_tests::`
Expected: FAIL — `cannot find function extract_quote_context`.

- [ ] **Step 3: Implement (vetted port)**

Append ABOVE the new test module (after the existing P1a code). This is byte-faithful to `treeSitterAnalysis.ts` and operates on byte offsets:

```rust
// ---------------------------------------------------------------------------
// Quote-context extraction. Ports the `extractQuoteContext` half of
// claude-code `src/utils/bash/treeSitterAnalysis.ts`. tree-sitter offsets
// (start_byte/end_byte) are BYTE offsets, so every span is a byte range into
// command.as_bytes(); final strings are rebuilt via String::from_utf8_lossy.
// ---------------------------------------------------------------------------

/// Quote spans collected in one DFS (TS `QuoteSpans`). Each `(start,end)` is a
/// half-open byte range. `raw`=single-quoted, `ansi_c`=`$'…'` (span INCLUDES the
/// leading `$`), `double`=`"…"` outermost-only, `heredoc`=QUOTED heredoc only.
#[derive(Default)]
struct QuoteSpans {
    raw: Vec<(usize, usize)>,
    ansi_c: Vec<(usize, usize)>,
    double: Vec<(usize, usize)>,
    heredoc: Vec<(usize, usize)>,
}

/// Single-pass DFS collecting every quote span (TS `collectQuoteSpans`):
/// raw_string/ansi_c_string push + RETURN; string pushes outermost only then
/// recurses `in_double=true` then RETURN; quoted heredoc_redirect (first
/// heredoc_start byte is ' " or \\) pushes + RETURN, else falls through.
fn collect_quote_spans(node: Node<'_>, src: &[u8], out: &mut QuoteSpans, in_double: bool) {
    let span = (node.start_byte(), node.end_byte());
    match node.kind() {
        "raw_string" => {
            out.raw.push(span);
            return;
        }
        "ansi_c_string" => {
            out.ansi_c.push(span);
            return;
        }
        "string" => {
            if !in_double {
                out.double.push(span);
            }
            for child in children(node) {
                collect_quote_spans(child, src, out, true);
            }
            return;
        }
        "heredoc_redirect" => {
            let mut is_quoted = false;
            for child in children(node) {
                if child.kind() == "heredoc_start" {
                    let first = src.get(child.start_byte()).copied();
                    is_quoted = matches!(first, Some(b'\'') | Some(b'"') | Some(b'\\'));
                    break;
                }
            }
            if is_quoted {
                out.heredoc.push(span);
                return;
            }
            // unquoted: fall through and recurse (preserving in_double)
        }
        _ => {}
    }
    for child in children(node) {
        collect_quote_spans(child, src, out, in_double);
    }
}

/// Set of every byte position covered by `spans` (TS `buildPositionSet`).
fn build_position_set(spans: &[(usize, usize)]) -> std::collections::HashSet<usize> {
    let mut set = std::collections::HashSet::new();
    for &(start, end) in spans {
        for i in start..end {
            set.insert(i);
        }
    }
    set
}

/// Drop spans strictly contained within another (TS `dropContainedSpans`):
/// drop `s` when some other `o` has `o.0<=s.0 && o.1>=s.1` and is strictly
/// larger on a side. Equal-extent duplicates both survive. Generic over the
/// (start,end)-prefixed tuple via a `bounds` closure.
fn drop_contained_spans<T: Clone>(spans: &[T], bounds: impl Fn(&T) -> (usize, usize)) -> Vec<T> {
    spans
        .iter()
        .enumerate()
        .filter(|(i, s)| {
            let (ss, se) = bounds(s);
            !spans.iter().enumerate().any(|(j, other)| {
                if j == *i {
                    return false;
                }
                let (os, oe) = bounds(other);
                os <= ss && oe >= se && (os < ss || oe > se)
            })
        })
        .map(|(_, s)| s.clone())
        .collect()
}

/// Remove `spans` from `command` (TS `removeSpans`): drop contained, sort by
/// start descending, splice out (descending keeps offsets valid).
fn remove_spans(command: &str, spans: &[(usize, usize)]) -> String {
    if spans.is_empty() {
        return command.to_string();
    }
    let mut sorted: Vec<(usize, usize)> = drop_contained_spans(spans, |&(s, e)| (s, e));
    sorted.sort_by(|a, b| b.0.cmp(&a.0));
    let mut bytes = command.as_bytes().to_vec();
    for &(start, end) in &sorted {
        bytes.drain(start..end);
    }
    String::from_utf8_lossy(&bytes).into_owned()
}

/// Replace each span's content with `open + close` (TS `replaceSpansKeepQuotes`).
fn replace_spans_keep_quotes(command: &str, spans: &[(usize, usize, String, String)]) -> String {
    if spans.is_empty() {
        return command.to_string();
    }
    let mut sorted = drop_contained_spans(spans, |s| (s.0, s.1));
    sorted.sort_by(|a, b| b.0.cmp(&a.0));
    let mut bytes = command.as_bytes().to_vec();
    for (start, end, open, close) in &sorted {
        let mut repl = Vec::with_capacity(open.len() + close.len());
        repl.extend_from_slice(open.as_bytes());
        repl.extend_from_slice(close.as_bytes());
        bytes.splice(*start..*end, repl);
    }
    String::from_utf8_lossy(&bytes).into_owned()
}

/// Three quote-stripped views of a command (TS `QuoteContext`).
pub struct QuoteContext {
    /// Single/ANSI-C/quoted-heredoc content removed; double-quoted content kept
    /// but its `"` delimiters dropped.
    pub with_double_quotes: String,
    /// All quoted content removed entirely.
    pub fully_unquoted: String,
    /// Like `fully_unquoted` but quote delimiter chars (`'`, `"`, `$'`) preserved;
    /// quoted heredocs stripped whole.
    pub unquoted_keep_quote_chars: String,
}

/// Extract quote context from the AST (TS `extractQuoteContext`). `None` when
/// the command is empty / over the length cap / unparseable (same gate as
/// `has_actual_operator_nodes`) ⇒ caller keeps the legacy regex path.
#[must_use]
pub fn extract_quote_context(command: &str) -> Option<QuoteContext> {
    let tree = parse(command)?;
    let src = command.as_bytes();

    let mut spans = QuoteSpans::default();
    collect_quote_spans(tree.root_node(), src, &mut spans, false);

    // withDoubleQuotes: exclude single/ansi/heredoc positions + the two `"`
    // delimiter positions of each double span (keep the bytes between). Raw
    // position-set path — does NOT use drop_contained_spans (matches TS).
    let mut single_quote_positions: Vec<(usize, usize)> = Vec::new();
    single_quote_positions.extend_from_slice(&spans.raw);
    single_quote_positions.extend_from_slice(&spans.ansi_c);
    single_quote_positions.extend_from_slice(&spans.heredoc);
    let single_quote_set = build_position_set(&single_quote_positions);
    let mut double_quote_delim_set = std::collections::HashSet::new();
    for &(start, end) in &spans.double {
        double_quote_delim_set.insert(start);
        double_quote_delim_set.insert(end - 1);
    }
    let mut with_double_bytes: Vec<u8> = Vec::with_capacity(src.len());
    for (i, &b) in src.iter().enumerate() {
        if single_quote_set.contains(&i) || double_quote_delim_set.contains(&i) {
            continue;
        }
        with_double_bytes.push(b);
    }
    let with_double_quotes = String::from_utf8_lossy(&with_double_bytes).into_owned();

    // fullyUnquoted: remove every quote span (drop_contained inside).
    let mut all_quote_spans: Vec<(usize, usize)> = Vec::new();
    all_quote_spans.extend_from_slice(&spans.raw);
    all_quote_spans.extend_from_slice(&spans.ansi_c);
    all_quote_spans.extend_from_slice(&spans.double);
    all_quote_spans.extend_from_slice(&spans.heredoc);
    let fully_unquoted = remove_spans(command, &all_quote_spans);

    // unquotedKeepQuoteChars: replace each span with its delimiters.
    let mut swap: Vec<(usize, usize, String, String)> = Vec::new();
    for &(s, e) in &spans.raw {
        swap.push((s, e, "'".to_string(), "'".to_string()));
    }
    for &(s, e) in &spans.ansi_c {
        swap.push((s, e, "$'".to_string(), "'".to_string()));
    }
    for &(s, e) in &spans.double {
        swap.push((s, e, "\"".to_string(), "\"".to_string()));
    }
    for &(s, e) in &spans.heredoc {
        swap.push((s, e, String::new(), String::new()));
    }
    let unquoted_keep_quote_chars = replace_spans_keep_quotes(command, &swap);

    Some(QuoteContext {
        with_double_quotes,
        fully_unquoted,
        unquoted_keep_quote_chars,
    })
}
```

- [ ] **Step 4: Run to verify it passes**

Run: `cargo test -p permission --features bash-ast --lib quote_context_tests::`
Expected: PASS (5 tests). If `nested_single_inside_double` fails, print `to_sexp()` for `echo "$(echo 'hi')"` and confirm the node kinds (`string` > `command_substitution` > … > `raw_string`); the asymmetry (position-set path keeps inner `$(echo )`, splice paths drop the whole outer span) is the load-bearing detail.

- [ ] **Step 5: Commit**

```bash
git add lingxi-code/permission/src/bash_tree_sitter.rs
git commit -m "feat(permission): bash_tree_sitter extract_quote_context (AST quoteContext)"
```

---

## Task 2: swap the AST quoteContext into `Ctx` + add `tree_sitter_present`

**Files:** Modify `lingxi-code/permission/src/bash_security.rs`

- [ ] **Step 1: Add the `tree_sitter_present` field to `Ctx`**

In `struct Ctx { ... }` (the field block that ends with `has_actual_operator_nodes: Option<bool>,` from P1a), add after it:

```rust
    /// Whether an AST `quoteContext` was obtained (claude-code `treeSitter != null`,
    /// bashSecurity.ts:1998). `true` ⇒ the quote-stripped views above are AST-derived
    /// and `validate_comment_quote_desync` is a passthrough. `false` on the legacy
    /// path (feature off, or empty/over-cap/unparseable command).
    tree_sitter_present: bool,
```

- [ ] **Step 2: Source the quote views from the AST when available**

In `bash_command_is_safe`, find the current quote construction (around line 1631-1633):

```rust
    let is_jq = base_command == "jq";
    let (with_double_quotes, fully_unquoted, unquoted_keep_quote_chars) =
        extract_quoted_content(command, is_jq);
```

Replace it with the AST-or-legacy split (note: `is_jq` is still needed for the legacy fallback):

```rust
    let is_jq = base_command == "jq";
    // claude-code's tree-sitter path sources the quote-stripped views from the
    // AST `quoteContext` (bashSecurity.ts:2468-2488), which is structural — NO
    // is_jq special-case (that quirk is legacy-regex-only). `None` (empty /
    // over-cap / unparseable) falls back to the legacy regex stripper.
    #[cfg(feature = "bash-ast")]
    let (with_double_quotes, fully_unquoted, unquoted_keep_quote_chars, tree_sitter_present) =
        match crate::bash_tree_sitter::extract_quote_context(command) {
            Some(qc) => (
                qc.with_double_quotes,
                qc.fully_unquoted,
                qc.unquoted_keep_quote_chars,
                true,
            ),
            None => {
                let (a, b, c) = extract_quoted_content(command, is_jq);
                (a, b, c, false)
            }
        };
    #[cfg(not(feature = "bash-ast"))]
    let (with_double_quotes, fully_unquoted, unquoted_keep_quote_chars, tree_sitter_present) = {
        let (a, b, c) = extract_quoted_content(command, is_jq);
        (a, b, c, false)
    };
```

Then in the `Ctx { ... }` literal (the same one P1a added `has_actual_operator_nodes,` to), add `tree_sitter_present,` as a field. The `strip_safe_redirections(&fully_unquoted)` (for `fully_unquoted`) and `fully_unquoted_pre_strip: fully_unquoted` lines are UNCHANGED — they wrap whichever `fully_unquoted` was chosen.

- [ ] **Step 3: Compile both feature states**

Run: `cargo build -p permission && cargo build -p permission --features bash-ast`
Expected: both compile. (`tree_sitter_present` is read in Task 3 — if a deny-warnings build fails on the unused field, continue to Task 3; do not add `#[allow]`.)

- [ ] **Step 4: Commit**

```bash
git add lingxi-code/permission/src/bash_security.rs
git commit -m "feat(permission): source bash-safety quote views from AST quoteContext"
```

---

## Task 3: comment-quote-desync passthrough + reconcile flipped tests

**Files:** Modify `lingxi-code/permission/src/bash_security.rs`

- [ ] **Step 1: Add the passthrough + write the flip tests**

In `validate_comment_quote_desync` (around line 1203), add the passthrough at the very top of the body and update the stale doc:

```rust
/// TS `validateCommentQuoteDesync` (`bashSecurity.ts:1990-2089`), including the
/// tree-sitter passthrough (`:1998-2003`): when an AST quoteContext is present,
/// the other validators already read AST-accurate quote stripping, so a quote
/// inside a `#` comment cannot desync them — passthrough.
fn validate_comment_quote_desync(ctx: &Ctx) -> Option<String> {
    if ctx.tree_sitter_present {
        return None;
    }
    // … existing scanner body unchanged …
```

Now make the two known flips feature-aware. **(a)** the desync test (find `fn comment_quote_desync_asks`):

```rust
    // Feature OFF (legacy): catches quote-in-#-comment desync.
    #[cfg(not(feature = "bash-ast"))]
    #[test]
    fn comment_quote_desync_asks() {
        // (keep the existing body of this test verbatim)
    }

    // Feature ON: AST quoteContext is authoritative ⇒ passthrough (Safe). The
    // real protection (a hidden command on a later line) is carried by the other
    // validators reading the AST-accurate fully_unquoted. Matches bashSecurity.ts:1998.
    #[cfg(feature = "bash-ast")]
    #[test]
    fn comment_quote_desync_passthrough_when_tree_present() {
        // Use the SAME command the legacy test uses; assert it no longer asks
        // with the "# comment" message (it is Safe, or asks via a different
        // validator — assert specifically that the desync message is absent).
        // (fill the command from the legacy test body)
    }
```

**(b)** the jq metachar test (find `fn metachar_in_quoted_arg_asks`, `jq "a;b" data.json`):

```rust
    // Feature OFF (legacy): jq keeps " delimiters so the metachar regex fires.
    #[cfg(not(feature = "bash-ast"))]
    #[test]
    fn metachar_in_quoted_arg_asks() {
        // (keep the existing body verbatim)
    }

    // Feature ON: claude-code's AST quoteContext is structural (no is_jq), so the
    // " delimiters are stripped and the `["']…[;&]…["']` regex can't match — the
    // quoted `;` is jq syntax, not shell separation. Safe, matching claude-code's
    // tree-sitter path.
    #[cfg(feature = "bash-ast")]
    #[test]
    fn metachar_in_quoted_jq_arg_safe_under_ast() {
        assert!(!asks(r#"jq "a;b" data.json"#));
    }
```

- [ ] **Step 2: Run the flip tests**

Run: `cargo test -p permission --features bash-ast --lib comment_quote_desync metachar`
Expected: the new feature-ON tests PASS (desync passthrough; jq metachar safe).

- [ ] **Step 3: Run the FULL suites both states and reconcile any remaining flips**

Run: `cargo test -p permission --lib` → all pass (legacy unchanged).
Run: `cargo test -p permission --features bash-ast --lib` → **likely a few more flips.** For EACH failing feature-ON test, decide and apply per this policy, then `#[cfg]`-split it (keep the legacy assertion under `#[cfg(not(feature = "bash-ast"))]`, add the feature-ON assertion under `#[cfg(feature = "bash-ast")]`) with a one-line parity comment:
  - **Quote-delimiter-dependent flips → Safe** (like the jq case): faithful — the AST strips delimiters uniformly. Acceptable.
  - **A command-substitution / dangerous-pattern / `rm -rf` case that flips to Safe → STOP and report.** Those use `fully_unquoted_pre_strip` where `$()`/backticks are NOT quote nodes and must survive — a flip there means a real bug in the swap, not a faithful change. Do not paper over it.
  - The candidate flip list to expect (from research): `comment_quote_desync_asks` (done above), `metachar_in_quoted_arg_asks` (done above), possibly `malformed_token_unbalanced_quote_with_separator_asks` (unbalanced-quote case — verify the AST `unquoted_keep_quote_chars` view) and `quoted_newline_hash_asks`. Report every test you split and the command + old/new verdict.

- [ ] **Step 4: Commit**

```bash
git add lingxi-code/permission/src/bash_security.rs
git commit -m "feat(permission): comment-quote-desync passthrough under AST + reconcile quote flips"
```

---

## Task 4: full gate

- [ ] **Step 1: Struct-trap** — Run: `cargo test --workspace --no-run` → builds, no errors.
- [ ] **Step 2: Tests both states** — Run: `cargo test -p permission && cargo test -p permission --features bash-ast` → all pass.
- [ ] **Step 3: Clippy both states** — Run: `cargo clippy -p permission --all-targets --no-deps -- -D warnings && cargo clippy -p permission --all-targets --features bash-ast --no-deps -- -D warnings` → clean.
- [ ] **Step 4: Legacy byte-identical** — Confirm `cargo test -p permission --lib` count is unchanged from before P1b (feature off = today's behavior).
- [ ] **Step 5: Engines build** — Run: `cargo build -p engine-desktop && cargo build -p engine-mobile` → both pass (desktop already enables `bash-ast` from P1a; mobile unaffected).

---

## Notes & follow-ups

- **`is_jq` decision (verified, not guessed):** claude-code `validateShellMetacharacters` (bashSecurity.ts:783) reads `unquotedContent` and matches `["'][^"']*[;&][^"']*["']` — it needs quote delimiters present; there is no `is_jq` branch in the validator. The jq-keeps-quotes behavior is entirely in the legacy `extractQuotedContent`. claude-code's AST `extractQuoteContext` is structural, so `jq "a;b"` is Safe in its tree-sitter path. We match that. Not a bypass (quoted jq syntax).
- **Double parse:** `bash_command_is_safe` now parses twice under the feature (once for `has_actual_operator_nodes`, once for `extract_quote_context`). Bash commands are short; acceptable. A follow-up could add a single `analyze_command` returning both (sharing one `Tree`).
- This **completes** the tree-sitter bash AST work begun in P1a. `compoundStructure` and `extractDangerousPatterns` remain unported — `bashSecurity.ts` only reads `dangerousPatterns.hasHeredoc` for divergence *logging* (which lingxi does not port), and never reads `compoundStructure`.
