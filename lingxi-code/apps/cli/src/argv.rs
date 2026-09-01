//! clap-derive argv struct for `lingxi-cli`. See plan M5-12 Task 0 step 1 +
//! step 5 for the locked flag names and help-text first lines.
//!
//! The help-text first lines are **byte-locked** by the plan; the
//! `clippy::doc_markdown` allow below is required to keep `end_turn`,
//! `messages_create`, etc. rendered exactly as documented (no backticks).
//! `clippy::struct_excessive_bools` is allowed because the flag set is a
//! direct projection of the locked CLI surface — refactoring into enums
//! would diverge from the spec table.

use clap::Parser;
use std::path::PathBuf;

/// clap value-parser for `--max-budget-usd`. Mirrors claude-code's arg parser
/// (`main.tsx`): `Number(value)` then reject `isNaN(amount) || amount <= 0`
/// with the byte-identical error message. A non-numeric argument (which JS would
/// coerce to `NaN`) and a non-positive number both fail the same way here.
fn parse_positive_budget_usd(value: &str) -> Result<f64, String> {
    let amount: f64 = value
        .parse()
        .map_err(|_| "--max-budget-usd must be a positive number greater than 0".to_string())?;
    // JS `Number("inf")` is `NaN` (rejected) and `Number("1e400")` is `Infinity`,
    // and claude's guard is `isNaN(amount) || amount <= 0`. Rust's `f64::FromStr`
    // instead parses "inf"/"INF"/"Infinity"/"1e400" all to `f64::INFINITY`, which
    // is neither NaN nor `<= 0` — so reject any non-finite value to match the
    // oracle's parse-time rejection (and bound the downstream cost cap).
    if !amount.is_finite() || amount <= 0.0 {
        return Err("--max-budget-usd must be a positive number greater than 0".to_string());
    }
    Ok(amount)
}

/// clap value-parser for `--task-budget <tokens>`, 1:1 with the oracle's
/// argParser (@307403649):
///
/// ```js
/// (l)=>{let c=Nte(l);if(isNaN(c)||c<=0||!Number.isInteger(c))
///        throw new j3t("--task-budget must be a positive integer");return c}
/// ```
///
/// The rejection copy is byte-exact from that `j3t` throw.
fn parse_task_budget(value: &str) -> Result<u64, String> {
    const INVALID: &str = "--task-budget must be a positive integer";
    let parsed: f64 = value.trim().parse().map_err(|_| INVALID.to_string())?;
    if !parsed.is_finite() || parsed <= 0.0 || parsed.fract() != 0.0 {
        return Err(INVALID.to_string());
    }
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    Ok(parsed as u64)
}

/// Resolved `--autocompact <auto|tokens>` value.
///
/// 1:1 with the return of claude-code 2.1.238's `DUn` (cc-238.js @222905882):
/// the string `"auto"` or a rounded token count inside `[Lli, hRa]`
/// (`Lli=1e5`, `hRa=1e6`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AutocompactWindow {
    /// `auto` — no pinned window (the oracle's `lvp` maps this to `undefined`,
    /// dropping any configured `autoCompactWindow`).
    Auto,
    /// A pinned auto-compact window, in tokens (100_000..=1_000_000).
    Tokens(u64),
}

/// JS `parseFloat` — parse the longest leading decimal-literal prefix, ignoring
/// trailing garbage; `NaN` when there is no such prefix. This is what the
/// oracle's `--autocompact` `k`/`m` branches call on the SUFFIXED string
/// (`parseFloat("500k") === 500`).
fn js_parse_float(value: &str) -> f64 {
    let bytes = value.as_bytes();
    let mut end = 0usize;
    if end < bytes.len() && (bytes[end] == b'+' || bytes[end] == b'-') {
        end += 1;
    }
    let int_start = end;
    while end < bytes.len() && bytes[end].is_ascii_digit() {
        end += 1;
    }
    let mut has_digits = end > int_start;
    if end < bytes.len() && bytes[end] == b'.' {
        let frac_start = end + 1;
        let mut frac_end = frac_start;
        while frac_end < bytes.len() && bytes[frac_end].is_ascii_digit() {
            frac_end += 1;
        }
        if has_digits || frac_end > frac_start {
            has_digits = true;
            end = frac_end;
        }
    }
    if !has_digits {
        return f64::NAN;
    }
    // Optional exponent, only when it is complete (`"1e"` parses as `1`).
    if end < bytes.len() && (bytes[end] == b'e' || bytes[end] == b'E') {
        let mut exp_end = end + 1;
        if exp_end < bytes.len() && (bytes[exp_end] == b'+' || bytes[exp_end] == b'-') {
            exp_end += 1;
        }
        let digits_start = exp_end;
        while exp_end < bytes.len() && bytes[exp_end].is_ascii_digit() {
            exp_end += 1;
        }
        if exp_end > digits_start {
            end = exp_end;
        }
    }
    value[..end].parse::<f64>().unwrap_or(f64::NAN)
}

/// JS `parseInt(value, 10)` — leading sign + decimal digits, trailing garbage
/// ignored; `NaN` when no digits lead.
fn js_parse_int(value: &str) -> f64 {
    let bytes = value.as_bytes();
    let mut end = 0usize;
    if end < bytes.len() && (bytes[end] == b'+' || bytes[end] == b'-') {
        end += 1;
    }
    let digits_start = end;
    while end < bytes.len() && bytes[end].is_ascii_digit() {
        end += 1;
    }
    if end == digits_start {
        return f64::NAN;
    }
    value[..end].parse::<f64>().unwrap_or(f64::NAN)
}

/// The oracle's `By` (cc-238.js @217558529): `T8y(t) ?? parseInt(t, 10)`, where
/// `T8y` (@217558371) accepts, for inputs of at most 32 chars, an exponent
/// literal that is an integer (`v8y`) or a grouped-thousands literal (`Ggu`,
/// separators `_ , NBSP NNBSP SPACE` stripped via `Vgu`).
fn js_parse_loose_number(value: &str) -> f64 {
    if value.chars().count() <= 32 {
        if matches_exponent_number(value) {
            let parsed = value.parse::<f64>().unwrap_or(f64::NAN);
            // `Number.isInteger(t) ? t : NaN`.
            return if parsed.is_finite() && parsed.fract() == 0.0 {
                parsed
            } else {
                f64::NAN
            };
        }
        if matches_grouped_thousands(value) {
            let stripped: String = value
                .chars()
                .filter(|c| !matches!(c, '_' | ',' | '\u{00A0}' | '\u{202F}' | ' '))
                .collect();
            return js_parse_int(&stripped);
        }
    }
    js_parse_int(value)
}

/// `v8y = /^[+-]?(\d+(\.\d*)?|\.\d+)[eE][+-]?\d+$/`.
fn matches_exponent_number(value: &str) -> bool {
    let rest = value.strip_prefix(['+', '-']).unwrap_or(value);
    let Some((mantissa, exponent)) = rest.split_once(['e', 'E']) else {
        return false;
    };
    let mantissa_ok = match mantissa.split_once('.') {
        Some((int_part, frac_part)) => {
            (!int_part.is_empty()
                && int_part.bytes().all(|b| b.is_ascii_digit())
                && frac_part.bytes().all(|b| b.is_ascii_digit()))
                || (int_part.is_empty()
                    && !frac_part.is_empty()
                    && frac_part.bytes().all(|b| b.is_ascii_digit()))
        }
        None => !mantissa.is_empty() && mantissa.bytes().all(|b| b.is_ascii_digit()),
    };
    let exponent = exponent.strip_prefix(['+', '-']).unwrap_or(exponent);
    mantissa_ok && !exponent.is_empty() && exponent.bytes().all(|b| b.is_ascii_digit())
}

/// `Ggu = /^[+-]?\d{1,3}([_,\u00A0\u202F ])\d{3}(?:\1\d{3})*$/`.
fn matches_grouped_thousands(value: &str) -> bool {
    let rest = value.strip_prefix(['+', '-']).unwrap_or(value);
    let mut chars = rest.chars();
    let mut lead = String::new();
    let separator = loop {
        match chars.next() {
            Some(c) if c.is_ascii_digit() => {
                lead.push(c);
                if lead.len() > 3 {
                    return false;
                }
            }
            Some(c) if matches!(c, '_' | ',' | '\u{00A0}' | '\u{202F}' | ' ') => break c,
            _ => return false,
        }
    };
    if lead.is_empty() {
        return false;
    }
    let mut groups = 0usize;
    loop {
        let group: String = chars.by_ref().take(3).collect();
        if group.len() != 3 || !group.bytes().all(|b| b.is_ascii_digit()) {
            return false;
        }
        groups += 1;
        match chars.next() {
            None => return groups >= 1,
            Some(c) if c == separator => {}
            Some(_) => return false,
        }
    }
}

