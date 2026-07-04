//! `lingxi-cli plugin` (alias `plugins`) — Manage LingXi plugins.
//!
//! Byte-faithful clap surface vs `claude plugin --help` (claude-code 2.1.191):
//! the family has 12 children plus a nested `marketplace` group with 4 of its
//! own. Bare `lingxi-cli plugin` (no child) prints help, never errors —
//! matching commander's parent-with-subcommands behavior (the `command` field
//! is `Option`, and `run` prints help when it is `None`).
//!
//! Implementation policy (conservative — a green build beats coverage):
//!
//! * REAL-IMPLEMENTED locally (read-only / pure-validation, no network, no
//!   billable turn): `list`, `details <name>`, `validate <path>`. These drive
//!   the `plugin` crate's on-disk discovery + manifest reading directly against
//!   `$LINGXI_CONFIG_DIR`/`~/.lingxi/plugins`.
//! * NOTICE (`NOT_IMPLEMENTED`): every action that needs the network /
//!   marketplace resolution / a settings-write seam / the heavy `PluginManager`
//!   registry wiring — `install`, `uninstall`, `update`, `enable`, `disable`,
//!   `init`, `tag`, `prune`, and all `marketplace *`. These are parsed
//!   faithfully (so `--help` and dispatch are correct) and then print a clear
//!   "not yet implemented" line — they never fake success and never start a
//!   chat turn.
//!
//! `enable`/`disable` are intentionally NOTICE rather than wired: claude's
//! `plugin enable/disable` toggles the on-disk `settings.enabledPlugins`
//! allowlist, but `plugin::PluginManager::{enable,disable}` operate on an
//! in-memory state map materialised into the 8 live engine registries
//! (commands/agents/skills/hooks/output-styles/mcp/lsp/tools) and require
//! `PluginManager::new`'s full registry + credential + blocklist + policy +
//! fs/http/runtime wiring. That is the wrong seam for a CLI toggle and there is
//! no exposed settings-write path, so we decline rather than guess.

use clap::{Args, Subcommand};

use crate::exit_codes::{NOT_IMPLEMENTED, RUNTIME_ERROR, SUCCESS};

/// `plugin` args — byte-parity with `claude plugin|plugins [options] [command]`.
///
/// `command` is `Option` so a bare `lingxi-cli plugin` prints help instead of
/// erroring (commander's parent-with-children behavior).
#[derive(Debug, Clone, Args)]
pub struct Cli {
    /// The chosen plugin subcommand (absent ⇒ print help).
    #[command(subcommand)]
    pub command: Option<Sub>,
}

/// The `plugin` subcommands. Names, aliases, args, options, and one-line
/// descriptions byte-match `claude plugin --help`.
#[derive(Debug, Clone, Subcommand)]
pub enum Sub {
    /// Show a plugin's component inventory and projected token cost
    Details(DetailsArgs),

    /// Disable an enabled plugin
    Disable(DisableArgs),

    /// Enable a disabled plugin
    Enable(EnableArgs),

    /// Scaffold a new plugin at ~/.lingxi/skills/<name>/ (auto-loads next
    /// session as <name>@skills-dir)
    #[command(name = "init", visible_alias = "new")]
    Init(InitArgs),

    /// Install a plugin from available marketplaces (use plugin@marketplace for
    /// specific marketplace)
    #[command(name = "install", visible_alias = "i")]
    Install(InstallArgs),

    /// List installed plugins
    List(ListArgs),

    /// Manage LingXi marketplaces
    Marketplace(MarketplaceArgs),

    /// Remove auto-installed dependencies that are no longer needed
    #[command(name = "prune", visible_alias = "autoremove")]
    Prune(PruneArgs),

    /// Create a {name}--v{version} git tag for a plugin release, validating that
    /// plugin.json and any enclosing marketplace entry agree
    Tag(TagArgs),

    /// Uninstall an installed plugin
    #[command(name = "uninstall", visible_alias = "remove")]
    Uninstall(UninstallArgs),

    /// Update a plugin to the latest version (restart required to apply)
    Update(UpdateArgs),

    /// Validate a plugin or marketplace manifest
    Validate(ValidateArgs),
}

/// `plugin details <name>` — no options beyond the positional.
#[derive(Debug, Clone, Args)]
pub struct DetailsArgs {
    /// Plugin name to inspect.
    #[arg(value_name = "name")]
    pub name: String,
}

