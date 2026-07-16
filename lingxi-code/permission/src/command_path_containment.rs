//! Per-command PATH CONTAINMENT — faithful port of the ASK-producing parts of
//! claude-code `src/tools/BashTool/pathValidation.ts::validateCommandPaths`
//! (~:603) and the per-command `PATH_EXTRACTORS` table (~:190-552), plus the
//! `validatePath` pre-guards from `src/utils/permissions/pathValidation.ts`
//! (~:373-485) and the `COMMAND_VALIDATOR` (mv/cp-with-flags → ask, ~:596-601).
//!
//! ## The gap this closes
//! Even with a `Bash(cat:*)` allow rule, `cat /etc/passwd` (cwd `/proj/work`)
//! must return **ask** — "concatenate files from … was blocked. For security,
//! LingXi may only … the allowed working directories for this session: …".
//! The sibling [`crate::path_constraints`] only covers output-redirection + `cd`
//! targets; THIS module covers the ~31 path-taking commands' POSITIONAL FILE
//! ARGUMENTS (cat/head/tail/grep/rg/find/ls/sort/uniq/wc/cut/paste/column/tr/
//! file/stat/diff/awk/strings/hexdump/od/base64/nl/sha*sum/jq/mv/cp/touch/mkdir/
//! sed/`git diff --no-index`).
//!
//! ## TS wiring point (`bashPermissions.ts:1106-1122`)
//! `checkPathConstraints` (which calls `validateCommandPaths` per subcommand)
//! runs at step 3 of `bashToolHasPermission` — AFTER the deny/ask rule walks
//! (steps 2/2b) and BEFORE the exact-match-allow (step 4) and the prefix-allow
//! (step 5). So neither a `Bash(cat:*)` prefix rule NOR an exact `Bash(cat
//! /etc/passwd)` rule bypasses it. The Rust wiring in [`crate::policy`] invokes
//! this at the SAME slot as [`crate::path_constraints::check_path_constraints`]
//! (after the deny/ask walks, before the `shell_exact_allow` short-circuit and
//! the allow walk), roots- + shell-gated.
//!
//! ## Scope ported (ASK paths only) — same scoping decision as
//! [`crate::path_constraints`]
//! TS `validateCommandPaths` → `validatePath` → `isPathAllowed` produces `deny`
//! (matched deny rule), `ask`, and `allow` (matched allow rule / sandbox
//! allowlist / working-dir) outcomes. The deny-rule and allow-rule outcomes are
//! already produced by [`crate::policy`]'s rule walks; what is UNIQUE here —
//! not reproducible by rule matching — is the **working-directory containment
//! ASK** plus the `validatePath` PRE-GUARDS that flip a target to ask before
//! containment (`$`/`%`/`=`/tilde-variant; glob in a write/create path; the
//! mv/cp-with-flags validator; the compound-`cd`-with-write arm). Those are the
//! parts ported. The deny/allow-rule branches of `isPathAllowed` are the policy
//! walks' job and are intentionally NOT duplicated.
//!
//! ## Documented divergences
//! - **No AST branch.** TS has a dual path (tree-sitter `astCommands` argv vs.
//!   the fallback `splitCommand_DEPRECATED` + shell-quote). This port keeps ONLY
//!   the split-command path the rest of the crate uses
//!   ([`crate::shell_command::split_command`]) — matching the non-AST TS path,
//!   the conservative (more-asks) direction.
//! - **Lexical containment**, reusing [`crate::filesystem`]'s `expand_path` +
//!   `path_in_allowed_working_path` — the same already-accepted no-`realpath`
//!   divergence as the rest of the crate (the D7 symlink case is an accepted
//!   crate-wide divergence and is intentionally not modeled).
//! - **No `git ls-remote` validator.** The v2.1.183 binary's `COMMAND_VALIDATOR`
//!   gained a `cd` arm (count positional args, `n <= 1`) and the read-only
//!   command battery a `git ls-remote` arm; the TS oracle this port follows has
//!   neither, and the existing [`crate::path_constraints`] already covers `cd`
//!   containment. Documented binary-vs-source delta; out of scope for D1-D6.

use crate::filesystem::{path_in_allowed_working_path, FsRoots};
use crate::path_constraints::PathConstraintAsk;
use std::path::PathBuf;

/// The per-command file-operation type — 1:1 with TS `FileOperationType`
/// (`utils/permissions/pathValidation.ts:27`). Drives both the glob-in-write
/// pre-guard and the compound-`cd`-with-write arm.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OperationType {
    Read,
    Write,
    Create,
}

/// TS `COMMAND_OPERATION_TYPE` (`pathValidation.ts:552-589`) + `ACTION_VERBS`
/// (`:513-550`) + `PATH_EXTRACTORS` selector (`:190-509`), keyed by base
/// command. Returns `None` for any command NOT in `SUPPORTED_PATH_COMMANDS`
/// (TS: such a command is "not a path-restricted command" → passthrough).
///
/// The verb strings are byte-locked against the v2.1.183 binary (`e5d` object).
// `grep`/`rg` deliberately share the verb "search for patterns in files from"
// (1:1 with the TS `ACTION_VERBS` table, which spells out each command); the
// per-command arms are kept explicit for byte-fidelity rather than collapsed.
#[allow(clippy::match_same_arms)]
fn command_spec(command: &str) -> Option<(OperationType, &'static str)> {
    use OperationType::{Create, Read, Write};
    // (operation_type, ACTION_VERBS[command])
    let spec = match command {
        "cd" => (Read, "change directories to"),
        "ls" => (Read, "list files in"),
        "find" => (Read, "search files in"),
        "mkdir" => (Create, "create directories in"),
        "touch" => (Create, "create or modify files in"),
        "rm" => (Write, "remove files from"),
        "rmdir" => (Write, "remove directories from"),
        "mv" => (Write, "move files to/from"),
        "cp" => (Write, "copy files to/from"),
        "cat" => (Read, "concatenate files from"),
        "head" => (Read, "read the beginning of files from"),
        "tail" => (Read, "read the end of files from"),
        "sort" => (Read, "sort contents of files from"),
        "uniq" => (Read, "filter duplicate lines from files in"),
        "wc" => (Read, "count lines/words/bytes in files from"),
        "cut" => (Read, "extract columns from files in"),
        "paste" => (Read, "merge files from"),
        "column" => (Read, "format files from"),
        "tr" => (Read, "transform text from files in"),
        "file" => (Read, "examine file types in"),
        "stat" => (Read, "read file stats from"),
        "diff" => (Read, "compare files from"),
        "awk" => (Read, "process text from files in"),
        "strings" => (Read, "extract strings from files in"),
        "hexdump" => (Read, "display hex dump of files from"),
        "od" => (Read, "display octal dump of files from"),
        "base64" => (Read, "encode/decode files from"),
        "nl" => (Read, "number lines in files from"),
        "grep" => (Read, "search for patterns in files from"),
        "rg" => (Read, "search for patterns in files from"),
        "sed" => (Write, "edit files in"),
        "git" => (Read, "access files with git from"),
        "jq" => (Read, "process JSON from files in"),
        "sha256sum" => (Read, "compute SHA-256 checksums for files in"),
        "sha1sum" => (Read, "compute SHA-1 checksums for files in"),
        "md5sum" => (Read, "compute MD5 checksums for files in"),
        _ => return None,
    };
    Some(spec)
}

// ───────────────────────────────────────────────────────────────────────────
// Argument tokenizing + flag filtering (ports of the helpers PATH_EXTRACTORS
// builds on).
// ───────────────────────────────────────────────────────────────────────────

/// Quote-aware token splitter for one subcommand's argv. Reproduces the effect
/// of TS `parseCommandArguments` (shell-quote → bare strings) for the cases the
/// containment guard needs: whitespace splits tokens, single/double quotes
/// group, a backslash escapes the next char. Surrounding quotes are STRIPPED
/// from each token (matching `parseCommandArguments`, which yields bare
/// strings). Identical in behavior to [`crate::dangerous_removal`]'s splitter;
/// kept module-local so each guard stays self-contained (the crate convention).
fn split_argv(subcommand: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut cur = String::new();
    let mut has_token = false;
    let mut in_single = false;
    let mut in_double = false;
    let mut chars = subcommand.chars().peekable();
    while let Some(c) = chars.next() {
        if in_single {
            if c == '\'' {
                in_single = false;
            } else {
                cur.push(c);
            }
            continue;
        }
        if in_double {
            if c == '\\' {
                if let Some(&n) = chars.peek() {
                    if matches!(n, '"' | '\\' | '$' | '`') {
                        cur.push(n);
                        chars.next();
                        continue;
                    }
                }
                cur.push(c);
            } else if c == '"' {
                in_double = false;
            } else {
                cur.push(c);
            }
            continue;
        }
        match c {
            ' ' | '\t' | '\n' | '\r' => {
                if has_token {
                    tokens.push(std::mem::take(&mut cur));
                    has_token = false;
                }
            }
            '\'' => {
                in_single = true;
                has_token = true;
            }
            '"' => {
                in_double = true;
                has_token = true;
            }
            '\\' => {
                if let Some(n) = chars.next() {
                    cur.push(n);
                }
                has_token = true;
            }
            _ => {
                cur.push(c);
                has_token = true;
            }
        }
    }
    if has_token {
        tokens.push(cur);
    }
    tokens
}