/// clap value-parser for `--autocompact <auto|tokens>`, 1:1 with the oracle's
/// `argParser` (cc-238.js @243908809 → `DUn` @222905882):
///
/// ```js
/// function DUn(e){let t=e.trim().toLowerCase();if(t==="auto")return"auto";let r;
/// if(t.endsWith("m"))r=parseFloat(t)*1e6;else if(t.endsWith("k"))r=parseFloat(t)*1000;
/// else{let n=By(t);r=n>=100&&n<=1000?n*1000:n}
/// if(!Number.isFinite(r)||r<Lli||r>hRa)return;return Math.round(r)}
/// ```
///
/// The rejection copy is byte-exact from the `j3t` throw at the same offset.
fn parse_autocompact_window(value: &str) -> Result<AutocompactWindow, String> {
    const INVALID: &str =
        "It must be 'auto', or between 100k and 1M (e.g. 500k, 200000, or 200 as shorthand)";
    let normalized = value.trim().to_lowercase();
    if normalized == "auto" {
        return Ok(AutocompactWindow::Auto);
    }
    // `parseFloat` runs on the SUFFIXED string — it stops at the `k`/`m`.
    let tokens = if normalized.ends_with('m') {
        js_parse_float(&normalized) * 1e6
    } else if normalized.ends_with('k') {
        js_parse_float(&normalized) * 1000.0
    } else {
        let bare = js_parse_loose_number(&normalized);
        if (100.0..=1000.0).contains(&bare) {
            bare * 1000.0
        } else {
            bare
        }
    };
    if !tokens.is_finite() || tokens < 100_000.0 || tokens > 1_000_000.0 {
        return Err(INVALID.to_string());
    }
    // `Math.round(r)`: `r` is positive here (anything below 1e5 was rejected),
    // so JS's round-half-up and Rust's round-half-away-from-zero agree.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let rounded = tokens.round() as u64;
    Ok(AutocompactWindow::Tokens(rounded))
}

/// Normalize Claude's ordered comma-separated fallback list while keeping the
/// existing public field backward compatible as a single CSV string.
fn parse_fallback_model_list(value: &str) -> Result<String, String> {
    let mut models: Vec<String> = Vec::new();
    for raw in value.split(',') {
        let model = raw.trim();
        if model.is_empty() {
            return Err("--fallback-model entries must not be empty".to_string());
        }
        if !models.iter().any(|existing| existing.as_str() == model) {
            models.push(model.to_string());
        }
    }
    Ok(models.join(","))
}

/// Help layout matching the oracle's section order (`Usage:` first, then the
/// description, then Arguments / Options / Commands).
pub const HELP_TEMPLATE: &str = "\
Usage: {usage}

{about-with-newline}
{all-args}{after-help}";

/// The 2.1.251 restricted-mode help text. Keep this as one canonical literal
/// so the CLI help and downstream snapshots cannot drift from the launcher's
/// security contract.
pub const RESTRICTED_HELP: &str = "Restricted mode: removes the built-in tools that run commands or code (Bash, PowerShell, REPL and the other code-running tools) and WebFetch unless --tools names them, and ignores user, project and local settings files (managed settings and --settings still apply; add --strict-mcp-config to skip MCP servers too). Also confines the file tools to the working directories (--add-dir included), refuses bypassPermissions, and lets only a person or the configured permission handler approve writes to settings, git and tool-configuration files.";

/// AI coding assistant — runs a single turn or REPL
#[derive(Debug, Parser, Clone, Default)]
// Section ORDER follows the oracle: `Usage:` first, then the description, then
// Arguments / Options / Commands. clap's default leads with the description and
// puts Commands before Options, which was the "--help layout" divergence
// recorded in the 2.1.216 audit.
//
// The remaining differences are structural to clap vs commander (its two-column
// wrap and its `[OPTIONS]` usage placeholder) or are deliberate branding, so
// this aligns the layout without pretending the text can be byte-identical to a
// differently-named product with a different command set.
#[command(
    name = "lingxi-cli",
    version,
    disable_version_flag = true,
    about,
    long_about = None,
    help_template = crate::argv::HELP_TEMPLATE,
    term_width = 80
)]
#[allow(clippy::struct_excessive_bools, clippy::doc_markdown)]
pub struct Argv {
    /// Top-level subcommand (mcp, auth, plugin, project, setup-token, agents, attach,
    /// install, update, doctor, auto-mode, ultrareview, gateway). Declared BEFORE the
    /// `prompt` positional so clap resolves a leading subcommand-name token as
    /// the subcommand (and an optional-value global flag like `-d`/`-r` before
    /// it can't swallow it) rather than as the chat `[prompt]`. `None` = the
    /// normal chat / REPL / print path.
    #[command(subcommand)]
    pub command: Option<crate::commands::Commands>,

    /// The user prompt for this one-shot conversation
    // When absent (and `--resume` is not set), enters REPL mode (M5-13).
    pub prompt: Option<String>,

