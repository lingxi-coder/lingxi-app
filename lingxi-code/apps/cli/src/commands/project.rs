//! `lingxi-cli project` — Manage Claude Code project state.
//!
//! Byte-faithful clap surface for the `project` family plus a real (safe)
//! implementation of `project purge`. The single child `purge [path]` deletes a
//! project's locally-stored Claude Code state: its session transcripts under
//! `<config-home>/projects/<encoded>/` and its entry in the global config
//! (`~/.lingxi.json` `projects` map). Two sub-stores claude also nominally
//! tracks — per-list task spools and the temp file-history — are NOT keyed by
//! project path in this port (tasks are keyed by task-list name; file history
//! lives in an ephemeral temp dir keyed by the raw cwd), so they are reported
//! but not deleted (we never delete a path we cannot confirm belongs to THIS
//! project). See the family return notice for the precise seam.
//!
//! Never starts a chat turn / touches the network — pure local FS + config I/O.

use std::io::Write as _;
use std::path::{Path, PathBuf};

use clap::{Args, Subcommand};

use crate::exit_codes::{RUNTIME_ERROR, SUCCESS};

/// `project` args. Bare `lingxi-cli project` (no child) prints help.
#[derive(Debug, Clone, Args)]
pub struct Cli {
    /// Subcommand (bare parent prints help — matches claude).
    #[command(subcommand)]
    pub command: Option<Sub>,
}

/// `project` children. Matches `claude project --help` (one child: `purge`).
#[derive(Debug, Clone, Subcommand)]
pub enum Sub {
    /// Delete all Claude Code state for a project (transcripts, tasks, file
    /// history, config entry).
    Purge(PurgeArgs),
}

/// `project purge [options] [path]` — byte-faithful with `claude project
/// purge --help`.
#[derive(Debug, Clone, Args)]
pub struct PurgeArgs {
    /// Purge state for every project (mutually exclusive with [path]).
    #[arg(long = "all")]
    pub all: bool,

    /// List what would be deleted without deleting anything.
    #[arg(long = "dry-run")]
    pub dry_run: bool,

    /// Prompt for each item before deleting.
    #[arg(short = 'i', long = "interactive")]
    pub interactive: bool,

    /// Skip confirmation prompt.
    #[arg(short = 'y', long = "yes")]
    pub yes: bool,

    /// Project directory whose state to purge (defaults to the cwd).
    #[arg(value_name = "path")]
    pub path: Option<String>,
}

/// Run the `project` family. Bare parent (no child) prints help and exits 0.
pub async fn run(cli: &Cli) -> i32 {
    match &cli.command {
        None => {
            print_family_help();
            SUCCESS
        }
        Some(Sub::Purge(args)) => run_purge(args),
    }
}

/// `lingxi-cli project` help (bare parent), mirroring the claude family help.
fn print_family_help() {
    println!("Usage: lingxi-cli project [options] [command]");
    println!();
    println!("Manage Claude Code project state");
    println!();
    println!("Options:");
    println!("  -h, --help              Display help for command");
    println!();
    println!("Commands:");
    println!("  help [command]          display help for command");
    println!("  purge [options] [path]  Delete all Claude Code state for a project");
    println!("                          (transcripts, tasks, file history, config entry)");
}

/// What a single project's purge resolved to.
struct ProjectTargets {
    /// The global-config `projects` map key (git-root-else-cwd, posix form).
    config_key: String,
    /// `<config-home>/projects/<encoded>` transcript dir (may not exist).
    transcript_dir: PathBuf,
}

