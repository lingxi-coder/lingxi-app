//! `plugin tag [path]` — create a `{name}--v{version}` git tag for a plugin
//! release, validating that `plugin.json` (and any enclosing marketplace entry)
//! agree, 1:1 with claude-code 2.1.201 (verified end-to-end against the real
//! binary; branding-adjusted).
//!
//! Flow (in order; the first failure returns, all output — warnings, errors, and
//! the success block alike — goes to STDOUT with exit 1 on failure):
//!
//! 1. resolve `<path>/.lingxi-plugin/plugin.json` (default `path` = cwd); missing
//!    ⇒ `✘ No plugin manifest found. Expected <abs>.`
//! 2. parse JSON; malformed ⇒ `✘ Invalid JSON in <abs>: <err>`.
//! 3. hard-validate the manifest (`name`/`version` zod-style type checks); any
//!    issue ⇒ `✘ Plugin validation failed for <abs>:` + one `  field: message`
//!    line per issue (short-circuits before warnings).
//! 4. emit soft `⚠` warnings (non-kebab name, missing version/description/author).
//! 5. require a `version` string ⇒ else `✘ No version to tag. …`.
//! 6. require valid semver (node `semver.valid`) ⇒ else `✘ Version "…" is not
//!    valid semver. …`.
//! 7. cross-check any enclosing marketplace entry's `version`; a disagreement ⇒
//!    `✘ Version mismatch: …`.
//! 8. require the plugin to live inside a git repo (`git rev-parse
//!    --show-toplevel`) ⇒ else `✘ <abs> is not inside a git repository. …`.
//! 9. require a legal git ref (`git check-ref-format`) ⇒ else `✘ Computed tag
//!    name "…" is not a valid git ref. …`.
//! 10. unless `--force`, refuse a dirty working tree *scoped to the plugin dir*
//!     (`git status --porcelain -- <plugin>`) and refuse an existing tag.
//! 11. print the `Plugin:/Version:/[Marketplace entry:]/Tag:` block, then either
//!     the `--dry-run` command preview or the real `git tag -a` (+ optional
//!     `--push`).
//!
//! Residuals (follow-ups): (a) the malformed-JSON error text is serde_json's, not
//! bun's byte-for-byte parser message; (b) the enclosing-marketplace lookup here
//! walks the filesystem up from the plugin dir, whereas claude resolves it via the
//! `known_marketplaces.json` registry — so in practice the oracle only fires the
//! marketplace cross-check for registered marketplaces. The exact error/info
//! strings are 1:1; the discovery mechanism is the documented-intent port.

use std::path::{Path, PathBuf};
use std::process::Command;

use branding::PLUGIN_MANIFEST_DIR;
use serde_json::Value;

/// Run `plugin tag`. Returns the full STDOUT block on success (`Ok`) or on
/// failure (`Err`); the caller prints BOTH to stdout and maps `Err` to exit 1
/// (matching the oracle, which never writes to stderr).
pub fn run_tag(
    path: Option<&str>,
    dry_run: bool,
    force: bool,
    message: Option<&str>,
    push: bool,
    remote: &str,
) -> Result<String, String> {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    run_tag_from_cwd(&cwd, path, dry_run, force, message, push, remote)
}

