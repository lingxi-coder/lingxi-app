//! `/skill-doctor` — show which loaded custom commands / file-based skills are
//! unused and costing context.
//!
//! Faithful port of the claude-code compiled `Bcf` handler (bound to the
//! `local` command object `$cf` — `name:"skill-doctor"`,
//! `description:"Show which loaded skills are unused and costing context"`,
//! `isEnabled:()=>!0`, `supportsNonInteractive:!0`) plus its two render
//! helpers `Ucf` (the table body) and `Fcf` (the per-row "days since use"
//! label), byte-verified against the v2.1.198 binary:
//!
//! ```js
//! var Bcf=async(e,t)=>{let n=vYe(),r=[];for(let a of t.options.commands){
//!   if(a.type!=="prompt")continue;
//!   if(a.source==="bundled"||a.source==="builtin"||a.source==="policySettings"||a.source==="plugin")continue;
//!   let l=U_l(a.name,a.unqualifiedName);
//!   r.push({name:a.name,source:a.pluginInfo?.pluginManifest.name??a.source,usageCount:l?.usageCount??0,daysSinceUse:l?.daysSinceUse??null})
//! }
//! r.sort((a,l)=>(l.daysSinceUse??1/0)-(a.daysSinceUse??1/0));
//! let o=r.filter(a=>a.usageCount===0),s=[];
//! s.push(bold("Skills loaded this session")),s.push(""),s.push(Ucf(r)),s.push("");
//! if(o.length>0)s.push(yellow(`${o.length} ${un(o.length,"skill")} loaded but never invoked. Each one adds to the system prompt every turn. Disable in /skills, or remove from .claude/skills.`));
//! else s.push(green("All loaded skills have been used at least once."));
//! let i=await n; // stale-plugin list; always [] in LingXi (no marketplace-recency tracker) so this section never renders
//! if(i.length>0){ /* … "Plugins not used recently" block, omitted here … */ }
//! return{type:"text",value:s.join("\n")}}
//!
//! function Fcf(e){if(e===null)return yellow("never");if(e===0)return"today";return `${e} ${un(e,"day")}`}
//! function Ucf(e){if(e.length===0)return dim("  (no skills loaded)");
//!   let t=Math.max(5,...e.map(r=>r.name.length)),n=Math.max(6,...e.map(r=>r.source.length));
//!   return e.map(r=>{let o=`  ${r.name.padEnd(t)}  ${dim(r.source.padEnd(n))}  ${String(r.usageCount).padStart(4)}×  ${Fcf(r.daysSinceUse)}`;
//!     return r.usageCount===0?yellow(o):o}).join("\n")}
//! ```
//!
//! Colors (`bold`/`yellow`/`green`/`dim`) are the TUI's job, not this
//! handler's — every other headless handler in this crate ([`crate::doctor`],
//! [`crate::skills`], [`crate::model`]) returns plain text and lets the
//! render surface apply markup, so this port does the same: the strings below
//! are byte-exact modulo the stripped ANSI wrapper.
//!
//! ## Why this handler bypasses [`traits::OrchestratorHandle`]
//!
//! `OrchestratorHandle` (see `traits/src/orchestrator.rs`) has no
//! command-dispatch-history or skill-invocation-history surface — nothing
//! resembling claude-code's `t.options.commands` (the live, already-merged
//! command list with per-command `source`/`pluginInfo`) or its usage-tracking
//! store (`U_l`). Extending the trait for a single new command is
//! disproportionate, and there is no existing seam to reuse. Instead — like
//! [`crate::skills::SkillsHandler::with_all_roots`] — this handler is
//! constructed with explicit filesystem roots and loads its own view of "what
//! commands/skills are on disk" directly through [`command_api::markdown_loader`],
//! bypassing both the handle and the live [`command_api::CommandRegistry`].
//!
//! Usage is read from the shared `command_api::skill_usage` store. The registry
//! dispatcher records every successful user/project/local Markdown expansion,
//! so interactive and headless invocations feed the same counters this handler
//! reports.

use async_trait::async_trait;
use command_api::markdown_loader::{
    build_markdown_command, build_skill_command, load_command_markdown_files,
    load_skill_markdown_files_with_roots,
};
use command_api::model::{BuiltinCommandHandler, CommandResult, CommandSource};
use command_api::parser::ParsedSlashCommand;
use command_api::skill_usage::{read_skill_usage, SkillUsageRecord};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