    /// Output the version number
    // Claude advertises lowercase `-v` and also accepts legacy uppercase `-V`.
    #[arg(
        short = 'v',
        short_alias = 'V',
        long = "version",
        action = clap::ArgAction::Version
    )]
    pub version: Option<bool>,

    /// Print mode: exit after first end_turn
    #[arg(short = 'p', long = "print")]
    pub print: bool,

    /// Resume a previous session by UUID (or interactive picker if absent)
    //
    // claude-code: `-r, --resume [value]` — "Resume a conversation by session
    // ID, or open interactive picker with optional search term" (`main.tsx:988`).
    // The value is OPTIONAL (`[value]`): `-r`/`--resume` with no argument yields
    // the empty-string picker sentinel; with an argument it carries the id /
    // search term. (The user-facing help first line is byte-locked by plan
    // M5-12 / `cli_help.rs`, so it is kept as the original wording above.)
    #[arg(short = 'r', long = "resume", value_name = "ID", num_args = 0..=1, default_missing_value = "")]
    pub resume: Option<String>,

    /// Continue the most recent conversation in the current directory
    //
    // claude-code: `-c, --continue` (`main.tsx:988`). FLAG PARSE ONLY here — the
    // continue runtime (load-most-recent-in-cwd) is wired by the CLI entrypoint
    // (`lib.rs` / `run.rs`), not this struct.
    #[arg(short = 'c', long = "continue")]
    pub continue_session: bool,

    /// When resuming, create a new session ID instead of reusing the original (use with --resume or --continue)
    //
    // claude-code: `--fork-session` (`main.tsx:988`). FLAG PARSE ONLY here — the
    // fork runtime (mint a fresh session id on resume) is wired downstream.
    #[arg(long = "fork-session")]
    pub fork_session: bool,

    /// Override the active model (e.g. claude-opus-4-7)
    #[arg(long = "model", value_name = "NAME")]
    pub model: Option<String>,

    /// Enable automatic fallback to specified model when default model is overloaded (only works with --print)
    //
    // Maps to `OrchestratorConfig::fallback_model`. claude-code accepts this
    // flag unconditionally but only HONORS it in `--print`/non-interactive mode
    // (`main.tsx:1000` documents "only works with --print"; it is consumed only
    // on the print/query path). We mirror that SOFT restriction: parse it always
    // (no parse-time `requires` error, matching claude-code), and the honoring is
    // gated to print mode by the consumer. When the primary model hits the
    // consecutive-529 Opus gate, the turn loop switches to this model
    // (`query.ts:894-948`).
    #[arg(
        long = "fallback-model",
        value_name = "MODEL",
        value_parser = parse_fallback_model_list
    )]
    pub fallback_model: Option<String>,

    /// Start a remote-control session using Anthropic's hosted relay. LingXi
    /// parses this surface for compatibility and then fails explicitly because
    /// that private relay contract is unavailable.
    #[arg(
        long = "remote-control",
        value_name = "name",
        num_args = 0..=1,
        default_missing_value = ""
    )]
    pub remote_control: Option<String>,

    /// Prefix used by remote-control session names.
    #[arg(long = "remote-control-session-name-prefix", value_name = "prefix")]
    pub remote_control_session_name_prefix: Option<String>,

    /// Maximum number of agentic turns before the loop early-exits (claude-code
    /// `--max-turns <turns>`, "only works with --print"). Maps to
    /// `OrchestratorConfig::max_turns`; unset (or `0`) = unbounded.
    ///
    /// HIDDEN in claude-code 2.1.191 (`.hideHelp()`; absent from `claude
    /// --help`) — mirrored here with `hide = true`.
    #[arg(long = "max-turns", value_name = "turns", hide = true)]
    pub max_turns: Option<u32>,

    /// `--plan-mode-instructions <instructions>` (hidden, `--print`-only): custom
    /// workflow body for plan mode, threaded to
    /// `OrchestratorConfig::plan_mode_instructions`.
    ///
    /// 206 help text: "Custom workflow body for plan mode. Replaces the default
    /// code-implementation phases in the plan-mode system reminder; the read-only
    /// enforcement preamble and ExitPlanMode protocol footer are always kept."
    /// HIDDEN in 206 (`.hideHelp()`) — mirrored with `hide = true`.
    #[arg(
        long = "plan-mode-instructions",
        value_name = "instructions",
        hide = true
    )]
    pub plan_mode_instructions: Option<String>,

    /// Maximum dollar amount to spend on API calls (only works with --print).
    /// Must be a positive number greater than 0.
    // Maps to `OrchestratorConfig::max_budget_nano_usd` (× 1e9); unset = no cap.
    // Parity with claude-code's arg parser.
    #[arg(long = "max-budget-usd", value_name = "amount", value_parser = parse_positive_budget_usd)]
    pub max_budget_usd: Option<f64>,

    /// Change to this directory before initialising
    #[arg(long = "cwd", value_name = "DIR")]
    pub cwd: Option<PathBuf>,

    /// Disable streaming SSE; use batched messages_create instead
    #[arg(long = "no-stream")]
    pub no_stream: bool,

    /// Emit machine-readable NDJSON to stdout (one event per line)
    //
    // LingXi-specific alias for `--output-format json`. The binary equivalent is
    // `--output-format json`; `--json` keeps backward compatibility with callers
    // that used the LingXi-specific flag before `--output-format` was added.
    #[arg(long = "json")]
    pub json: bool,

    /// Output format (only works with --print): "text" (default), "json" (single result), or "stream-json" (realtime streaming)
    ///
    /// `text` = plain stdout (default). `json` = same as `--json` (NDJSON per event).
    /// `stream-json` = realtime bidirectional NDJSON I/O protocol (SDK consumers).
    #[arg(long = "output-format", value_name = "format", value_parser = ["text", "json", "stream-json"])]
    pub output_format: Option<String>,

    /// Input format (only works with --print): "text" (default), or "stream-json" (realtime streaming input)
    #[arg(long = "input-format", value_name = "format", value_parser = ["text", "stream-json"])]
    pub input_format: Option<String>,

    /// System prompt to use for the session
    ///
    /// When set, replaces the default assembled system prompt for the session.
    /// Only works with --print (non-interactive mode).
    #[arg(long = "system-prompt", value_name = "prompt")]
    pub system_prompt: Option<String>,

    /// Append a system prompt to the default system prompt
    ///
    /// When set, appended to the end of the default system prompt for the session.
    /// Only works with --print (non-interactive mode).
    #[arg(long = "append-system-prompt", value_name = "prompt")]
    pub append_system_prompt: Option<String>,

    /// System prompt file to use for the session (hidden flag)
    ///
    /// Like `--system-prompt` but reads the prompt from a file.
    #[arg(long = "system-prompt-file", value_name = "file", hide = true)]
    pub system_prompt_file: Option<PathBuf>,

    /// Append a system prompt from a file to the default system prompt (hidden flag)
    ///
    /// Like `--append-system-prompt` but reads the prompt to append from a file.
    #[arg(long = "append-system-prompt-file", value_name = "file", hide = true)]
    pub append_system_prompt_file: Option<PathBuf>,

    /// Comma or space-separated list of tool names to allow (e.g. "Bash(git *) Edit")
    #[arg(long = "allowedTools", visible_alias = "allowed-tools", value_name = "tools", num_args = 1..)]
    pub allowed_tools: Option<Vec<String>>,

    /// Comma or space-separated list of tool names to deny (e.g. "Bash(git *) Edit")
    #[arg(long = "disallowedTools", visible_alias = "disallowed-tools", value_name = "tools", num_args = 1..)]
    pub disallowed_tools: Option<Vec<String>>,

    /// Specify the list of available tools from the built-in set. Use "" to disable all tools
    #[arg(long = "tools", value_name = "tools", num_args = 1..)]
    pub tools: Option<Vec<String>>,

    /// Additional directories to allow tool access to
    //
    // claude-code `--add-dir <directories...>`. Unioned into the permission
    // policy's working-directory set (like `permissions.additionalDirectories`)
    // so file tools (Read/Edit/Bash) may operate outside `cwd`.
    #[arg(long = "add-dir", value_name = "directories", num_args = 1..)]
    pub add_dir: Option<Vec<PathBuf>>,

    /// Load settings from a JSON file or JSON string
    #[arg(long = "settings", value_name = "file-or-json")]
    pub settings: Option<String>,

    /// Load MCP servers from JSON files or strings (space-separated)
    #[arg(long = "mcp-config", value_name = "configs", num_args = 1..)]
    pub mcp_config: Option<Vec<String>>,

    /// Override verbose mode setting from config
    #[arg(long = "verbose")]
    pub verbose: bool,

    /// Minimal mode: skip hooks, LSP, plugin sync, attribution, auto-memory,
    /// background prefetches, keychain reads, and LINGXI.md auto-discovery. Sets
    /// LINGXI_SIMPLE=1. Anthropic auth is strictly ANTHROPIC_API_KEY or
    /// apiKeyHelper via --settings (OAuth and keychain are never read).
    //
    // WIRED: `run_cli` exports `LINGXI_SIMPLE=1` (binary
    // `process.env.CLAUDE_CODE_SIMPLE="1"` on a pre-`--` `--bare` token) and
    // threads `CustomizationGates{bare}` through `resolve_desktop_config` into
    // `engine_desktop::build` (skips settings hooks, plugins incl. plugin LSP,
    // skill/custom-command dirs, custom agents; LINGXI.md unless `--add-dir`).
    #[arg(long = "bare")]
    pub bare: bool,

    /// Start with all customizations disabled — useful for troubleshooting
    //
    // WIRED: `run_cli` exports `LINGXI_SAFE_MODE=1` +
    // `LINGXI_DISABLE_LINGXI_MDS=1` (binary @223917313 `if(Ql())process.env.
    // CLAUDE_CODE_SAFE_MODE="1",process.env.CLAUDE_CODE_DISABLE_CLAUDE_MDS=
    // "1"`) and threads `CustomizationGates{safe_mode}` into `engine_desktop::
    // build` (disables settings hooks, plugins, skills/custom commands, custom
    // agents, discovered `.mcp.json` servers — `--mcp-config` servers survive,
    // binary `fQ`'s `L2()` — and the LINGXI.md hierarchy).
    #[arg(long = "safe-mode")]
    pub safe_mode: bool,

    /// JSON object defining custom agents (e.g. '{"reviewer": {"description":
    /// "Reviews code", "prompt": "You are a code reviewer"}}')
    //
    // claude-code `--agents <json>` takes EXACTLY ONE value (a JSON object
    // string parsed downstream), not a space-separated list.
    //
    // WIRED: threads raw into `DesktopConfig.cli_agents_json`;
    // `engine_desktop::build` parses it with the strict flag-record schema
    // (`agent::parse_agents_from_flag_json_checked` at the CLI boundary;
    // invalid JSON/definitions abort before runtime construction) and
    // merges the result over dir-loaded agents (`flagSettings` precedence).
    // Ignored (warn) in safe mode; survives `--bare`.
    #[arg(long = "agents", value_name = "json")]
    pub agents: Option<String>,

    /// Agent for the current session. Overrides the 'agent' setting.
    //
    // claude-code `--agent <agent>` takes EXACTLY ONE value.
    //
    // WIRED: threads into `DesktopConfig.cli_agent`; `engine_desktop::build`
    // resolves it against the final agent catalog (binary `dts`: exact
    // agentType, else `…:{name}` FQN suffix) and logs the byte-matched
    // `Warning: agent "X" not found …` on a miss. On a HIT it APPLIES the agent
    // to the MAIN thread (`bde`/`mainThreadAgentDefinition`): the `agentType`
    // rides lifecycle hook payloads, the system prompt becomes the main-loop
    // prompt (`nre`, `--system-prompt` still winning), the `tools:` /
    // `disallowedTools` frontmatter narrows the tool pool (`HJ`), the agent
    // `model` overrides the main loop (`jb(Zo(model))`) unless `--model` was
    // given, and the frontmatter `hooks` register as `mainThreadAgentHooks`
    // (`Rft`→`o_n`, `is_agent=false` so Stop stays Stop; gated by the `g9e`
    // trusted-source set) BEFORE the SessionStart fire. The applied `agentType`
    // is PERSISTED as an `agent-setting` transcript record, so a later `--resume`
    // with NO `--agent` re-adopts it via `rVe` (prompt + tools + model + hooks
    // re-applied; a byte-exact `Resumed session had agent "X" but it is no longer
    // available. Using default behavior.` warning + default fallback on a miss).
    // A re-passed `--agent` on `--resume` wins (`rVe`'s `if(t)return`).
    // RESIDUAL (blocked on LingXi substrate, not CC parity): frontmatter
    // `mcpServers` swap (scope `"agent"`) — the composition-root MCP tool build
    // Arc-seals the `Arc<ToolRegistry>` before the final agent catalog is
    // assembled, so a late `connect_all` cannot surface the servers' tools to the
    // model (the same limitation plugin MCP servers already have; no
    // runtime-mutable tool registry exists).
    #[arg(long = "agent", value_name = "agent")]
    pub agent: Option<String>,

    /// Load a plugin from a directory or .zip for this session only
    /// (repeatable: --plugin-dir A --plugin-dir B.zip)
    //
    // WIRED: repeatable (commander `.option(...)` with an
    // array default `[]`); each entry threads into
    // `DesktopConfig.cli_plugin_dirs` and loads through the inline-plugin
    // path (`EBm` port `plugin::discover_cli_plugin_dirs`): missing path =
    // warn + skip, `.zip` extracted (guarded) then loaded like a dir, loaded
    // plugins enable through the SAME manager path as marketplace installs.
    // Survives `--bare` (its help lists `--plugin-dir` as explicit context);
    // not safe mode.
    #[arg(long = "plugin-dir", value_name = "path", action = clap::ArgAction::Append)]
    pub plugin_dir: Vec<PathBuf>,

    /// Load a plugin zip from a URL for this session only
    /// (repeatable: --plugin-url A --plugin-url B)
    //
    // WIRED: repeatable, downloaded at runtime into the same session-only temp
    // area the inline `--plugin-dir` zip loader uses, then threaded through
    // `DesktopConfig.cli_plugin_dirs` so the existing session-plugin path loads
    // it exactly like a local zip.
    #[arg(long = "plugin-url", value_name = "url", action = clap::ArgAction::Append)]
    pub plugin_url: Vec<String>,

    /// Disable session persistence - sessions will not be saved to disk and cannot be resumed (only works with --print)
    //
    // WIRED: non-print use hard-errors in `run_cli` (binary
    // @223929381 `if(a.sessionPersistence===!1&&!We)return Es("Error: --no-
    // session-persistence can only be used with --print mode.")`); the
    // accepted print case threads `DesktopConfig.session_persistence: false`
    // so `engine_desktop::build` wires NO session `JsonlWriter`.
    #[arg(long = "no-session-persistence")]
    pub no_session_persistence: bool,

    /// Resume a session linked to a PR by PR number/URL, or open interactive picker
    //
    // WIRED into the resume pickers: the binary hands the
    // picker `filterByPr: rt` (bare flag → `!0` = only PR-linked sessions;
    // a value is parsed by `wqc` — leading int, else a
    // `/(pull|pull-requests|-\/merge_requests)\/(\d+)/` URL — and filters
    // `prNumber === n`; an unparseable value applies NO narrowing). LingXi
    // routes `--from-pr` through the same pickers and projects persisted
    // `pr-link` metadata onto each resume row.
    // No `require_equals`: commander's `--from-pr [value]` consumes the next
    // SPACE-separated token as the value (`--from-pr 123`), so we must NOT force
    // the `--from-pr=123` form or `123` would be mis-parsed as the prompt.
    #[arg(long = "from-pr", value_name = "value", num_args = 0..=1, default_missing_value = "")]
    pub from_pr: Option<String>,

    /// Effort level for the current session (low, medium, high, xhigh, max)
    //
    // WIRED: `run_cli` normalizes via [`Argv::normalized_effort`]
    // (the binary's `--effort` argParser `u4i` @ the root option table:
    // trim+lowercase, alias `med`→`medium`, must be in `UR = ["low","medium",
    // "high","xhigh","max"]`; an unknown value writes `Warning: Unknown
    // --effort value '<raw>' — ignoring it and using the default effort.
    // Valid values: …` to stderr and IGNORES the flag). The valid level
    // threads into `DesktopConfig.initial_effort` → the main-loop requests'
    // `output_config.effort`.
    #[arg(long = "effort", value_name = "level")]
    pub effort: Option<String>,

    /// Enable beta features (Warning: Custom betas are only available for API key users)
    // Values are stable-deduplicated during startup and injected only into
    // first-party Anthropic API-key messages requests.
    #[arg(long = "betas", value_name = "betas", num_args = 1..)]
    pub betas: Option<Vec<String>>,

    /// Write debug logs to a specific file path (implicitly enables debug mode)
    #[arg(long = "debug-file", value_name = "path")]
    pub debug_file: Option<PathBuf>,

    /// Include partial message chunks as they arrive (only works with --print and --output-format=stream-json)
    #[arg(long = "include-partial-messages")]
    pub include_partial_messages: bool,

    /// Include all hook lifecycle events in the output stream (only works with --output-format=stream-json)
    #[arg(long = "include-hook-events")]
    pub include_hook_events: bool,

    /// Forward subagent text and thinking blocks as assistant/user messages with parent_tool_use_id set (only works with --print and --output-format=stream-json)
    #[arg(long = "forward-subagent-text")]
    pub forward_subagent_text: bool,

    /// Re-emit user messages from stdin back on stdout for acknowledgment (only works with --input-format=stream-json and --output-format=stream-json)
    #[arg(long = "replay-user-messages")]
    pub replay_user_messages: bool,

    /// Thinking mode: enabled (equivalent to adaptive), disabled (hidden flag)
    // Resolved into the boot session ThinkingConfig after explicit token budgets.
    #[arg(long = "thinking", value_name = "mode", value_parser = ["enabled", "adaptive", "disabled"], hide = true)]
    pub thinking: Option<String>,

    /// How thinking content appears in the response (hidden flag)
    // Applied to TUI/stream-json sinks; `omitted` suppresses display only and
    // leaves model-facing transcript reasoning intact.
    #[arg(long = "thinking-display", value_name = "display", value_parser = ["summarized", "omitted"], hide = true)]
    pub thinking_display: Option<String>,

    /// [DEPRECATED. Use --thinking instead for newer models] (hidden flag)
    //
    // Wired: `init::resolve_desktop_config` threads this into the boot session
    // `ThinkingConfig` via `llm_client::model::thinking::session_thinking_from_env`
    // as claude-code's `a.maxThinkingTokens` (the `wn` request-build arm) — used
    // when `MAX_THINKING_TOKENS` env is unset; a value `> 0` pins a fixed budget
    // (pre-empting adaptive), `0` disables thinking.
    #[arg(long = "max-thinking-tokens", value_name = "tokens", hide = true)]
    pub max_thinking_tokens: Option<u32>,

    /// Enable prompt suggestions. In print/SDK mode, emits a prompt_suggestion
    /// message after each turn with a predicted next user prompt
    //
    // VISIBLE in claude-code 2.1.191 with a fixed choices list and preset
    // "true": bare `--prompt-suggestions` → "true"; an out-of-choices value
    // (e.g. "banana") is HARD-REJECTED. The `value_parser` below mirrors that.
    //
    // VALIDATED: the binary's argParser returns a BOOLEAN
    // (`!Hl(i)` — falsy tokens false/0/no/off → false, the rest true), and a
    // TRUTHY value outside `--print` + `--output-format=stream-json` is a
    // fatal `Es(...)` (see [`Argv::validate_prompt_suggestions_args`],
    // enforced in `run_cli`; verified live: exit 1). RESIDUAL: the actual
    // per-turn `prompt_suggestion` stream-json message needs the
    // binary's post-turn prediction side-call (`prompt_suggestion_generate`),
    // which lingxi's print pipeline does not have — accepted flag is carried
    // but no suggestion messages are emitted yet.
    #[arg(
        long = "prompt-suggestions",
        value_name = "value",
        num_args = 0..=1,
        default_missing_value = "true",
        value_parser = ["true", "false", "1", "0", "yes", "no", "on", "off"]
    )]
    pub prompt_suggestions: Option<String>,

    /// Validate the final result against this JSON Schema, forcing structured
    /// output (only works with `--print`). The model is compelled to call a
    /// `StructuredOutput` tool whose schema is this; the result is validated and
    /// retried up to `MAX_STRUCTURED_OUTPUT_RETRIES` times. Pass a JSON Schema as
    /// a string.
    #[arg(long = "json-schema", value_name = "schema")]
    pub json_schema: Option<String>,

    /// Enable verbose logging to stderr (debug mode) with optional category
    /// filtering (e.g. "api,hooks" or "!1p,!file")
    //
    // claude-code: `-d, --debug [filter]`. The value is OPTIONAL: bare `-d` /
    // `--debug` enables debug mode unfiltered (empty sentinel); `--debug
    // api,hooks` carries the category filter. Stored as `Option<String>`:
    // `None` = off, `Some("")` = on/unfiltered, `Some(filter)` = on/filtered.
    // `debug_enabled()` collapses it back to the old bool for callers that
    // only need on/off (e.g. `logging::init`).
    // No `require_equals`: commander's `-d, --debug [filter]` consumes the next
    // SPACE-separated token as the optional filter (`--debug api,hooks`), and a
    // bare `--debug` before a subcommand-name token binds that token as the
    // filter exactly as the real binary does (it does NOT route to the
    // subcommand and does NOT start a billable turn — the leading `command`
    // subcommand resolution only triggers when the FIRST token is the command).
    #[arg(short = 'd', long = "debug", value_name = "filter", num_args = 0..=1, default_missing_value = "")]
    pub debug: Option<String>,

    /// Disable TUI; use stdio REPL (line-editing fallback)
    #[arg(long = "no-tui")]
    pub no_tui: bool,

    /// Bypass all permission checks. Recommended only for sandboxes with no
    /// internet access.
    #[arg(long = "dangerously-skip-permissions")]
    pub dangerously_skip_permissions: bool,

    /// Initial permission mode (`--permission-mode <mode>`).
    // claude-code 2.1.207 commander `.choices(bha)` where `bha=WB.map(e=>
    // e==="default"?"manual":e)` DISPLAYS `manual` in place of `default`, while
    // the accepted set `$7_=[...WB,"manual"]` keeps BOTH spellings (the shared
    // `ZS(e)=e==="manual"?"default":e` preprocess normalizes `manual`→`default`).
    // An out-of-set value is HARD-REJECTED at parse time. We mirror that with a
    // `PossibleValuesParser`: `manual` sits in the `default` slot (visible) and
    // `default` is a hidden-but-accepted alias, so help/errors show `manual` yet
    // `--permission-mode default` still parses. `auto` resolves to
    // `PermissionMode::Auto` and `manual`/`default` to `Default` downstream.
    #[arg(
        long = "permission-mode",
        // lowercase placeholder so the help line and the commander-style
        // invalid-value error read `--permission-mode <mode>` (not `<MODE>`).
        value_name = "mode",
        value_parser = clap::builder::PossibleValuesParser::new([
            clap::builder::PossibleValue::new("acceptEdits"),
            clap::builder::PossibleValue::new("auto"),
            clap::builder::PossibleValue::new("bypassPermissions"),
            clap::builder::PossibleValue::new("manual"),
            clap::builder::PossibleValue::new("default").hide(true),
            clap::builder::PossibleValue::new("dontAsk"),
            clap::builder::PossibleValue::new("plan"),
        ])
    )]
    pub permission_mode: Option<String>,

    // ── v2.1.191 parity: previously-missing public top-level flags ───────────
    // These are accepted by clap so `lingxi-cli` no longer HARD-ERRORS on a
    // visible `claude --help` flag. Behavioral wiring for the not-yet-ported
    // ones is tracked in task "Wire un-ported flag backing features"; until
    // then they parse-and-carry (accepted, inert) so scripts/SDK callers stop
    // breaking on contact. claude-code source: main.tsx flag registration.
    /// Use a specific session ID for the conversation (must be a valid UUID)
    //
    // claude-code `--session-id <uuid>`. Overrides the generated session id.
    #[arg(long = "session-id", value_name = "uuid")]
    pub session_id: Option<String>,

    /// Set a display name for this session (shown in the prompt box, /resume
    /// picker, and terminal title)
    #[arg(short = 'n', long = "name", value_name = "name")]
    pub name: Option<String>,

    /// Comma-separated list of setting sources to load (user, project, local).
    #[arg(long = "setting-sources", value_name = "sources")]
    pub setting_sources: Option<String>,

    /// Only use MCP servers from --mcp-config, ignoring all other MCP
    /// configurations
    #[arg(long = "strict-mcp-config")]
    pub strict_mcp_config: bool,

    /// Restricted mode: removes command/code-running built-ins and WebFetch,
    /// ignores user/project/local settings, confines file tools to cwd and
    /// --add-dir, refuses bypassPermissions, and protects settings/git/tool
    /// configuration writes from non-human approval.
    #[arg(long = "restricted", help = RESTRICTED_HELP)]
    pub restricted: bool,

    /// Move per-machine sections (cwd, env info, memory paths, git status) from
    /// the system prompt into the first user message. Improves cross-user
    /// prompt-cache reuse. Only applies with the default system prompt.
    #[arg(long = "exclude-dynamic-system-prompt-sections")]
    pub exclude_dynamic_system_prompt_sections: bool,

    /// [DEPRECATED. Use --debug instead] Enable MCP debug mode (shows MCP
    /// server errors)
    #[arg(long = "mcp-debug")]
    pub mcp_debug: bool,

    /// Automatically connect to IDE on startup if exactly one valid IDE is
    /// available
    #[arg(long = "ide")]
    pub ide: bool,

    /// MCP tool to use for permission prompts (only works with --print). Hidden
    /// SDK flag (claude-code registers it with `.hideHelp()`).
    #[arg(long = "permission-prompt-tool", value_name = "tool", hide = true)]
    pub permission_prompt_tool: Option<String>,

    /// Enable bypassing all permission checks as an option, without it being
    /// enabled by default. Recommended only for sandboxes with no internet
    /// access.
    #[arg(long = "allow-dangerously-skip-permissions")]
    pub allow_dangerously_skip_permissions: bool,

    /// Disable all skills
    #[arg(long = "disable-slash-commands")]
    pub disable_slash_commands: bool,

    /// Enable Claude in Chrome integration
    #[arg(long = "chrome", conflicts_with = "no_chrome")]
    pub chrome: bool,

    /// Disable Claude in Chrome integration
    #[arg(long = "no-chrome", conflicts_with = "chrome")]
    pub no_chrome: bool,

    /// Render screen-reader friendly output (flat text, no decorative borders
    /// or animations). Overridden by the LINGXI_AX_SCREEN_READER env var and
    /// the --ax-screen-reader CLI flag.
    #[arg(long = "ax-screen-reader")]
    pub ax_screen_reader: bool,

    /// File resources to download at startup. Format: file_id:relative_path
    /// (e.g. --file file_abc:doc.txt file_def:img.png)
    #[arg(long = "file", value_name = "specs", num_args = 1..)]
    pub file: Option<Vec<String>>,

    /// Create a new git worktree for this session (optionally specify a name)
    //
    // claude-code `-w, --worktree [name]`. Value is OPTIONAL: bare `-w` mints
    // an auto-named worktree (empty sentinel); `-w name` names it.
    #[arg(short = 'w', long = "worktree", value_name = "name", num_args = 0..=1, default_missing_value = "")]
    pub worktree: Option<String>,

    /// Create a tmux session for the worktree (requires --worktree). Uses iTerm2
    /// native panes when available; use --tmux=classic for traditional tmux.
    ///
    /// `--tmux` alone = native (empty sentinel); `--tmux=classic` = classic.
    // `require_equals` so the value only binds via `=` (a bare `--tmux` won't
    // swallow a following prompt token), matching commander's boolean-ish flag.
    #[arg(long = "tmux", value_name = "mode", num_args = 0..=1, require_equals = true, default_missing_value = "")]
    pub tmux: Option<String>,

    /// Auto-compact window size (auto, or 100k–1M tokens)
    //
    // claude-code 2.1.238 `--autocompact <auto|tokens>` (cc-238.js @243908809,
    // binary @307413819). NEW in 2.1.238 (0 hits in 2.1.220) and NOT hidden —
    // unlike its `--advisor` neighbour it carries no `.hideHelp()`, so it
    // renders in root `--help`. The oracle's main action resolves it with
    // `lvp(t.autocompact, Vo().autoCompactWindow)` (`lvp` @222906138:
    // `e===void 0 ? t : e==="auto" ? void 0 : e`) — absent ⇒ the configured
    // window, `auto` ⇒ no pin, a number ⇒ that pin.
    //
    // WIRED: `run_cli` projects this onto `LINGXI_AUTO_COMPACT_WINDOW`, the
    // port's only auto-compact-window knob (read by
    // `compaction::thresholds::effective_context_window_size`, and the source
    // the `/autocompact` handler reports). `auto` CLEARS a pre-set env pin,
    // matching `lvp`'s `void 0`.
    #[arg(long = "autocompact", value_name = "auto|tokens", value_parser = parse_autocompact_window)]
    pub autocompact: Option<AutocompactWindow>,

    /// Start the session as a background agent and return immediately (manage
    /// with `lingxi-cli agents`)
    #[arg(long = "bg", visible_alias = "background")]
    pub background: bool,

    /// Enable SendUserMessage tool for agent-to-user communication
    //
    // claude-code `--brief` (root `--help`, 2.1.215). DEFAULT-OFF: the
    // `SendUserMessage` (Brief) tool is invisible to the model unless this flag
    // is set — matching the shipped binary, whose `isBriefEnabled` (`aKr()`)
    // gates the tool on `Z.CLAUDE_CODE_BRIEF || dge("tengu_kairos_brief", !1)`.
    //
    // WIRED: `run_cli` publishes `e.brief` into the live session flag consumed
    // by `tool_ui::brief::brief_tool_enabled()`. The default toolset is
    // unchanged when the flag is absent; `/brief` can toggle the same state
    // during an interactive session. The Brief
    // entitlement / `userMsgOptIn` classification pipeline (`isBriefEntitled`,
    // `getBriefEnforceText`) is not ported — LingXi has no entitlement gate — so
    // the flag simply exposes the tool.
    #[arg(long = "brief")]
    pub brief: bool,

    // ─────────────────────────────────────────────────────────────────────
    // (CLI-12/13/15/16/17 + SC-09, cc 2.1.238) The remaining root-option
    // surface. Every spec + description below was read out of the 2.1.238
    // binary's root registration block; each carries `.hideHelp()` there, so
    // each is `hide = true` here and none of them changes `--help`.
    //
    // Where LingXi owns the machinery the flag is WIRED (noted per field).
    // Where it does not, the flag is PARSED and inert exactly as commander
    // parses it — the port must not hard-error on an argv the oracle accepts,
    // and must not fabricate the behaviour either. The load-bearing ones
    // (`--rewind-files`, the truncating-resume pair) carry their oracle
    // validation gates in `run_cli` and take the not-implemented path rather
    // than reporting a success they did not perform.
    // ─────────────────────────────────────────────────────────────────────
    /// (deprecated) Enable debug mode (to stderr)
    //
    // Oracle @307399127: `new bp("-d2e, --debug-to-stderr", …).argParser(Boolean)
    // .hideHelp().implies({debug:!0})`. The `-d2e` short form is a MULTI-char
    // "short" that clap cannot express (`Arg::short` takes one `char`), so only
    // the long form is registered; the `.implies({debug:!0})` half IS ported —
    // `debug_enabled()` treats this flag as `--debug`.
    #[arg(long = "debug-to-stderr", hide = true)]
    pub debug_to_stderr: bool,

    /// Run Setup hooks with init trigger, then continue
    //
    // PARSED, inert: the port has `HookEventType::Setup` in the hooks crate but
    // no firer outside it — nothing in `apps/cli` runs a Setup trigger, so the
    // flag cannot be wired from here without inventing the event.
    #[arg(long = "init", hide = true)]
    pub init: bool,

    /// Run Setup and SessionStart:startup hooks, then exit
    #[arg(long = "init-only", hide = true)]
    pub init_only: bool,

    /// Run Setup hooks with maintenance trigger, then continue
    #[arg(long = "maintenance", hide = true)]
    pub maintenance: bool,

    /// Emit transcript_mirror frames on stdout (SDK-internal; set by
    /// ProcessTransport when sessionStore is configured)
    #[arg(long = "session-mirror", hide = true)]
    pub session_mirror: bool,

    /// API-side task budget in tokens (output_config.task_budget)
    //
    // Server-side `output_config` field on the Anthropic request body; LingXi
    // is multi-provider and does not emit `output_config`. Parsed, inert.
    #[arg(long = "task-budget", value_name = "tokens", hide = true, value_parser = parse_task_budget)]
    pub task_budget: Option<u64>,

    /// Enable auth status messages in SDK mode
    #[arg(long = "enable-auth-status", hide = true)]
    pub enable_auth_status: bool,

    /// Workload tag for billing-header attribution (cc_workload).
    /// Process-scoped; set by SDK daemon callers that spawn subprocesses for
    /// cron work. (only works with --print)
    //
    // `cc_workload` is an Anthropic billing header. Parsed, inert.
    #[arg(long = "workload", value_name = "tag", hide = true)]
    pub workload: Option<String>,

    /// Policy-tier settings JSON from a spawning parent process (SDK use only)
    #[arg(long = "managed-settings", value_name = "json", hide = true)]
    pub managed_settings: Option<String>,

    /// Like --plugin-dir but the engine will not read this plugin's .mcp.json
    /// (caller owns its MCP connections)
    //
    // Oracle @307411913 — the ROOT copy (the `agents` subcommand's shorter
    // twin is already ported at `commands/agents.rs`). `argParser((l,c)=>[...c,l])`
    // + `.default([])` ⇒ repeatable, so `ArgAction::Append` like `--plugin-dir`.
    #[arg(long = "plugin-dir-no-mcp", value_name = "path", hide = true, action = clap::ArgAction::Append)]
    pub plugin_dir_no_mcp: Option<Vec<String>>,

    /// Enable the server-side advisor tool with the specified model (alias or
    /// full ID).
    //
    // The advisor is a SERVER-side Anthropic tool; LingXi has no such tool.
    #[arg(long = "advisor", value_name = "model", hide = true)]
    pub advisor: Option<String>,

    /// MCP servers whose channel notifications (inbound push) should register
    /// this session. Space-separated server names.
    #[arg(long = "channels", value_name = "servers", num_args = 1.., hide = true)]
    pub channels: Option<Vec<String>>,

    /// Load channel servers not on the approved allowlist. For local channel
    /// development only. Shows a confirmation dialog at startup.
    #[arg(
        long = "dangerously-load-development-channels",
        value_name = "servers",
        num_args = 1..,
        hide = true
    )]
    pub dangerously_load_development_channels: Option<Vec<String>>,

    /// Use remote WebSocket endpoint for SDK I/O streaming (only with -p and
    /// stream-json format)
    #[arg(long = "sdk-url", value_name = "url", hide = true)]
    pub sdk_url: Option<String>,

    /// Pre-fill the prompt input with text without submitting it
    #[arg(long = "prefill", value_name = "text", hide = true)]
    pub prefill: Option<String>,

    /// Base64url-encoded --prefill value (deep-link shell-safe launch paths)
    //
    // Oracle argParser: `Buffer.from(l,"base64url").toString("utf8")` — Node's
    // decoder never throws, so the raw string is kept here and decoded lazily
    // by [`Argv::resolve_prefill`] with the same lenient semantics.
    #[arg(long = "prefill-b64", value_name = "b64", hide = true)]
    pub prefill_b64: Option<String>,

    /// Signal that this session was launched from a deep link
    #[arg(long = "deep-link-origin", hide = true)]
    pub deep_link_origin: bool,

    /// Repo slug the deep link ?repo= parameter resolved to the current cwd
    #[arg(long = "deep-link-repo", value_name = "slug", hide = true)]
    pub deep_link_repo: Option<String>,

    /// FETCH_HEAD mtime in epoch ms, precomputed by the deep link trampoline
    //
    // Oracle argParser: `let c=Number(l); return Number.isFinite(c)?c:void 0` —
    // a non-numeric value is DROPPED, not rejected. A typed `Option<f64>` here
    // would make clap hard-error, so the raw token is kept and
    // [`Argv::deep_link_last_fetch_ms`] applies the oracle's finite check.
    #[arg(long = "deep-link-last-fetch", value_name = "ms", hide = true)]
    pub deep_link_last_fetch: Option<String>,

    /// Base64url-encoded working directory (deep-link shell-safe launch paths)
    #[arg(long = "deep-link-cwd-b64", value_name = "b64", hide = true)]
    pub deep_link_cwd_b64: Option<String>,

    /// When resuming, immediately query if the loaded transcript ends in a
    /// user-role message (set by /background mid-turn so the fork continues the
    /// in-flight turn).
    #[arg(long = "reply-on-resume", hide = true)]
    pub reply_on_resume: bool,

    /// (deprecated) Opt in to auto mode
    #[arg(long = "enable-auto-mode", hide = true)]
    pub enable_auto_mode: bool,

    /// Append a system prompt to every Task-tool subagent's system prompt,
    /// propagated to nested subagents (only works with --print). Implies
    /// CLAUDE_CODE_ENABLE_APPEND_SUBAGENT_PROMPT=1.
    //
    // (CLI-15) Oracle @307405786. The flag is an IMPLIES-style env setter:
    // `wby(e,t=process.env){if(e)t.CLAUDE_CODE_ENABLE_APPEND_SUBAGENT_PROMPT="1"}`
    // (@306637528), and the subagent prompt assembler splices it as the last
    // section: `!C && !d?.isolatedContext && isEnvTruthy(env.CLAUDE_CODE_ENABLE_
    // APPEND_SUBAGENT_PROMPT) && options.appendSubagentSystemPrompt ?
    // join([...sections, appendSubagentSystemPrompt]) : sections` (@292360822).
    // `run_cli` performs the env implication half; the splice itself lives in
    // the subagent prompt assembler (`tools/agent`), which this seam only feeds.
    #[arg(
        long = "append-subagent-system-prompt",
        value_name = "prompt",
        hide = true
    )]
    pub append_subagent_system_prompt: Option<String>,

    /// Cross-session messaging server path: a Unix domain socket on Mac/Linux,
    /// a \\.\pipe\ name on Windows (defaults to an auto-generated path)
    //
    // (CLI-12) Oracle @307414302 — NEW in 2.1.238 (0 hits in 2.1.220).
    // WIRED: `crate::mode::ensure_live_messaging` binds the process UDS inbox
    // at `platform_api::uds_inbox::default_socket_path(pid)`; this flag overrides
    // that default via [`crate::mode::set_messaging_socket_override`].
    #[arg(long = "messaging-socket-path", value_name = "path", hide = true)]
    pub messaging_socket_path: Option<String>,

    /// When resuming, only messages up to and including the chain entry with
    /// <message.id> — any chain-entry UUID, typically the kept turn's last
    /// entry (use with --resume in print mode)
    //
    // (CLI-13/SC-09) Oracle @307408593. The 2.1.238 wording replaced 2.1.220's
    // "the assistant message with <message.id>" — the flag now takes ANY
    // chain-entry uuid. `argParser(String)`, `.hideHelp()`.
    #[arg(long = "resume-session-at", value_name = "message id", hide = true)]
    pub resume_session_at: Option<String>,

    /// With --resume-session-at in print mode: declare the prompt uuid of the
    /// turn the truncating resume intends to discard; the resume is refused if
    /// the discarded range contains anything not attributable to that turn
    /// (absorbed queued messages, task notifications, content from other
    /// turns). Ignored outside print mode, like --resume-session-at.
    //
    // (SC-09) Oracle @307408861 — NEW in 2.1.238 (0 hits in 2.1.220).
    #[arg(long = "resume-drops-turn", value_name = "message id", hide = true)]
    pub resume_drops_turn: Option<String>,

    /// Restore files to state at the specified user message and exit (requires
    /// --resume)
    //
    // (CLI-16) Oracle @307409495. WIRED: `run_cli` resolves the `--resume`
    // target, verifies the uuid names a USER entry in that transcript, and
    // calls `session::file_history::rewind_from_disk` — the machinery the port
    // already had but never reached from argv.
    #[arg(long = "rewind-files", value_name = "user-message-id", hide = true)]
    pub rewind_files: Option<String>,
}

