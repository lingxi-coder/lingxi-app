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
//! `--with <components…>` additionally scaffolds any of: `skills`, `agents`,
//! `hooks`, `mcp`, `lsp`, `output-style`, `channel` (1:1 with the binary). An
//! unknown component name errors before anything is written. `channel` also
//! appends a `channels` entry to `plugin.json` and (like `mcp`) writes an
//! `.mcp.json`; when both are requested `channel`'s file wins.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value};

use crate::commands::{plugin_policy, plugin_settings};

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

/// Build the scaffolded `plugin.json` (key order matches the binary). When
/// `with_channel` is set the manifest gains a `channels` entry (server +
/// displayName both = `name`), appended after `skills` exactly as the binary
/// does for `--with channel`.
fn plugin_json(
    name: &str,
    author: &str,
    email: &str,
    description: &str,
    with_channel: bool,
) -> String {
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
    root.insert(
        "description".to_string(),
        Value::String(description.to_string()),
    );
    root.insert("author".to_string(), Value::Object(author_obj));
    root.insert(
        "skills".to_string(),
        Value::Array(vec![Value::String("./".to_string())]),
    );

    if with_channel {
        let mut channel = Map::new();
        channel.insert("server".to_string(), Value::String(name.to_string()));
        channel.insert("displayName".to_string(), Value::String(name.to_string()));
        root.insert(
            "channels".to_string(),
            Value::Array(vec![Value::Object(channel)]),
        );
    }

    serde_json::to_string_pretty(&Value::Object(root)).unwrap_or_default()
}

/// The seven valid `--with` component names, in the binary's canonical scaffold
/// order (mcp before channel so channel's `.mcp.json` wins when both are asked).
const WITH_COMPONENTS: [&str; 7] = [
    "skills",
    "agents",
    "hooks",
    "mcp",
    "lsp",
    "output-style",
    "channel",
];

/// `agents/example.md` — an example subagent definition.
const AGENT_EXAMPLE_MD: &str = "---\n\
     name: example\n\
     description: TODO — when should Claude delegate to this subagent?\n\
     tools:\n  \
     - Read\n  \
     - Grep\n\
     ---\n\
     \n\
     TODO: system prompt for the subagent.\n";

/// `hooks/hooks.json` — a `SessionStart` hook wired to the handler script.
/// Branding: `${CLAUDE_PLUGIN_ROOT}` → `${LINGXI_PLUGIN_ROOT}`.
const HOOKS_JSON: &str = "{\n  \
     \"hooks\": {\n    \
     \"SessionStart\": [\n      \
     {\n        \
     \"hooks\": [\n          \
     {\n            \
     \"type\": \"command\",\n            \
     \"command\": \"bun ${LINGXI_PLUGIN_ROOT}/hooks-handlers/on-session-start.ts\"\n          \
     }\n        \
     ]\n      \
     }\n    \
     ]\n  \
     }\n}\n";

/// `hooks-handlers/on-session-start.ts` — the SessionStart handler stub.
const ON_SESSION_START_TS: &str = "#!/usr/bin/env bun\n\
     // SessionStart hook handler. Reads the event from stdin, writes a JSON result\n\
     // to stdout. Swap \"bun\" for \"node\" or \"python3\" in hooks/hooks.json if your\n\
     // users' environment lacks bun.\n\
     const input = await new Response(Bun.stdin.stream()).text()\n\
     const event = JSON.parse(input)\n\
     process.stdout.write(JSON.stringify({}))\n";

/// `.mcp.json` for `--with mcp` — example remote + local server stubs.
const MCP_JSON: &str = "{\n  \
     \"mcpServers\": {\n    \
     \"example-remote\": {\n      \
     \"type\": \"http\",\n      \
     \"url\": \"https://example.com/mcp\"\n    \
     },\n    \
     \"example-local\": {\n      \
     \"command\": \"npx\",\n      \
     \"args\": [\n        \
     \"<your-mcp-server-package>\"\n      \
     ]\n    \
     }\n  \
     }\n}\n";