const DESCRIPTION: &str = "Show which loaded skills are unused and costing context";
const HEADER: &str = "Skills loaded this session";
const EMPTY_TABLE: &str = "  (no skills loaded)";
const ALL_USED: &str = "All loaded skills have been used at least once.";

/// Seconds per day, for turning a unix-timestamp delta into whole days.
const SECONDS_PER_DAY: i64 = 86_400;

/// Explicit filesystem roots this handler was constructed with. Mirrors
/// [`crate::skills::SkillsHandler`]'s private `SkillsRoots`.
#[derive(Debug, Clone, Default)]
struct SkillDoctorRoots {
    cwd: PathBuf,
    lingxi_home: PathBuf,
    managed_dir: Option<PathBuf>,
    additional_skill_dirs: Vec<PathBuf>,
}

/// `/skill-doctor` handler — renders unused-skill/command diagnostics.
#[derive(Debug, Clone)]
pub struct SkillDoctorHandler {
    roots: SkillDoctorRoots,
}

impl SkillDoctorHandler {
    /// Construct a handler pinned to explicit `(cwd, lingxi_home, managed_dir,
    /// additional_skill_dirs)` roots — the same four parameters as
    /// [`crate::skills::SkillsHandler::with_all_roots`], passed by the
    /// composition root.
    #[must_use]
    pub fn new(
        cwd: PathBuf,
        lingxi_home: PathBuf,
        managed_dir: Option<PathBuf>,
        additional_skill_dirs: Vec<PathBuf>,
    ) -> Self {
        Self {
            roots: SkillDoctorRoots {
                cwd,
                lingxi_home,
                managed_dir,
                additional_skill_dirs,
            },
        }
    }
}

#[async_trait]
impl BuiltinCommandHandler for SkillDoctorHandler {
    // Upstream's `load:()=>Promise.resolve({call:async(e,t)=>{...Bcf(e,t)}})`
    // never reads its raw-args parameter `e`; `args` is ignored here too.
    async fn handle(&self, _args: &ParsedSlashCommand) -> CommandResult {
        let now_unix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX));
        match load_entries(&self.roots, now_unix).await {
            Ok(mut entries) => {
                sort_entries(&mut entries);
                CommandResult::Done {
                    display: Some(render_skill_doctor(&entries)),
                }
            }
            // TS: `catch(n){return ke(...),{type:"text",value:`Couldn't compute
            // skill usage. Run with --debug for details. (${he(n)})`}}`
            Err(e) => CommandResult::Done {
                display: Some(format!(
                    "Couldn't compute skill usage. Run with --debug for details. ({e})"
                )),
            },
        }
    }

    fn name(&self) -> &str {
        "skill-doctor"
    }

    fn description(&self) -> &str {
        DESCRIPTION
    }
}

/// One row of the rendered table.
#[derive(Debug, Clone, PartialEq, Eq)]
struct SkillDoctorEntry {
    name: String,
    source: CommandSource,
    usage_count: u64,
    days_since_use: Option<u64>,
}

/// Record one real dispatch of a Markdown/skill-kind command: increments its
/// usage count and stamps `last_used_unix` to now.
pub fn record_skill_usage(lingxi_home: &Path, name: &str) -> Result<(), String> {
    command_api::skill_usage::record_skill_usage(lingxi_home, name)
}

/// TS `a.source==="bundled"||a.source==="builtin"||a.source==="policySettings"||a.source==="plugin"`
/// → `continue` (skip). Kept: user, project, and (forward-compatibility, not
/// currently produced by either loader) local.
fn is_countable_source(source: CommandSource) -> bool {
    matches!(
        source,
        CommandSource::User | CommandSource::Project | CommandSource::Local
    )
}

fn source_label(source: CommandSource) -> &'static str {
    match source {
        CommandSource::Builtin => "builtin",
        CommandSource::User => "user",
        CommandSource::Project => "project",
        CommandSource::Local => "local",
        CommandSource::Plugin => "plugin",
        CommandSource::Managed => "managed",
        CommandSource::Mcp => "mcp",
        CommandSource::Bundled => "bundled",
    }
}