fn run_tag_from_cwd(
    cwd: &Path,
    path: Option<&str>,
    dry_run: bool,
    force: bool,
    message: Option<&str>,
    push: bool,
    remote: &str,
) -> Result<String, String> {
    // Resolve the plugin root (absolute, canonicalized when it exists so paths
    // match the oracle's `/private/...` realpaths).
    let arg = path.unwrap_or(".");
    let root_lexical = if Path::new(arg).is_absolute() {
        PathBuf::from(arg)
    } else {
        cwd.join(arg)
    };
    let plugin_root = std::fs::canonicalize(&root_lexical).unwrap_or(root_lexical);

    let plugin_json = plugin_root.join(PLUGIN_MANIFEST_DIR).join("plugin.json");
    let abs_json = plugin_json.display().to_string();
    let rel_json = display_path(&path_relative(cwd, &plugin_json));

    if !plugin_json.exists() {
        return Err(format!("✘ No plugin manifest found. Expected {abs_json}."));
    }

    let raw = std::fs::read_to_string(&plugin_json)
        .map_err(|e| format!("✘ Invalid JSON in {abs_json}: {e}"))?;
    let value: Value =
        serde_json::from_str(&raw).map_err(|e| format!("✘ Invalid JSON in {abs_json}: {e}"))?;

    // --- (3) hard validation (short-circuits before warnings) ---
    let mut issues: Vec<String> = Vec::new();
    match value.get("name") {
        // §8: reuse the shared oracle `se` validator (empty / spaces / the
        // 2.1.247 control-and-bidirectional-formatting-character hardening)
        // instead of re-deriving the same three checks here.
        Some(Value::String(s)) => {
            if let Err(reason) = plugin::validate_plugin_name(s) {
                issues.push(format!("name: {reason}"));
            }
        }
        other => issues.push(format!(
            "name: Invalid input: expected string, received {}",
            json_type(other)
        )),
    }
    // `version` is optional, but when present it must be a string.
    if let Some(v) = value.get("version") {
        if !v.is_string() {
            issues.push(format!(
                "version: Invalid input: expected string, received {}",
                json_type(Some(v))
            ));
        }
    }
    if !issues.is_empty() {
        let mut out = format!("✘ Plugin validation failed for {abs_json}:");
        for i in &issues {
            out.push_str("\n  ");
            out.push_str(i);
        }
        return Err(out);
    }

    // At this point `name` is a non-empty, space-free string.
    let name = value.get("name").and_then(Value::as_str).unwrap_or("");

    // --- (4) soft warnings (order: kebab, version, description, author) ---
    let mut lines: Vec<String> = Vec::new();
    if !is_kebab_case(name) {
        lines.push(format!(
            "⚠ {rel_json}: Plugin name \"{name}\" is not kebab-case. Claude Code accepts it, but \
             the Claude.ai marketplace sync requires kebab-case (lowercase letters, digits, and \
             hyphens only, e.g., \"my-plugin\")."
        ));
    }
    let version_field = value.get("version").and_then(Value::as_str);
    if version_field.map(str::is_empty).unwrap_or(true) {
        lines.push(format!(
            "⚠ {rel_json}: No version specified. Consider adding a version following semver (e.g., \
             \"1.0.0\")"
        ));
    }
    if value.get("description").is_none() {
        lines.push(format!(
            "⚠ {rel_json}: No description provided. Adding a description helps users understand \
             what your plugin does"
        ));
    }
    if value.get("author").is_none() {
        lines.push(format!(
            "⚠ {rel_json}: No author information provided. Consider adding author details for \
             plugin attribution"
        ));
    }

    // --- (5) version present ---
    let version = match version_field.filter(|s| !s.is_empty()) {
        Some(v) => v,
        None => {
            return Err(finish(
                lines,
                format!(
                    "✘ No version to tag. Set \"version\" in {rel_json}. Tags are only used for \
                     dependency version constraints, which require an explicit semver — the \
                     git-SHA fallback does not need a tag."
                ),
            ));
        }
    };

    // --- (6) semver ---
    if !is_valid_semver(version) {
        return Err(finish(
            lines,
            format!(
                "✘ Version \"{version}\" is not valid semver. Dependency resolution \
                 (resolveVersionRange) ignores tags whose suffix doesn't parse as semver, so this \
                 tag would never be selected."
            ),
        ));
    }

    let tag = format!("{name}--v{version}");

    // --- (7) enclosing marketplace cross-check (best-effort filesystem walk) ---
    let market = find_enclosing_marketplace(&plugin_root, name, cwd);
    if let Some(entry) = &market {
        if let Some(ev) = &entry.version {
            if ev.as_str() != version {
                return Err(finish(
                    lines,
                    format!(
                        "✘ Version mismatch: plugin.json says \"{version}\" but {} plugins[{}].version \
                         says \"{ev}\". plugin.json wins at install time, so update the marketplace \
                         entry to \"{version}\" (or remove it) before tagging.",
                        entry.path, entry.index
                    ),
                ));
            }
        }
    }

    // --- (8) inside a git repo? ---
    let toplevel = match git_stdout(&["rev-parse", "--show-toplevel"], &plugin_root) {
        Some(t) if !t.is_empty() => t,
        _ => {
            return Err(finish(
                lines,
                format!(
                    "✘ {} is not inside a git repository. Dependency tags are resolved via git \
                     ls-remote, so the plugin must live in a git repo.",
                    plugin_root.display()
                ),
            ));
        }
    };
    let repo = std::path::Path::new(toplevel.as_str());
    let repo_d = repo.display();

    // --- (9) legal git ref? ---
    let ref_ok = git(&["check-ref-format", &format!("refs/tags/{tag}")], repo)
        .map(|o| o.status.success())
        .unwrap_or(false);
    if !ref_ok {
        return Err(finish(
            lines,
            format!(
                "✘ Computed tag name \"{tag}\" is not a valid git ref. Check the plugin name for \
                 characters git rejects (spaces, ~, ^, :, ?, *, [, \\, or sequences like .., @{{, \
                 //)."
            ),
        ));
    }

    // --- (10) dirty tree + already-exists (unless --force) ---
    if !force {
        let dirty = git_stdout(
            &[
                "status",
                "--porcelain",
                "--",
                &plugin_root.display().to_string(),
            ],
            repo,
        )
        .unwrap_or_default();
        let files: Vec<&str> = dirty
            .lines()
            .filter(|l| l.len() > 3)
            .map(|l| l[3..].trim_end())
            .collect();
        if !files.is_empty() {
            let mut msg = String::from(
                "✘ Uncommitted changes affecting this release — commit them first so the tag \
                 points at the version you intend to release (or use --force):",
            );
            for f in files.iter().take(5) {
                msg.push_str("\n  ");
                msg.push_str(f);
            }
            if files.len() > 5 {
                msg.push_str(&format!("\n  …and {} more", files.len() - 5));
            }
            return Err(finish(lines, msg));
        }

        let exists = git(
            &[
                "rev-parse",
                "--verify",
                "--quiet",
                &format!("refs/tags/{tag}"),
            ],
            repo,
        )
        .map(|o| o.status.success())
        .unwrap_or(false);
        if exists {
            return Err(finish(
                lines,
                format!(
                    "✘ Tag \"{tag}\" already exists locally. Bump the version in plugin.json, or \
                     re-run with --force to move the tag."
                ),
            ));
        }
    }

    // --- (11) info block + create/dry-run ---
    lines.push(format!("Plugin:  {name}"));
    lines.push(format!("Version: {version} (from plugin.json)"));
    if let Some(entry) = &market {
        if let Some(ev) = &entry.version {
            lines.push(format!(
                "Marketplace entry: plugins[{}] in {} (version: {ev})",
                entry.index, entry.path
            ));
        }
    }
    lines.push(format!("Tag:     {tag}"));
    lines.push(String::new());

    let msg = match message {
        Some(m) => m.replace("%s", version),
        None => format!("{name} {version}"),
    };
    let force_flag = if force { "-f " } else { "" };
    let push_force = if force { "--force " } else { "" };

    if dry_run {
        lines.push(format!(
            "✔ Dry run — would create tag {tag} at HEAD in {repo_d}"
        ));
        lines.push(format!(
            "  git -C {repo_d} tag {force_flag}-a {tag} -m \"{msg}\""
        ));
        lines.push(format!(
            "  git -C {repo_d} push {push_force}{remote} refs/tags/{tag}"
        ));
        return Ok(lines.join("\n"));
    }

    // Create the annotated tag.
    let mut args: Vec<&str> = vec!["tag"];
    if force {
        args.push("-f");
    }
    args.push("-a");
    args.push(&tag);
    args.push("-m");
    args.push(&msg);
    let created = git(&args, repo);
    match created {
        Some(o) if o.status.success() => {}
        Some(o) => {
            let err = String::from_utf8_lossy(&o.stderr);
            return Err(finish(
                lines,
                format!("✘ Failed to create tag: {}", err.trim_end()),
            ));
        }
        None => {
            return Err(finish(
                lines,
                "✘ Failed to create tag: git not available".to_string(),
            ));
        }
    }

    if push {
        let mut pargs: Vec<&str> = vec!["push"];
        if force {
            pargs.push("--force");
        }
        pargs.push(remote);
        let refspec = format!("refs/tags/{tag}");
        pargs.push(&refspec);
        match git(&pargs, repo) {
            Some(o) if o.status.success() => {
                lines.push(format!("✔ Created tag {tag}"));
                lines.push(format!("✔ Pushed to {remote}"));
                Ok(lines.join("\n"))
            }
            Some(o) => {
                let code = o.status.code().unwrap_or(-1);
                let err = String::from_utf8_lossy(&o.stderr);
                lines.push(format!(
                    "✘ Tag created locally but push failed (exit {code}): {}",
                    err.trim_end()
                ));
                Err(lines.join("\n"))
            }
            None => {
                lines.push(
                    "✘ Tag created locally but push failed (exit -1): git not available"
                        .to_string(),
                );
                Err(lines.join("\n"))
            }
        }
    } else {
        lines.push(format!("✔ Created tag {tag}"));
        lines.push(format!(
            "  Push with: git -C {repo_d} push {push_force}{remote} refs/tags/{tag}"
        ));
        Ok(lines.join("\n"))
    }
}