/// TS `filterOutFlags` (`pathValidation.ts:126-139`): keep positional (non-flag)
/// arguments, correctly handling the POSIX `--` end-of-options delimiter — after
/// `--`, ALL arguments are positional even if they start with `-`.
fn filter_out_flags(args: &[String]) -> Vec<String> {
    let mut result = Vec::new();
    let mut after_double_dash = false;
    for arg in args {
        if after_double_dash {
            result.push(arg.clone());
        } else if arg == "--" {
            after_double_dash = true;
        } else if !arg.starts_with('-') {
            result.push(arg.clone());
        }
    }
    result
}

/// TS `tAu(arg, prefixes)` (`pathValidation.ts`): extract the VALUE of an
/// attached flag form — `--file=X` → `X` (when `--file` ∈ prefixes) or a 2-char
/// short flag `-fX` → `X` (when `-f` ∈ prefixes). `None` for a bare/non-matching
/// flag.
fn attached_flag_value(arg: &str, prefixes: &[&str]) -> Option<String> {
    if !arg.starts_with('-') {
        return None;
    }
    if let Some(eq) = arg.find('=') {
        if prefixes.contains(&&arg[..eq]) {
            return Some(arg[eq + 1..].to_string());
        }
        return None;
    }
    for p in prefixes {
        if p.len() == 2 && p.starts_with('-') && arg.starts_with(p) && arg != *p {
            return Some(arg[2..].to_string());
        }
    }
    None
}

/// TS `Zwu` (`parsePatternCommand`, `pathValidation.ts`): grep/rg-style
/// extraction. The first non-flag is the PATTERN and the rest are paths;
/// `-e`/`--regexp`/`-f`/`--file` mark the pattern as found, and `-f`/`--file`
/// ADDITIONALLY push their argument (the pattern FILE) as a path to validate
/// (PATH-05 — `--file=X`, `-f X`, and attached `-fX` after positional start via
/// `tAu`). `flags_with_args` consume the following arg; `--` ends options.
/// Returns `defaults` when no paths were collected.
fn parse_pattern_command(
    args: &[String],
    flags_with_args: &[&str],
    defaults: &[&str],
) -> Vec<String> {
    let mut paths: Vec<String> = Vec::new();
    let mut pattern_found = false; // o
    let mut after_double_dash = false; // i
    let mut positional_started = false; // s
    let mut a = 0;
    while a < args.len() {
        let l = &args[a];
        if !after_double_dash && !positional_started && l == "--" {
            after_double_dash = true;
            a += 1;
            continue;
        }
        if !after_double_dash && !positional_started && l != "-" && l.starts_with('-') {
            let eq = l.find('=');
            let u = match eq {
                Some(e) => &l[..e],
                None => l.as_str(),
            };
            if matches!(u, "-e" | "--regexp" | "-f" | "--file") {
                pattern_found = true;
                if u == "-f" || u == "--file" {
                    let d = match eq {
                        Some(e) => Some(l[e + 1..].to_string()),
                        None => args.get(a + 1).cloned(),
                    };
                    if let Some(d) = d {
                        if !d.is_empty() {
                            paths.push(d);
                        }
                    }
                }
            }
            if flags_with_args.contains(&u) && eq.is_none() {
                a += 1;
            }
            a += 1;
            continue;
        }
        // After the first positional, flag parsing is disabled; an attached
        // `-fX`/`--file=X` still contributes its pattern-file path.
        if positional_started && !after_double_dash {
            if let Some(cv) = attached_flag_value(l, &["-f", "--file"]) {
                paths.push(cv);
            }
        }
        positional_started = true;
        if !pattern_found {
            pattern_found = true;
            a += 1;
            continue;
        }
        paths.push(l.clone());
        a += 1;
    }
    if paths.is_empty() {
        defaults.iter().map(|s| (*s).to_string()).collect()
    } else {
        paths
    }
}

/// TS `hQi(flagsWithArgs)` (`pathValidation.ts`): the cut/paste/column extractor
/// family. Skips leading flags (consuming the arg of any flag in the set), then
/// once the first positional appears EVERY subsequent token (including flags) is
/// a path; `--` also starts the positional-passthrough. No `=`-form handling
/// (matches the binary, which tests `flagsWithArgs.has(wholeArg)`).
fn hqi_extract(args: &[String], flags_with_args: &[&str]) -> Vec<String> {
    let mut result: Vec<String> = Vec::new();
    let mut after_double_dash = false; // n
    let mut positional_started = false; // o
    let mut i = 0;
    while i < args.len() {
        let s = &args[i];
        if after_double_dash || positional_started {
            result.push(s.clone());
        } else if s == "--" {
            after_double_dash = true;
        } else if s != "-" && s.starts_with('-') {
            if flags_with_args.contains(&s.as_str()) {
                i += 1;
            }
        } else {
            result.push(s.clone());
            positional_started = true;
        }
        i += 1;
    }
    result
}

/// TS `PATH_EXTRACTORS.awk` (`pathValidation.ts`): awk's bespoke extractor. Skips
/// the program text; `-F`/`--field-separator`/`-v`/`--assign` consume their arg
/// (never a path) and `-e`/`--source` mark the program found; `-f`/`--file`/
/// `-E`/`--exec` push their SCRIPT-FILE argument (incl. `=`/attached forms via
/// `tAu` after positional start). The first bare positional is the program (when
/// none was given via a flag); the rest are data files.
fn extract_awk(args: &[String]) -> Vec<String> {
    const CONSUME_ARG: [&str; 6] = [
        "-F",
        "--field-separator",
        "-v",
        "--assign",
        "-e",
        "--source",
    ];
    const SCRIPT_FILE: [&str; 4] = ["-f", "--file", "-E", "--exec"];
    let mut n: Vec<String> = Vec::new();
    let mut after_double_dash = false; // o
    let mut program_found = false; // i
    let mut positional_started = false; // s
    let mut a = 0;
    while a < args.len() {
        let l = &args[a];
        if !after_double_dash && !positional_started && l == "--" {
            after_double_dash = true;
            a += 1;
            continue;
        }
        if !after_double_dash && !positional_started && l != "-" && l.starts_with('-') {
            let eq = l.find('=');
            let u = match eq {
                Some(e) => &l[..e],
                None => l.as_str(),
            };
            if CONSUME_ARG.contains(&u) {
                if u == "-e" || u == "--source" {
                    program_found = true;
                }
                if eq.is_none() {
                    a += 1;
                }
                a += 1;
                continue;
            }
            if SCRIPT_FILE.contains(&u) {
                program_found = true;
                match eq {
                    Some(e) => n.push(l[e + 1..].to_string()),
                    None => {
                        if let Some(d) = args.get(a + 1) {
                            n.push(d.clone());
                            a += 1;
                        }
                    }
                }
                a += 1;
                continue;
            }
            // Unknown flag → ignore.
            a += 1;
            continue;
        }
        if positional_started && !after_double_dash {
            if let Some(cv) = attached_flag_value(l, &["-f", "--file", "-E", "--exec"]) {
                n.push(cv);
            }
        }
        positional_started = true;
        if !program_found {
            program_found = true;
            a += 1;
            continue;
        }
        n.push(l.clone());
        a += 1;
    }
    n
}

