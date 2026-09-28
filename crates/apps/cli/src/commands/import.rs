//! `lingxi-cli import` / `lingxi-cli import-conversations` — the config- and
//! transcript-import families.
//!
//! (CLI-14, cc 2.1.238) Neither existed in the port. Oracle registration
//! (cc-238.js @~245142681, strings @308647783):
//!
//! ```js
//! r.command("import").argument("[source]","Which agent to import from (codex, gemini)")
//!  .option("--dry-run","Show what would be imported without writing anything")
//!  .option("--yes","Skip the interactive picker. On headless surfaces, pass --yes=<digest> from the `/import` preview.")
//!  .description("Import config from another AI coding agent into Claude Code")
//!  .action(()=>{ if(!N9e()) return Da(Bvl);
//!    Da(`Usage: claude import [codex|gemini] [--dry-run] [--yes[=<digest>]]\n\nStarts an interactive session and runs /import.`)})
//! r.command("import-conversations <exportPath>",{hidden:!0})
//!  .option("--cwd <dir>","Archive directory the imported sessions anchor to")
//!  .option("--dry-run","Parse and verify manifest without writing files")
//! ```
//!
//! Two facts decided the port's behaviour, both read off the binary rather
//! than inferred:
//!
//! 1. `N9e()` is `it("tengu_import", !1)` (@295117606) — a gate whose DEFAULT
//!    is `false`. So on a stock build `claude import` takes the first arm and
//!    prints `Bvl` (@295118296):
//!    ``"`claude import` is not yet available in this build. Run `claude` and
//!    use /mcp or edit ~/.claude/settings.json directly."``
//! 2. `Da` (@297392051) is `if(e)Cot(e); cD("cli_error"), process.exit(1)` and
//!    `Cot` (@297392010) is `console.error(chalk.red(e))` — i.e. BOTH arms go
//!    to **stderr** and exit **1**. The usage blurb is not a success path.
//!
//! lingxi-cli has no `/import` slash command, so the gate is off here for the
//! same reason it is off upstream and the gate-off arm is the byte-faithful
//! one. Only the LingXi branding is substituted (the standing `.lingxi` /
//! `lingxi-cli` divergence).
//!
//! `import-conversations` is a hidden internal that rewrites a session archive
//! into the transcript store; lingxi-cli ships no such importer, so it parses
//! its surface and takes the not-implemented path instead of pretending to
//! have written sessions.

use clap::Args;

/// The gate-off notice — `Bvl` with LingXi branding.
const IMPORT_UNAVAILABLE: &str = "`lingxi-cli import` is not yet available in this build. Run `lingxi-cli` and use /mcp or edit ~/.lingxi/settings.json directly.";

/// `import [source]` args.
#[derive(Debug, Clone, Args)]
pub struct Cli {
    /// Which agent to import from (codex, gemini)
    #[arg(value_name = "source")]
    pub source: Option<String>,

    /// Show what would be imported without writing anything
    #[arg(long = "dry-run")]
    pub dry_run: bool,

    /// Skip the interactive picker. On headless surfaces, pass --yes=<digest>
    /// from the `/import` preview.
    //
    // commander declares this as a BOOLEAN option, but a boolean option given
    // `--yes=<digest>` still captures the digest string — which is exactly the
    // headless contract the 2.1.238 copy documents (2.1.220's copy had no
    // digest). `require_equals` reproduces that: `--yes` alone is the bare
    // flag, `--yes=abc` carries the digest, and `--yes abc` leaves `abc` as a
    // positional, as commander does.
    #[arg(
        long = "yes",
        value_name = "digest",
        num_args = 0..=1,
        require_equals = true,
        default_missing_value = ""
    )]
    pub yes: Option<String>,
}

/// `import-conversations <exportPath>` args (hidden).
#[derive(Debug, Clone, Args)]
pub struct ConversationsCli {
    /// Path to the session export to import.
    #[arg(value_name = "exportPath")]
    pub export_path: String,

    /// Archive directory the imported sessions anchor to
    #[arg(long = "cwd", value_name = "dir")]
    pub cwd: Option<String>,

    /// Parse and verify manifest without writing files
    #[arg(long = "dry-run")]
    pub dry_run: bool,
}

/// Run `import`. Mirrors the oracle's gate-off arm: red stderr line, exit 1.
pub async fn run(_cli: &Cli) -> i32 {
    eprintln!("{IMPORT_UNAVAILABLE}");
    crate::exit_codes::RUNTIME_ERROR
}

/// Run `import-conversations`. lingxi-cli has no session-archive importer, so
/// this reports the family as unavailable rather than reporting sessions it
/// never wrote. `--dry-run` gets the same answer — a dry run that printed a
/// plan would be the same fabrication.
pub async fn run_conversations(_cli: &ConversationsCli) -> i32 {
    eprintln!(
        "lingxi-cli import-conversations: the session-archive importer is not \
         available in lingxi-cli."
    );
    crate::exit_codes::NOT_IMPLEMENTED
}