/// Recursively graft a hidden trailing catch-all positional onto every LEAF
/// subcommand (a node with no further subcommands) so surplus positional args
/// are silently ignored like commander's `allowExcessArguments` default. Groups
/// (incl. the top-level) are only recursed into — they must keep rejecting an
/// unknown first token as an unknown subcommand. A leaf that already has a
/// variadic positional (e.g. `mcp add <name> <commandOrUrl> [args...]`) is left
/// alone, since it already absorbs the surplus and a second catch-all would
/// conflict.
fn with_excess_catchall(mut cmd: clap::Command) -> clap::Command {
    let sub_names: Vec<String> = cmd
        .get_subcommands()
        .map(|c| c.get_name().to_string())
        .collect();
    if sub_names.is_empty() {
        if !has_variadic_positional(&cmd) {
            cmd = cmd.arg(
                clap::Arg::new("__excess_ignored")
                    .num_args(0..)
                    .hide(true)
                    .help("Excess positional arguments are ignored"),
            );
        }
    } else {
        for name in sub_names {
            cmd = cmd.mut_subcommand(name, with_excess_catchall);
        }
    }
    cmd
}

/// True iff `cmd` already has a positional that accepts more than one value (a
/// `Vec`/trailing-var-arg positional), which would conflict with a second
/// catch-all positional.
fn has_variadic_positional(cmd: &clap::Command) -> bool {
    cmd.get_positionals().any(|a| {
        a.get_num_args()
            .map(|r| r.max_values() > 1)
            .unwrap_or(false)
    })
}