/// `.lsp.json` for `--with lsp` — an example language-server stub.
const LSP_JSON: &str = "{\n  \
     \"example\": {\n    \
     \"command\": \"example-language-server\",\n    \
     \"args\": [\n      \
     \"--stdio\"\n    \
     ],\n    \
     \"extensionToLanguage\": {\n      \
     \".example\": \"example\"\n    \
     }\n  \
     }\n}\n";

/// `output-styles/<name>.md` for `--with output-style`.
fn output_style_md(name: &str) -> String {
    format!(
        "---\n\
         name: {name}\n\
         description: TODO — one line shown in the Output style picker in /config\n\
         force-for-plugin: true\n\
         keep-coding-instructions: true\n\
         ---\n\
         \n\
         TODO: the style prompt. This is appended to Claude's system prompt while the\n\
         style is active. With force-for-plugin: true, the style applies automatically\n\
         when this plugin is enabled.\n"
    )
}

/// `package.json` for `--with channel`. Branding: pkg name `claude-channel-` →
/// `lingxi-channel-`.
fn channel_package_json(name: &str) -> String {
    format!(
        "{{\n  \
         \"name\": \"lingxi-channel-{name}\",\n  \
         \"version\": \"0.1.0\",\n  \
         \"type\": \"module\",\n  \
         \"scripts\": {{\n    \
         \"start\": \"bun install --no-summary && bun server.ts\"\n  \
         }},\n  \
         \"dependencies\": {{\n    \
         \"@modelcontextprotocol/sdk\": \"^1.0.0\"\n  \
         }}\n}}\n"
    )
}

/// `.mcp.json` for `--with channel` — points the plugin's MCP server at the
/// bundled channel `server.ts`. Branding: `${CLAUDE_PLUGIN_ROOT}` →
/// `${LINGXI_PLUGIN_ROOT}`.
fn channel_mcp_json(name: &str) -> String {
    format!(
        "{{\n  \
         \"mcpServers\": {{\n    \
         \"{name}\": {{\n      \
         \"command\": \"bun\",\n      \
         \"args\": [\n        \
         \"run\",\n        \
         \"--cwd\",\n        \
         \"${{LINGXI_PLUGIN_ROOT}}\",\n        \
         \"--shell=bun\",\n        \
         \"--silent\",\n        \
         \"start\"\n      \
         ]\n    \
         }}\n  \
         }}\n}}\n"
    )
}

/// `server.ts` for `--with channel` — a stdio MCP server implementing the
/// channel contract. No branding tokens: the `claude/channel` protocol key and
/// `docs.claude.com` URL are kept verbatim; only the plugin `name` is
/// interpolated (5 sites).
fn channel_server_ts(name: &str) -> String {
    CHANNEL_SERVER_TS_TEMPLATE.replace("__NAME__", name)
}

const CHANNEL_SERVER_TS_TEMPLATE: &str = r#"#!/usr/bin/env bun
/**
 * __NAME__ channel server — stdio MCP server implementing the channel contract.
 * See https://docs.claude.com/en/docs/claude-code/channels-reference.
 */
import { Server } from '@modelcontextprotocol/sdk/server/index.js'
import { StdioServerTransport } from '@modelcontextprotocol/sdk/server/stdio.js'
import {
  CallToolRequestSchema,
  ListToolsRequestSchema,
} from '@modelcontextprotocol/sdk/types.js'

const mcp = new Server(
  { name: '__NAME__', version: '0.1.0' },
  {
    capabilities: {
      tools: {},
      // Required: presence of this key registers the channel notification
      // listener on Claude's side.
      experimental: { 'claude/channel': {} },
    },
    instructions:
      "Events from __NAME__ arrive as <channel source=\"__NAME__\" ...>. Anything " +
      "you want the sender to see must go through the reply tool — your " +
      "transcript output never reaches the channel.",
  },
)

mcp.setRequestHandler(ListToolsRequestSchema, async () => ({
  tools: [
    {
      name: 'reply',
      description: 'Send a message back to the __NAME__ channel.',
      inputSchema: {
        type: 'object',
        properties: { text: { type: 'string' } },
        required: ['text'],
      },
    },
  ],
}))

