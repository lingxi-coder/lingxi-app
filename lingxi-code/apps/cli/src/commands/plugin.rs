//! `lingxi-cli plugin` (alias `plugins`) — Manage LingXi plugins.
//!
//! Byte-faithful clap surface vs `claude plugin --help` (claude-code 2.1.191):
//! the family has 12 children plus a nested `marketplace` group with 4 of its
//! own. Bare `lingxi-cli plugin` (no child) prints help, never errors —
//! matching commander's parent-with-subcommands behavior (the `command` field
//! is `Option`, and `run` prints help when it is `None`).
//!
//! All public subcommands are wired: read-only inspection/validation, scoped
//! settings toggles, install/update/uninstall/prune, scaffolding/tagging, and
//! marketplace management. The implementations reuse the CLI's confined
//! settings/archive/git helpers and never start a billable model turn.

use clap::{Args, Subcommand};

use crate::exit_codes::{RUNTIME_ERROR, SUCCESS};

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

    /// Run eval cases against a plugin and report scored results
    #[command(
        long_about = "Run eval cases (evals/**/case.yaml or evals/**/prompt.md + graders/*.md) against a plugin and report scored results. Target is a path, a plugin name, or a `plugin@marketplace` id — installed and skills-dir plugins both resolve (and add a no-plugin baseline arm)"
    )]
    Eval(crate::commands::plugin_eval::Cli),

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
        Sub::Eval(args) => {
            crate::commands::plugin_eval::run(
                args,
                &plugins_dir(),
                &crate::run::lingxi_home_dir(),
                &scope_cwd(),
            )
            .await
        }

        // On-disk `enabledPlugins` allowlist toggle (the CLI seam — settings.json
        // read-modify-write at the chosen scope), 1:1 with claude 2.1.201.
        Sub::Enable(args) => run_enable(args),
        Sub::Disable(args) => run_disable(args),

        // Install/uninstall — marketplace-registry materialization + on-disk state.
        Sub::Install(args) => market_result(crate::commands::plugin_install::run_install(
            &args.plugin,
            Some(&args.scope),
            &args.config,
            &plugins_dir(),
            &crate::run::lingxi_home_dir(),
            &scope_cwd(),
        )),
        Sub::Uninstall(args) => market_result(crate::commands::plugin_install::run_uninstall(
            &args.plugin,
            Some(&args.scope),
            args.keep_data,
            args.prune,
            args.yes,
            &plugins_dir(),
            &crate::run::lingxi_home_dir(),
            &scope_cwd(),
        )),

        // `init` — scaffold a skill-plugin under ~/.lingxi/skills/ (the default
        // scaffold; `--with` component scaffolds are a tracked follow-up).
        Sub::Init(args) => market_result(crate::commands::plugin_init::run_init(
            &args.name,
            args.author.as_deref(),
            args.author_email.as_deref(),
            args.description.as_deref(),
            args.force,
            &args.with,
            &crate::run::lingxi_home_dir(),
        )),

        // Prune orphaned auto-installed dependencies from the v2 installed record.
        Sub::Prune(args) => market_result(crate::commands::plugin_prune::run_prune(
            args.dry_run,
            args.yes,
            &args.scope,
            &plugins_dir(),
            &crate::run::lingxi_home_dir(),
            &scope_cwd(),
        )),
        // Create a `{name}--v{version}` git tag (all output, incl. errors, to STDOUT).
        Sub::Tag(args) => tag_result(crate::commands::plugin_tag::run_tag(
            args.path.as_deref(),
            args.dry_run,
            args.force,
            args.message.as_deref(),
            args.push,
            &args.remote,
        )),
        // Re-materialize an installed plugin from its marketplace + bump the record.
        Sub::Update(args) => market_result(crate::commands::plugin_install::run_update(
            &args.plugin,
            &args.scope,
            &plugins_dir(),
            &crate::run::lingxi_home_dir(),
            &scope_cwd(),
        )),
        Sub::Marketplace(args) => {
            let Some(sub) = args.command.as_ref() else {
                print_marketplace_help();
                return SUCCESS;
            };
            use crate::commands::plugin_marketplace as market;
            match sub {
                // `list` reads the resolved `known_marketplaces.json` registry.
                MarketplaceSub::List(list_args) => {
                    println!("{}", market::run_list(&plugins_dir(), list_args.json));
                    SUCCESS
                }
                MarketplaceSub::Add(add_args) => market_result(market::run_add(
                    &add_args.source,
                    add_args.scope.as_deref(),
                    &add_args.sparse,
                    &plugins_dir(),
                    &crate::run::lingxi_home_dir(),
                    &scope_cwd(),
                )),
                MarketplaceSub::Remove(rm_args) => market_result(market::run_remove(
                    &rm_args.name,
                    rm_args.scope.as_deref(),
                    &plugins_dir(),
                    &crate::run::lingxi_home_dir(),
                    &scope_cwd(),
                )),
                MarketplaceSub::Update(up_args) => market_result(market::run_update(
                    up_args.name.as_deref(),
                    &plugins_dir(),
                    &crate::run::lingxi_home_dir(),
                    &scope_cwd(),
                )),
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

/// Print a `plugin tag` result. Unlike [`market_result`], BOTH the Ok and Err
/// blocks go to STDOUT — claude's `plugin tag` never writes to stderr; Err maps
/// to exit 1.
fn tag_result(res: Result<String, String>) -> i32 {
    match res {
        Ok(msg) => {
            println!("{msg}");
            SUCCESS
        }
        Err(msg) => {
            println!("{msg}");
            RUNTIME_ERROR
        }
    }
}

/// The current working directory used for project/local scope resolution
/// (a failure to read it — unusual — degrades to `.`, matching a bare relative
/// join).
fn scope_cwd() -> std::path::PathBuf {
    std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."))
}

/// Print a marketplace command's result (Ok → stdout/SUCCESS, Err →
/// stderr/RUNTIME_ERROR).
fn market_result(res: Result<String, String>) -> i32 {
    match res {
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

/// `plugin enable <plugin>` — toggle the on-disk `enabledPlugins` allowlist.
/// Prints the `✔`/`✘` line and maps success/failure to the exit code.
fn run_enable(args: &EnableArgs) -> i32 {
    let home = crate::run::lingxi_home_dir();
    let cwd = scope_cwd();
    match crate::commands::plugin_settings::run_enable(
        &args.plugin,
        args.scope.as_deref(),
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
/// versioned cache manifest. `--available --json` additionally reads every
/// configured marketplace's local catalog and excludes installed plugin ids.
/// Read-only; never refreshes or fetches a marketplace.
async fn run_list(args: &ListArgs) -> i32 {
    // `--available` requires `--json` (claude gates it the same way).
    if args.available && !args.json {
        eprintln!("error: --available requires --json");
        return RUNTIME_ERROR;
    }

    let dir = plugins_dir();
    let discovered = plugin::discover_recorded_plugins(&dir).await;

    if args.json {
        let (durable_installed, diagnostics) =
            installed_json_items(&dir, &crate::run::lingxi_home_dir(), &scope_cwd());
        for diagnostic in diagnostics {
            eprintln!("warning: {diagnostic}");
        }
        let installed = durable_installed.unwrap_or_else(|| {
            // Legacy/corrupt records cannot provide Claude's durable metadata;
            // preserve the previous best-effort manifest view as a fallback.
            discovered
                .iter()
                .map(|(_, m, path)| {
                    serde_json::json!({
                        "name": m.name,
                        "version": m.version,
                        "description": m.description,
                        "path": path.display().to_string(),
                    })
                })
                .collect()
        });

        let value = if args.available {
            let (available, diagnostics) = available_plugins(&dir);
            for diagnostic in diagnostics {
                eprintln!("warning: {diagnostic}");
            }
            serde_json::json!({
                "installed": installed,
                "available": available,
            })
        } else {
            serde_json::Value::Array(installed)
        };

        match serde_json::to_string_pretty(&value) {
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
            // Oracle: "No plugins installed. Use `claude plugin install` to
            // install a plugin." The remediation hint names this binary.
            println!("No plugins installed. Use `lingxi-cli plugin install` to install a plugin.");
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

/// Resolve the settings file associated with an installed record. Project and
/// local records retain the project root they were installed from, so listing
/// remains correct even when invoked from another working directory.
fn installed_record_settings_path(
    record: &serde_json::Map<String, serde_json::Value>,
    home: &std::path::Path,
    cwd: &std::path::Path,
) -> Option<std::path::PathBuf> {
    use crate::commands::plugin_settings::Scope;

    let scope = record.get("scope")?.as_str()?;
    let project_root = record
        .get("projectPath")
        .and_then(serde_json::Value::as_str)
        .map(std::path::Path::new)
        .unwrap_or(cwd);
    match scope {
        "user" => Some(Scope::User.path(home, cwd)),
        "project" => Some(Scope::Project.path(home, project_root)),
        "local" => Some(Scope::Local.path(home, project_root)),
        _ => None,
    }
}

/// Render current v2 installed records in Claude's public JSON shape. This is
/// intentionally driven by the durable database rather than the cache: a
/// missing plugin directory is still an installed record and must remain
/// visible to repair/uninstall workflows.
fn installed_json_items(
    plugins_dir: &std::path::Path,
    home: &std::path::Path,
    cwd: &std::path::Path,
) -> (Option<Vec<serde_json::Value>>, Vec<String>) {
    let path = plugins_dir.join("installed_plugins.json");
    let raw = match std::fs::read_to_string(&path) {
        Ok(raw) => raw,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return (None, Vec::new()),
        Err(error) => {
            return (
                None,
                vec![format!("failed to read {}: {error}", path.display())],
            );
        }
    };
    let value: serde_json::Value = match serde_json::from_str(&raw) {
        Ok(value) => value,
        Err(error) => {
            return (
                None,
                vec![format!("failed to parse {}: {error}", path.display())],
            );
        }
    };
    let Some(plugins) = value.get("plugins").and_then(serde_json::Value::as_object) else {
        return (
            None,
            vec![format!(
                "{} field 'plugins' must be an object",
                path.display()
            )],
        );
    };
    let mut out = Vec::new();
    let mut diagnostics = Vec::new();

    for (id, records) in plugins {
        if let Some(records) = records.as_array() {
            for (index, record) in records.iter().enumerate() {
                let Some(record) = record.as_object() else {
                    diagnostics.push(format!(
                        "installed plugin '{id}' record[{index}] must be an object; skipping"
                    ));
                    continue;
                };
                out.push(installed_json_item(id, record, home, cwd));
            }
            continue;
        }

        // Legacy schema: `plugins[marketplace][name] = record`. Render these
        // alongside v2 rows instead of letting one old marketplace hide every
        // valid current record.
        let Some(legacy_records) = records.as_object() else {
            diagnostics.push(format!(
                "installed plugin entry '{id}' must be an array or legacy object; skipping"
            ));
            continue;
        };
        for (name, record) in legacy_records {
            let Some(record) = record.as_object() else {
                diagnostics.push(format!(
                    "legacy installed plugin '{name}@{id}' must be an object; skipping"
                ));
                continue;
            };
            out.push(installed_json_item(
                &format!("{name}@{id}"),
                record,
                home,
                cwd,
            ));
        }
    }
    (Some(out), diagnostics)
}

fn installed_json_item(
    id: &str,
    record: &serde_json::Map<String, serde_json::Value>,
    home: &std::path::Path,
    cwd: &std::path::Path,
) -> serde_json::Value {
    let settings = installed_record_settings_path(record, home, cwd)
        .and_then(|path| migrations::settings_update::read_settings_map(&path).ok())
        .unwrap_or_default();
    let enabled = settings
        .get("enabledPlugins")
        .and_then(serde_json::Value::as_object)
        .and_then(|map| map.get(id))
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);

    let mut item = serde_json::Map::new();
    item.insert("id".to_string(), serde_json::Value::String(id.to_string()));
    for key in ["version", "scope"] {
        if let Some(value) = record.get(key) {
            item.insert(key.to_string(), value.clone());
        }
    }
    item.insert("enabled".to_string(), serde_json::Value::Bool(enabled));
    for key in ["installPath", "installedAt", "lastUpdated", "projectPath"] {
        if let Some(value) = record.get(key) {
            item.insert(key.to_string(), value.clone());
        }
    }
    if let Some(mcp_servers) = settings
        .get("pluginConfigs")
        .and_then(serde_json::Value::as_object)
        .and_then(|configs| configs.get(id))
        .and_then(|config| config.get("mcpServers"))
    {
        item.insert("mcpServers".to_string(), mcp_servers.clone());
    }
    serde_json::Value::Object(item)
}

/// Read the durable install database and return its full `plugin@marketplace`
/// ids. Both the current v2 array shape and the legacy marketplace-nested shape
/// are accepted so an older install is never offered as "available" again.
fn installed_plugin_ids(plugins_dir: &std::path::Path) -> std::collections::BTreeSet<String> {
    let Some(plugins) = std::fs::read_to_string(plugins_dir.join("installed_plugins.json"))
        .ok()
        .and_then(|raw| serde_json::from_str::<serde_json::Value>(&raw).ok())
        .and_then(|value| {
            value
                .get("plugins")
                .and_then(serde_json::Value::as_object)
                .cloned()
        })
    else {
        return std::collections::BTreeSet::new();
    };

    let mut ids = std::collections::BTreeSet::new();
    for (key, value) in plugins {
        if let Some(records) = value.as_array() {
            // Current schema: `plugins["name@marketplace"] = [records...]`.
            // Match `installed_json_items`: empty arrays and arrays containing
            // only malformed scalar records render no installed row, so they
            // must not suppress a healthy marketplace entry.
            if records.iter().any(serde_json::Value::is_object) {
                ids.insert(key);
            }
        } else if let Some(nested) = value.as_object() {
            // Legacy schema: `plugins["marketplace"]["name"] = record`.
            // Mixed-schema rendering skips malformed legacy scalar records;
            // apply the identical rule to availability suppression.
            ids.extend(
                nested
                    .iter()
                    .filter(|(_, record)| record.is_object())
                    .map(|(name, _)| format!("{name}@{key}")),
            );
        }
    }
    ids
}

/// Convert a marketplace catalog entry to the public `--available` JSON shape.
/// Unknown catalog metadata is intentionally not copied: Claude's command
/// exposes only these stable fields.
fn available_json_entry(
    entry: &serde_json::Map<String, serde_json::Value>,
    name: &str,
    marketplace: &str,
) -> serde_json::Value {
    let mut out = serde_json::Map::new();
    out.insert(
        "pluginId".to_string(),
        serde_json::Value::String(format!("{name}@{marketplace}")),
    );
    out.insert(
        "name".to_string(),
        serde_json::Value::String(name.to_string()),
    );
    if let Some(description) = entry.get("description") {
        out.insert("description".to_string(), description.clone());
    }
    out.insert(
        "marketplaceName".to_string(),
        serde_json::Value::String(marketplace.to_string()),
    );
    for key in ["version", "source", "installCount"] {
        if let Some(value) = entry.get(key) {
            out.insert(key.to_string(), value.clone());
        }
    }
    serde_json::Value::Object(out)
}

/// Enumerate locally-resolved marketplace catalogs.
///
/// A broken marketplace must not hide healthy catalogs. It is skipped with a
/// diagnostic returned to the caller (stderr), while duplicate ids keep the
/// first entry and report the conflict. This makes corruption visible without
/// making a read-only list unusable.
fn available_plugins(plugins_dir: &std::path::Path) -> (Vec<serde_json::Value>, Vec<String>) {
    let mut diagnostics = Vec::new();
    let registry_path = plugins_dir.join("known_marketplaces.json");
    let registry = match std::fs::read_to_string(&registry_path) {
        Ok(raw) => match serde_json::from_str::<serde_json::Value>(&raw) {
            Ok(serde_json::Value::Object(registry)) => registry,
            Ok(_) => {
                diagnostics.push(format!(
                    "{} must contain a JSON object",
                    registry_path.display()
                ));
                return (Vec::new(), diagnostics);
            }
            Err(error) => {
                diagnostics.push(format!(
                    "failed to parse {}: {error}",
                    registry_path.display()
                ));
                return (Vec::new(), diagnostics);
            }
        },
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return (Vec::new(), diagnostics);
        }
        Err(error) => {
            diagnostics.push(format!(
                "failed to read {}: {error}",
                registry_path.display()
            ));
            return (Vec::new(), diagnostics);
        }
    };

    let installed = installed_plugin_ids(plugins_dir);
    let mut seen = std::collections::BTreeSet::new();
    let mut available = Vec::new();

    for (marketplace, registry_entry) in registry {
        let Some(root) = registry_entry
            .get("installLocation")
            .and_then(serde_json::Value::as_str)
            .filter(|path| !path.is_empty())
        else {
            diagnostics.push(format!(
                "marketplace '{marketplace}' has no installLocation; skipping"
            ));
            continue;
        };
        let manifest_path = std::path::Path::new(root)
            .join(branding::PLUGIN_MANIFEST_DIR)
            .join("marketplace.json");
        let catalog = match std::fs::read_to_string(&manifest_path) {
            Ok(raw) => match serde_json::from_str::<serde_json::Value>(&raw) {
                Ok(value) => value,
                Err(error) => {
                    diagnostics.push(format!(
                        "failed to parse marketplace '{marketplace}' at {}: {error}",
                        manifest_path.display()
                    ));
                    continue;
                }
            },
            Err(error) => {
                diagnostics.push(format!(
                    "failed to read marketplace '{marketplace}' at {}: {error}",
                    manifest_path.display()
                ));
                continue;
            }
        };
        let Some(entries) = catalog.get("plugins").and_then(serde_json::Value::as_array) else {
            diagnostics.push(format!(
                "marketplace '{marketplace}' manifest field 'plugins' must be an array; skipping"
            ));
            continue;
        };

        for (index, entry) in entries.iter().enumerate() {
            let Some(entry) = entry.as_object() else {
                diagnostics.push(format!(
                    "marketplace '{marketplace}' plugins[{index}] must be an object; skipping"
                ));
                continue;
            };
            let Some(name) = entry
                .get("name")
                .and_then(serde_json::Value::as_str)
                .filter(|name| !name.is_empty())
            else {
                diagnostics.push(format!(
                    "marketplace '{marketplace}' plugins[{index}] has no non-empty name; skipping"
                ));
                continue;
            };
            let id = format!("{name}@{marketplace}");
            if installed.contains(&id) {
                continue;
            }
            if !seen.insert(id.clone()) {
                diagnostics.push(format!(
                    "duplicate plugin id '{id}' in marketplace catalog; keeping first entry"
                ));
                continue;
            }
            available.push(available_json_entry(entry, name, &marketplace));
        }
    }

    (available, diagnostics)
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

#[cfg(test)]
mod list_tests {
    use std::fs;
    use std::path::{Path, PathBuf};

    use serde_json::json;

    use super::{available_plugins, installed_json_items, installed_plugin_ids};

    fn write_json(path: &Path, value: &serde_json::Value) {
        fs::create_dir_all(path.parent().expect("fixture file parent")).unwrap();
        fs::write(path, serde_json::to_vec_pretty(value).unwrap()).unwrap();
    }

    fn add_marketplace(plugins_dir: &Path, root: &Path, name: &str, entries: serde_json::Value) {
        write_json(
            &root
                .join(branding::PLUGIN_MANIFEST_DIR)
                .join("marketplace.json"),
            &json!({"name": name, "plugins": entries}),
        );
        let mut registry = serde_json::Map::new();
        registry.insert(
            name.to_string(),
            json!({
                "source": {"source": "directory", "path": root},
                "installLocation": root,
            }),
        );
        write_json(
            &plugins_dir.join("known_marketplaces.json"),
            &serde_json::Value::Object(registry),
        );
    }

    #[test]
    fn available_excludes_current_and_legacy_installed_ids() {
        let temp = tempfile::tempdir().unwrap();
        let plugins_dir = temp.path().join("plugins");
        let marketplace = temp.path().join("marketplace");
        add_marketplace(
            &plugins_dir,
            &marketplace,
            "example",
            json!([
                {"name":"current", "description":"already here", "source":"./current"},
                {"name":"legacy", "source":"./legacy"},
                {"name":"fresh", "description":"new", "version":"1.2.3", "source":{"source":"url", "url":"https://example.test/fresh.git"}, "installCount":7}
            ]),
        );
        write_json(
            &plugins_dir.join("installed_plugins.json"),
            &json!({
                "version": 2,
                "plugins": {
                    "current@example": [{"scope":"user"}],
                    "example": {"legacy": {"version":"0.1.0"}}
                }
            }),
        );

        assert_eq!(
            installed_plugin_ids(&plugins_dir),
            ["current@example".to_string(), "legacy@example".to_string()]
                .into_iter()
                .collect()
        );
        let (available, diagnostics) = available_plugins(&plugins_dir);
        assert!(diagnostics.is_empty(), "{diagnostics:?}");
        assert_eq!(
            available,
            vec![json!({
                "pluginId": "fresh@example",
                "name": "fresh",
                "description": "new",
                "marketplaceName": "example",
                "version": "1.2.3",
                "source": {"source":"url", "url":"https://example.test/fresh.git"},
                "installCount": 7
            })]
        );
    }

    #[test]
    fn empty_or_malformed_install_records_do_not_hide_available_plugins() {
        let temp = tempfile::tempdir().unwrap();
        let plugins_dir = temp.path().join("plugins");
        let marketplace = temp.path().join("marketplace");
        add_marketplace(
            &plugins_dir,
            &marketplace,
            "example",
            json!([
                {"name":"valid", "source":"./valid"},
                {"name":"empty", "source":"./empty"},
                {"name":"malformed", "source":"./malformed"},
                {"name":"legacy-valid", "source":"./legacy-valid"},
                {"name":"legacy-malformed", "source":"./legacy-malformed"},
                {"name":"fresh", "source":"./fresh"}
            ]),
        );
        write_json(
            &plugins_dir.join("installed_plugins.json"),
            &json!({
                "version": 2,
                "plugins": {
                    "valid@example": [{}],
                    "empty@example": [],
                    "malformed@example": [null, "not-an-object"],
                    "example": {
                        "legacy-valid": {"version":"0.9.0"},
                        "legacy-malformed": "not-an-object"
                    }
                }
            }),
        );

        assert_eq!(
            installed_plugin_ids(&plugins_dir),
            [
                "legacy-valid@example".to_string(),
                "valid@example".to_string()
            ]
            .into_iter()
            .collect()
        );
        let (available, diagnostics) = available_plugins(&plugins_dir);
        assert!(diagnostics.is_empty(), "{diagnostics:?}");
        let ids: Vec<&str> = available
            .iter()
            .filter_map(|entry| entry.get("pluginId").and_then(serde_json::Value::as_str))
            .collect();
        assert_eq!(
            ids,
            vec![
                "empty@example",
                "malformed@example",
                "legacy-malformed@example",
                "fresh@example"
            ]
        );
    }

    #[test]
    fn installed_json_uses_durable_records_and_scope_settings() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("home");
        let cwd = temp.path().join("project");
        let plugins_dir = home.join("plugins");
        write_json(
            &plugins_dir.join("installed_plugins.json"),
            &json!({
                "version": 2,
                "plugins": {
                    "demo@example": [{
                        "scope": "user",
                        "installPath": "/missing/cache/is-still-listed",
                        "version": "1.0.0",
                        "installedAt": "2026-01-01T00:00:00.000Z",
                        "lastUpdated": "2026-01-02T00:00:00.000Z"
                    }]
                }
            }),
        );
        write_json(
            &home.join("settings.json"),
            &json!({
                "enabledPlugins": {"demo@example": true},
                "pluginConfigs": {
                    "demo@example": {"mcpServers": {"demo": {"command": "demo-mcp"}}}
                }
            }),
        );

        let (installed, diagnostics) = installed_json_items(&plugins_dir, &home, &cwd);
        assert!(diagnostics.is_empty(), "{diagnostics:?}");
        assert_eq!(
            installed.unwrap(),
            vec![json!({
                "id": "demo@example",
                "version": "1.0.0",
                "scope": "user",
                "enabled": true,
                "installPath": "/missing/cache/is-still-listed",
                "installedAt": "2026-01-01T00:00:00.000Z",
                "lastUpdated": "2026-01-02T00:00:00.000Z",
                "mcpServers": {"demo": {"command": "demo-mcp"}}
            })]
        );
    }

    #[test]
    fn mixed_installed_schema_keeps_v2_and_legacy_records_independently() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("home");
        let cwd = temp.path().join("project");
        let plugins_dir = home.join("plugins");
        write_json(
            &plugins_dir.join("installed_plugins.json"),
            &json!({
                "version": 2,
                "plugins": {
                    "current@example": [{"scope":"user", "version":"2.0.0"}],
                    "example": {
                        "legacy": {"version":"0.9.0"},
                        "broken": "not-an-object"
                    }
                }
            }),
        );

        let (installed, diagnostics) = installed_json_items(&plugins_dir, &home, &cwd);
        let installed = installed.expect("valid top-level database");
        assert_eq!(installed.len(), 2);
        assert_eq!(installed[0]["id"], "current@example");
        assert_eq!(installed[1]["id"], "legacy@example");
        assert_eq!(diagnostics.len(), 1, "{diagnostics:?}");
        assert!(diagnostics[0].contains("broken@example"));
    }

    #[test]
    fn bad_catalog_is_diagnosed_without_hiding_healthy_catalogs() {
        let temp = tempfile::tempdir().unwrap();
        let plugins_dir = temp.path().join("plugins");
        let good_root = temp.path().join("good");
        let bad_root = temp.path().join("bad");
        write_json(
            &good_root
                .join(branding::PLUGIN_MANIFEST_DIR)
                .join("marketplace.json"),
            &json!({"plugins":[{"name":"ok", "source":"./ok"}]}),
        );
        let bad_manifest = bad_root
            .join(branding::PLUGIN_MANIFEST_DIR)
            .join("marketplace.json");
        fs::create_dir_all(bad_manifest.parent().unwrap()).unwrap();
        fs::write(&bad_manifest, b"{not-json").unwrap();
        write_json(
            &plugins_dir.join("known_marketplaces.json"),
            &json!({
                "bad": {"installLocation": bad_root},
                "good": {"installLocation": good_root}
            }),
        );

        let (available, diagnostics) = available_plugins(&plugins_dir);
        assert_eq!(available.len(), 1);
        assert_eq!(available[0]["pluginId"], "ok@good");
        assert!(
            diagnostics
                .iter()
                .any(|line| line.contains("failed to parse marketplace 'bad'")),
            "{diagnostics:?}"
        );
    }

    #[test]
    fn duplicate_or_malformed_entries_are_skipped_with_diagnostics() {
        let temp = tempfile::tempdir().unwrap();
        let plugins_dir = temp.path().join("plugins");
        let root = temp.path().join("marketplace");
        add_marketplace(
            &plugins_dir,
            &root,
            "example",
            json!([
                {"name":"one", "source":"./one"},
                {"name":"one", "source":"./duplicate"},
                {"description":"missing name"},
                "not-an-object"
            ]),
        );

        let (available, diagnostics) = available_plugins(&plugins_dir);
        assert_eq!(available.len(), 1);
        assert_eq!(available[0]["source"], "./one");
        assert_eq!(diagnostics.len(), 3, "{diagnostics:?}");
        assert!(diagnostics.iter().any(|line| line.contains("duplicate")));
        assert!(diagnostics
            .iter()
            .any(|line| line.contains("non-empty name")));
        assert!(diagnostics
            .iter()
            .any(|line| line.contains("must be an object")));
    }

    #[test]
    fn absent_registry_is_an_empty_success() {
        let temp = tempfile::tempdir().unwrap();
        let plugins_dir = PathBuf::from(temp.path()).join("plugins");
        let (available, diagnostics) = available_plugins(&plugins_dir);
        assert!(available.is_empty());
        assert!(diagnostics.is_empty());
    }
}