/// Implement `project purge`.
fn run_purge(args: &PurgeArgs) -> i32 {
    // `--all` is mutually exclusive with an explicit [path] (claude rejects the
    // combination). Match that guard.
    if args.all && args.path.is_some() {
        eprintln!("error: --all cannot be combined with a [path] argument");
        return RUNTIME_ERROR;
    }

    let Some(config_home) = migrations::global_config::claude_config_home() else {
        eprintln!("lingxi-cli project purge: cannot resolve Claude config home (no $HOME / $LINGXI_CONFIG_DIR)");
        return RUNTIME_ERROR;
    };
    let projects_root = config_home.join("projects");
    let global_config = migrations::global_config::global_config_path();

    // Resolve the set of project keys to purge.
    let targets: Vec<ProjectTargets> = if args.all {
        match collect_all_targets(&projects_root, global_config.as_deref()) {
            Ok(t) => t,
            Err(e) => {
                eprintln!("lingxi-cli project purge: {e}");
                return RUNTIME_ERROR;
            }
        }
    } else {
        let dir = match &args.path {
            Some(p) => PathBuf::from(p),
            None => match std::env::current_dir() {
                Ok(d) => d,
                Err(e) => {
                    eprintln!("lingxi-cli project purge: cannot resolve current directory: {e}");
                    return RUNTIME_ERROR;
                }
            },
        };
        // TWO DIFFERENT KEYS for two different stores:
        //  - the ~/.lingxi.json `projects` map is keyed by the GIT-ROOT config
        //    key (`project_path_for_config`, canonicalize + walk to repo root);
        //  - the transcript dir is keyed by the RAW cwd via the SAME writer
        //    helper the engine uses (`session::jsonl::path::project_dir_name`,
        //    which also applies the >200-char truncation + djb2 suffix).
        // They DIVERGE in a worktree/subdir; using the config key for the
        // transcript dir would delete the WRONG project's transcripts.
        let config_key = migrations::global_config::project_path_for_config(&dir);
        let dir_component = session::jsonl::path::project_dir_name(&dir.to_string_lossy());
        // Guard: refuse when we cannot resolve a non-empty project key — e.g.
        // `purge ""` would otherwise resolve the transcript dir to the projects
        // ROOT and delete EVERY project's transcripts.
        if dir_component.is_empty() || config_key.trim().is_empty() {
            eprintln!(
                "lingxi-cli project purge: refusing to purge — could not resolve a project key for {}",
                dir.display()
            );
            return RUNTIME_ERROR;
        }
        let transcript_dir = projects_root.join(&dir_component);
        vec![ProjectTargets {
            config_key,
            transcript_dir,
        }]
    };

    if targets.is_empty() {
        println!("No project state found to purge.");
        return SUCCESS;
    }

    // Confirmation prompt (skipped by --yes / --dry-run). Non-interactive
    // (no tty) without --yes refuses to delete, to avoid silent destruction.
    if !args.dry_run && !args.yes {
        let scope = if args.all {
            format!("ALL {} project(s)", targets.len())
        } else {
            format!("project '{}'", targets[0].config_key)
        };
        if !confirm(&format!("Delete all Claude Code state for {scope}?")) {
            println!("Aborted. Nothing was deleted.");
            return SUCCESS;
        }
    }

    let mut any_error = false;
    for target in &targets {
        if !purge_one(target, &projects_root, global_config.as_deref(), args) {
            any_error = true;
        }
    }

    if any_error {
        RUNTIME_ERROR
    } else {
        SUCCESS
    }
}

/// Purge one project's confirmable state. Returns false on a hard I/O error.
fn purge_one(
    target: &ProjectTargets,
    projects_root: &Path,
    global_config: Option<&Path>,
    args: &PurgeArgs,
) -> bool {
    let mut ok = true;
    println!("Project: {}", target.config_key);

    // Defense-in-depth: NEVER `remove_dir_all` the projects root itself or any
    // path outside it (guards against any resolution edge that collapses the
    // encoded component to empty). The legitimate target is always a strict
    // descendant `<projects_root>/<encoded>`.
    let safe_target = target.transcript_dir.starts_with(projects_root)
        && target.transcript_dir != projects_root;
    if !safe_target {
        eprintln!(
            "  refusing to remove transcripts at an unsafe path: {}",
            target.transcript_dir.display()
        );
        return false;
    }

    // (1) Transcripts: `<config-home>/projects/<encoded>/`.
    if target.transcript_dir.exists() {
        if args.dry_run {
            println!("  would remove transcripts: {}", target.transcript_dir.display());
        } else if !args.interactive
            || confirm(&format!(
                "  remove transcripts {}?",
                target.transcript_dir.display()
            ))
        {
            match std::fs::remove_dir_all(&target.transcript_dir) {
                Ok(()) => println!("  removed transcripts: {}", target.transcript_dir.display()),
                Err(e) => {
                    eprintln!(
                        "  failed to remove transcripts {}: {e}",
                        target.transcript_dir.display()
                    );
                    ok = false;
                }
            }
        }
    } else {
        println!("  no transcripts found ({})", target.transcript_dir.display());
    }

    // (2) Config entry: remove `projects[<key>]` from `~/.lingxi.json`.
    match global_config {
        Some(path) => {
            if args.dry_run {
                let present = project_entry_present(path, &target.config_key);
                if present {
                    println!("  would remove config entry: projects[\"{}\"]", target.config_key);
                } else {
                    println!("  no config entry found");
                }
            } else if !args.interactive
                || confirm(&format!(
                    "  remove config entry projects[\"{}\"]?",
                    target.config_key
                ))
            {
                match remove_config_entry(path, &target.config_key) {
                    Ok(true) => {
                        println!("  removed config entry: projects[\"{}\"]", target.config_key)
                    }
                    Ok(false) => println!("  no config entry found"),
                    Err(e) => {
                        eprintln!("  failed to update config {}: {e}", path.display());
                        ok = false;
                    }
                }
            }
        }
        None => {
            println!("  config file unavailable; skipped config entry");
        }
    }

    // (3) Tasks + (4) file history: not keyed by project path in this port.
    // We report them as not-purged rather than deleting paths we cannot
    // confirm belong to THIS project (tasks are keyed by task-list name; the
    // temp file-history dir is keyed by the raw cwd in an ephemeral tmp tree).
    println!(
        "  note: tasks and file history are not purged by this command (not keyed by project path)"
    );

    ok
}