/// `plugin disable [plugin]`.
#[derive(Debug, Clone, Args)]
pub struct DisableArgs {
    /// Disable all enabled plugins
    #[arg(short = 'a', long)]
    pub all: bool,

    /// Installation scope: user, project, local (default: auto-detect)
    #[arg(short = 's', long, value_name = "scope")]
    pub scope: Option<String>,

    /// Plugin to disable.
    #[arg(value_name = "plugin")]
    pub plugin: Option<String>,
}

/// `plugin enable <plugin>`.
#[derive(Debug, Clone, Args)]
pub struct EnableArgs {
    /// Installation scope: user, project, local (default: auto-detect)
    #[arg(short = 's', long, value_name = "scope")]
    pub scope: Option<String>,

    /// Plugin to enable.
    #[arg(value_name = "plugin")]
    pub plugin: String,
}

/// `plugin init|new <name>`.
#[derive(Debug, Clone, Args)]
pub struct InitArgs {
    /// Author name (default: git config user.name)
    #[arg(long, value_name = "name")]
    pub author: Option<String>,

    /// Author email (default: git config user.email)
    #[arg(long = "author-email", value_name = "email")]
    pub author_email: Option<String>,

    /// Manifest description
    #[arg(long, value_name = "text")]
    pub description: Option<String>,

    /// Overwrite an existing .lingxi-plugin/ at the target
    #[arg(short = 'f', long)]
    pub force: bool,

    /// Also scaffold: skills, agents, hooks, mcp, lsp, output-style, channel
    #[arg(long = "with", value_name = "components", num_args = 1..)]
    pub with: Vec<String>,

    /// Plugin name to scaffold.
    #[arg(value_name = "name")]
    pub name: String,
}

/// `plugin install|i <plugin>`.
#[derive(Debug, Clone, Args)]
pub struct InstallArgs {
    /// Set a userConfig option declared in the plugin's manifest (repeatable).
    /// Values are validated against the schema and stored via the same path as
    /// the interactive /plugin configure flow.
    #[arg(long = "config", value_name = "key=value")]
    pub config: Vec<String>,

    /// Installation scope: user, project, or local (default: "user")
    #[arg(short = 's', long, value_name = "scope", default_value = "user")]
    pub scope: String,

    /// Plugin to install (use plugin@marketplace for a specific marketplace).
    #[arg(value_name = "plugin")]
    pub plugin: String,
}

/// `plugin list`.
#[derive(Debug, Clone, Args)]
pub struct ListArgs {
    /// Include available plugins from marketplaces (requires --json)
    #[arg(long)]
    pub available: bool,

    /// Output as JSON
    #[arg(long)]
    pub json: bool,
}

/// `plugin prune|autoremove`.
#[derive(Debug, Clone, Args)]
pub struct PruneArgs {
    /// List what would be removed without removing
    #[arg(long = "dry-run")]
    pub dry_run: bool,

    /// Prune at scope: user, project, or local (default: "user")
    #[arg(short = 's', long, value_name = "scope", default_value = "user")]
    pub scope: String,

    /// Skip the confirmation prompt (required when stdin or stdout is not a
    /// TTY)
    #[arg(short = 'y', long)]
    pub yes: bool,
}

/// `plugin tag [path]`.
#[derive(Debug, Clone, Args)]
pub struct TagArgs {
    /// Print what would be tagged without creating it
    #[arg(long = "dry-run")]
    pub dry_run: bool,

    /// Skip the dirty-working-tree and tag-already-exists checks
    #[arg(short = 'f', long)]
    pub force: bool,

    /// Tag annotation message (use %s for the version)
    #[arg(short = 'm', long, value_name = "msg")]
    pub message: Option<String>,

    /// Push the tag to --remote after creating it
    #[arg(long)]
    pub push: bool,

    /// Remote to push to with --push (default: "origin")
    #[arg(long, value_name = "name", default_value = "origin")]
    pub remote: String,

    /// Path to the plugin to tag (default: current directory).
    #[arg(value_name = "path")]
    pub path: Option<String>,
}

/// `plugin uninstall|remove <plugin>`.
#[derive(Debug, Clone, Args)]
pub struct UninstallArgs {
    /// Preserve the plugin's persistent data directory
    /// (~/.lingxi/plugins/data/{id}/)
    #[arg(long = "keep-data")]
    pub keep_data: bool,