/// TS `PATH_EXTRACTORS[command](args)` (`pathValidation.ts:190-509`): extract
/// the candidate FILE PATHS to validate for a given base `command` from its
/// `args` (everything after the base command, already wrapper-stripped). `home`
/// supplies the `cd`-with-no-args home target. Mirrors each command's bespoke
/// extractor; unsupported commands never reach here ([`command_spec`] gates).
fn extract_paths(command: &str, args: &[String], home: Option<&str>) -> Vec<String> {
    match command {
        // cd: special case — all args form one path (no args → home dir).
        "cd" => {
            if args.is_empty() {
                home.map(|h| vec![h.to_string()]).unwrap_or_default()
            } else {
                vec![args.join(" ")]
            }
        }
        // ls: filter flags, default to current dir.
        "ls" => {
            let paths = filter_out_flags(args);
            if paths.is_empty() {
                vec![".".to_string()]
            } else {
                paths
            }
        }
        // find: collect roots until the first non-global flag; also path-taking flags.
        "find" => extract_find(args),
        // tr: skip the character SET operands.
        "tr" => extract_tr(args),
        // grep: pattern then paths; `-r`/`-R` with no paths → current dir.
        "grep" => {
            let flags = [
                "-e",
                "--regexp",
                "-f",
                "--file",
                "--exclude",
                "--include",
                "--exclude-dir",
                "--include-dir",
                "-m",
                "--max-count",
                "-A",
                "--after-context",
                "-B",
                "--before-context",
                "-C",
                "--context",
            ];
            let paths = parse_pattern_command(args, &flags, &[]);
            if paths.is_empty()
                && args
                    .iter()
                    .any(|a| matches!(a.as_str(), "-r" | "-R" | "--recursive"))
            {
                return vec![".".to_string()];
            }
            paths
        }
        // rg: pattern then paths, default to current dir.
        "rg" => {
            let flags = [
                "-e",
                "--regexp",
                "-f",
                "--file",
                "-t",
                "--type",
                "-T",
                "--type-not",
                "-g",
                "--glob",
                "-m",
                "--max-count",
                "--max-depth",
                "-r",
                "--replace",
                "-A",
                "--after-context",
                "-B",
                "--before-context",
                "-C",
                "--context",
            ];
            parse_pattern_command(args, &flags, &["."])
        }
        // sed: in-place / read-from-stdin; `-f scriptfile` validated, `-e expr` skipped.
        "sed" => extract_sed(args),
        // jq: filter then file paths (similar to grep), default to stdin (none).
        "jq" => extract_jq(args),
        // git: only `git diff --no-index A B` extracts paths (exactly 2).
        "git" => extract_git(args),
        // awk: bespoke extractor (skip program, validate -f/-E script files).
        "awk" => extract_awk(args),
        // cut/paste/column: hQi flag-arg consumption then positional passthrough.
        "cut" => hqi_extract(
            args,
            &[
                "-d",
                "--delimiter",
                "-f",
                "--fields",
                "-b",
                "--bytes",
                "-c",
                "--characters",
                "--output-delimiter",
            ],
        ),
        "paste" => hqi_extract(args, &["-d", "--delimiters"]),
        "column" => hqi_extract(
            args,
            &[
                "-s",
                "--separator",
                "-o",
                "--output-separator",
                "-c",
                "--output-width",
            ],
        ),
        // All remaining simple commands: just filter out flags (TS `Bx`).
        _ => filter_out_flags(args),
    }
}

/// TS `PATH_EXTRACTORS.find` (`pathValidation.ts:211-269`).
fn extract_find(args: &[String]) -> Vec<String> {
    const PATH_FLAGS: [&str; 11] = [
        "-newer",
        "-anewer",
        "-cnewer",
        "-mnewer",
        "-samefile",
        "-path",
        "-wholename",
        "-ilname",
        "-lname",
        "-ipath",
        "-iwholename",
    ];
    let mut paths: Vec<String> = Vec::new();
    let mut found_non_global_flag = false;
    let mut after_double_dash = false;
    let mut i = 0;
    while i < args.len() {
        let arg = &args[i];
        if after_double_dash {
            paths.push(arg.clone());
            i += 1;
            continue;
        }
        if arg == "--" {
            after_double_dash = true;
            i += 1;
            continue;
        }
        if arg.starts_with('-') {
            // Global options don't stop collection.
            if matches!(arg.as_str(), "-H" | "-L" | "-P") {
                i += 1;
                continue;
            }
            found_non_global_flag = true;
            // `-newer[acmBt][acmtB]` mtime-comparison flags take a path arg.
            let is_newer_xy = is_find_newer_pattern(arg);
            if PATH_FLAGS.contains(&arg.as_str()) || is_newer_xy {
                if let Some(next) = args.get(i + 1) {
                    paths.push(next.clone());
                    i += 1; // skip the path we just consumed
                }
            }
            i += 1;
            continue;
        }
        // Only collect non-flag args before the first non-global flag.
        if !found_non_global_flag {
            paths.push(arg.clone());
        }
        i += 1;
    }
    if paths.is_empty() {
        vec![".".to_string()]
    } else {
        paths
    }
}

/// TS `newerPattern = /^-newer[acmBt][acmtB]$/`.
fn is_find_newer_pattern(arg: &str) -> bool {
    let b = arg.as_bytes();
    if b.len() != 8 {
        return false;
    }
    arg.starts_with("-newer")
        && matches!(b[6], b'a' | b'c' | b'm' | b'B' | b't')
        && matches!(b[7], b'a' | b'c' | b'm' | b't' | b'B')
}

/// TS `PATH_EXTRACTORS.tr` (`pathValidation.ts:301-310`): skip SET1 (or
/// SET1+SET2 when not deleting) operands.
fn extract_tr(args: &[String]) -> Vec<String> {
    let has_delete = args
        .iter()
        .any(|a| a == "-d" || a == "--delete" || (a.starts_with('-') && a.contains('d')));
    let non_flags = filter_out_flags(args);
    let skip = if has_delete { 1 } else { 2 };
    if non_flags.len() > skip {
        non_flags[skip..].to_vec()
    } else {
        Vec::new()
    }
}

/// TS `PATH_EXTRACTORS.sed` (`pathValidation.ts:372-428`).
fn extract_sed(args: &[String]) -> Vec<String> {
    let mut paths: Vec<String> = Vec::new();
    let mut skip_next = false;
    let mut script_found = false;
    let mut after_double_dash = false;
    let mut i = 0;
    while i < args.len() {
        if skip_next {
            skip_next = false;
            i += 1;
            continue;
        }
        let arg = &args[i];
        if !after_double_dash && arg == "--" {
            after_double_dash = true;
            i += 1;
            continue;
        }
        if !after_double_dash && arg.starts_with('-') {
            if matches!(arg.as_str(), "-f" | "--file") {
                // Next arg is a script FILE that needs validation.
                if let Some(script) = args.get(i + 1) {
                    paths.push(script.clone());
                    skip_next = true;
                }
                script_found = true;
            } else if matches!(arg.as_str(), "-e" | "--expression") {
                // Next arg is an expression, not a file.
                skip_next = true;
                script_found = true;
            } else if arg.contains('e') || arg.contains('f') {
                script_found = true;
            }
            i += 1;
            continue;
        }
        // First non-flag is the script (if not already found via -e/-f).
        if !script_found {
            script_found = true;
            i += 1;
            continue;
        }
        paths.push(arg.clone());
        i += 1;
    }
    paths
}

/// TS `PATH_EXTRACTORS.jq` (`pathValidation.ts`). PATH-05: `-f`/`--from-file`
/// pushes its SCRIPT-FILE argument (`--from-file=X`, `-f X`), and
/// `--slurpfile`/`--rawfile` push the FILE (the SECOND arg after the flag — the
/// first is the variable name). The remaining consume-one flags
/// (`-e`/`--arg`/`--argjson`/`--args`/`--jsonargs`/`-L`/`--library-path`/
/// `--indent`/`--tab`) skip their arg. The first bare positional is the filter.
fn extract_jq(args: &[String]) -> Vec<String> {
    const CONSUME_ARG: [&str; 10] = [
        "-e",
        "--expression",
        "--arg",
        "--argjson",
        "--args",
        "--jsonargs",
        "-L",
        "--library-path",
        "--indent",
        "--tab",
    ];
    let mut paths: Vec<String> = Vec::new();
    let mut filter_found = false;
    let mut after_double_dash = false;
    let mut i = 0;
    while i < args.len() {
        let s = &args[i];
        if !after_double_dash && s == "--" {
            after_double_dash = true;
            i += 1;
            continue;
        }
        if !after_double_dash && s.starts_with('-') {
            let eq = s.find('=');
            let l = match eq {
                Some(e) => &s[..e],
                None => s.as_str(),
            };
            if matches!(l, "-e" | "--expression") {
                filter_found = true;
            }
            if matches!(l, "-f" | "--from-file") {
                filter_found = true;
                match eq {
                    Some(e) => paths.push(s[e + 1..].to_string()),
                    None => {
                        if let Some(c) = args.get(i + 1) {
                            paths.push(c.clone());
                            i += 1;
                        }
                    }
                }
                i += 1;
                continue;
            }
            if matches!(l, "--slurpfile" | "--rawfile") {
                if let Some(c) = args.get(i + 2) {
                    paths.push(c.clone());
                }
                i += 3;
                continue;
            }
            if CONSUME_ARG.contains(&l) && eq.is_none() {
                i += 1;
            }
            i += 1;
            continue;
        }
        // First non-flag is the filter, rest are file paths.
        if !filter_found {
            filter_found = true;
            i += 1;
            continue;
        }
        paths.push(s.clone());
        i += 1;
    }
    paths
}

