//! `plugin init <name>` — scaffold a new skill-plugin under `~/.lingxi/skills/`.
//!
//! 1:1 with claude-code 2.1.201's default `plugin init` (verified end-to-end):
//! creates `<skills>/<name>/.lingxi-plugin/plugin.json` + `<skills>/<name>/SKILL.md`
//! and prints the three-line success block. The plugin auto-loads next session as
//! `<name>@skills-dir`.
//!
//! Author name/email default to `git config user.name` / `user.email` (overridable
//! via `--author` / `--author-email`); `--description` sets the manifest
//! description; `--force` overwrites an existing `.lingxi-plugin/`.
//!
//! Residual (follow-up): the `--with <components…>` scaffolds (skills, agents,
//! hooks, mcp, lsp, output-style, channel) — templates are captured in the port
//! plan; only the default skill scaffold is wired here.

use std::path::{Path, PathBuf};

use serde_json::{Map, Value};

/// The `SKILL.md` body for a freshly-scaffolded skill plugin (name interpolated).
fn skill_md(name: &str) -> String {
    format!(
        "---\n\
         name: {name}\n\
         description: TODO — describe WHEN Claude should use this. Include trigger phrases users\n  \
         might say (\"do X\", \"set up Y\", \"review Z\"). Be specific; this string is what Claude\n  \
         matches the user's request against.\n\
         ---\n\
         \n\
         # {name}\n\
         \n\
         TODO: what this skill does, and the steps Claude should take.\n"
    )
}

/// `git config <key>` trimmed, or empty when unset / git unavailable.
fn git_config(key: &str) -> String {
    std::process::Command::new("git")
        .args(["config", key])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default()
}

/// Abbreviate `path` to `~/…` when it lives under the OS home directory.
fn tilde(path: &Path) -> String {
    if let Some(home) = dirs::home_dir() {
        if let Ok(rel) = path.strip_prefix(&home) {
            return format!("~/{}", rel.display());
        }
    }
    path.display().to_string()
}

/// Build the scaffolded `plugin.json` (key order matches the binary).
fn plugin_json(name: &str, author: &str, email: &str, description: &str) -> String {
    let mut author_obj = Map::new();
    author_obj.insert("name".to_string(), Value::String(author.to_string()));
    author_obj.insert("email".to_string(), Value::String(email.to_string()));

    let mut root = Map::new();
    root.insert(
        "$schema".to_string(),
        Value::String("https://anthropic.com/claude-code/plugin.schema.json".to_string()),
    );
    root.insert("name".to_string(), Value::String(name.to_string()));
    root.insert("version".to_string(), Value::String("0.1.0".to_string()));
    root.insert("description".to_string(), Value::String(description.to_string()));
    root.insert("author".to_string(), Value::Object(author_obj));
    root.insert("skills".to_string(), Value::Array(vec![Value::String("./".to_string())]));

    serde_json::to_string_pretty(&Value::Object(root)).unwrap_or_default()
}

/// `plugin init <name>` (default skill scaffold). Returns the success block, or
/// the already-formatted error line.
#[allow(clippy::too_many_arguments)]
pub fn run_init(
    name: &str,
    author: Option<&str>,
    author_email: Option<&str>,
    description: Option<&str>,
    force: bool,
    home: &Path,
) -> Result<String, String> {
    let plugin_root: PathBuf = home.join("skills").join(name);
    let manifest_dir = plugin_root.join(branding::PLUGIN_MANIFEST_DIR);

    if manifest_dir.exists() && !force {
        return Err(format!(
            "✘ {} already exists. Use --force to overwrite.",
            manifest_dir.display()
        ));
    }

    let author = author
        .map(str::to_string)
        .unwrap_or_else(|| git_config("user.name"));
    let email = author_email
        .map(str::to_string)
        .unwrap_or_else(|| git_config("user.email"));
    let description = description.unwrap_or("TODO: describe what this plugin provides");

    std::fs::create_dir_all(&manifest_dir)
        .map_err(|e| format!("✘ Failed to create {}: {e}", manifest_dir.display()))?;
    std::fs::write(
        manifest_dir.join("plugin.json"),
        plugin_json(name, &author, &email, description),
    )
    .map_err(|e| format!("✘ Failed to write plugin.json: {e}"))?;
    std::fs::write(plugin_root.join("SKILL.md"), skill_md(name))
        .map_err(|e| format!("✘ Failed to write SKILL.md: {e}"))?;

    Ok(format!(
        "✔ Created plugin \"{name}\" at {}\n  \
         It will auto-load next session as {name}@skills-dir. Run /reload-plugins to load it now.\n  \
         Disable: lingxi-cli plugin disable {name}@skills-dir. Remove: delete the directory.",
        tilde(&plugin_root)
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Env {
        _tmp: tempfile::TempDir,
        home: PathBuf,
    }

    fn env() -> Env {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join(".lingxi");
        std::fs::create_dir_all(&home).unwrap();
        Env { _tmp: tmp, home }
    }

    #[test]
    fn init_scaffolds_manifest_and_skill() {
        let e = env();
        let msg = run_init("myplug", Some("Bob"), Some("a@b.c"), None, false, &e.home).unwrap();
        assert!(
            msg.starts_with("✔ Created plugin \"myplug\" at "),
            "got: {msg}"
        );
        assert!(msg.contains("auto-load next session as myplug@skills-dir"));
        assert!(msg.contains("Disable: lingxi-cli plugin disable myplug@skills-dir"));

        let root = e.home.join("skills").join("myplug");
        let manifest: Value = serde_json::from_str(
            &std::fs::read_to_string(root.join(".lingxi-plugin").join("plugin.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(manifest["$schema"], "https://anthropic.com/claude-code/plugin.schema.json");
        assert_eq!(manifest["name"], "myplug");
        assert_eq!(manifest["version"], "0.1.0");
        assert_eq!(manifest["description"], "TODO: describe what this plugin provides");
        assert_eq!(manifest["author"], serde_json::json!({"name": "Bob", "email": "a@b.c"}));
        assert_eq!(manifest["skills"], serde_json::json!(["./"]));

        let skill = std::fs::read_to_string(root.join("SKILL.md")).unwrap();
        assert!(skill.starts_with("---\nname: myplug\ndescription: TODO — describe WHEN Claude"));
        assert!(skill.trim_end().ends_with("the steps Claude should take."));
    }

    #[test]
    fn init_custom_description() {
        let e = env();
        run_init("p", Some("A"), Some("a@b"), Some("My desc"), false, &e.home).unwrap();
        let manifest: Value = serde_json::from_str(
            &std::fs::read_to_string(e.home.join("skills/p/.lingxi-plugin/plugin.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(manifest["description"], "My desc");
    }

    #[test]
    fn init_duplicate_without_force_errors() {
        let e = env();
        run_init("dup", Some("A"), Some("a@b"), None, false, &e.home).unwrap();
        let err = run_init("dup", Some("A"), Some("a@b"), None, false, &e.home).unwrap_err();
        assert!(err.ends_with(".lingxi-plugin already exists. Use --force to overwrite."), "got: {err}");
        assert!(err.starts_with("✘ "));
    }

    #[test]
    fn init_force_overwrites() {
        let e = env();
        run_init("f", Some("A"), Some("a@b"), None, false, &e.home).unwrap();
        // Second call with force succeeds.
        let msg = run_init("f", Some("A"), Some("a@b"), None, true, &e.home).unwrap();
        assert!(msg.starts_with("✔ Created plugin \"f\""));
    }
}