    /// Also remove auto-installed dependencies that are no longer needed
    /// (requires -y in non-interactive contexts)
    #[arg(long)]
    pub prune: bool,

    /// Uninstall from scope: user, project, or local (default: "user")
    #[arg(short = 's', long, value_name = "scope", default_value = "user")]
    pub scope: String,

    /// Skip the --prune confirmation prompt (required when stdin or stdout is
    /// not a TTY)
    #[arg(short = 'y', long)]
    pub yes: bool,

    /// Plugin to uninstall.
    #[arg(value_name = "plugin")]
    pub plugin: String,
}

/// `plugin update <plugin>`.
#[derive(Debug, Clone, Args)]
pub struct UpdateArgs {
    /// Installation scope: user, project, local, managed (default: user)
    #[arg(short = 's', long, value_name = "scope", default_value = "user")]
    pub scope: String,

    /// Plugin to update.
    #[arg(value_name = "plugin")]
    pub plugin: String,
}

/// `plugin validate <path>`.
#[derive(Debug, Clone, Args)]
pub struct ValidateArgs {
    /// Treat warnings as errors (exit 1). Use in CI to fail on unrecognized
    /// fields, missing metadata, and other issues that the runtime tolerates.
    #[arg(long)]
    pub strict: bool,

    /// Path to the plugin or marketplace manifest to validate.
    #[arg(value_name = "path")]
    pub path: String,
}

/// `plugin marketplace [command]` — wraps the marketplace children in an
/// `Option` so a bare `lingxi-cli plugin marketplace` prints help (commander's
/// parent-with-children behavior) instead of erroring on a missing subcommand.
#[derive(Debug, Clone, Args)]
pub struct MarketplaceArgs {
    /// The chosen marketplace subcommand (absent ⇒ print help).
    #[command(subcommand)]
    pub command: Option<MarketplaceSub>,
}

/// The `plugin marketplace` subcommands — byte-parity with
/// `claude plugin marketplace --help`.
#[derive(Debug, Clone, Subcommand)]
pub enum MarketplaceSub {
    /// Add a marketplace from a URL, path, or GitHub repo
    Add(MarketplaceAddArgs),

    /// List all configured marketplaces
    List(MarketplaceListArgs),

    /// Remove a configured marketplace
    #[command(name = "remove", visible_alias = "rm")]
    Remove(MarketplaceRemoveArgs),

    /// Update marketplace(s) from their source - updates all if no name
    /// specified
    Update(MarketplaceUpdateArgs),
}

/// `plugin marketplace add <source>`.
#[derive(Debug, Clone, Args)]
pub struct MarketplaceAddArgs {
    /// Where to declare the marketplace: user (default), project, or local
    #[arg(long, value_name = "scope")]
    pub scope: Option<String>,

    /// Limit checkout to specific directories via git sparse-checkout (for
    /// monorepos). Example: --sparse .lingxi-plugin plugins
    #[arg(long, value_name = "paths", num_args = 1..)]
    pub sparse: Vec<String>,

    /// Marketplace source: a URL, path, or GitHub repo.
    #[arg(value_name = "source")]
    pub source: String,
}

/// `plugin marketplace list`.
#[derive(Debug, Clone, Args)]
pub struct MarketplaceListArgs {
    /// Output as JSON
    #[arg(long)]
    pub json: bool,
}

/// `plugin marketplace remove|rm <name>`.
#[derive(Debug, Clone, Args)]
pub struct MarketplaceRemoveArgs {
    /// Remove the marketplace declaration from a specific settings scope: user,
    /// project, or local. Omit to remove it from every scope.
    #[arg(long, value_name = "scope")]
    pub scope: Option<String>,

    /// Marketplace name to remove.
    #[arg(value_name = "name")]
    pub name: String,
}

/// `plugin marketplace update [name]`.
#[derive(Debug, Clone, Args)]
pub struct MarketplaceUpdateArgs {
    /// Marketplace name to update (omit to update all).
    #[arg(value_name = "name")]
    pub name: Option<String>,
}