/// TS `PATH_EXTRACTORS.git` (`pathValidation.ts:491-508`): only
/// `git diff --no-index A B` extracts paths (the first two positional args after
/// `diff`). Every other git subcommand is git's own security boundary → no paths.
fn extract_git(args: &[String]) -> Vec<String> {
    if args.first().map(String::as_str) == Some("diff") && args.iter().any(|a| a == "--no-index") {
        // PATH-05: ALL positional args after `diff` (TS `Bx(e.slice(1))`), not a
        // 2-path cap — `git diff --no-index A B C` validates every operand.
        return filter_out_flags(&args[1..]);
    }
    Vec::new()
}

// ───────────────────────────────────────────────────────────────────────────
// validatePath pre-guards (utils/permissions/pathValidation.ts:373-485)
// ───────────────────────────────────────────────────────────────────────────

/// One outcome of the [`validate_path`] pre-guards: either the path needs an
/// ASK with a specific reason, or it resolved to an absolute path to be
/// containment-checked.
enum PathGuard {
    /// A pre-guard tripped → ask with this reason (the message doubles as
    /// `decisionReason.reason`, TS `type: 'other'`).
    Ask(String),
    /// The path passed every pre-guard → containment-check this resolved path
    /// (and surface this display form in the containment message).
    Check(PathBuf),
}

/// Strip ONE leading and ONE trailing `'`/`"` (TS `path.replace(/^['"]|['"]$/g,
/// '')`). NOT matched-pair stripping — a leading quote and a trailing quote are
/// each removed independently.
fn strip_surrounding_quotes(s: &str) -> &str {
    let mut t = s;
    if let Some(rest) = t.strip_prefix(['\'', '"']) {
        t = rest;
    }
    if let Some(rest) = t.strip_suffix(['\'', '"']) {
        t = rest;
    }
    t
}

/// Expand a leading bare `~` / `~/` to `home` (TS `expandTilde`,
/// `pathValidation.ts:80`). `~user`/`~+`/`~-` are left literal (rejected later
/// by the tilde-variant pre-guard). Unchanged when `home` is `None`.
fn expand_tilde(path: &str, home: Option<&str>) -> String {
    let Some(home) = home else {
        return path.to_string();
    };
    if path == "~" || path.starts_with("~/") {
        format!("{}{}", home, &path[1..])
    } else {
        path.to_string()
    }
}

/// v2.1.185 glob detector `yPt` — `*`, `?`, `[`/`]`. Brace `{}` is NO LONGER a
/// glob metachar here: v2.1.185 split braces into a SEPARATE write-target guard
/// (`TEd`, applied before `yPt` — see the "4b. Brace expansion" check at the
/// glob-check call site). (The exact `[`/`]` pairing semantics of `yPt`
/// are a residual edge case; a bare `]` is still treated as a metachar.)
fn has_glob_metachar(s: &str) -> bool {
    s.bytes().any(|b| matches!(b, b'*' | b'?' | b'[' | b']'))
}

/// Port of the ASK-producing parts of TS `validatePath`
/// (`utils/permissions/pathValidation.ts:373-485`). Applies, in TS order:
///
/// 1. Strip surrounding quotes + `expandTilde`.
/// 2. (UNC pre-guard — modeled by the dangerous-removal/containment crate
///    elsewhere; not re-checked here, the safe direction.)
/// 3. **Tilde-variant** (`~user`/`~+`/`~-`/`~N`, anything still starting `~`
///    after `expandTilde`) → ask (`:401-411`).
/// 4. **Shell-expansion** (`$`/`%` anywhere, or a leading `=`) → ask (`:423-436`).
/// 5. **Glob in a write/create** path → ask (`:443-454`). (For a READ op a glob
///    is validated against its base directory — modeled by stripping to the
///    glob base before containment, matching `validateGlobPattern`.)
///
/// Everything that survives is lexically resolved (absolute kept; relative
/// joined to `cwd`; `.`/`..` collapsed) for the containment check. Returns the
/// FIRST tripped pre-guard, or the resolved path to containment-check.
///
/// Unlike TS `validatePath`, the deny-rule / allow-rule / sandbox-allowlist
/// branches of `isPathAllowed` are NOT evaluated here — those outcomes are
/// produced by [`crate::policy`]'s rule walks (see the module-level scope note).
/// TS `SUr(path)`: `true` when a `..` segment appears AFTER a real directory
/// segment (a possible symlink escape). Splits on `/` (also `\` on Windows),
/// skips empty and `.` segments, and flags a `..` seen once any non-`..` segment
/// has been passed.
fn dotdot_after_directory_segment(path: &str) -> bool {
    let mut seen_real = false;
    for seg in path.split(|c| c == '/' || (cfg!(target_os = "windows") && c == '\\')) {
        if seg.is_empty() || seg == "." {
            continue;
        }
        if seg == ".." {
            if seen_real {
                return true;
            }
        } else {
            seen_real = true;
        }
    }
    false
}

fn validate_path(path: &str, operation_type: OperationType, roots: &FsRoots) -> PathGuard {
    let home = roots
        .home
        .as_deref()
        .map(|p| p.to_string_lossy().into_owned());
    let dequoted = strip_surrounding_quotes(path);
    let clean_path = expand_tilde(dequoted, home.as_deref());

    // 3. Tilde variants expandTilde didn't handle (~user, ~+, ~-, ~N). After
    //    expandTilde, a bare `~`/`~/…` is now absolute (starts `/…`), so only
    //    unexpanded variants still start with `~`.
    if clean_path.starts_with('~') {
        return PathGuard::Ask(
            "Tilde expansion variants (~user, ~+, ~-) in paths require manual approval".to_string(),
        );
    }

    // 4. Shell expansion syntax: `$VAR` / `${VAR}` / `$(cmd)`, backtick command
    // substitution, `%VAR%` (Windows only), or a leading `=`. 1:1 with claude-code
    // `TPt` (binary @197017575):
    // `o.includes("$") || zt()==="windows"&&o.includes("%") || o.includes("`") || o.startsWith("=")`.
    // The `%` check is gated to Windows — on posix a bare `%` is a legal path
    // character, so checking it unconditionally over-asks vs claude-code. The
    // backtick (command substitution) check was previously MISSING, so a path arg
    // containing a backtick under-asked vs claude-code.
    if clean_path.contains('$')
        || (cfg!(target_os = "windows") && clean_path.contains('%'))
        || clean_path.contains('`')
        || clean_path.starts_with('=')
    {
        return PathGuard::Ask(
            "Shell expansion syntax in paths requires manual approval".to_string(),
        );
    }

    // 4a. `..`-after-directory traversal (claude-code `SUr`, run by `EUr` after
    //     the shell-expansion guard, before the brace/glob guards). A `..`
    //     segment appearing AFTER a real directory segment may follow a symlink
    //     outside the working directory (`expand_path` would otherwise collapse
    //     `..` lexically and mask the escape), so it ASKS — even when the path
    //     resolves back inside cwd (`sub/../ok.txt`).
    if dotdot_after_directory_segment(&clean_path) {
        return PathGuard::Ask(
            "Path contains '..' traversal after a directory segment, which may follow a symlink outside the working directory"
                .to_string(),
        );
    }

    // 4b. Brace expansion in a WRITE/CREATE target — a SEPARATE guard AHEAD of
    //     the glob check (claude-code `TEd=/[{}]/` runs before `yPt`, binary
    //     @197017575). `bash` may brace-expand `{a,b}` to paths outside the
    //     working directory, so a braced write target asks. A braced READ path
    //     falls through: v2.1.185's `yPt` does not treat `{}` as a glob, so it
    //     containment-checks the full path below (rather than the glob base).
    if matches!(operation_type, OperationType::Write | OperationType::Create)
        && (clean_path.contains('{') || clean_path.contains('}'))
    {
        return PathGuard::Ask(
            "Brace characters in write target require manual approval \u{2014} bash may brace-expand to paths outside the working directory"
                .to_string(),
        );
    }

    // 5. Glob metachars: blocked outright for write/create; for read, validate
    //    the glob's base directory (TS validateGlobPattern).
    if has_glob_metachar(&clean_path) {
        if matches!(operation_type, OperationType::Write | OperationType::Create) {
            return PathGuard::Ask(
                "Glob patterns are not allowed in write operations. Please specify an exact file path."
                    .to_string(),
            );
        }
        // Read op: containment-check the glob's base directory.
        let base = glob_base_directory(&clean_path);
        return PathGuard::Check(crate::filesystem::expand_path(&base, roots));
    }

    PathGuard::Check(crate::filesystem::expand_path(&clean_path, roots))
}