mcp.setRequestHandler(CallToolRequestSchema, async req => {
  const args = (req.params.arguments ?? {}) as Record<string, unknown>
  if (req.params.name === 'reply') {
    // TODO: deliver args.text to the external service.
    return { content: [{ type: 'text', text: 'sent' }] }
  }
  return { content: [{ type: 'text', text: 'unknown tool' }], isError: true }
})

// TODO: when the external service has an inbound event, push it to Claude:
//
//   await mcp.notification({
//     method: 'notifications/claude/channel',
//     params: {
//       content: 'the event body',
//       meta: { chat_id: '...', sender: '...' },
//     },
//   })
//
// Each meta key becomes an attribute on the <channel> tag. Keys must be
// identifiers (letters/digits/underscores) — others are silently dropped.

await mcp.connect(new StdioServerTransport())
"#;

/// Scaffold a single `--with` component into an already-created plugin root.
/// `name` is the plugin name (used for `output-style` filename + `channel`
/// interpolation). Components are dispatched in [`WITH_COMPONENTS`] order.
fn scaffold_component(component: &str, plugin_root: &Path, name: &str) -> Result<(), String> {
    let write = |rel: &str, body: &str| -> Result<(), String> {
        let dest = plugin_root.join(rel);
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("✘ Failed to create {}: {e}", parent.display()))?;
        }
        std::fs::write(&dest, body).map_err(|e| format!("✘ Failed to write {rel}: {e}"))
    };

    match component {
        "skills" => write("skills/example/SKILL.md", &skill_md("example")),
        "agents" => write("agents/example.md", AGENT_EXAMPLE_MD),
        "hooks" => {
            write("hooks/hooks.json", HOOKS_JSON)?;
            write("hooks-handlers/on-session-start.ts", ON_SESSION_START_TS)
        }
        "mcp" => write(".mcp.json", MCP_JSON),
        "lsp" => write(".lsp.json", LSP_JSON),
        "output-style" => write(&format!("output-styles/{name}.md"), &output_style_md(name)),
        "channel" => {
            write("package.json", &channel_package_json(name))?;
            write("server.ts", &channel_server_ts(name))?;
            write(".mcp.json", &channel_mcp_json(name))
        }
        // Unreachable: callers validate against WITH_COMPONENTS first.
        _ => Ok(()),
    }
}

/// §22: the marketplace name a freshly-scaffolded skills-dir plugin loads
/// under (oracle `Zc`).
const SKILLS_DIR_MARKETPLACE: &str = "skills-dir";

/// §22 — `plugin init`'s post-create name-collision line (oracle: the
/// `if(j) … else if(I) … else if(A) … else …` chain in the `/plugin init`
/// success handler). `None` when nothing conflicts, in which case the caller
/// prints the ordinary auto-load line instead.
///
/// Checked in the oracle's own priority order:
/// 1. `name` is claimed by managed settings' `enabledPlugins` (any boolean
///    value keyed `name@*`) — that entry wins regardless of what it names, so
///    this copy scaffolds but can never load under this name.
/// 2. An already-enabled, non-skills-dir, non-blocked, cache-known
///    marketplace plugin has the same plain name — it loads first, so this
///    copy never will.
/// 3. This exact `name@skills-dir` id was already explicitly disabled in a
///    settings scope (user < project < local; a later scope's value wins).
fn name_collision_warning(name: &str, home: &Path, cwd: &Path) -> Option<String> {
    let qualified = format!("{name}@{SKILLS_DIR_MARKETPLACE}");
    let manifest_dir = branding::PLUGIN_MANIFEST_DIR;

    if plugin_policy::managed_locked_plugin_names().contains(name) {
        return Some(format!(
            "  \u{26a0} A plugin named \"{name}\" is locked by managed settings, which takes \
             precedence \u{2014} {qualified} won't load. To load this copy, give it a different \
             \"name\" in {manifest_dir}/plugin.json."
        ));
    }

    // Merge the editable enabledPlugins scopes: user < project < local (a
    // later scope's value for the same key wins), matching this port's other
    // scope-precedence readers (e.g. `load_enabled_plugins`).
    let mut merged: BTreeMap<String, bool> = BTreeMap::new();
    for scope in plugin_settings::SCOPES {
        for (key, value) in plugin_settings::read_enabled(&scope.path(home, cwd)) {
            if let Some(enabled) = value.as_bool() {
                merged.insert(key, enabled);
            }
        }
    }

    let blocked = plugin_policy::blocked_marketplaces();
    let conflict = merged.iter().find(|(key, &enabled)| {
        enabled
            && key.as_str() != qualified
            && key.split_once('@').is_some_and(|(other_name, marketplace)| {
                other_name == name
                    && marketplace != SKILLS_DIR_MARKETPLACE
                    && !blocked.contains(marketplace)
                    && plugin_settings::marketplace_source(home, marketplace).is_some()
            })
    });
    if let Some((conflicting_id, _)) = conflict {
        return Some(format!(
            "  \u{26a0} The name \"{name}\" is already taken by {conflicting_id} \u{2014} when \
             that plugin loads, {qualified} won't. To load this copy, give it a different \
             \"name\" in {manifest_dir}/plugin.json or uninstall the conflicting plugin."
        ));
    }

    if merged.get(&qualified) == Some(&false) {
        return Some(format!(
            "  \u{26a0} A disabled setting for {qualified} exists, so it won't load until you \
             re-enable it in /plugin"
        ));
    }

    None
}