/// Run the `plugin` family. A bare `lingxi-cli plugin` (no child) prints help
/// and exits success.
pub async fn run(cli: &Cli) -> i32 {
    let Some(command) = cli.command.as_ref() else {
        print_help();
        return SUCCESS;
    };

    match command {
        Sub::List(args) => run_list(args).await,
        Sub::Details(args) => run_details(args).await,
        Sub::Validate(args) => run_validate(args).await,

        // On-disk `enabledPlugins` allowlist toggle (the CLI seam — settings.json
        // read-modify-write at the chosen scope), 1:1 with claude 2.1.201.
        Sub::Enable(args) => run_enable(args),
        Sub::Disable(args) => run_disable(args),

        // NOTICE actions — parsed faithfully, declined cleanly (no network,
        // no settings-write seam, no heavy registry wiring, no fake success).
        Sub::Init(_) => notice("init"),
        Sub::Install(_) => notice("install"),
        Sub::Prune(_) => notice("prune"),
        Sub::Tag(_) => notice("tag"),
        Sub::Uninstall(_) => notice("uninstall"),
        Sub::Update(_) => notice("update"),
        Sub::Marketplace(args) => {
            let Some(sub) = args.command.as_ref() else {
                print_marketplace_help();
                return SUCCESS;
            };
            match sub {
                // `list` reads the resolved `known_marketplaces.json` registry.
                MarketplaceSub::List(list_args) => {
                    println!(
                        "{}",
                        crate::commands::plugin_marketplace::run_list(&plugins_dir(), list_args.json)
                    );
                    SUCCESS
                }
                MarketplaceSub::Add(_) => notice("marketplace add"),
                MarketplaceSub::Remove(_) => notice("marketplace remove"),
                MarketplaceSub::Update(_) => notice("marketplace update"),
            }
        }
    }
}

/// Print the `plugin marketplace` group help (bare-parent case).
fn print_marketplace_help() {
    let mut cmd = <MarketplaceArgs as clap::Args>::augment_args(clap::Command::new("marketplace"))
        .bin_name("lingxi-cli plugin marketplace")
        .about("Manage LingXi marketplaces");
    let _ = cmd.print_help();
    println!();
}

/// Print the family help (used for the bare-parent case). Mirrors clap's own
/// help rendering for the parent via `Cli`'s derived command.
fn print_help() {
    // The parent command is the `Plugin(plugin::Cli)` variant of the top-level
    // `Commands` enum. `Cli` derives `clap::Args` (a subcommand payload, not a
    // `Parser`), so render its help by augmenting a fresh `Command` with the
    // derived surface — this gives the children + options block byte-faithfully
    // without reaching across modules into the top-level `Commands`.
    let mut cmd = <Cli as clap::Args>::augment_args(clap::Command::new("plugin"))
        .bin_name("lingxi-cli plugin")
        .about("Manage LingXi plugins");
    let _ = cmd.print_help();
    println!();
}

/// Emit the standard "not yet implemented" line for a declined action and
/// return `NOT_IMPLEMENTED`.
fn notice(action: &str) -> i32 {
    eprintln!("lingxi-cli plugin {action}: not yet implemented");
    NOT_IMPLEMENTED
}

/// The current working directory used for project/local scope resolution
/// (a failure to read it — unusual — degrades to `.`, matching a bare relative
/// join).
fn scope_cwd() -> std::path::PathBuf {
    std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."))
}

/// `plugin enable <plugin>` — toggle the on-disk `enabledPlugins` allowlist.
/// Prints the `✔`/`✘` line and maps success/failure to the exit code.
fn run_enable(args: &EnableArgs) -> i32 {
    let home = crate::run::lingxi_home_dir();
    let cwd = scope_cwd();
    match crate::commands::plugin_settings::run_enable(&args.plugin, args.scope.as_deref(), &home, &cwd)
    {
        Ok(msg) => {
            println!("{msg}");
            SUCCESS
        }
        Err(msg) => {
            eprintln!("{msg}");
            RUNTIME_ERROR
        }
    }
}

/// `plugin disable [plugin] [--all]` — toggle the on-disk `enabledPlugins`
/// allowlist off.
fn run_disable(args: &DisableArgs) -> i32 {
    let home = crate::run::lingxi_home_dir();
    let cwd = scope_cwd();
    match crate::commands::plugin_settings::run_disable(
        args.plugin.as_deref(),
        args.scope.as_deref(),
        args.all,
        &home,
        &cwd,
    ) {
        Ok(msg) => {
            println!("{msg}");
            SUCCESS
        }
        Err(msg) => {
            eprintln!("{msg}");
            RUNTIME_ERROR
        }
    }
}

/// The user-tier plugins directory: `$LINGXI_CONFIG_DIR`/`~/.claude` + `plugins`.
fn plugins_dir() -> std::path::PathBuf {
    crate::run::lingxi_home_dir().join("plugins")
}