/// TS `getGlobBaseDirectory` (`utils/permissions/pathValidation.ts:57-74`):
/// everything before the first glob metachar, truncated to its last `/`. Used
/// for READ globs so the directory the glob expands in is the containment
/// subject. (`containsPathTraversal` full-resolve branch is folded into the
/// later lexical `expand_path`, the safe direction.)
fn glob_base_directory(path: &str) -> String {
    let Some(idx) = path
        .bytes()
        .position(|b| matches!(b, b'*' | b'?' | b'[' | b']' | b'{' | b'}'))
    else {
        return path.to_string();
    };
    let before_glob = &path[..idx];
    match before_glob.rfind('/') {
        None => ".".to_string(),
        Some(0) => "/".to_string(),
        Some(last) => before_glob[..last].to_string(),
    }
}

// ───────────────────────────────────────────────────────────────────────────
// Working-dir set + message formatting (shared with path_constraints.rs's
// formatting, kept module-local for self-containment).
// ───────────────────────────────────────────────────────────────────────────

/// Maximum directories listed verbatim before "and N more" — TS
/// `MAX_DIRS_TO_LIST` (`utils/permissions/pathValidation.ts:24`).
const MAX_DIRS_TO_LIST: usize = 5;

/// TS `formatDirectoryList` (`utils/permissions/pathValidation.ts:38-51`).
fn format_directory_list(dirs: &[String]) -> String {
    let dir_count = dirs.len();
    if dir_count <= MAX_DIRS_TO_LIST {
        return dirs
            .iter()
            .map(|d| format!("'{d}'"))
            .collect::<Vec<_>>()
            .join(", ");
    }
    let first = dirs[..MAX_DIRS_TO_LIST]
        .iter()
        .map(|d| format!("'{d}'"))
        .collect::<Vec<_>>()
        .join(", ");
    format!("{first}, and {} more", dir_count - MAX_DIRS_TO_LIST)
}

/// TS `allWorkingDirectories` (`filesystem.ts:667-674`): cwd unioned with the
/// additional working directories, in insertion order (cwd first), as display
/// strings.
fn all_working_directories(roots: &FsRoots, additional: &[PathBuf]) -> Vec<String> {
    let mut out = Vec::with_capacity(1 + additional.len());
    out.push(roots.cwd.to_string_lossy().into_owned());
    for dir in additional {
        let s = dir.to_string_lossy().into_owned();
        if !out.contains(&s) {
            out.push(s);
        }
    }
    out
}

/// Working-dir set as `PathBuf`s for the containment check (cwd + additional).
fn working_dir_paths(roots: &FsRoots, additional: &[PathBuf]) -> Vec<PathBuf> {
    let mut dirs = Vec::with_capacity(1 + additional.len());
    dirs.push(roots.cwd.clone());
    dirs.extend(additional.iter().cloned());
    dirs
}

// ───────────────────────────────────────────────────────────────────────────
// validateCommandPaths — the per-command containment driver
// ───────────────────────────────────────────────────────────────────────────

/// Check a bash `command` for per-command path-containment violations that must
/// ASK even when an allow rule matches (claude-code `validateCommandPaths` run
/// per subcommand by `checkPathConstraints`). Returns the FIRST violation in TS
/// evaluation order across all subcommands, or `None` when every path-taking
/// command stays within the allowed working dirs.
///
/// `roots` supplies cwd/home for lexical expansion + containment; `additional`
/// are the extra allowed working dirs (TS `additionalWorkingDirectories`).
///
/// This is the companion to [`crate::path_constraints::check_path_constraints`]
/// (redirections + `cd`) — it covers the POSITIONAL FILE ARGUMENTS of the ~31
/// `PATH_EXTRACTORS` commands. Both share the same [`crate::policy`] wiring slot.
#[must_use]
pub fn check_command_path_containment(
    command: &str,
    roots: &FsRoots,
    additional: &[PathBuf],
) -> Option<PathConstraintAsk> {
    let subs = crate::shell_command::split_command(command);
    let compound_has_cd = compound_has_cd(&subs);
    let home = roots
        .home
        .as_deref()
        .map(|p| p.to_string_lossy().into_owned());
    let work_dirs = working_dir_paths(roots, additional);

    for sub in &subs {
        // SECURITY: strip wrapper commands (timeout/nice/nohup/time/stdbuf) so
        // `timeout 10 cat /etc/passwd` validates `cat`, not `timeout`
        // (TS `stripSafeWrappers` in `validateSinglePathCommand`).
        let stripped = crate::shell_command::strip_safe_wrappers(sub);
        let tokens = split_argv(&stripped);
        let Some((base, args)) = tokens.split_first() else {
            continue;
        };
        let Some((mut operation_type, action_verb)) = command_spec(base) else {
            continue; // not a path-restricted command → passthrough
        };

        // sed read-only override: a purely-reading sed (`sed -n '1,10p' f`)
        // validates its file args as READ, not write (TS
        // `validateSinglePathCommand` `:869-872` →
        // `sedCommandIsAllowedByAllowlist(strippedCmd)`). `sed_constraint_verdict`
        // with `allow_file_writes=false` returns `Safe` iff the read-only
        // allowlist matches — exactly that predicate.
        if base == "sed"
            && matches!(
                crate::sed_validation::sed_constraint_verdict(&stripped, false, roots, additional),
                crate::sed_validation::SedVerdict::Safe
            )
        {
            operation_type = OperationType::Read;
        }

        // COMMAND_VALIDATOR (mv/cp with ANY flag → ask) — TS
        // `pathValidation.ts:596-628`. `--target-directory=PATH` and friends can
        // bypass path extraction, so ALL flags on mv/cp force manual approval.
        if matches!(base.as_str(), "mv" | "cp") && args.iter().any(|a| a.starts_with('-')) {
            let msg = format!(
                "{base} with flags requires manual approval to ensure path safety. For security, LingXi cannot automatically validate {base} commands that use flags, as some flags like --target-directory=PATH can bypass path validation."
            );
            return Some(PathConstraintAsk {
                message: msg,
                reason: format!("{base} command with flags requires manual approval"),
            });
        }

        // Compound `cd` + non-read operation → ask (TS `pathValidation.ts:645`).
        // A `cd` anywhere in the compound makes a write/create command's paths
        // unresolvable against the final cwd.
        if compound_has_cd && operation_type != OperationType::Read {
            return Some(PathConstraintAsk {
                message: "Commands that change directories and perform write operations require explicit approval to ensure paths are evaluated correctly. For security, LingXi cannot automatically determine the final working directory when 'cd' is used in compound commands.".to_string(),
                reason: "Compound command contains cd with write operation - manual approval required to prevent path resolution bypass".to_string(),
            });
        }

        // Extract + validate every candidate path (TS `validateCommandPaths`
        // `:657-694`). `cd` is handled by `path_constraints.rs`; we skip it here
        // to avoid double-asking on the same target (the existing `cd`
        // containment message wins).
        if base == "cd" {
            continue;
        }
        let paths = extract_paths(base, args, home.as_deref());
        for path in &paths {
            match validate_path(path, operation_type, roots) {
                PathGuard::Ask(reason) => {
                    // A pre-guard ask (tilde-variant / shell-expansion / glob in
                    // write) — the reason IS the message (TS uses the custom
                    // `decisionReason.reason` as the message, `:673-677`).
                    return Some(PathConstraintAsk {
                        message: reason.clone(),
                        reason,
                    });
                }
                PathGuard::Check(resolved) => {
                    if !path_in_allowed_working_path(&resolved, &work_dirs, roots) {
                        let dirs = all_working_directories(roots, additional);
                        let dir_list = format_directory_list(&dirs);
                        let resolved_disp = resolved.to_string_lossy();
                        // TS `validateCommandPaths:677` containment message —
                        // byte-locked against the v2.1.183 binary (`${command}
                        // in '${resolvedPath}' was blocked. For security, Claude
                        // Code may only ${ACTION_VERBS[command]} the allowed
                        // working directories for this session: ${dirListStr}.`).
                        // No custom `decisionReason.reason` is attached in TS for
                        // the containment case, so the message doubles as reason.
                        let message = format!(
                            "{base} in '{resolved_disp}' was blocked. For security, LingXi may only {action_verb} the allowed working directories for this session: {dir_list}."
                        );
                        return Some(PathConstraintAsk {
                            reason: message.clone(),
                            message,
                        });
                    }
                }
            }
        }
    }
    None
}