/// `lingxi_home` is `~/.lingxi`; its parent is the upward-walk stop boundary
/// (`home`) the loaders need. Same convention as
/// `skill_api::listing::project_skills_dirs`'s `lingxi_home.parent()`.
fn derive_home(lingxi_home: &Path) -> PathBuf {
    lingxi_home
        .parent()
        .map_or_else(|| lingxi_home.to_path_buf(), Path::to_path_buf)
}

fn days_between(last_used_unix: i64, now_unix: i64) -> u64 {
    let delta = now_unix.saturating_sub(last_used_unix);
    if delta <= 0 {
        0
    } else {
        u64::try_from(delta / SECONDS_PER_DAY).unwrap_or(0)
    }
}

fn make_entry(
    name: String,
    source: CommandSource,
    usage: &HashMap<String, SkillUsageRecord>,
    now_unix: i64,
) -> SkillDoctorEntry {
    let record = usage.get(&name);
    let usage_count = record.map_or(0, |r| r.count);
    let days_since_use = record
        .and_then(|r| r.last_used_unix)
        .map(|last| days_between(last, now_unix));
    SkillDoctorEntry {
        name,
        source,
        usage_count,
        days_since_use,
    }
}

/// Enumerate the union of custom `.lingxi/commands/**.md` markdown commands
/// and directory-format `.lingxi/skills/<name>/SKILL.md` skills, filtered to
/// `CommandSource::{User, Project, Local}`, joined with usage stats.
///
/// Both loaders already return `managed, user, project[, additional]` order
/// (`managed > user > project` priority); after dropping `Managed` the
/// remaining order is `user` before `project`, so a first-wins dedup-by-name
/// (mirroring [`crate::custom_commands::load_and_register_custom_commands`]'s
/// own `HashSet` guard) reproduces the same "user overrides project"
/// precedence without needing the live [`command_api::CommandRegistry`].
async fn load_entries(
    roots: &SkillDoctorRoots,
    now_unix: i64,
) -> Result<Vec<SkillDoctorEntry>, String> {
    let home = derive_home(&roots.lingxi_home);
    let usage = read_skill_usage(&roots.lingxi_home)?;

    // `load_command_markdown_files` takes `managed_dir: &Path` (not
    // `Option`); when no managed root is configured, point it at a directory
    // that cannot exist so the loader's `read_dir` fails closed (returns
    // nothing) rather than accidentally resolving a real path.
    let no_managed_dir = roots.lingxi_home.join(".skill-doctor-no-managed-dir");
    let managed_for_commands = roots.managed_dir.clone().unwrap_or(no_managed_dir);

    let command_files =
        load_command_markdown_files(&roots.cwd, &roots.lingxi_home, &managed_for_commands, &home)
            .await;
    let skill_files = load_skill_markdown_files_with_roots(
        &roots.cwd,
        &roots.lingxi_home,
        roots.managed_dir.as_deref(),
        &home,
        &roots.additional_skill_dirs,
    )
    .await;

    let mut seen: HashSet<String> = HashSet::new();
    let mut entries = Vec::new();
    for file in &skill_files {
        if !is_countable_source(file.source) {
            continue;
        }
        let cmd = build_skill_command(file, file.source);
        if seen.insert(cmd.name.clone()) {
            entries.push(make_entry(cmd.name, file.source, &usage, now_unix));
        }
    }
    for file in &command_files {
        if !is_countable_source(file.source) {
            continue;
        }
        let cmd = build_markdown_command(file, file.source);
        if seen.insert(cmd.name.clone()) {
            entries.push(make_entry(cmd.name, file.source, &usage, now_unix));
        }
    }

    Ok(entries)
}

/// TS `r.sort((a,l)=>(l.daysSinceUse??1/0)-(a.daysSinceUse??1/0))` — descending
/// by `daysSinceUse`, `null`/never sorting as `+Infinity` (first). Rust's
/// `sort_by` is stable, matching `Array.prototype.sort`.
fn sort_entries(entries: &mut [SkillDoctorEntry]) {
    entries.sort_by(|a, b| {
        let key = |e: &SkillDoctorEntry| e.days_since_use.unwrap_or(u64::MAX);
        key(b).cmp(&key(a))
    });
}