/// Best-effort encode of a global-config `projects` KEY (a git-root config
/// path) into a transcript directory name, used ONLY by the `--all` path to
/// *try* a config-key-derived dir. NOTE: this is NOT the authoritative writer
/// key — the engine names transcript dirs from the RAW cwd via
/// [`session::jsonl::path::project_dir_name`] (which also truncates >200 chars +
/// appends a djb2 suffix). The two diverge for worktrees/subdirs/long paths, so
/// a config-key dir computed here may simply not exist (a harmless no-op); the
/// real transcript dirs are picked up by the on-disk enumeration in
/// [`collect_all_targets`]. The single-project `purge <path>` path does NOT use
/// this — it keys the transcript dir off the raw cwd, matching the writer.
fn encode_project_dir(key: &str) -> String {
    key.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
}

/// True iff `projects[<key>]` exists in the global config.
fn project_entry_present(path: &Path, key: &str) -> bool {
    matches!(
        migrations::global_config::read_map(path),
        Ok(map)
            if map
                .get("projects")
                .and_then(serde_json::Value::as_object)
                .is_some_and(|p| p.contains_key(key))
    )
}

/// Remove `projects[<key>]` from the global config. Returns `Ok(true)` if an
/// entry was present and removed (a write happened), `Ok(false)` if absent.
fn remove_config_entry(path: &Path, key: &str) -> Result<bool, String> {
    let key_owned = key.to_string();
    // `save_map` value-compares the mutator output; an absent key => unchanged
    // => `Ok(false)` (no write). We pre-check presence to report accurately.
    let present = project_entry_present(path, key);
    if !present {
        return Ok(false);
    }
    migrations::global_config::save_map(path, |mut map| {
        if let Some(serde_json::Value::Object(projects)) = map.get_mut("projects") {
            projects.remove(&key_owned);
        }
        map
    })
    .map(|_wrote| true)
    .map_err(|e| e.to_string())
}

/// Collect every project's targets for `--all`: the union of (a) every entry in
/// the global config `projects` map and (b) every directory under
/// `<config-home>/projects/`. Each is reported once, keyed by its config key.
fn collect_all_targets(
    projects_root: &Path,
    global_config: Option<&Path>,
) -> Result<Vec<ProjectTargets>, String> {
    use std::collections::BTreeMap;

    // config-key -> transcript-dir. BTreeMap dedups + gives stable ordering.
    let mut by_key: BTreeMap<String, PathBuf> = BTreeMap::new();

    // (a) Config `projects` map keys.
    if let Some(path) = global_config {
        match migrations::global_config::read_map(path) {
            Ok(map) => {
                if let Some(projects) = map.get("projects").and_then(serde_json::Value::as_object) {
                    for key in projects.keys() {
                        by_key
                            .entry(key.clone())
                            .or_insert_with(|| projects_root.join(encode_project_dir(key)));
                    }
                }
            }
            Err(e) => return Err(format!("cannot read config {}: {e}", path.display())),
        }
    }

    // (b) Transcript dirs on disk. We cannot reverse the lossy `sanitizePath`
    // encoding back to a real path, so use the encoded dir name as the key for
    // any dir not already covered by a config entry. This still lets `--all`
    // delete orphaned transcript dirs.
    if let Ok(entries) = std::fs::read_dir(projects_root) {
        for entry in entries.flatten() {
            let p = entry.path();
            if !p.is_dir() {
                continue;
            }
            let name = entry.file_name().to_string_lossy().into_owned();
            // Only add if no config key already maps to this exact dir.
            let already = by_key.values().any(|d| *d == p);
            if !already {
                by_key.entry(name).or_insert(p);
            }
        }
    }

    Ok(by_key
        .into_iter()
        .map(|(config_key, transcript_dir)| ProjectTargets {
            config_key,
            transcript_dir,
        })
        .collect())
}

/// Read a `y`/`n` confirmation from stdin. Returns false when stdin is not a
/// tty or on any read failure (fail-closed: never delete without a clear yes).
fn confirm(prompt: &str) -> bool {
    use std::io::IsTerminal as _;
    if !std::io::stdin().is_terminal() {
        eprintln!("{prompt} [refusing without a tty; pass --yes to confirm]");
        return false;
    }
    print!("{prompt} [y/N] ");
    if std::io::stdout().flush().is_err() {
        return false;
    }
    let mut line = String::new();
    if std::io::stdin().read_line(&mut line).is_err() {
        return false;
    }
    let answer = line.trim().to_ascii_lowercase();
    answer == "y" || answer == "yes"
}