/// A resolved enclosing-marketplace entry for the plugin under test.
struct MarketEntry {
    /// Index of the entry in the marketplace's `plugins[]`.
    index: usize,
    /// The marketplace.json path, relative to cwd (for the info line).
    path: String,
    /// The entry's declared `version`, if any.
    version: Option<String>,
}

/// Walk up from `plugin_root` looking for an enclosing
/// `.lingxi-plugin/marketplace.json` whose `plugins[]` lists this plugin (by a
/// `source` that resolves to `plugin_root`, else by `name`). Best-effort port of
/// the documented cross-check (see the module residual note).
fn find_enclosing_marketplace(
    plugin_root: &Path,
    plugin_name: &str,
    cwd: &Path,
) -> Option<MarketEntry> {
    let mut dir: Option<&Path> = Some(plugin_root);
    while let Some(d) = dir {
        let mj = d.join(PLUGIN_MANIFEST_DIR).join("marketplace.json");
        if mj.is_file() {
            if let Some(entry) = match_entry(&mj, d, plugin_root, plugin_name) {
                return Some(MarketEntry {
                    index: entry.0,
                    path: display_path(&path_relative(cwd, &mj)),
                    version: entry.1,
                });
            }
        }
        dir = d.parent();
    }
    None
}

/// Find the `plugins[]` index + version of the entry matching this plugin within
/// `marketplace.json` (rooted at `market_root`).
fn match_entry(
    marketplace_json: &Path,
    market_root: &Path,
    plugin_root: &Path,
    plugin_name: &str,
) -> Option<(usize, Option<String>)> {
    let raw = std::fs::read_to_string(marketplace_json).ok()?;
    let value: Value = serde_json::from_str(&raw).ok()?;
    let plugins = value.get("plugins").and_then(Value::as_array)?;
    for (i, p) in plugins.iter().enumerate() {
        let matches = match p.get("source").and_then(Value::as_str) {
            Some(src) => {
                let resolved = market_root.join(src);
                let resolved = std::fs::canonicalize(&resolved).unwrap_or(resolved);
                resolved == *plugin_root
            }
            None => p.get("name").and_then(Value::as_str) == Some(plugin_name),
        };
        if matches {
            let ver = p.get("version").and_then(Value::as_str).map(str::to_string);
            return Some((i, ver));
        }
    }
    None
}