fn plural(n: u64, word: &str) -> String {
    if n == 1 {
        word.to_string()
    } else {
        format!("{word}s")
    }
}

/// Port of `Fcf`.
fn format_days_since_use(days: Option<u64>) -> String {
    match days {
        None => "never".to_string(),
        Some(0) => "today".to_string(),
        Some(n) => format!("{n} {}", plural(n, "day")),
    }
}

/// Port of `Ucf`.
fn render_table(entries: &[SkillDoctorEntry]) -> String {
    if entries.is_empty() {
        return EMPTY_TABLE.to_string();
    }
    let name_width = entries
        .iter()
        .map(|e| e.name.chars().count())
        .max()
        .unwrap_or(0)
        .max(5);
    let source_width = entries
        .iter()
        .map(|e| source_label(e.source).chars().count())
        .max()
        .unwrap_or(0)
        .max(6);
    entries
        .iter()
        .map(|e| {
            format!(
                "  {name:<name_width$}  {source:<source_width$}  {count:>4}\u{00D7}  {days}",
                name = e.name,
                source = source_label(e.source),
                count = e.usage_count,
                days = format_days_since_use(e.days_since_use),
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Port of `Bcf`'s `s.join("\n")` (minus the never-populated "Plugins not
/// used recently" section — see the module docs point 5).
fn render_skill_doctor(entries: &[SkillDoctorEntry]) -> String {
    let unused = entries.iter().filter(|e| e.usage_count == 0).count();
    let mut lines: Vec<String> = Vec::with_capacity(5);
    lines.push(HEADER.to_string());
    lines.push(String::new());
    lines.push(render_table(entries));
    lines.push(String::new());
    if unused > 0 {
        let n = u64::try_from(unused).unwrap_or(u64::MAX);
        lines.push(format!(
            "{n} {} loaded but never invoked. Each one adds to the system prompt every turn. Disable in /skills, or remove from .lingxi/skills.",
            plural(n, "skill")
        ));
    } else {
        lines.push(ALL_USED.to_string());
    }
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn tmp_root(name: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock before epoch")
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "lingxi-skill-doctor-{name}-{}-{nanos}",
            std::process::id()
        ));
        fs::create_dir_all(&dir).expect("create temp root");
        dir
    }

    fn args() -> ParsedSlashCommand {
        ParsedSlashCommand {
            name: "skill-doctor".into(),
            raw_args: String::new(),
            positional_args: vec![],
        }
    }

    fn write(path: &Path, contents: &str) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, contents).unwrap();
    }

    #[tokio::test]
    async fn handler_name_and_description() {
        let h = SkillDoctorHandler::new(PathBuf::from("."), PathBuf::from("."), None, Vec::new());
        assert_eq!(h.name(), "skill-doctor");
        assert_eq!(h.description(), DESCRIPTION);
    }

    #[tokio::test]
    async fn empty_state_matches_binary_copy() {
        let root = tmp_root("empty");
        let cwd = root.join("repo");
        let lingxi_home = root.join("home").join(".lingxi");
        fs::create_dir_all(&cwd).unwrap();
        fs::create_dir_all(&lingxi_home).unwrap();

        let h = SkillDoctorHandler::new(cwd, lingxi_home, None, Vec::new());
        match h.handle(&args()).await {
            CommandResult::Done { display: Some(s) } => {
                assert_eq!(
                    s,
                    "Skills loaded this session\n\n  (no skills loaded)\n\nAll loaded skills have been used at least once."
                );
            }
            other => panic!("expected Done display, got {other:?}"),
        }
        fs::remove_dir_all(root).ok();
    }

    #[tokio::test]
    async fn never_used_project_skill_renders_never_and_unused_line() {
        let root = tmp_root("never-used");
        let cwd = root.join("repo");
        let lingxi_home = root.join("home").join(".lingxi");
        fs::create_dir_all(cwd.join(".git")).unwrap();
        fs::create_dir_all(&lingxi_home).unwrap();
        write(
            &cwd.join(".lingxi")
                .join("skills")
                .join("demo")
                .join("SKILL.md"),
            "---\ndescription: Demo skill\n---\nBody\n",
        );

        let h = SkillDoctorHandler::new(cwd, lingxi_home, None, Vec::new());
        match h.handle(&args()).await {
            CommandResult::Done { display: Some(s) } => {
                assert!(
                    s.contains("demo"),
                    "table should list the discovered skill: {s}"
                );
                assert!(
                    s.contains("project"),
                    "source column should read project: {s}"
                );
                assert!(s.contains("never"), "unrecorded usage renders never: {s}");
                assert!(
                    s.contains(
                        "1 skill loaded but never invoked. Each one adds to the system prompt every turn. Disable in /skills, or remove from .lingxi/skills."
                    ),
                    "unused-count line: {s}"
                );
            }
            other => panic!("expected Done display, got {other:?}"),
        }
        fs::remove_dir_all(root).ok();
    }

    #[tokio::test]
    async fn recorded_usage_marks_all_used_and_shows_today() {
        let root = tmp_root("recorded");
        let cwd = root.join("repo");
        let lingxi_home = root.join("home").join(".lingxi");
        fs::create_dir_all(cwd.join(".git")).unwrap();
        fs::create_dir_all(&lingxi_home).unwrap();
        write(
            &cwd.join(".lingxi").join("commands").join("foo.md"),
            "# Foo\n\nDo the foo",
        );
        record_skill_usage(&lingxi_home, "foo").expect("record usage");

        let h = SkillDoctorHandler::new(cwd, lingxi_home, None, Vec::new());
        match h.handle(&args()).await {
            CommandResult::Done { display: Some(s) } => {
                assert!(s.contains("foo"), "table should list foo: {s}");
                assert!(
                    s.contains("today"),
                    "just-recorded usage renders today: {s}"
                );
                assert_eq!(
                    s.lines().last(),
                    Some(ALL_USED),
                    "all-used line when every entry has usage: {s}"
                );
            }
            other => panic!("expected Done display, got {other:?}"),
        }
        fs::remove_dir_all(root).ok();
    }

    #[tokio::test]
    async fn corrupt_usage_log_falls_back_to_locked_error_string() {
        let root = tmp_root("corrupt");
        let cwd = root.join("repo");
        let lingxi_home = root.join("home").join(".lingxi");
        fs::create_dir_all(&cwd).unwrap();
        fs::create_dir_all(&lingxi_home).unwrap();
        write(&lingxi_home.join("skill_usage.json"), "{not json");

        let h = SkillDoctorHandler::new(cwd, lingxi_home, None, Vec::new());
        match h.handle(&args()).await {
            CommandResult::Done { display: Some(s) } => {
                assert!(
                    s.starts_with("Couldn't compute skill usage. Run with --debug for details. ("),
                    "fallback error string: {s}"
                );
            }
            other => panic!("expected Done display, got {other:?}"),
        }
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn sort_puts_never_used_first_then_descending_days() {
        let mut entries = vec![
            SkillDoctorEntry {
                name: "a".into(),
                source: CommandSource::Project,
                usage_count: 1,
                days_since_use: Some(2),
            },
            SkillDoctorEntry {
                name: "b".into(),
                source: CommandSource::Project,
                usage_count: 0,
                days_since_use: None,
            },
            SkillDoctorEntry {
                name: "c".into(),
                source: CommandSource::Project,
                usage_count: 3,
                days_since_use: Some(10),
            },
        ];
        sort_entries(&mut entries);
        let names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, vec!["b", "c", "a"]);
    }

    #[test]
    fn format_days_matches_locked_labels() {
        assert_eq!(format_days_since_use(None), "never");
        assert_eq!(format_days_since_use(Some(0)), "today");
        assert_eq!(format_days_since_use(Some(1)), "1 day");
        assert_eq!(format_days_since_use(Some(2)), "2 days");
    }

    #[test]
    fn record_skill_usage_increments_and_stamps() {
        let root = tmp_root("record-fn");
        let lingxi_home = root.join("home").join(".lingxi");
        fs::create_dir_all(&lingxi_home).unwrap();

        record_skill_usage(&lingxi_home, "foo").unwrap();
        record_skill_usage(&lingxi_home, "foo").unwrap();
        let usage = read_skill_usage(&lingxi_home).expect("read back");
        assert_eq!(usage.get("foo").map(|r| r.count), Some(2));
        assert!(usage.get("foo").unwrap().last_used_unix.is_some());
        fs::remove_dir_all(root).ok();
    }
}