impl Argv {
    /// Parse from any iterable of `OsString` (used by integration tests).
    ///
    /// Named `from_iter` for ergonomic parity with `Vec::from_iter`-style
    /// constructors; clippy's `should_implement_trait` is silenced because
    /// the canonical `std::iter::FromIterator` is the wrong shape (no
    /// `Result` return).
    #[allow(clippy::should_implement_trait)]
    pub fn from_iter<I, T>(iter: I) -> Result<Self, clap::Error>
    where
        I: IntoIterator<Item = T>,
        T: Into<std::ffi::OsString> + Clone,
    {
        use clap::{CommandFactory, FromArgMatches};
        // Commander's `allowExcessArguments` defaults to true: surplus POSITIONAL
        // args are silently ignored, not errored (e.g. `mcp get foo extra` runs
        // `get foo`). clap rejects them, so we graft a hidden trailing catch-all
        // positional onto every LEAF subcommand that lacks a variadic positional,
        // then deserialize via `from_arg_matches` (which ignores the catch-all).
        // Unknown FLAGS and unknown SUBCOMMANDS still error — only excess
        // positionals are absorbed.
        let cmd = with_excess_catchall(Self::command());
        let matches = cmd.try_get_matches_from(iter)?;
        Self::from_arg_matches(&matches)
    }