/// Does any subcommand start with `cd`? (TS `compoundCommandHasCd`.) Gates the
/// compound-`cd`-with-write ask. A leading-word `cd` in ANY subcommand counts.
fn compound_has_cd(subs: &[String]) -> bool {
    subs.iter().any(|s| {
        let stripped = crate::shell_command::strip_safe_wrappers(s);
        split_argv(&stripped)
            .first()
            .is_some_and(|first| first == "cd")
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn roots() -> FsRoots {
        FsRoots {
            cwd: PathBuf::from("/proj/work"),
            home: Some(PathBuf::from("/home/u")),
            lingxi_home: PathBuf::from("/home/u/.lingxi"),
        }
    }

    fn check(cmd: &str) -> Option<PathConstraintAsk> {
        check_command_path_containment(cmd, &roots(), &[])
    }

    // ── D1: read commands out-of-cwd → ask ─────────────────────────────────

    #[test]
    fn cat_out_of_cwd_asks() {
        let a = check("cat /etc/passwd").expect("should ask");
        assert_eq!(
            a.message,
            "cat in '/etc/passwd' was blocked. For security, LingXi may \
             only concatenate files from the allowed working directories for \
             this session: '/proj/work'."
        );
        assert_eq!(a.reason, a.message);
    }

    #[test]
    fn grep_out_of_cwd_asks() {
        // grep PATTERN /etc/hosts — pattern then path; /etc/hosts is out of cwd.
        let a = check("grep root /etc/hosts").expect("should ask");
        assert!(a.message.starts_with("grep in '/etc/hosts' was blocked."));
        assert!(a.message.contains("search for patterns in files from"));
    }

    #[test]
    fn head_tail_wc_out_of_cwd_ask() {
        assert!(check("head /etc/passwd")
            .unwrap()
            .message
            .contains("read the beginning of files from"));
        assert!(check("tail /var/log/syslog")
            .unwrap()
            .message
            .contains("read the end of files from"));
        assert!(check("wc /etc/passwd")
            .unwrap()
            .message
            .contains("count lines/words/bytes in files from"));
    }

    #[test]
    fn sha256sum_out_of_cwd_asks() {
        let a = check("sha256sum /etc/passwd").expect("ask");
        assert!(a.message.contains("compute SHA-256 checksums for files in"));
    }

    #[test]
    fn ls_out_of_cwd_asks() {
        let a = check("ls /etc").expect("ask");
        assert!(a.message.starts_with("ls in '/etc' was blocked."));
        assert!(a.message.contains("list files in"));
    }

    #[test]
    fn rg_out_of_cwd_asks() {
        let a = check("rg pattern /etc").expect("ask");
        assert!(a.message.starts_with("rg in '/etc' was blocked."));
    }

    #[test]
    fn find_root_arg_out_of_cwd_asks() {
        let a = check("find /etc -name foo").expect("ask");
        assert!(a.message.starts_with("find in '/etc' was blocked."));
    }

    // ── D1: read commands in-cwd → not blocked ─────────────────────────────

    #[test]
    fn cat_in_cwd_not_blocked() {
        assert!(check("cat ./local.txt").is_none());
        assert!(check("cat local.txt").is_none());
        assert!(check("cat sub/dir/file").is_none());
        assert!(check("cat /proj/work/x").is_none());
    }

    #[test]
    fn grep_no_path_reads_stdin_not_blocked() {
        // grep with no path reads stdin → no path to validate.
        assert!(check("grep foo").is_none());
        // piped: `cat x | grep foo` — grep has no path arg.
        assert!(check("cat ./x | grep foo").is_none());
    }

    #[test]
    fn ls_no_arg_defaults_to_cwd_not_blocked() {
        assert!(check("ls").is_none());
        assert!(check("ls -la").is_none());
    }

    #[test]
    fn find_no_root_defaults_to_cwd_not_blocked() {
        assert!(check("find . -name foo").is_none());
        assert!(check("find -name foo").is_none());
    }

    // ── D2: write/create out-of-cwd → ask ──────────────────────────────────

    #[test]
    fn touch_out_of_cwd_asks() {
        let a = check("touch /etc/newfile").expect("ask");
        assert!(a
            .message
            .starts_with("touch in '/etc/newfile' was blocked."));
        assert!(a.message.contains("create or modify files in"));
    }

    #[test]
    fn mkdir_out_of_cwd_asks() {
        let a = check("mkdir /opt/newdir").expect("ask");
        assert!(a.message.contains("create directories in"));
    }

    #[test]
    fn mv_out_of_cwd_asks() {
        // mv with no flags but an out-of-cwd target.
        let a = check("mv /proj/work/x /etc/y").expect("ask");
        assert!(a.message.starts_with("mv in "));
        assert!(a.message.contains("move files to/from"));
    }

    #[test]
    fn cp_out_of_cwd_asks() {
        let a = check("cp /proj/work/x /etc/y").expect("ask");
        // /proj/work/x is inside cwd (ok), /etc/y is the one that trips it.
        assert!(a.message.contains("/etc/y"));
        assert!(a.message.contains("copy files to/from"));
    }

    #[test]
    fn sed_in_place_out_of_cwd_asks() {
        // sed -i (write) editing a file out of cwd → ask (write op).
        let a = check("sed -i s/a/b/ /etc/hosts").expect("ask");
        assert!(a.message.starts_with("sed in '/etc/hosts' was blocked."));
        assert!(a.message.contains("edit files in"));
    }

    #[test]
    fn write_create_in_cwd_not_blocked() {
        assert!(check("touch ./newfile").is_none());
        assert!(check("mkdir sub/newdir").is_none());
        assert!(check("mv a.txt b.txt").is_none());
        assert!(check("cp a.txt b.txt").is_none());
    }

    // ── D2: mv/cp WITH a flag → ask (COMMAND_VALIDATOR) ────────────────────

    #[test]
    fn mv_with_flag_asks_even_in_cwd() {
        let a = check("mv -f a.txt b.txt").expect("ask");
        assert_eq!(
            a.message,
            "mv with flags requires manual approval to ensure path safety. For \
             security, LingXi cannot automatically validate mv commands \
             that use flags, as some flags like --target-directory=PATH can \
             bypass path validation."
        );
        assert_eq!(a.reason, "mv command with flags requires manual approval");
    }

    #[test]
    fn cp_with_flag_asks_even_in_cwd() {
        let a = check("cp -r src dst").expect("ask");
        assert!(a
            .message
            .starts_with("cp with flags requires manual approval"));
        assert_eq!(a.reason, "cp command with flags requires manual approval");
    }

    #[test]
    fn cp_target_directory_flag_cannot_bypass() {
        // The exact attack the validator defends: --target-directory=/etc.
        let a = check("cp --target-directory=/etc a.txt").expect("ask");
        assert!(a
            .message
            .starts_with("cp with flags requires manual approval"));
    }

    // ── D3: git diff --no-index path extraction ────────────────────────────

    #[test]
    fn git_diff_no_index_out_of_cwd_asks() {
        let a = check("git diff --no-index /proj/work/a /etc/passwd").expect("ask");
        assert!(a.message.contains("/etc/passwd"));
        assert!(a.message.contains("access files with git from"));
    }

    #[test]
    fn git_diff_no_index_both_in_cwd_not_blocked() {
        assert!(check("git diff --no-index a.txt b.txt").is_none());
    }

    #[test]
    fn git_other_subcommands_not_path_validated() {
        // Plain `git diff` / `git show` are git's own boundary → no extraction.
        assert!(check("git diff HEAD~1").is_none());
        assert!(check("git show /etc/passwd").is_none());
        assert!(check("git add /etc/x").is_none());
    }

    // ── D4: $/%/=/tilde-variant in a path arg → ask ────────────────────────

    #[test]
    fn dollar_expansion_in_path_asks() {
        let a = check("cat $HOME/secret").expect("ask");
        assert_eq!(
            a.message,
            "Shell expansion syntax in paths requires manual approval"
        );
        assert_eq!(a.reason, a.message);
    }

    #[test]
    fn percent_in_path_is_not_shell_expansion_on_posix() {
        // `%VAR%` is gated to Windows in claude-code's `TPt`
        // (`zt()==="windows" && o.includes("%")`); on posix a bare `%` is a legal
        // path character and must NOT raise the shell-expansion ask. LingXi
        // previously checked `%` unconditionally → over-asked vs claude-code.
        if let Some(a) = check("cat %TEMP%/x") {
            assert_ne!(
                a.message, "Shell expansion syntax in paths requires manual approval",
                "`%` must not raise the shell-expansion ask on posix; got {a:?}"
            );
        }
    }

    #[test]
    fn backtick_command_substitution_in_path_asks() {
        // Backtick command substitution must raise the shell-expansion ask
        // (claude-code `TPt` `o.includes("`")`); LingXi previously omitted this
        // check, under-asking vs claude-code.
        let a = check("cat /tmp/`id`").expect("ask");
        assert_eq!(
            a.message,
            "Shell expansion syntax in paths requires manual approval"
        );
    }

    #[test]
    fn equals_prefix_in_path_asks() {
        // Leading `=` triggers Zsh equals expansion.
        let a = check("cat =rg").expect("ask");
        assert_eq!(
            a.message,
            "Shell expansion syntax in paths requires manual approval"
        );
    }

    #[test]
    fn tilde_variant_in_path_asks() {
        // ~root is NOT expanded by expandTilde → tilde-variant ask.
        let a = check("cat ~root/.ssh/id_rsa").expect("ask");
        assert_eq!(
            a.message,
            "Tilde expansion variants (~user, ~+, ~-) in paths require manual approval"
        );
        assert_eq!(a.reason, a.message);
    }

    #[test]
    fn tilde_plus_minus_in_path_asks() {
        assert_eq!(
            check("cat ~+/x").unwrap().message,
            "Tilde expansion variants (~user, ~+, ~-) in paths require manual approval"
        );
        assert_eq!(
            check("cat ~-/x").unwrap().message,
            "Tilde expansion variants (~user, ~+, ~-) in paths require manual approval"
        );
    }

    #[test]
    fn bare_tilde_path_is_expanded_then_containment_checked() {
        // `cat ~/foo` → /home/u/foo is OUTSIDE cwd → containment ask (NOT the
        // tilde-variant ask: bare ~ IS expanded).
        let a = check("cat ~/foo").expect("ask");
        assert!(a.message.starts_with("cat in '/home/u/foo' was blocked."));
    }

    // ── D5: glob metachar in a write/create path → ask ─────────────────────

    #[test]
    fn glob_in_write_path_asks() {
        // touch is a create op; a glob in a create path is rejected outright.
        let a = check("touch /proj/work/*.txt").expect("ask");
        assert_eq!(
            a.message,
            "Glob patterns are not allowed in write operations. Please specify an exact file path."
        );
        assert_eq!(a.reason, a.message);
    }

    #[test]
    fn brace_in_write_path_asks_with_brace_message() {
        // touch is a create op; a brace in the write target gets the SEPARATE
        // brace guard (claude-code `TEd`, v2.1.185), NOT the glob message.
        let a = check("touch /proj/work/{a,b}.txt").expect("ask");
        assert_eq!(
            a.message,
            "Brace characters in write target require manual approval \u{2014} bash may brace-expand to paths outside the working directory"
        );
        assert_eq!(a.reason, a.message);
    }

    #[test]
    fn brace_in_read_path_is_not_glob_or_brace_ask() {
        // A braced READ path is not a glob in v2.1.185 (`yPt` excludes `{}`), and
        // the brace guard is write-only — so it raises neither the glob message
        // nor the brace message (it containment-checks the full path).
        if let Some(a) = check("cat /proj/work/{a,b}.txt") {
            assert_ne!(
                a.message,
                "Glob patterns are not allowed in write operations. Please specify an exact file path.",
                "a braced read path must not raise the glob message"
            );
            assert!(
                !a.message.starts_with("Brace characters in write target"),
                "the brace guard is write-only; got {a:?}"
            );
        }
    }

    #[test]
    fn glob_in_mkdir_path_asks() {
        let a = check("mkdir /proj/work/d?ir").expect("ask");
        assert!(a
            .message
            .starts_with("Glob patterns are not allowed in write operations"));
    }

    #[test]
    fn glob_in_read_path_validates_base_dir() {
        // For a READ op a glob validates its BASE directory. `cat /etc/*.conf`
        // → base /etc → out of cwd → containment ask (NOT the glob-write ask).
        let a = check("cat /etc/*.conf").expect("ask");
        assert!(a.message.starts_with("cat in '/etc' was blocked."));
    }

    #[test]
    fn glob_in_read_path_inside_cwd_not_blocked() {
        // `cat ./*.txt` → base cwd → inside → not blocked.
        assert!(check("cat ./*.txt").is_none());
        assert!(check("cat sub/*.txt").is_none());
    }

    // ── D6: compound cd + non-read write → ask ─────────────────────────────

    #[test]
    fn compound_cd_with_write_asks() {
        // `cd .lingxi/ && mv test.txt settings.json` — cd + a write op.
        let a = check("cd ./.claude && mv test.txt settings.json").expect("ask");
        assert_eq!(
            a.message,
            "Commands that change directories and perform write operations require explicit approval to ensure paths are evaluated correctly. For security, LingXi cannot automatically determine the final working directory when 'cd' is used in compound commands."
        );
        assert_eq!(
            a.reason,
            "Compound command contains cd with write operation - manual approval required to prevent path resolution bypass"
        );
    }

    #[test]
    fn compound_cd_with_touch_asks() {
        let a = check("cd sub && touch newfile").expect("ask");
        assert!(a
            .message
            .starts_with("Commands that change directories and perform write operations"));
    }

    #[test]
    fn compound_cd_with_read_not_blocked_by_this_guard() {
        // `cd sub && cat local.txt` — cat is a READ op, so the cd-write arm does
        // NOT fire; cat's own path (local.txt under cwd) is fine. (The cd target
        // is `path_constraints.rs`'s concern; here cd is skipped.)
        assert!(check("cd ./sub && cat local.txt").is_none());
    }

    // ── wrapper stripping ──────────────────────────────────────────────────

    #[test]
    fn wrapper_stripped_before_extraction() {
        // `timeout 10 cat /etc/passwd` must validate `cat`, not `timeout`.
        let a = check("timeout 10 cat /etc/passwd").expect("ask");
        assert!(a.message.starts_with("cat in '/etc/passwd' was blocked."));
    }

    #[test]
    fn nice_wrapper_stripped() {
        let a = check("nice cat /etc/passwd").expect("ask");
        assert!(a.message.starts_with("cat in '/etc/passwd' was blocked."));
    }

    // ── additional working dirs widen the allowance ────────────────────────

    #[test]
    fn read_into_additional_working_dir_passes() {
        let extra = vec![PathBuf::from("/tmp/scratch")];
        assert!(check_command_path_containment("cat /tmp/scratch/x", &roots(), &extra).is_none());
        let a = check_command_path_containment("cat /etc/passwd", &roots(), &extra).expect("ask");
        assert!(a.message.contains("'/proj/work', '/tmp/scratch'"));
    }

    // ── sed read-only override ─────────────────────────────────────────────

    #[test]
    fn sed_read_only_in_cwd_not_blocked() {
        // `sed -n '1,10p' ./local.txt` is read-only and inside cwd → not blocked.
        assert!(check("sed -n '1,10p' ./local.txt").is_none());
    }

    #[test]
    fn sed_read_only_out_of_cwd_asks_as_read() {
        // A read-only sed reading an out-of-cwd file → containment ask. Because
        // it's reclassified READ, the cd-write arm wouldn't apply even compound.
        let a = check("sed -n '1,10p' /etc/hosts").expect("ask");
        assert!(a.message.starts_with("sed in '/etc/hosts' was blocked."));
    }

    // ── non-path commands ride the allow rule (passthrough) ────────────────

    #[test]
    fn non_path_command_not_validated() {
        assert!(check("echo hello").is_none());
        assert!(check("npm install").is_none());
        assert!(check("python script.py").is_none());
        assert!(check("git status").is_none());
    }

    // ── POSIX `--` end-of-options handling ─────────────────────────────────

    #[test]
    fn double_dash_makes_dash_path_positional() {
        // `cat -- -/etc/passwd` — the `--` makes `-/etc/passwd` positional. It
        // resolves under cwd lexically (`/proj/work/-/etc/passwd`) so this exact
        // payload is in-cwd; the KEY is it IS extracted (not silently dropped).
        // Verify extraction directly:
        let toks = split_argv("cat -- -/x");
        let (_, rest) = toks.split_first().unwrap();
        assert_eq!(filter_out_flags(rest), vec!["-/x".to_string()]);
        // And a `--`-delimited out-of-cwd absolute IS caught.
        let a = check("cat -- /etc/passwd").expect("ask");
        assert!(a.message.starts_with("cat in '/etc/passwd' was blocked."));
    }

    // ── multiple subcommands: first violation wins ─────────────────────────

    #[test]
    fn first_violation_across_subcommands() {
        // `cat ./ok && cat /etc/passwd` — the second subcommand trips.
        let a = check("cat ./ok && cat /etc/passwd").expect("ask");
        assert!(a.message.contains("/etc/passwd"));
    }

    // ── quoted out-of-cwd path ─────────────────────────────────────────────

    #[test]
    fn quoted_out_of_cwd_path_asks() {
        let a = check("cat \"/etc/passwd\"").expect("ask");
        assert!(a.message.starts_with("cat in '/etc/passwd' was blocked."));
    }

    // ── PATH_EXTRACTORS unit coverage for the trickier commands ────────────

    #[test]
    fn tr_skips_character_sets() {
        // `tr a-z A-Z /etc/x` — SET1 + SET2 skipped, /etc/x is the path.
        assert_eq!(
            extract_paths("tr", &svec(&["a-z", "A-Z", "/etc/x"]), Some("/home/u")),
            vec!["/etc/x".to_string()]
        );
        // With -d only SET1 is skipped.
        assert_eq!(
            extract_paths("tr", &svec(&["-d", "a-z", "/etc/x"]), Some("/home/u")),
            vec!["/etc/x".to_string()]
        );
    }

    #[test]
    fn jq_filter_then_files() {
        // `jq '.x' a.json b.json` — filter then two file paths.
        assert_eq!(
            extract_paths("jq", &svec(&[".x", "a.json", "b.json"]), None),
            vec!["a.json".to_string(), "b.json".to_string()]
        );
        // No files → stdin (empty).
        assert!(extract_paths("jq", &svec(&[".x"]), None).is_empty());
    }

    #[test]
    fn sed_f_scriptfile_is_a_path() {
        // `sed -f script.sed file.txt` — both script.sed AND file.txt validated.
        assert_eq!(
            extract_paths("sed", &svec(&["-f", "script.sed", "file.txt"]), None),
            vec!["script.sed".to_string(), "file.txt".to_string()]
        );
        // `sed -e 's/a/b/' file.txt` — the expression is skipped, file kept.
        assert_eq!(
            extract_paths("sed", &svec(&["-e", "s/a/b/", "file.txt"]), None),
            vec!["file.txt".to_string()]
        );
    }

    #[test]
    fn find_path_taking_flags() {
        // `find . -newer ref.txt` — ref.txt is a path-taking flag arg.
        assert_eq!(
            extract_paths("find", &svec(&[".", "-newer", "ref.txt"]), None),
            vec![".".to_string(), "ref.txt".to_string()]
        );
        // `-name foo` is NOT a path flag → only `.` collected.
        assert_eq!(
            extract_paths("find", &svec(&[".", "-name", "foo"]), None),
            vec![".".to_string()]
        );
    }

    #[test]
    fn grep_recursive_no_path_defaults_cwd() {
        // `grep -r pattern` (recursive, no path) → current dir.
        assert_eq!(
            extract_paths("grep", &svec(&["-r", "pattern"]), None),
            vec![".".to_string()]
        );
    }

    // ── PATH-05: 2.1.211 extractor rework ──────────────────────────────────

    #[test]
    fn grep_f_pattern_file_is_validated() {
        // `grep -f /etc/shadow x.txt` — the pattern FILE (/etc/shadow) is pushed
        // AND the data file (x.txt).
        assert_eq!(
            extract_paths("grep", &svec(&["-f", "/etc/shadow", "x.txt"]), None),
            vec!["/etc/shadow".to_string(), "x.txt".to_string()]
        );
        // `--file=` and attached `-f` forms.
        assert_eq!(
            extract_paths("grep", &svec(&["--file=/etc/shadow"]), None),
            vec!["/etc/shadow".to_string()]
        );
        // rg too (with the `.` default absent because a path was collected).
        assert_eq!(
            extract_paths("rg", &svec(&["-f", "/etc/shadow"]), None),
            vec!["/etc/shadow".to_string()]
        );
    }

    #[test]
    fn awk_program_skipped_separator_not_a_path() {
        // `awk -F : '{print}' data.txt` — the `:` separator is consumed (not a
        // path), the `{print}` program is skipped, only data.txt is validated.
        assert_eq!(
            extract_paths("awk", &svec(&["-F", ":", "{print}", "data.txt"]), None),
            vec!["data.txt".to_string()]
        );
        // `awk '$1>5' f.txt` — the program (with `$`) is NOT treated as a path.
        assert_eq!(
            extract_paths("awk", &svec(&["$1>5", "f.txt"]), None),
            vec!["f.txt".to_string()]
        );
        // `-f script.awk data.txt` — the script FILE is validated.
        assert_eq!(
            extract_paths("awk", &svec(&["-f", "script.awk", "data.txt"]), None),
            vec!["script.awk".to_string(), "data.txt".to_string()]
        );
    }

    #[test]
    fn cut_paste_column_consume_flag_args() {
        // `cut -d , -f 1 file.txt` — `,` and `1` are flag args, only file.txt.
        assert_eq!(
            extract_paths("cut", &svec(&["-d", ",", "-f", "1", "file.txt"]), None),
            vec!["file.txt".to_string()]
        );
        // `column -s / f` — `/` is the separator arg, only `f` is a path.
        assert_eq!(
            extract_paths("column", &svec(&["-s", "/", "f"]), None),
            vec!["f".to_string()]
        );
        // `paste -d , a b` — `,` consumed, a and b are paths.
        assert_eq!(
            extract_paths("paste", &svec(&["-d", ",", "a", "b"]), None),
            vec!["a".to_string(), "b".to_string()]
        );
    }

    #[test]
    fn jq_from_file_and_slurpfile_validated() {
        // `-f prog.jq a.json` — the program file AND data file.
        assert_eq!(
            extract_paths("jq", &svec(&["-f", "prog.jq", "a.json"]), None),
            vec!["prog.jq".to_string(), "a.json".to_string()]
        );
        // `--slurpfile v data.json '.'` — the FILE (data.json, 2nd arg) is pushed,
        // the variable name `v` is not.
        assert_eq!(
            extract_paths("jq", &svec(&["--slurpfile", "v", "data.json", "."]), None),
            vec!["data.json".to_string()]
        );
    }

    #[test]
    fn git_diff_no_index_takes_all_positionals() {
        // `git diff --no-index A B C` — all three positionals validated (no 2-cap).
        assert_eq!(
            extract_paths(
                "git",
                &svec(&["diff", "--no-index", "A", "B", "C"]),
                None
            ),
            vec!["A".to_string(), "B".to_string(), "C".to_string()]
        );
    }

    // ── PATH-04: `..`-after-directory traversal pre-guard (SUr) ────────────

    #[test]
    fn dotdot_after_real_segment_is_flagged() {
        assert!(dotdot_after_directory_segment("sub/../ok.txt"));
        assert!(dotdot_after_directory_segment("a/b/../c"));
        assert!(dotdot_after_directory_segment("./sub/../x"));
        // Leading `..` (no real segment yet) is NOT flagged.
        assert!(!dotdot_after_directory_segment("../foo"));
        assert!(!dotdot_after_directory_segment("../../x"));
        assert!(!dotdot_after_directory_segment("foo/bar"));
        assert!(!dotdot_after_directory_segment("./x"));
    }

    #[test]
    fn cat_dotdot_after_segment_asks_with_traversal_message() {
        // `cat sub/../ok.txt` resolves inside cwd but still asks (symlink escape
        // defense) with the byte-locked message.
        let a = check_command_path_containment("cat sub/../ok.txt", &roots(), &[]).expect("ask");
        assert_eq!(
            a.message,
            "Path contains '..' traversal after a directory segment, which may follow a symlink outside the working directory"
        );
        // A leading `..` escaping cwd gets the generic containment message, NOT
        // the traversal one (SUr does not fire).
        let b = check_command_path_containment("cat ../secret", &roots(), &[]).expect("ask");
        assert!(!b
            .message
            .contains("traversal after a directory segment"));
    }

    fn svec(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| (*s).to_string()).collect()
    }
}