/// Join accumulated warning lines with a terminal error line for a single stdout
/// block (warnings precede the error, matching the oracle).
fn finish(mut lines: Vec<String>, error: String) -> String {
    lines.push(error);
    lines.join("\n")
}

/// The JSON type name zod reports (`undefined` for an absent key).
fn json_type(v: Option<&Value>) -> &'static str {
    match v {
        None => "undefined",
        Some(Value::Null) => "null",
        Some(Value::Bool(_)) => "boolean",
        Some(Value::Number(_)) => "number",
        Some(Value::String(_)) => "string",
        Some(Value::Array(_)) => "array",
        Some(Value::Object(_)) => "object",
    }
}

/// Kebab-case = lowercase ASCII letters, digits, and hyphens only.
fn is_kebab_case(s: &str) -> bool {
    !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

/// node `semver.valid` semantics: trims a leading `v`/`=`/whitespace run, then
/// requires strict `MAJOR.MINOR.PATCH` (no leading zeros) with optional
/// `-prerelease` and `+build`.
fn is_valid_semver(raw: &str) -> bool {
    let s = raw
        .trim()
        .trim_start_matches(|c| c == 'v' || c == '=' || c == ' ' || c == '\t');
    let (main_pre, build) = match s.split_once('+') {
        Some((a, b)) => (a, Some(b)),
        None => (s, None),
    };
    let (core, pre) = match main_pre.split_once('-') {
        Some((a, b)) => (a, Some(b)),
        None => (main_pre, None),
    };
    let mut parts = core.split('.');
    let (Some(maj), Some(min), Some(pat), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return false;
    };
    if !is_numeric_id(maj) || !is_numeric_id(min) || !is_numeric_id(pat) {
        return false;
    }
    if let Some(pre) = pre {
        if pre.is_empty() || !pre.split('.').all(is_pre_id) {
            return false;
        }
    }
    if let Some(build) = build {
        if build.is_empty() || !build.split('.').all(is_build_id) {
            return false;
        }
    }
    true
}

/// A numeric identifier: non-empty, all digits, no leading zero unless `"0"`.
fn is_numeric_id(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()) && (s.len() == 1 || !s.starts_with('0'))
}

/// A prerelease identifier: alphanumeric-with-hyphen; if all-digits it must have
/// no leading zero (unless `"0"`).
fn is_pre_id(s: &str) -> bool {
    if s.is_empty() || !s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-') {
        return false;
    }
    if s.bytes().all(|b| b.is_ascii_digit()) {
        return s.len() == 1 || !s.starts_with('0');
    }
    true
}

/// A build identifier: non-empty alphanumeric-with-hyphen (leading zeros allowed).
fn is_build_id(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
}

/// `path.relative(from, to)` — the shortest relative path (with `..`) from `from`
/// to `to`, both taken component-wise (callers pass absolute paths).
fn path_relative(from: &Path, to: &Path) -> PathBuf {
    let from_c: Vec<_> = from.components().collect();
    let to_c: Vec<_> = to.components().collect();
    let common = from_c.iter().zip(&to_c).take_while(|(a, b)| a == b).count();
    let mut result = PathBuf::new();
    for _ in common..from_c.len() {
        result.push("..");
    }
    for c in &to_c[common..] {
        result.push(c.as_os_str());
    }
    result
}

/// Display a path with `/` separators (empty → empty string).
fn display_path(p: &Path) -> String {
    p.to_string_lossy().replace('\\', "/")
}

/// Run `git <args>` with `-C <cwd>` and capture output.
fn git(args: &[&str], cwd: &Path) -> Option<std::process::Output> {
    Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args(args)
        .output()
        .ok()
}