    /// True iff debug mode is on (`-d`/`--debug` in any form, the deprecated
    /// `--mcp-debug` alias, or `--debug-file` which implicitly enables it).
    /// Collapses the new `Option<String>` filter back to the old on/off bool.
    #[must_use]
    pub fn debug_enabled(&self) -> bool {
        // `-d2e/--debug-to-stderr` carries `.implies({debug:!0})` in the oracle
        // (@307399127), so it is a third alias onto the same switch.
        self.debug.is_some() || self.mcp_debug || self.debug_file.is_some() || self.debug_to_stderr
    }

    /// The `--debug` category filter (e.g. "api,hooks" or "!1p,!file"), or
    /// `None` when debug is off or enabled without a filter. An empty string
    /// (bare `-d`/`--debug`) means "on, unfiltered" → `None` filter.
    #[must_use]
    pub fn debug_filter(&self) -> Option<&str> {
        match self.debug.as_deref() {
            Some(f) if !f.is_empty() => Some(f),
            _ => None,
        }
    }

    /// Whether this launch is in Claude Code's restricted mode. The CLI flag
    /// wins over the inherited process environment, while the environment form
    /// is intentionally limited to the documented `=1` contract. The internal
    /// LingXi bit is also accepted so nested workers inherit the resolved
    /// session capability exported by the launcher.
    #[must_use]
    pub fn restricted_enabled(&self) -> bool {
        self.restricted
            || std::env::var("CLAUDE_CODE_RESTRICTED").ok().as_deref() == Some("1")
            || std::env::var("LINGXI_RESTRICTED").ok().as_deref() == Some("1")
    }