/// `plugin list` — enumerate installed plugins from the durable on-disk install
/// record (`~/.lingxi/plugins/installed_plugins.json`), resolving each to its
/// versioned cache manifest. Read-only; never fetches.
async fn run_list(args: &ListArgs) -> i32 {
    // `--available` requires `--json` (claude gates it the same way); without a
    // marketplace fetch we cannot enumerate available plugins, so decline that
    // combination explicitly rather than silently dropping the flag.
    if args.available {
        if !args.json {
            eprintln!("error: --available requires --json");
            return RUNTIME_ERROR;
        }
        eprintln!(
            "lingxi-cli plugin list --available: not yet implemented \
             (requires marketplace resolution)"
        );
        return NOT_IMPLEMENTED;
    }

    let dir = plugins_dir();
    let discovered = plugin::discover_recorded_plugins(&dir).await;

    if args.json {
        let items: Vec<serde_json::Value> = discovered
            .iter()
            .map(|(_, m, path)| {
                serde_json::json!({
                    "name": m.name,
                    "version": m.version,
                    "description": m.description,
                    "path": path.display().to_string(),
                })
            })
            .collect();
        match serde_json::to_string_pretty(&items) {
            Ok(s) => {
                println!("{s}");
                SUCCESS
            }
            Err(e) => {
                eprintln!("lingxi-cli plugin list: failed to serialize JSON: {e}");
                RUNTIME_ERROR
            }
        }
    } else {
        if discovered.is_empty() {
            println!("No plugins installed.");
            return SUCCESS;
        }
        for (_, m, _) in &discovered {
            let version = if m.version.is_empty() {
                String::new()
            } else {
                format!(" v{}", m.version)
            };
            if m.description.is_empty() {
                println!("{}{version}", m.name);
            } else {
                println!("{}{version} — {}", m.name, m.description);
            }
        }
        SUCCESS
    }
}

/// `plugin details <name>` — show the component inventory of an installed
/// plugin. Read-only; resolves the named plugin from the install record and
/// counts the components auto-detected by the discovery loader.
async fn run_details(args: &DetailsArgs) -> i32 {
    let dir = plugins_dir();
    let discovered = plugin::discover_recorded_plugins(&dir).await;

    // `name` may be bare (`foo`) or qualified (`foo@marketplace`); match on the
    // manifest's own name (the discovery loader stores the manifest `name`).
    let wanted = args.name.split('@').next().unwrap_or(&args.name);
    let Some((_, manifest, path)) = discovered.iter().find(|(_, m, _)| m.name == wanted) else {
        eprintln!("lingxi-cli plugin details: plugin not found: {}", args.name);
        return RUNTIME_ERROR;
    };

    let c = &manifest.components;
    println!("{}", manifest.name);
    if !manifest.version.is_empty() {
        println!("  version: {}", manifest.version);
    }
    if !manifest.description.is_empty() {
        println!("  description: {}", manifest.description);
    }
    if let Some(author) = &manifest.author {
        println!("  author: {author}");
    }
    println!("  path: {}", path.display());
    println!("  components:");
    println!("    commands: {}", c.commands.len());
    println!("    agents: {}", c.agents.len());
    println!("    skills: {}", c.skills.len());
    println!("    output-styles: {}", c.output_styles.len());
    println!("    hooks: {}", c.hooks.len());
    println!("    mcp-servers: {}", c.mcp_servers.len());
    println!("    lsp-servers: {}", c.lsp_servers.len());

    // Projected token cost is computed by claude from the loaded component
    // markdown; that estimator is not ported here, so we do not fabricate a
    // number. The component inventory above is the read-only, faithful part.
    SUCCESS
}