/// `plugin init <name>` (default skill scaffold plus optional `--with`
/// components). Returns the success block, or the already-formatted error line.
#[allow(clippy::too_many_arguments)]
pub fn run_init(
    name: &str,
    author: Option<&str>,
    author_email: Option<&str>,
    description: Option<&str>,
    force: bool,
    with: &[String],
    home: &Path,
    cwd: &Path,
) -> Result<String, String> {
    // Reject names that would escape `~/.lingxi/skills/` (path traversal /
    // arbitrary-write) — the binary validates this and writes nothing.
    if name.is_empty()
        || name.contains('/')
        || name.contains('\\')
        || name.contains("..")
        || name == "."
    {
        return Err(format!(
            "✘ Invalid plugin name \"{name}\": Plugin name cannot contain path separators \
             (/ or \\), \"..\" sequences, or be \".\""
        ));
    }
    // §8 (the 2.1.247 hardening, oracle `se`'s `yt`-backed refine): a name
    // built from Unicode control/bidi-formatting characters could visually
    // spoof a different plugin name once scaffolded and later listed.
    if plugin::has_control_or_bidi_formatting(name) {
        return Err(format!(
            "✘ Invalid plugin name \"{name}\": Plugin name cannot contain control or \
             bidirectional-formatting characters"
        ));
    }

    // Validate `--with` components up-front (before touching the filesystem), so
    // an unknown name errors cleanly and scaffolds nothing — matching the binary.
    for component in with {
        if !WITH_COMPONENTS.contains(&component.as_str()) {
            return Err(format!(
                "✘ Unknown --with component \"{component}\". Valid: {}",
                WITH_COMPONENTS.join(", ")
            ));
        }
    }

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
    let with_channel = with.iter().any(|c| c == "channel");

    std::fs::create_dir_all(&manifest_dir)
        .map_err(|e| format!("✘ Failed to create {}: {e}", manifest_dir.display()))?;
    std::fs::write(
        manifest_dir.join("plugin.json"),
        // The binary writes plugin.json with a trailing newline (`}\n`).
        format!(
            "{}\n",
            plugin_json(name, &author, &email, description, with_channel)
        ),
    )
    .map_err(|e| format!("✘ Failed to write plugin.json: {e}"))?;
    std::fs::write(plugin_root.join("SKILL.md"), skill_md(name))
        .map_err(|e| format!("✘ Failed to write SKILL.md: {e}"))?;

    // Scaffold requested components in canonical order (so `channel` runs after
    // `mcp` and its `.mcp.json` wins when both are requested).
    for component in WITH_COMPONENTS {
        if with.iter().any(|c| c == component) {
            scaffold_component(component, &plugin_root, name)?;
        }
    }

    // §22: only one of the collision warning / plain auto-load line prints —
    // the trailing Disable/Remove line always follows, regardless of which.
    let status_line = name_collision_warning(name, home, cwd).unwrap_or_else(|| {
        format!(
            "  It will auto-load next session as {name}@skills-dir. Run /reload-plugins to load \
             it now."
        )
    });

    Ok(format!(
        "✔ Created plugin \"{name}\" at {}\n\
         {status_line}\n  \
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
        /// A separate project dir: no scope settings files here by default,
        /// so the §22 collision check finds nothing and existing tests are
        /// unaffected.
        cwd: PathBuf,
    }

    fn env() -> Env {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join(".lingxi");
        let cwd = tmp.path().join("proj");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::create_dir_all(&cwd).unwrap();
        Env {
            _tmp: tmp,
            home,
            cwd,
        }
    }

    #[test]
    fn init_scaffolds_manifest_and_skill() {
        let e = env();
        let msg = run_init(
            "myplug",
            Some("Bob"),
            Some("a@b.c"),
            None,
            false,
            &[],
            &e.home,
            &e.cwd,
        )
        .unwrap();
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
        assert_eq!(
            manifest["$schema"],
            "https://anthropic.com/claude-code/plugin.schema.json"
        );
        assert_eq!(manifest["name"], "myplug");
        assert_eq!(manifest["version"], "0.1.0");
        assert_eq!(
            manifest["description"],
            "TODO: describe what this plugin provides"
        );
        assert_eq!(
            manifest["author"],
            serde_json::json!({"name": "Bob", "email": "a@b.c"})
        );
        assert_eq!(manifest["skills"], serde_json::json!(["./"]));

        let skill = std::fs::read_to_string(root.join("SKILL.md")).unwrap();
        assert!(skill.starts_with("---\nname: myplug\ndescription: TODO — describe WHEN Claude"));
        assert!(skill.trim_end().ends_with("the steps Claude should take."));
    }

    #[test]
    fn init_custom_description() {
        let e = env();
        run_init(
            "p",
            Some("A"),
            Some("a@b"),
            Some("My desc"),
            false,
            &[],
            &e.home,
            &e.cwd,
        )
        .unwrap();
        let manifest: Value = serde_json::from_str(
            &std::fs::read_to_string(e.home.join("skills/p/.lingxi-plugin/plugin.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(manifest["description"], "My desc");
    }

    #[test]
    fn init_duplicate_without_force_errors() {
        let e = env();
        run_init("dup", Some("A"), Some("a@b"), None, false, &[], &e.home, &e.cwd).unwrap();
        let err = run_init("dup", Some("A"), Some("a@b"), None, false, &[], &e.home, &e.cwd).unwrap_err();
        assert!(
            err.ends_with(".lingxi-plugin already exists. Use --force to overwrite."),
            "got: {err}"
        );
        assert!(err.starts_with("✘ "));
    }

    #[test]
    fn init_force_overwrites() {
        let e = env();
        run_init("f", Some("A"), Some("a@b"), None, false, &[], &e.home, &e.cwd).unwrap();
        // Second call with force succeeds.
        let msg = run_init("f", Some("A"), Some("a@b"), None, true, &[], &e.home, &e.cwd).unwrap();
        assert!(msg.starts_with("✔ Created plugin \"f\""));
    }

    // --- `--with <components…>` scaffolds ---------------------------------

    fn with(vals: &[&str]) -> Vec<String> {
        vals.iter().map(|s| s.to_string()).collect()
    }

    fn root(e: &Env, name: &str) -> PathBuf {
        e.home.join("skills").join(name)
    }

    #[test]
    fn init_with_unknown_component_errors_and_writes_nothing() {
        let e = env();
        let err = run_init("u", None, None, None, false, &with(&["bogus"]), &e.home, &e.cwd).unwrap_err();
        assert_eq!(
            err,
            "✘ Unknown --with component \"bogus\". Valid: skills, agents, hooks, mcp, lsp, output-style, channel"
        );
        assert!(!root(&e, "u").exists(), "no files should be scaffolded");
    }

    #[test]
    fn init_with_skills_writes_example_skill() {
        let e = env();
        run_init(
            "s",
            Some("A"),
            Some("a@b"),
            None,
            false,
            &with(&["skills"]),
            &e.home,
            &e.cwd,
        )
        .unwrap();
        let body = std::fs::read_to_string(root(&e, "s").join("skills/example/SKILL.md")).unwrap();
        assert_eq!(body, skill_md("example"));
        assert!(body.starts_with("---\nname: example\n"));
    }

    #[test]
    fn init_with_agents_writes_example_agent() {
        let e = env();
        run_init(
            "a",
            Some("A"),
            Some("a@b"),
            None,
            false,
            &with(&["agents"]),
            &e.home,
            &e.cwd,
        )
        .unwrap();
        let body = std::fs::read_to_string(root(&e, "a").join("agents/example.md")).unwrap();
        assert_eq!(body, AGENT_EXAMPLE_MD);
        assert!(body.contains("description: TODO — when should Claude delegate to this subagent?"));
        assert!(body.ends_with("TODO: system prompt for the subagent.\n"));
    }

    #[test]
    fn init_with_hooks_writes_hook_config_and_handler() {
        let e = env();
        run_init(
            "h",
            Some("A"),
            Some("a@b"),
            None,
            false,
            &with(&["hooks"]),
            &e.home,
            &e.cwd,
        )
        .unwrap();
        let cfg = std::fs::read_to_string(root(&e, "h").join("hooks/hooks.json")).unwrap();
        assert_eq!(cfg, HOOKS_JSON);
        // Branding: env token is LINGXI_PLUGIN_ROOT (never CLAUDE_PLUGIN_ROOT).
        assert!(cfg.contains("bun ${LINGXI_PLUGIN_ROOT}/hooks-handlers/on-session-start.ts"));
        assert!(!cfg.contains("CLAUDE_PLUGIN_ROOT"));
        // Config is valid JSON with the SessionStart hook.
        let v: Value = serde_json::from_str(&cfg).unwrap();
        assert!(v["hooks"]["SessionStart"].is_array());
        let ts = std::fs::read_to_string(root(&e, "h").join("hooks-handlers/on-session-start.ts"))
            .unwrap();
        assert_eq!(ts, ON_SESSION_START_TS);
        assert!(ts.starts_with("#!/usr/bin/env bun\n"));
    }

    #[test]
    fn init_with_mcp_writes_example_servers() {
        let e = env();
        run_init(
            "m",
            Some("A"),
            Some("a@b"),
            None,
            false,
            &with(&["mcp"]),
            &e.home,
            &e.cwd,
        )
        .unwrap();
        let body = std::fs::read_to_string(root(&e, "m").join(".mcp.json")).unwrap();
        assert_eq!(body, MCP_JSON);
        let v: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["mcpServers"]["example-remote"]["type"], "http");
        assert_eq!(v["mcpServers"]["example-local"]["command"], "npx");
    }

    #[test]
    fn init_with_lsp_writes_example_server() {
        let e = env();
        run_init(
            "l",
            Some("A"),
            Some("a@b"),
            None,
            false,
            &with(&["lsp"]),
            &e.home,
            &e.cwd,
        )
        .unwrap();
        let body = std::fs::read_to_string(root(&e, "l").join(".lsp.json")).unwrap();
        assert_eq!(body, LSP_JSON);
        let v: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["example"]["command"], "example-language-server");
    }

    #[test]
    fn init_with_output_style_writes_named_style() {
        let e = env();
        run_init(
            "os",
            Some("A"),
            Some("a@b"),
            None,
            false,
            &with(&["output-style"]),
            &e.home,
            &e.cwd,
        )
        .unwrap();
        let body = std::fs::read_to_string(root(&e, "os").join("output-styles/os.md")).unwrap();
        assert_eq!(body, output_style_md("os"));
        assert!(body.starts_with("---\nname: os\n"));
        assert!(body.contains("force-for-plugin: true"));
    }

    #[test]
    fn init_with_channel_writes_server_and_manifest_channels() {
        let e = env();
        run_init(
            "ch",
            Some("A"),
            Some("a@b"),
            None,
            false,
            &with(&["channel"]),
            &e.home,
            &e.cwd,
        )
        .unwrap();
        let r = root(&e, "ch");

        // plugin.json gains a `channels` entry (server + displayName = name).
        let manifest: Value = serde_json::from_str(
            &std::fs::read_to_string(r.join(".lingxi-plugin/plugin.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(
            manifest["channels"],
            serde_json::json!([{"server": "ch", "displayName": "ch"}])
        );

        // package.json: pkg name branded to lingxi-channel-.
        let pkg = std::fs::read_to_string(r.join("package.json")).unwrap();
        assert_eq!(pkg, channel_package_json("ch"));
        assert!(pkg.contains("\"name\": \"lingxi-channel-ch\""));
        assert!(!pkg.contains("claude-channel-"));

        // server.ts: keeps claude/channel protocol key + docs.claude.com URL,
        // interpolates the plugin name, and carries no CLAUDE_PLUGIN_ROOT token.
        let server = std::fs::read_to_string(r.join("server.ts")).unwrap();
        assert_eq!(server, channel_server_ts("ch"));
        assert!(server.contains("'claude/channel': {}"));
        assert!(server.contains("https://docs.claude.com/en/docs/claude-code/channels-reference"));
        assert!(server.contains("{ name: 'ch', version: '0.1.0' }"));
        assert!(!server.contains("__NAME__"));

        // channel's .mcp.json points bun at the bundled server via LINGXI_PLUGIN_ROOT.
        let mcp = std::fs::read_to_string(r.join(".mcp.json")).unwrap();
        assert_eq!(mcp, channel_mcp_json("ch"));
        assert!(mcp.contains("${LINGXI_PLUGIN_ROOT}"));
        assert!(mcp.contains("\"ch\": {"));
    }

    #[test]
    fn init_with_mcp_and_channel_channel_mcp_json_wins() {
        let e = env();
        // Even when `mcp` is listed after `channel`, the canonical scaffold
        // order runs `channel` last, so its `.mcp.json` is the one on disk.
        run_init(
            "both",
            Some("A"),
            Some("a@b"),
            None,
            false,
            &with(&["channel", "mcp"]),
            &e.home,
            &e.cwd,
        )
        .unwrap();
        let mcp = std::fs::read_to_string(root(&e, "both").join(".mcp.json")).unwrap();
        assert_eq!(mcp, channel_mcp_json("both"));
        assert!(!mcp.contains("example-remote"));
    }

    #[test]
    fn init_default_manifest_has_no_channels() {
        let e = env();
        run_init("plain", Some("A"), Some("a@b"), None, false, &[], &e.home, &e.cwd).unwrap();
        let manifest =
            std::fs::read_to_string(root(&e, "plain").join(".lingxi-plugin/plugin.json")).unwrap();
        assert!(!manifest.contains("channels"));
    }

    #[test]
    fn init_rejects_path_traversal_name() {
        let e = env();
        for bad in ["../pwned", "a/b", "..", "."] {
            let err = run_init(bad, Some("A"), Some("a@b"), None, false, &[], &e.home, &e.cwd).unwrap_err();
            assert!(
                err.starts_with(&format!("✘ Invalid plugin name \"{bad}\":")),
                "got: {err}"
            );
        }
        // Nothing escaped the skills dir.
        assert!(!e.home.join("pwned").exists());
        assert!(!e.home.join("skills").join("a").exists());
    }

    /// §8 (the 2.1.247 hardening): a bidi-formatting character in a
    /// scaffolded plugin name was accepted nowhere in this port before now.
    #[test]
    fn init_rejects_bidi_formatting_name() {
        let e = env();
        let bad = "evil\u{202E}reversed";
        let err = run_init(bad, Some("A"), Some("a@b"), None, false, &[], &e.home, &e.cwd).unwrap_err();
        assert_eq!(
            err,
            format!(
                "✘ Invalid plugin name \"{bad}\": Plugin name cannot contain control or \
                 bidirectional-formatting characters"
            )
        );
        assert!(!e.home.join("skills").join(bad).exists());
    }

    #[test]
    fn init_plugin_json_has_trailing_newline() {
        let e = env();
        run_init("p", Some("A"), Some("a@b"), None, false, &[], &e.home, &e.cwd).unwrap();
        let raw =
            std::fs::read_to_string(root(&e, "p").join(".lingxi-plugin/plugin.json")).unwrap();
        assert!(raw.ends_with("}\n"), "expected trailing newline");
    }

    // --- §22: the post-create name-collision warnings ----------------------

    /// An already-enabled, cache-known, non-skills-dir marketplace plugin
    /// with the same name wins — the freshly scaffolded copy is warned it
    /// will never load, with the `.lingxi-plugin/plugin.json` remediation.
    #[test]
    fn init_warns_when_the_name_is_already_taken_by_an_enabled_marketplace_plugin() {
        let e = env();
        std::fs::create_dir_all(e.home.join("plugins")).unwrap();
        std::fs::write(
            e.home.join("plugins").join("known_marketplaces.json"),
            serde_json::json!({
                "othermkt": {"source": {"source": "directory", "path": "/tmp/othermkt"}}
            })
            .to_string(),
        )
        .unwrap();
        std::fs::write(
            e.home.join("settings.json"),
            serde_json::json!({"enabledPlugins": {"conflict@othermkt": true}}).to_string(),
        )
        .unwrap();

        let msg = run_init(
            "conflict",
            Some("A"),
            Some("a@b"),
            None,
            false,
            &[],
            &e.home,
            &e.cwd,
        )
        .unwrap();
        assert!(
            msg.contains(
                "The name \"conflict\" is already taken by conflict@othermkt \u{2014} when that \
                 plugin loads, conflict@skills-dir won't. To load this copy, give it a different \
                 \"name\" in .lingxi-plugin/plugin.json or uninstall the conflicting plugin."
            ),
            "got: {msg}"
        );
        assert!(!msg.contains("It will auto-load next session"));
        // The trailing Disable/Remove line still prints regardless.
        assert!(msg.contains("Disable: lingxi-cli plugin disable conflict@skills-dir."));
    }

    /// A marketplace entry unknown to `known_marketplaces.json` (never
    /// fetched/cached) does not block the new copy — only a marketplace the
    /// cache actually knows about takes precedence.
    #[test]
    fn init_ignores_an_enabled_entry_whose_marketplace_is_not_cached() {
        let e = env();
        std::fs::write(
            e.home.join("settings.json"),
            serde_json::json!({"enabledPlugins": {"conflict@unknownmkt": true}}).to_string(),
        )
        .unwrap();

        let msg = run_init(
            "conflict",
            Some("A"),
            Some("a@b"),
            None,
            false,
            &[],
            &e.home,
            &e.cwd,
        )
        .unwrap();
        assert!(msg.contains("It will auto-load next session as conflict@skills-dir."));
    }

    /// This exact `name@skills-dir` id was already disabled in a settings
    /// scope (e.g. a stale entry from a previous scaffold) — warn instead of
    /// claiming it will auto-load.
    #[test]
    fn init_warns_when_this_exact_id_was_already_disabled() {
        let e = env();
        std::fs::write(
            e.home.join("settings.json"),
            serde_json::json!({"enabledPlugins": {"stale@skills-dir": false}}).to_string(),
        )
        .unwrap();

        let msg = run_init(
            "stale",
            Some("A"),
            Some("a@b"),
            None,
            false,
            &[],
            &e.home,
            &e.cwd,
        )
        .unwrap();
        assert!(
            msg.contains(
                "A disabled setting for stale@skills-dir exists, so it won't load until you \
                 re-enable it in /plugin"
            ),
            "got: {msg}"
        );
        assert!(!msg.contains("It will auto-load next session"));
    }

    /// A LOCAL-scope value overrides an earlier USER-scope value for the same
    /// key (user < project < local precedence).
    #[test]
    fn init_disabled_setting_check_honours_scope_precedence() {
        let e = env();
        // User scope says enabled; local scope (more specific) says disabled.
        std::fs::write(
            e.home.join("settings.json"),
            serde_json::json!({"enabledPlugins": {"p@skills-dir": true}}).to_string(),
        )
        .unwrap();
        let dot_dir = e.cwd.join(branding::DOT_DIR);
        std::fs::create_dir_all(&dot_dir).unwrap();
        std::fs::write(
            dot_dir.join("settings.local.json"),
            serde_json::json!({"enabledPlugins": {"p@skills-dir": false}}).to_string(),
        )
        .unwrap();

        let msg = run_init("p", Some("A"), Some("a@b"), None, false, &[], &e.home, &e.cwd).unwrap();
        assert!(msg.contains("A disabled setting for p@skills-dir exists"), "got: {msg}");
    }
}