    /// True iff the binary should start a FRESH REPL.
    ///
    /// Rules: prompt is None or trimmed-empty, AND neither `--resume` nor
    /// `--continue` is set. Resume-without-prompt enters a resumed-REPL (Task 8);
    /// `--continue` likewise reopens the most-recent conversation rather than a
    /// fresh session, so it is excluded here too.
    #[must_use]
    pub fn is_repl_mode(&self) -> bool {
        let no_prompt = self.prompt.as_deref().map_or(true, |s| s.trim().is_empty());
        no_prompt && self.resume.is_none() && !self.continue_session
    }

    /// Resolve whether the JSON output mode is active.
    ///
    /// `--json` (LingXi-specific) OR `--output-format json` both activate JSON
    /// NDJSON output. `--output-format stream-json` is handled separately via
    /// [`Self::is_stream_json`] and does NOT fall through here.
    #[must_use]
    pub fn is_json_output(&self) -> bool {
        self.json || matches!(self.output_format.as_deref(), Some("json"))
    }

    /// True iff `--output-format stream-json` is set (the realtime SDK output
    /// protocol). This is distinct from `--output-format json` (which emits a
    /// single result object) and from `--json` (LingXi legacy NDJSON).
    ///
    /// Note: the spec requires `--verbose` to accompany `--print +
    /// stream-json`; that validation is enforced in `run_cli` / `run_oneshot`.
    #[must_use]
    pub fn is_stream_json(&self) -> bool {
        matches!(self.output_format.as_deref(), Some("stream-json"))
    }

    /// Resolve the effective system-prompt override from `--system-prompt` /
    /// `--system-prompt-file` (with file taking precedence when both are set).
    ///
    /// Returns `None` when neither flag is set. File read failures are silently
    /// ignored (same as TS `fs.readFileSync` error → fall back to None).
    #[must_use]
    pub fn resolve_system_prompt(&self) -> Option<String> {
        if let Some(ref path) = self.system_prompt_file {
            if let Ok(content) = std::fs::read_to_string(path) {
                return Some(content);
            }
        }
        self.system_prompt.clone()
    }

    /// True iff `--input-format stream-json` is set.
    #[must_use]
    pub fn is_stream_json_input(&self) -> bool {
        matches!(self.input_format.as_deref(), Some("stream-json"))
    }

    /// Validate `--input-format=stream-json` cross-flag constraints.
    ///
    /// Checks (in binary order per §4.1 of SPEC-inferred.md):
    /// 1. `--input-format=stream-json` requires `--output-format=stream-json`
    ///    → `Error: --input-format=stream-json requires output-format=stream-json.`
    /// 2. `--input-format=stream-json` requires `--print`
    ///    → `Error: --input-format=stream-json requires --print.`
    /// 3. `--replay-user-messages` requires both `--input-format=stream-json`
    ///    and `--output-format=stream-json`
    ///    → `Error: --replay-user-messages requires both --input-format=stream-json and --output-format=stream-json.`
    ///
    /// Returns `Ok(())` when the combination is valid. The exact error strings
    /// are byte-locked to the binary (§4.1 SPEC-inferred.md).
    pub fn validate_stream_json_input_args(&self) -> Result<(), String> {
        if self.is_stream_json_input() {
            if !self.is_stream_json() {
                return Err(
                    "--input-format=stream-json requires output-format=stream-json.".to_string(),
                );
            }
            if !self.print {
                return Err("--input-format=stream-json requires --print.".to_string());
            }
        }
        if self.replay_user_messages {
            if !self.is_stream_json_input() || !self.is_stream_json() {
                return Err(
                    "--replay-user-messages requires both --input-format=stream-json and --output-format=stream-json."
                        .to_string(),
                );
            }
        }
        Ok(())
    }

    /// Upfront `--bg`/`--background` × `--print`/`-p` reject (cc 2.1.198
    /// changelog "rejected up front"). Byte-locked to the binary's bg
    /// fast-path validator `pof` @218854391: `if(o.some((i)=>{…return
    /// i==="--print"||i.startsWith("--print=")||a.includes("-p")||l==="-p"}))
    /// return"--bg and --print conflict: …"` — the caller (`handleBgFlag`
    /// `oof` @218839305 via `mee`) writes the string + `\n` to STDERR (no
    /// `Error:` prefix) and sets `process.exitCode=1`. The `—` renders as
    /// a real em dash. clap has already folded `--print=`/combined `-p` forms
    /// into `self.print`, matching the binary's peeled-token scan.
    pub fn validate_background_args(&self) -> Result<(), String> {
        if self.background && self.print {
            return Err(
                "--bg and --print conflict: --print never starts the interactive session that `lingxi-cli agents` attaches to, so the job would be unattachable. The prompt is the positional \u{2014} drop --print: `lingxi-cli --bg '<task>'`."
                    .to_string(),
            );
        }
        Ok(())
    }

    /// `--no-session-persistence` requires `--print` (cc 2.1.198 main action
    /// @223929381: `if(a.sessionPersistence===!1&&!We)return Es("Error: --no-
    /// session-persistence can only be used with --print mode.")`; `Es` =
    /// `console.error` + exit 1). The returned string EXCLUDES the `Error: `
    /// prefix — the caller prints `Error: {msg}` like the sibling stream-json
    /// gates.
    pub fn validate_session_persistence_args(&self) -> Result<(), String> {
        if self.no_session_persistence && !self.print {
            return Err("--no-session-persistence can only be used with --print mode.".to_string());
        }
        Ok(())
    }

    /// `--plan-mode-instructions` requires `--print` (206 gate, mirroring the
    /// sibling `--no-session-persistence` guard). The returned string EXCLUDES the
    /// `Error: ` prefix — the caller prints `Error: {msg}`.
    pub fn validate_plan_mode_instructions_args(&self) -> Result<(), String> {
        if self.plan_mode_instructions.is_some() && !self.print {
            return Err("--plan-mode-instructions can only be used with --print mode.".to_string());
        }
        Ok(())
    }

    /// Resolve the effective append-system-prompt from `--append-system-prompt` /
    /// `--append-system-prompt-file` (with file taking precedence when both are
    /// set). Returns `None` when neither flag is set.
    #[must_use]
    pub fn resolve_append_system_prompt(&self) -> Option<String> {
        if let Some(ref path) = self.append_system_prompt_file {
            if let Ok(content) = std::fs::read_to_string(path) {
                return Some(content);
            }
        }
        self.append_system_prompt.clone()
    }

    /// (M4 cc2.1.198) The boolean value of `--prompt-suggestions` (the
    /// binary's argParser returns `!Hl(i)`: falsy tokens `false`/`0`/`no`/
    /// `off` → `false`, everything else in the choices list → `true`; bare
    /// flag presets `"true"` → `true`). `None` when the flag is absent.
    /// Out-of-choices values never reach here (clap's `value_parser`
    /// hard-rejects them like commander's `.choices(...)`).
    #[must_use]
    pub fn prompt_suggestions_enabled(&self) -> Option<bool> {
        self.prompt_suggestions
            .as_deref()
            .map(|v| !matches!(v, "false" | "0" | "no" | "off"))
    }