/// `plugin validate <path>` — validate a plugin or marketplace manifest on
/// disk. Pure local validation: no network, no install. Resolves the manifest
/// (`.lingxi-plugin/plugin.json` or `.lingxi-plugin/marketplace.json`, whether
/// `path` is the plugin root or the manifest file itself), parses it as JSON,
/// and checks the required identity fields. `--strict` promotes warnings
/// (unrecognized-but-tolerated shapes) to a non-zero exit.
async fn run_validate(args: &ValidateArgs) -> i32 {
    let root = std::path::Path::new(&args.path);

    // Resolve the manifest path. Accept either a directory (probe its
    // `.lingxi-plugin/{plugin,marketplace}.json`) or a direct manifest file.
    let (manifest_path, kind) = match resolve_manifest(root).await {
        Some(v) => v,
        None => {
            eprintln!(
                "lingxi-cli plugin validate: no plugin.json or marketplace.json found at {}",
                args.path
            );
            return RUNTIME_ERROR;
        }
    };

    let raw = match tokio::fs::read_to_string(&manifest_path).await {
        Ok(r) => r,
        Err(e) => {
            eprintln!(
                "lingxi-cli plugin validate: cannot read {}: {e}",
                manifest_path.display()
            );
            return RUNTIME_ERROR;
        }
    };

    let value: serde_json::Value = match serde_json::from_str(&raw) {
        Ok(v) => v,
        Err(e) => {
            eprintln!(
                "lingxi-cli plugin validate: invalid JSON in {}: {e}",
                manifest_path.display()
            );
            return RUNTIME_ERROR;
        }
    };

    let mut errors: Vec<String> = Vec::new();
    let mut warnings: Vec<String> = Vec::new();

    match kind {
        ManifestKind::Plugin => {
            // `name` is the only hard-required field (mirrors the discovery
            // loader's `RawManifest`, where every other field is optional).
            match value.get("name").and_then(serde_json::Value::as_str) {
                Some(n) if !n.is_empty() => {}
                _ => errors.push("missing or empty required field: name".to_string()),
            }
            if value.get("version").is_none() {
                warnings.push("missing recommended field: version".to_string());
            }
            if value.get("description").is_none() {
                warnings.push("missing recommended field: description".to_string());
            }
        }
        ManifestKind::Marketplace => {
            // A marketplace catalog lists `plugins`; require the array exists.
            match value.get("plugins") {
                Some(serde_json::Value::Array(_)) => {}
                Some(_) => errors.push("field 'plugins' must be an array".to_string()),
                None => errors.push("missing required field: plugins".to_string()),
            }
            if value.get("name").is_none() {
                warnings.push("missing recommended field: name".to_string());
            }
        }
    }

    for w in &warnings {
        eprintln!("warning: {w}");
    }
    for e in &errors {
        eprintln!("error: {e}");
    }

    if !errors.is_empty() {
        eprintln!("Validation failed for {}", manifest_path.display());
        return RUNTIME_ERROR;
    }
    if args.strict && !warnings.is_empty() {
        eprintln!(
            "Validation failed (--strict) for {}",
            manifest_path.display()
        );
        return RUNTIME_ERROR;
    }
    println!(
        "OK: {} is a valid {} manifest",
        manifest_path.display(),
        kind.label()
    );
    SUCCESS
}

/// Which manifest flavor `validate` resolved.
#[derive(Clone, Copy)]
enum ManifestKind {
    Plugin,
    Marketplace,
}

impl ManifestKind {
    fn label(self) -> &'static str {
        match self {
            ManifestKind::Plugin => "plugin",
            ManifestKind::Marketplace => "marketplace",
        }
    }
}

/// Resolve `path` to a concrete manifest file + its kind.
///
/// Accepts: a direct `plugin.json`/`marketplace.json` file; a `.lingxi-plugin/`
/// directory; or a plugin/marketplace root holding `.lingxi-plugin/`.
async fn resolve_manifest(path: &std::path::Path) -> Option<(std::path::PathBuf, ManifestKind)> {
    // Direct manifest file?
    if path.is_file() {
        let name = path.file_name().and_then(|s| s.to_str()).unwrap_or("");
        return match name {
            "plugin.json" => Some((path.to_path_buf(), ManifestKind::Plugin)),
            "marketplace.json" => Some((path.to_path_buf(), ManifestKind::Marketplace)),
            _ => None,
        };
    }

    // Directory: probe `<dir>/.lingxi-plugin/{plugin,marketplace}.json` and
    // also the case where `dir` already IS the `.lingxi-plugin` directory.
    for base in [path.join(branding::PLUGIN_MANIFEST_DIR), path.to_path_buf()] {
        let plugin = base.join("plugin.json");
        if tokio::fs::try_exists(&plugin).await.unwrap_or(false) {
            return Some((plugin, ManifestKind::Plugin));
        }
        let market = base.join("marketplace.json");
        if tokio::fs::try_exists(&market).await.unwrap_or(false) {
            return Some((market, ManifestKind::Marketplace));
        }
    }
    None
}