/// Run `git <args>` and return trimmed stdout on success, else `None`.
fn git_stdout(args: &[&str], cwd: &Path) -> Option<String> {
    let out = git(args, cwd)?;
    if !out.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&out.stdout).trim_end().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Repo {
        _tmp: tempfile::TempDir,
        root: PathBuf,
    }

    fn git_init(root: &Path) {
        for args in [
            vec!["init", "-q"],
            vec!["config", "user.email", "t@t.com"],
            vec!["config", "user.name", "Tester"],
        ] {
            let ok = Command::new("git")
                .arg("-C")
                .arg(root)
                .args(&args)
                .output()
                .unwrap()
                .status
                .success();
            assert!(ok, "git {args:?} failed");
        }
    }

    fn commit(root: &Path, msg: &str) {
        Command::new("git")
            .arg("-C")
            .arg(root)
            .args(["add", "-A"])
            .output()
            .unwrap();
        let ok = Command::new("git")
            .arg("-C")
            .arg(root)
            .args(["commit", "-q", "-m", msg])
            .output()
            .unwrap()
            .status
            .success();
        assert!(ok, "commit failed");
    }

    /// A git repo whose root is a plugin with the given `plugin.json` body.
    fn repo_with(plugin_json: &str) -> Repo {
        let tmp = tempfile::tempdir().unwrap();
        // Canonicalize so `git rev-parse --show-toplevel` matches our paths on
        // macOS (/var → /private/var).
        let root = std::fs::canonicalize(tmp.path()).unwrap();
        std::fs::create_dir_all(root.join(PLUGIN_MANIFEST_DIR)).unwrap();
        std::fs::write(
            root.join(PLUGIN_MANIFEST_DIR).join("plugin.json"),
            plugin_json,
        )
        .unwrap();
        git_init(&root);
        commit(&root, "init");
        Repo { _tmp: tmp, root }
    }

    fn tag_exists(root: &Path, tag: &str) -> bool {
        Command::new("git")
            .arg("-C")
            .arg(root)
            .args([
                "rev-parse",
                "--verify",
                "--quiet",
                &format!("refs/tags/{tag}"),
            ])
            .output()
            .unwrap()
            .status
            .success()
    }

    #[test]
    fn creates_annotated_tag_with_push_hint() {
        let r = repo_with(
            r#"{ "name":"myplug","version":"1.2.3","description":"d","author":{"name":"x"} }"#,
        );
        let out = run_tag_from_cwd(&r.root, None, false, false, None, false, "origin").unwrap();
        assert!(out.contains("Plugin:  myplug"), "{out}");
        assert!(out.contains("Version: 1.2.3 (from plugin.json)"), "{out}");
        assert!(out.contains("Tag:     myplug--v1.2.3"), "{out}");
        assert!(out.contains("✔ Created tag myplug--v1.2.3"), "{out}");
        assert!(
            out.contains(&format!(
                "  Push with: git -C {} push origin refs/tags/myplug--v1.2.3",
                r.root.display()
            )),
            "{out}"
        );
        assert!(tag_exists(&r.root, "myplug--v1.2.3"));
        // Annotated tag → object type "tag".
        let ty = Command::new("git")
            .arg("-C")
            .arg(&r.root)
            .args(["cat-file", "-t", "myplug--v1.2.3"])
            .output()
            .unwrap();
        assert_eq!(String::from_utf8_lossy(&ty.stdout).trim(), "tag");
    }

    #[test]
    fn missing_manifest_errors_with_absolute_path() {
        let tmp = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(tmp.path()).unwrap();
        let err = run_tag(
            Some(root.to_str().unwrap()),
            false,
            false,
            None,
            false,
            "origin",
        )
        .unwrap_err();
        assert_eq!(
            err,
            format!(
                "✘ No plugin manifest found. Expected {}.",
                root.join(PLUGIN_MANIFEST_DIR).join("plugin.json").display()
            )
        );
    }

    #[test]
    fn dry_run_previews_git_commands_no_tag() {
        let r = repo_with(
            r#"{ "name":"p","version":"1.2.3","description":"d","author":{"name":"x"} }"#,
        );
        let out = run_tag_from_cwd(&r.root, None, true, false, None, false, "origin").unwrap();
        assert!(
            out.contains("✔ Dry run — would create tag p--v1.2.3 at HEAD in "),
            "{out}"
        );
        assert!(
            out.contains(&format!(
                "  git -C {} tag -a p--v1.2.3 -m \"p 1.2.3\"",
                r.root.display()
            )),
            "{out}"
        );
        assert!(
            out.contains(&format!(
                "  git -C {} push origin refs/tags/p--v1.2.3",
                r.root.display()
            )),
            "{out}"
        );
        assert!(
            !tag_exists(&r.root, "p--v1.2.3"),
            "dry-run must not create the tag"
        );
    }

    #[test]
    fn dry_run_force_adds_flags_and_remote() {
        let r = repo_with(
            r#"{ "name":"p","version":"1.2.3","description":"d","author":{"name":"x"} }"#,
        );
        let out = run_tag_from_cwd(&r.root, None, true, true, None, true, "upstream").unwrap();
        assert!(
            out.contains(&format!(
                "  git -C {} tag -f -a p--v1.2.3 -m \"p 1.2.3\"",
                r.root.display()
            )),
            "{out}"
        );
        assert!(
            out.contains(&format!(
                "  git -C {} push --force upstream refs/tags/p--v1.2.3",
                r.root.display()
            )),
            "{out}"
        );
    }

    #[test]
    fn message_replaces_percent_s_with_version() {
        let r = repo_with(
            r#"{ "name":"p","version":"1.2.3","description":"d","author":{"name":"x"} }"#,
        );
        run_tag_from_cwd(
            &r.root,
            None,
            false,
            false,
            Some("Release %s of plugin"),
            false,
            "origin",
        )
        .unwrap();
        let ann = Command::new("git")
            .arg("-C")
            .arg(&r.root)
            .args(["tag", "-l", "-n99", "p--v1.2.3"])
            .output()
            .unwrap();
        assert!(
            String::from_utf8_lossy(&ann.stdout).contains("Release 1.2.3 of plugin"),
            "annotation: {}",
            String::from_utf8_lossy(&ann.stdout)
        );
    }

    #[test]
    fn already_exists_without_force_errors() {
        let r = repo_with(
            r#"{ "name":"p","version":"1.2.3","description":"d","author":{"name":"x"} }"#,
        );
        run_tag_from_cwd(&r.root, None, false, false, None, false, "origin").unwrap();
        let err = run_tag_from_cwd(&r.root, None, false, false, None, false, "origin").unwrap_err();
        assert_eq!(
            err,
            "✘ Tag \"p--v1.2.3\" already exists locally. Bump the version in plugin.json, or \
             re-run with --force to move the tag."
        );
    }

    #[test]
    fn force_moves_existing_tag() {
        let r = repo_with(
            r#"{ "name":"p","version":"1.2.3","description":"d","author":{"name":"x"} }"#,
        );
        run_tag_from_cwd(&r.root, None, false, false, None, false, "origin").unwrap();
        let out = run_tag_from_cwd(&r.root, None, false, true, None, false, "origin").unwrap();
        assert!(out.contains("✔ Created tag p--v1.2.3"), "{out}");
        assert!(
            out.contains("push --force origin refs/tags/p--v1.2.3"),
            "{out}"
        );
    }

    #[test]
    fn dirty_tree_scoped_to_plugin_dir() {
        // Plugin in a subdir; a dirty file OUTSIDE it must NOT block, one INSIDE must.
        let tmp = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(tmp.path()).unwrap();
        let plug = root.join("plugins").join("myplug");
        std::fs::create_dir_all(plug.join(PLUGIN_MANIFEST_DIR)).unwrap();
        std::fs::create_dir_all(root.join("other")).unwrap();
        std::fs::write(
            plug.join(PLUGIN_MANIFEST_DIR).join("plugin.json"),
            r#"{ "name":"myplug","version":"1.2.3","description":"d","author":{"name":"x"} }"#,
        )
        .unwrap();
        git_init(&root);
        commit(&root, "init");

        // dirty only outside → succeeds
        std::fs::write(root.join("other").join("dirty.txt"), "x").unwrap();
        let out = run_tag_from_cwd(
            &root,
            Some("plugins/myplug"),
            false,
            false,
            None,
            false,
            "origin",
        )
        .unwrap();
        assert!(out.contains("✔ Created tag myplug--v1.2.3"), "{out}");
        Command::new("git")
            .arg("-C")
            .arg(&root)
            .args(["tag", "-d", "myplug--v1.2.3"])
            .output()
            .unwrap();

        // dirty inside → blocks, listing the repo-relative path
        std::fs::write(plug.join("inside.txt"), "x").unwrap();
        let err = run_tag_from_cwd(
            &root,
            Some("plugins/myplug"),
            false,
            false,
            None,
            false,
            "origin",
        )
        .unwrap_err();
        assert!(
            err.starts_with(
                "✘ Uncommitted changes affecting this release — commit them first so the tag \
                 points at the version you intend to release (or use --force):"
            ),
            "{err}"
        );
        assert!(err.contains("\n  plugins/myplug/inside.txt"), "{err}");
    }

    #[test]
    fn dirty_tree_truncates_after_five() {
        let r = repo_with(
            r#"{ "name":"p","version":"1.2.3","description":"d","author":{"name":"x"} }"#,
        );
        for i in 1..=15 {
            std::fs::write(r.root.join(format!("f{i}.txt")), "x").unwrap();
        }
        let err = run_tag_from_cwd(&r.root, None, false, false, None, false, "origin").unwrap_err();
        assert!(err.contains("\n  …and 10 more"), "{err}");
        // Only 5 file lines before the "…and N more".
        let listed = err.matches("\n  f").count();
        assert_eq!(listed, 5, "{err}");
    }

    #[test]
    fn no_version_warns_and_errors() {
        let r = repo_with(r#"{ "name":"p" }"#);
        let err = run_tag_from_cwd(&r.root, None, false, false, None, false, "origin").unwrap_err();
        assert!(
            err.contains(".lingxi-plugin/plugin.json: No version specified."),
            "{err}"
        );
        assert!(
            err.contains(
                "✘ No version to tag. Set \"version\" in .lingxi-plugin/plugin.json. Tags are only \
                 used for dependency version constraints, which require an explicit semver — the \
                 git-SHA fallback does not need a tag."
            ),
            "{err}"
        );
    }

    #[test]
    fn invalid_semver_errors() {
        let r =
            repo_with(r#"{ "name":"p","version":"nope","description":"d","author":{"name":"x"} }"#);
        let err = run_tag_from_cwd(&r.root, None, false, false, None, false, "origin").unwrap_err();
        assert!(
            err.contains(
                "✘ Version \"nope\" is not valid semver. Dependency resolution (resolveVersionRange) \
                 ignores tags whose suffix doesn't parse as semver, so this tag would never be \
                 selected."
            ),
            "{err}"
        );
    }

    #[test]
    fn name_type_validation_short_circuits() {
        let r = repo_with(r#"{ "name": 5, "version":"1.0.0" }"#);
        let err = run_tag_from_cwd(&r.root, None, false, false, None, false, "origin").unwrap_err();
        assert!(err.starts_with("✘ Plugin validation failed for "), "{err}");
        assert!(
            err.ends_with(":\n  name: Invalid input: expected string, received number"),
            "{err}"
        );
        // No warnings before a hard validation failure.
        assert!(!err.contains('⚠'), "{err}");
    }

    /// §8 (the 2.1.247 hardening): the shared oracle `se` validator now backs
    /// this hard-validation arm, so a bidi-formatting name is rejected here
    /// too, not just on the load path.
    #[test]
    fn name_control_bidi_validation_short_circuits() {
        let r = repo_with("{ \"name\": \"evil\u{202E}reversed\", \"version\":\"1.0.0\" }");
        let err = run_tag_from_cwd(&r.root, None, false, false, None, false, "origin").unwrap_err();
        assert!(err.starts_with("✘ Plugin validation failed for "), "{err}");
        assert!(
            err.ends_with(
                ":\n  name: Plugin name cannot contain control or bidirectional-formatting \
                 characters"
            ),
            "{err}"
        );
    }

    #[test]
    fn empty_name_errors() {
        let r = repo_with(r#"{ "name": "", "version":"1.0.0" }"#);
        let err = run_tag_from_cwd(&r.root, None, false, false, None, false, "origin").unwrap_err();
        assert!(
            err.ends_with(":\n  name: Plugin name cannot be empty"),
            "{err}"
        );
    }

    #[test]
    fn space_name_errors() {
        let r = repo_with(r#"{ "name": "my plug", "version":"1.0.0" }"#);
        let err = run_tag_from_cwd(&r.root, None, false, false, None, false, "origin").unwrap_err();
        assert!(
            err.ends_with(
                ":\n  name: Plugin name cannot contain spaces. Use kebab-case (e.g., \"my-plugin\")"
            ),
            "{err}"
        );
    }

    #[test]
    fn not_git_repo_errors() {
        let tmp = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(tmp.path()).unwrap();
        std::fs::create_dir_all(root.join(PLUGIN_MANIFEST_DIR)).unwrap();
        std::fs::write(
            root.join(PLUGIN_MANIFEST_DIR).join("plugin.json"),
            r#"{ "name":"p","version":"1.2.3","description":"d","author":{"name":"x"} }"#,
        )
        .unwrap();
        let err = run_tag_from_cwd(&root, None, false, false, None, false, "origin").unwrap_err();
        assert!(
            err.contains(&format!(
                "✘ {} is not inside a git repository. Dependency tags are resolved via git \
                 ls-remote, so the plugin must live in a git repo.",
                root.display()
            )),
            "{err}"
        );
    }

    #[test]
    fn enclosing_marketplace_version_mismatch_errors() {
        // marketplace.json at repo root, plugin in a subdir with a disagreeing version.
        let tmp = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(tmp.path()).unwrap();
        let plug = root.join("plugins").join("myplug");
        std::fs::create_dir_all(plug.join(PLUGIN_MANIFEST_DIR)).unwrap();
        std::fs::create_dir_all(root.join(PLUGIN_MANIFEST_DIR)).unwrap();
        std::fs::write(
            root.join(PLUGIN_MANIFEST_DIR).join("marketplace.json"),
            r#"{ "name":"m","plugins":[{"name":"myplug","source":"./plugins/myplug","version":"9.9.9"}] }"#,
        )
        .unwrap();
        std::fs::write(
            plug.join(PLUGIN_MANIFEST_DIR).join("plugin.json"),
            r#"{ "name":"myplug","version":"1.2.3","description":"d","author":{"name":"x"} }"#,
        )
        .unwrap();
        git_init(&root);
        commit(&root, "init");
        let err = run_tag_from_cwd(
            &root,
            Some("plugins/myplug"),
            false,
            false,
            None,
            false,
            "origin",
        )
        .unwrap_err();
        assert!(
            err.contains(
                "✘ Version mismatch: plugin.json says \"1.2.3\" but \
                 .lingxi-plugin/marketplace.json plugins[0].version says \
                 \"9.9.9\". plugin.json wins at install time, so update the marketplace entry to \
                 \"1.2.3\" (or remove it) before tagging."
            ),
            "{err}"
        );
    }

    #[test]
    fn enclosing_marketplace_agree_shows_info_line() {
        let tmp = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(tmp.path()).unwrap();
        let plug = root.join("plugins").join("myplug");
        std::fs::create_dir_all(plug.join(PLUGIN_MANIFEST_DIR)).unwrap();
        std::fs::create_dir_all(root.join(PLUGIN_MANIFEST_DIR)).unwrap();
        std::fs::write(
            root.join(PLUGIN_MANIFEST_DIR).join("marketplace.json"),
            r#"{ "name":"m","plugins":[{"name":"myplug","source":"./plugins/myplug","version":"1.2.3"}] }"#,
        )
        .unwrap();
        std::fs::write(
            plug.join(PLUGIN_MANIFEST_DIR).join("plugin.json"),
            r#"{ "name":"myplug","version":"1.2.3","description":"d","author":{"name":"x"} }"#,
        )
        .unwrap();
        git_init(&root);
        commit(&root, "init");
        let out = run_tag_from_cwd(
            &root,
            Some("plugins/myplug"),
            true,
            false,
            None,
            false,
            "origin",
        )
        .unwrap();
        assert!(
            out.contains(&format!(
                "Marketplace entry: plugins[0] in {}/marketplace.json (version: 1.2.3)",
                PLUGIN_MANIFEST_DIR
            )),
            "{out}"
        );
    }

    #[test]
    fn semver_boundaries() {
        assert!(is_valid_semver("1.2.3"));
        assert!(is_valid_semver("1.0.0-beta.1"));
        assert!(is_valid_semver("v1.2.3"));
        assert!(is_valid_semver("1.2.3+build.5"));
        assert!(!is_valid_semver("1.2"));
        assert!(!is_valid_semver("01.2.3"));
        assert!(!is_valid_semver("nope"));
        assert!(!is_valid_semver("1.2.3-"));
    }

    #[test]
    fn push_success_reports_pushed() {
        // Real bare remote so the push actually succeeds. Keep it in its OWN
        // tempdir (not r.root.parent(), which is the shared $TMPDIR — a fixed
        // path there persists across runs and collides with `tag already exists`).
        let r = repo_with(
            r#"{ "name":"p","version":"1.2.3","description":"d","author":{"name":"x"} }"#,
        );
        let bare_tmp = tempfile::tempdir().unwrap();
        let bare = bare_tmp.path().join("bare.git");
        Command::new("git")
            .args(["init", "-q", "--bare"])
            .arg(&bare)
            .output()
            .unwrap();
        Command::new("git")
            .arg("-C")
            .arg(&r.root)
            .args(["remote", "add", "origin"])
            .arg(&bare)
            .output()
            .unwrap();
        let out = run_tag_from_cwd(&r.root, None, false, false, None, true, "origin").unwrap();
        assert!(out.contains("✔ Created tag p--v1.2.3"), "{out}");
        assert!(out.contains("✔ Pushed to origin"), "{out}");
        // Remote actually has the tag.
        let ls = Command::new("git")
            .args(["ls-remote", "--tags"])
            .arg(&bare)
            .output()
            .unwrap();
        assert!(String::from_utf8_lossy(&ls.stdout).contains("refs/tags/p--v1.2.3"));
    }

    #[test]
    fn push_failure_reports_tag_created_locally() {
        // No remote configured → push fails, tag stays local.
        let r = repo_with(
            r#"{ "name":"p","version":"1.2.3","description":"d","author":{"name":"x"} }"#,
        );
        let err = run_tag_from_cwd(&r.root, None, false, false, None, true, "origin").unwrap_err();
        assert!(
            err.contains("✘ Tag created locally but push failed (exit "),
            "{err}"
        );
        // Success line is suppressed on push failure.
        assert!(!err.contains("✔ Created tag"), "{err}");
        assert!(tag_exists(&r.root, "p--v1.2.3"), "tag should exist locally");
    }
}