    /// (M4 cc2.1.198) A TRUTHY `--prompt-suggestions` requires `--print` +
    /// `--output-format=stream-json` (binary main action: `if(a.
    /// promptSuggestions&&(!We||M!=="stream-json"))return Es("Error: --prompt-
    /// suggestions requires --print and --output-format=stream-json
    /// (prompt_suggestion messages are only surfaced in stream-json
    /// output).")`; verified live: stderr line + exit 1; `--prompt-suggestions
    /// false` passes). The returned string EXCLUDES the `Error: ` prefix,
    /// matching the sibling gates' caller convention.
    pub fn validate_prompt_suggestions_args(&self) -> Result<(), String> {
        if self.prompt_suggestions_enabled() == Some(true) && !(self.print && self.is_stream_json())
        {
            return Err(
                "--prompt-suggestions requires --print and --output-format=stream-json (prompt_suggestion messages are only surfaced in stream-json output)."
                    .to_string(),
            );
        }
        Ok(())
    }

    /// (2.1.211) Effective `--forward-subagent-text` state. The binary computes
    /// `xe = k || Z.CLAUDE_CODE_FORWARD_SUBAGENT_TEXT`: the CLI flag OR the env
    /// var read RAW off `process.env` (`Z.…`) — NOT through `isEnvTruthy`. So the
    /// truthiness here is plain JS string truthiness: any present, non-empty
    /// value enables it (even `"0"`/`"false"`), an empty or unset value does not.
    /// This deliberately differs from the `--include-partial-messages` sibling,
    /// which the binary DOES gate through `Gt(…)`/`isEnvTruthy`
    /// (`Re = D || Gt(process.env.CLAUDE_CODE_INCLUDE_PARTIAL_MESSAGES)`). When
    /// the flag is absent a truthy env still enables forwarding, but if the
    /// runtime context is not `--print` + `--output-format=stream-json` the
    /// env-only opt-in is silently disabled (only an EXPLICIT flag is a fatal
    /// error — see `run_cli`). Kept verbatim (`CLAUDE_CODE_*`) to match the
    /// binary's env registry key.
    #[must_use]
    pub fn forward_subagent_text_effective(&self) -> bool {
        self.forward_subagent_text
            || std::env::var("CLAUDE_CODE_FORWARD_SUBAGENT_TEXT")
                .map(|v| !v.is_empty())
                .unwrap_or(false)
    }

    /// (M4 cc2.1.198) Normalize `--effort` exactly like the binary's argParser
    /// (`u4i`/`Xat`: `e.trim().toLowerCase()`, alias map `c4i = {med:
    /// "medium"}`, membership in `UR = ["low","medium","high","xhigh","max"]`).
    /// Returns `(level, warning)`: a valid value yields the normalized level;
    /// an unknown one yields `None` plus the byte-locked warning the binary
    /// writes to stderr (`process.stderr.write(\`Warning: ${l}\n\`)`) before
    /// continuing with the default effort. Absent flag → `(None, None)`.
    #[must_use]
    pub fn normalized_effort(&self) -> (Option<String>, Option<String>) {
        const UR: [&str; 5] = ["low", "medium", "high", "xhigh", "max"];
        let Some(raw) = self.effort.as_deref() else {
            return (None, None);
        };
        let mut t = raw.trim().to_lowercase();
        // `c4i[t] ?? t` — the only alias in 2.1.198 is `med` → `medium`.
        if t == "med" {
            t = "medium".to_string();
        }
        if UR.contains(&t.as_str()) {
            (Some(t), None)
        } else {
            (
                None,
                Some(format!(
                    "Warning: Unknown --effort value '{raw}' \u{2014} ignoring it and using the default effort. Valid values: {}.",
                    UR.join(", ")
                )),
            )
        }
    }

    /// The effective `--prefill` text: `--prefill-b64` wins when present, since
    /// the deep-link trampoline only ever sets one of the pair.
    ///
    /// Oracle argParser `Buffer.from(l,"base64url").toString("utf8")` — Node's
    /// base64 decoder is LENIENT (it skips characters outside the alphabet and
    /// tolerates missing padding) and never throws, and `toString("utf8")`
    /// replaces invalid sequences with U+FFFD. [`decode_base64url_lenient`]
    /// reproduces both halves, so a malformed value yields text rather than an
    /// argv error.
    #[must_use]
    pub fn resolve_prefill(&self) -> Option<String> {
        if let Some(ref b64) = self.prefill_b64 {
            return Some(decode_base64url_lenient(b64));
        }
        self.prefill.clone()
    }

    /// The deep-link working directory from `--deep-link-cwd-b64`, decoded with
    /// the same lenient base64url semantics as [`Self::resolve_prefill`].
    #[must_use]
    pub fn resolve_deep_link_cwd(&self) -> Option<String> {
        self.deep_link_cwd_b64
            .as_deref()
            .map(decode_base64url_lenient)
    }

    /// `--deep-link-last-fetch <ms>` as a number, applying the oracle's
    /// argParser (`let c=Number(l); return Number.isFinite(c)?c:void 0`): a
    /// non-finite/unparseable token is silently DROPPED, never an argv error.
    /// Empty and whitespace-only strings are `Number("") === 0` in JS, so they
    /// resolve to `0.0` here too.
    #[must_use]
    pub fn deep_link_last_fetch_ms(&self) -> Option<f64> {
        let raw = self.deep_link_last_fetch.as_deref()?;
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            // `Number("")` and `Number("   ")` are both `0`.
            return Some(0.0);
        }
        trimmed.parse::<f64>().ok().filter(|v| v.is_finite())
    }

    /// The truncating-resume / rewind cross-flag gates, in the oracle's order
    /// (`runHeadless` @307217327 — so they apply to PRINT mode only, matching
    /// "Ignored outside print mode"):
    ///
    /// ```js
    /// if(c.resumeSessionAt&&!c.resume){…"Error: --resume-session-at requires --resume"}
    /// if(c.resumeDropsTurn!==void 0&&!c.resumeSessionAt){…"Error: --resume-drops-turn requires --resume-session-at"}
    /// if(c.rewindFiles&&!c.resume){…"Error: --rewind-files requires --resume"}
    /// if(c.rewindFiles&&t){…"Error: --rewind-files is a standalone operation and cannot be used with a prompt"}
    /// ```
    ///
    /// Each writes the line (with its own `Error: ` prefix) to stderr and exits
    /// 1. The strings returned here EXCLUDE the prefix, matching the sibling
    /// gates' caller convention (`eprintln!("Error: {msg}")`).
    ///
    /// `t` in the fourth gate is the resolved headless input — a prompt string,
    /// or the input iterator under `--input-format stream-json` (an object, so
    /// truthy). The port keys the gate on those two argv-visible forms.
    pub fn validate_truncating_resume_args(&self) -> Result<(), String> {
        if !self.print {
            return Ok(());
        }
        let has_resume = self.resume.is_some();
        if self.resume_session_at.is_some() && !has_resume {
            return Err("--resume-session-at requires --resume".to_string());
        }
        if self.resume_drops_turn.is_some() && self.resume_session_at.is_none() {
            return Err("--resume-drops-turn requires --resume-session-at".to_string());
        }
        if self.rewind_files.is_some() {
            if !has_resume {
                return Err("--rewind-files requires --resume".to_string());
            }
            // `t` is the resolved headless input: a prompt STRING, or — with
            // `--input-format stream-json` — the input iterator, which is a
            // non-null object and therefore truthy. Both trip the fourth gate.
            // (A prompt piped in on plain stdin is also `t` upstream; the port
            // resolves that read later, so this gate keys on the argv-visible
            // forms only.)
            if self.is_stream_json_input() || self.prompt.as_deref().is_some_and(|p| !p.is_empty())
            {
                return Err(
                    "--rewind-files is a standalone operation and cannot be used with a prompt"
                        .to_string(),
                );
            }
        }
        Ok(())
    }
}

/// Node's `Buffer.from(s, "base64url").toString("utf8")`.
///
/// Deliberately LENIENT, because the oracle's `--prefill-b64` /
/// `--deep-link-cwd-b64` argParsers call exactly that and Node never throws
/// here: characters outside the base64url alphabet are skipped, `=` padding is
/// optional, a trailing group of one leftover character contributes nothing,
/// and invalid UTF-8 in the decoded bytes becomes U+FFFD (`from_utf8_lossy`).
/// A strict decoder would turn a malformed deep link into an argv error the
/// oracle does not raise.
fn decode_base64url_lenient(input: &str) -> String {
    fn sextet(b: u8) -> Option<u8> {
        match b {
            b'A'..=b'Z' => Some(b - b'A'),
            b'a'..=b'z' => Some(b - b'a' + 26),
            b'0'..=b'9' => Some(b - b'0' + 52),
            // base64url's `-`/`_` plus the standard `+`/`/`: Node's decoder
            // accepts BOTH alphabets under the `base64url` label.
            b'-' | b'+' => Some(62),
            b'_' | b'/' => Some(63),
            _ => None,
        }
    }
    let mut out: Vec<u8> = Vec::new();
    let mut acc: u32 = 0;
    let mut bits: u32 = 0;
    for b in input.bytes() {
        let Some(v) = sextet(b) else { continue };
        acc = (acc << 6) | u32::from(v);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push(((acc >> bits) & 0xff) as u8);
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
#[path = "argv_test.rs"]
mod argv_test;
