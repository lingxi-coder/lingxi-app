//! Agent catalog loader (M6-07).
//!
//! Reads markdown files from one or more `agents/` directories, parses the
//! YAML frontmatter, and projects each file into an
//! [`crate::definition::AgentDefinition`].
//!
//! Frontmatter shape (claude-code compatible — subset relevant to v0.7.0):
//! ```yaml
//! ---
//! name: reviewer
//! description: Reviews code for security and correctness.
//! tools: [Read, Grep, Bash]
//! model: sonnet
//! ---
//! Body of the system prompt goes here.
//! ```
//!
//! Files without a `---`-delimited YAML frontmatter block, or with
//! malformed YAML, are skipped at the loader level (logged via
//! `tracing::warn!`). The exposed [`parse_agent_markdown`] returns a
//! typed error so unit tests can assert on the failure mode.

use crate::definition::{
    parse_effort_value, AgentDefinition, AgentEffort, AgentIsolation, AgentMcpServerSpec,
    AgentMemoryScope, AgentModel, AgentPermissionMode, AgentSource, AgentToolPolicy, EFFORT_LEVELS,
};
use serde::Deserialize;
use std::path::{Path, PathBuf};

/// Validated agent color names — claude `AGENT_COLORS` (agentColorManager.ts).
const AGENT_COLOR_NAMES: [&str; 8] = [
    "red", "blue", "green", "yellow", "purple", "orange", "pink", "cyan",
];

/// User-addressable permission-mode strings — claude `PERMISSION_MODES`
/// (`EXTERNAL_PERMISSION_MODES`; `'auto'` is feature-gated in claude and not
/// surfaced here). Used to VALIDATE the raw frontmatter value before mapping it
/// onto the lossy [`AgentPermissionMode`] enum.
const PERMISSION_MODES: [&str; 5] = [
    "acceptEdits",
    "bypassPermissions",
    "default",
    "dontAsk",
    "plan",
];

/// Valid memory scopes — claude `VALID_MEMORY_SCOPES = ['user','project','local']`.
const VALID_MEMORY_SCOPES: [&str; 3] = ["user", "project", "local"];

/// Errors raised while loading an agent file.
#[derive(Debug, thiserror::Error)]
pub enum AgentLoadError {
    /// File could not be read.
    #[error("read {path}: {source}")]
    Io {
        /// Offending path.
        path: PathBuf,
        /// Underlying io error.
        #[source]
        source: std::io::Error,
    },
    /// Frontmatter was missing or its `---` terminator was not found.
    #[error("no valid frontmatter in {0}")]
    NoFrontmatter(PathBuf),
    /// Frontmatter YAML failed to deserialize.
    #[error("deserialize frontmatter {path}: {source}")]
    Yaml {
        /// Offending path.
        path: PathBuf,
        /// Underlying yaml error.
        #[source]
        source: serde_yaml::Error,
    },
    /// Required `name` field was missing/non-string. Mirrors claude
    /// `parseAgentFromMarkdown` returning `null` (a SILENT skip — the file is
    /// likely co-located reference documentation, not an agent attempt).
    #[error("missing required \"name\" field in {0}")]
    MissingName(PathBuf),
    /// Required `description` field was missing/non-string. claude logs and
    /// returns `null`.
    #[error("missing required \"description\" field in {0}")]
    MissingDescription(PathBuf),
}

/// An agent's YAML frontmatter. Each claude `BaseAgentDefinition` key is read
/// as an untyped `serde_yaml::Value` (matching claude reading
/// `frontmatter['...']` directly off the parsed object) so the bespoke claude
/// coercions can be applied per-field rather than relying on serde.
#[derive(Debug, Default, Deserialize)]
struct Frontmatter {
    #[serde(default)]
    name: Option<serde_yaml::Value>,
    #[serde(default)]
    description: Option<serde_yaml::Value>,
    #[serde(default)]
    tools: Option<serde_yaml::Value>,
    #[serde(default)]
    model: Option<serde_yaml::Value>,
    #[serde(default, rename = "disallowedTools")]
    disallowed_tools: Option<serde_yaml::Value>,
    #[serde(default)]
    skills: Option<serde_yaml::Value>,
    #[serde(default, rename = "mcpServers")]
    mcp_servers: Option<serde_yaml::Value>,
    #[serde(default)]
    hooks: Option<serde_yaml::Value>,
    #[serde(default, rename = "maxTurns")]
    max_turns: Option<serde_yaml::Value>,
    #[serde(default)]
    background: Option<serde_yaml::Value>,
    #[serde(default)]
    memory: Option<serde_yaml::Value>,
    #[serde(default)]
    isolation: Option<serde_yaml::Value>,
    #[serde(default, rename = "permissionMode")]
    permission_mode: Option<serde_yaml::Value>,
    #[serde(default)]
    effort: Option<serde_yaml::Value>,
    #[serde(default)]
    color: Option<serde_yaml::Value>,
    #[serde(default, rename = "initialPrompt")]
    initial_prompt: Option<serde_yaml::Value>,
}

/// Parse a single agent markdown buffer.
///
/// `source` and `base_dir` are supplied by the caller (so the loader can
/// tag files coming from `~/.lingxi/agents/` differently from project
/// files). `path_for_error` is used purely for error messages.
///
/// Frontmatter format: leading `---\n…\n---\n` (followed by the markdown
/// body). The body is stored as `system_prompt`.
pub fn parse_agent_markdown(
    raw: &str,
    source: AgentSource,
    base_dir: PathBuf,
    path_for_error: &Path,
) -> Result<AgentDefinition, AgentLoadError> {
    let rest = raw
        .strip_prefix("---")
        .ok_or_else(|| AgentLoadError::NoFrontmatter(path_for_error.to_path_buf()))?;
    // Accept either `\n---\n` or `\n---` at EOF.
    let (yaml, body) = if let Some(idx) = rest.find("\n---\n") {
        (&rest[..idx], rest[idx + 5..].trim_start().to_string())
    } else if let Some(idx) = rest.find("\n---") {
        // EOF terminator (no trailing newline after the closing ---).
        let after = idx + 4;
        let body = if after >= rest.len() {
            String::new()
        } else {
            rest[after..].trim_start().to_string()
        };
        (&rest[..idx], body)
    } else {
        return Err(AgentLoadError::NoFrontmatter(path_for_error.to_path_buf()));
    };

    let fm: Frontmatter = serde_yaml::from_str(yaml).map_err(|e| AgentLoadError::Yaml {
        path: path_for_error.to_path_buf(),
        source: e,
    })?;

    // (1) `name` required: missing/non-string/EMPTY -> silent skip. claude's
    // `!agentType` truthiness treats the empty string `""` as falsy, so an
    // empty `name` is dropped exactly like an absent one (likely co-located
    // reference documentation, not an agent attempt).
    let Some(agent_type) = fm
        .name
        .as_ref()
        .and_then(yaml_as_string)
        .filter(|s| !s.is_empty())
    else {
        return Err(AgentLoadError::MissingName(path_for_error.to_path_buf()));
    };

    // (2) `description` required: missing/non-string/EMPTY -> log + skip. claude
    // `!whenToUse` is falsy for `""`, so an empty description drops the agent.
    let Some(mut when_to_use) = fm
        .description
        .as_ref()
        .and_then(yaml_as_string)
        .filter(|s| !s.is_empty())
    else {
        tracing::debug!(
            path = %path_for_error.display(),
            "Agent file is missing required 'description' in frontmatter"
        );
        return Err(AgentLoadError::MissingDescription(
            path_for_error.to_path_buf(),
        ));
    };

    // (3) Unescape `\n` -> newline in description (claude
    // `whenToUse.replace(/\\n/g, '\n')`).
    when_to_use = when_to_use.replace("\\n", "\n");

    // (15) color: validate against AGENT_COLORS; only set if string && in set.
    let color = fm
        .color
        .as_ref()
        .and_then(yaml_as_string)
        .filter(|c| AGENT_COLOR_NAMES.contains(&c.as_str()));

    // (4) model: trim; case-insensitive 'inherit' -> Inherit; else Alias.
    let model = match fm.model.as_ref().and_then(yaml_as_string) {
        Some(s) if !s.trim().is_empty() => {
            let trimmed = s.trim();
            if trimmed.eq_ignore_ascii_case("inherit") {
                AgentModel::Inherit
            } else {
                AgentModel::Alias(trimmed.to_string())
            }
        }
        _ => AgentModel::Inherit,
    };

    // (5) background: true only for literal `true`/`"true"`; log on other
    // non-bool values; default false.
    let background = parse_background(fm.background.as_ref(), path_for_error);

    // (6) memory: validate against VALID_MEMORY_SCOPES; invalid -> log + None.
    let memory = parse_memory(fm.memory.as_ref(), path_for_error);

    // (7) isolation: valid set is ['worktree'] (3P) / ['worktree','remote']
    // (ant); invalid -> log + None.
    let isolation = parse_isolation(fm.isolation.as_ref(), path_for_error);

    // (8) effort: parse_effort_value; raw present but None -> log invalid.
    let effort = match fm.effort.as_ref() {
        Some(raw) => {
            let parsed = parse_effort_value(raw);
            if parsed.is_none() {
                tracing::debug!(
                    path = %path_for_error.display(),
                    "Agent file has invalid effort. Valid options: {} or an integer",
                    EFFORT_LEVELS.join(", ")
                );
            }
            parsed
        }
        None => None,
    };

    // (9) permissionMode: validate against claude PERMISSION_MODES; invalid ->
    // log. Map the valid raw string onto the (lossy) AgentPermissionMode enum:
    // only 'plan' has a behavioral analog (Plan); the others collapse to Bubble.
    let permission_mode = parse_permission_mode(fm.permission_mode.as_ref(), path_for_error);

    // (10) maxTurns: positive int; invalid -> log; absent -> keep default 100.
    let max_turns = match fm.max_turns.as_ref() {
        Some(raw) => match parse_positive_int_from_frontmatter(raw) {
            Some(n) => n,
            None => {
                tracing::debug!(
                    path = %path_for_error.display(),
                    "Agent file has invalid maxTurns. Must be a positive integer."
                );
                100
            }
        },
        None => 100,
    };

    // (11) tools: All-vs-Explicit per claude's coercion. Auto-memory tool
    // injection (Write/Edit/Read when `memory:` is set) is applied at spawn by
    // `AgentToolResolver::resolve`, not here — this is the parse layer.
    let tools_vec = parse_agent_tools_from_frontmatter(fm.tools.as_ref());
    let (tools_policy, allowed_tools) = match &tools_vec {
        // undefined = all tools
        None => (
            AgentToolPolicy::All {
                use_exact_tools: false,
            },
            Vec::new(),
        ),
        Some(list) => (AgentToolPolicy::Explicit(list.clone()), list.clone()),
    };

    // (12) disallowedTools: tool-list coercion -> Vec (None -> empty).
    let disallowed_tools = if fm.disallowed_tools.is_some() {
        parse_agent_tools_from_frontmatter(fm.disallowed_tools.as_ref()).unwrap_or_default()
    } else {
        Vec::new()
    };

    // (13) skills: comma/list coercion (missing -> []).
    let skills = parse_slash_command_tools_from_frontmatter(fm.skills.as_ref());

    // (14) initialPrompt: keep the UNtrimmed raw value when its trim is non-empty.
    let initial_prompt = fm
        .initial_prompt
        .as_ref()
        .and_then(yaml_as_string)
        .filter(|s| !s.trim().is_empty());

    // (16) mcpServers: parse each array item as ByName/Record; invalid -> log + skip.
    let mcp_servers = parse_mcp_servers(fm.mcp_servers.as_ref(), path_for_error);

    // (17) hooks: parse via the hooks crate (HookSource::FrontMatter).
    let frontmatter_hooks = parse_hooks_from_frontmatter(fm.hooks.as_ref(), &agent_type);

    let system_prompt = body.trim().to_string();

    Ok(AgentDefinition {
        agent_type,
        when_to_use,
        tools: tools_policy,
        max_turns,
        model,
        permission_mode,
        source,
        base_dir,
        system_prompt: if system_prompt.is_empty() {
            None
        } else {
            Some(system_prompt)
        },
        mcp_servers,
        frontmatter_hooks,
        icon: None,
        allowed_tools,
        worktree_requirement: None,
        disallowed_tools,
        skills,
        // claude does NOT parse requiredMcpServers from frontmatter.
        required_mcp_servers: Vec::new(),
        background,
        isolation,
        memory,
        effort,
        initial_prompt,
        color,
    })
}

// ─── Frontmatter coercion helpers (claude-faithful) ──────────────────────────

/// Read a YAML value as a string if (and only if) it is a YAML string.
/// Mirrors claude's `typeof x === 'string'` guards.
fn yaml_as_string(v: &serde_yaml::Value) -> Option<String> {
    v.as_str().map(str::to_string)
}

/// claude `parseToolListFromCLI`: split a tool string on top-level commas AND
/// whitespace (separators inside `(...)` are preserved), trimming each entry
/// and dropping empties. So `"Read Grep"` -> `["Read", "Grep"]` while
/// `"Bash(git:*)"` stays one token.
fn parse_tool_list_from_cli(tools: &[String]) -> Vec<String> {
    let mut result = Vec::new();
    for tool_string in tools {
        if tool_string.is_empty() {
            continue;
        }
        let mut current = String::new();
        let mut in_parens = false;
        for ch in tool_string.chars() {
            match ch {
                '(' => {
                    in_parens = true;
                    current.push(ch);
                }
                ')' => {
                    in_parens = false;
                    current.push(ch);
                }
                ',' => {
                    if in_parens {
                        current.push(ch);
                    } else if !current.trim().is_empty() {
                        result.push(current.trim().to_string());
                        current.clear();
                    } else {
                        current.clear();
                    }
                }
                ' ' => {
                    if in_parens {
                        current.push(ch);
                    } else if !current.trim().is_empty() {
                        // Space separator (outside parens): flush the current
                        // token and start a new one (claude pushes
                        // `current.trim()` and resets).
                        result.push(current.trim().to_string());
                        current.clear();
                    }
                    // else: leading/separator space outside parens is dropped.
                }
                _ => current.push(ch),
            }
        }
        if !current.trim().is_empty() {
            result.push(current.trim().to_string());
        }
    }
    result
}

/// claude `parseToolListString`: `None` (= caller default) for missing/null;
/// `Some([])` for present-but-falsy; `Some(['*'])` when the parsed list
/// contains `'*'`; otherwise `Some(parsed)`.
fn parse_tool_list_string(value: Option<&serde_yaml::Value>) -> Option<Vec<String>> {
    match value {
        // undefined / null -> null (caller decides default)
        None | Some(serde_yaml::Value::Null) => None,
        Some(v) => {
            // Falsy values (empty string, false, 0) -> [] (no tools).
            let is_falsy = match v {
                serde_yaml::Value::String(s) => s.is_empty(),
                serde_yaml::Value::Bool(b) => !b,
                serde_yaml::Value::Number(n) => n.as_f64() == Some(0.0),
                _ => false,
            };
            if is_falsy {
                return Some(Vec::new());
            }
            let tools_array: Vec<String> = match v {
                serde_yaml::Value::String(s) => vec![s.clone()],
                serde_yaml::Value::Sequence(seq) => seq
                    .iter()
                    .filter_map(|item| item.as_str().map(str::to_string))
                    .collect(),
                _ => Vec::new(),
            };
            if tools_array.is_empty() {
                return Some(Vec::new());
            }
            let parsed = parse_tool_list_from_cli(&tools_array);
            if parsed.iter().any(|t| t == "*") {
                return Some(vec!["*".to_string()]);
            }
            Some(parsed)
        }
    }
}

/// claude `parseAgentToolsFromFrontmatter`: `None` = all tools (the field was
/// missing); `Some([])` = no tools; `'*'` -> all tools (`None`).
fn parse_agent_tools_from_frontmatter(value: Option<&serde_yaml::Value>) -> Option<Vec<String>> {
    match parse_tool_list_string(value) {
        None => {
            // For agents: undefined = all tools (None); null = no tools ([]).
            // (`value === undefined ? undefined : []`)
            match value {
                None => None,
                Some(_) => Some(Vec::new()),
            }
        }
        Some(parsed) => {
            if parsed.iter().any(|t| t == "*") {
                None
            } else {
                Some(parsed)
            }
        }
    }
}

/// claude `parseSlashCommandToolsFromFrontmatter`: missing/empty -> `[]`.
fn parse_slash_command_tools_from_frontmatter(value: Option<&serde_yaml::Value>) -> Vec<String> {
    parse_tool_list_string(value).unwrap_or_default()
}

/// claude `parsePositiveIntFromFrontmatter`: number or numeric string; must be
/// a positive integer.
fn parse_positive_int_from_frontmatter(value: &serde_yaml::Value) -> Option<u32> {
    match value {
        serde_yaml::Value::Null => None,
        serde_yaml::Value::Number(n) => {
            // Number.isInteger(parsed) && parsed > 0
            n.as_i64()
                .and_then(|i| if i > 0 { u32::try_from(i).ok() } else { None })
        }
        serde_yaml::Value::String(s) => {
            // parseInt(String(value), 10); Number.isInteger && > 0
            parse_int_radix10(s).and_then(|i| if i > 0 { u32::try_from(i).ok() } else { None })
        }
        // booleans: String(true)='true' -> parseInt NaN -> None
        _ => None,
    }
}

/// JS-`parseInt(str, 10)`: leading sign + leading digits, ignore trailing
/// garbage; `None` (NaN) when no digits.
fn parse_int_radix10(s: &str) -> Option<i64> {
    let t = s.trim_start();
    let mut chars = t.chars().peekable();
    let mut out = String::new();
    if let Some(&c) = chars.peek() {
        if c == '+' || c == '-' {
            out.push(c);
            chars.next();
        }
    }
    let mut saw_digit = false;
    while let Some(&c) = chars.peek() {
        if c.is_ascii_digit() {
            out.push(c);
            saw_digit = true;
            chars.next();
        } else {
            break;
        }
    }
    if !saw_digit {
        return None;
    }
    out.parse::<i64>().ok()
}

/// claude background coercion: `true` only for literal `true`/`"true"`; log on
/// any other non-bool, non-`"false"` value; default `false`.
fn parse_background(value: Option<&serde_yaml::Value>, path: &Path) -> bool {
    let Some(v) = value else {
        return false;
    };
    let is_true_literal = matches!(v, serde_yaml::Value::Bool(true))
        || matches!(v, serde_yaml::Value::String(s) if s == "true");
    let is_false_literal = matches!(v, serde_yaml::Value::Bool(false))
        || matches!(v, serde_yaml::Value::String(s) if s == "false");
    if !is_true_literal && !is_false_literal {
        tracing::debug!(
            path = %path.display(),
            "Agent file has invalid background value. Must be 'true', 'false', or omitted."
        );
    }
    is_true_literal
}

/// claude memory coercion: validate against VALID_MEMORY_SCOPES; invalid -> log.
fn parse_memory(value: Option<&serde_yaml::Value>, path: &Path) -> Option<AgentMemoryScope> {
    let Some(raw) = value.and_then(yaml_as_string) else {
        // A non-string (or absent) value: claude reads `as string | undefined`,
        // so a non-string is treated as defined-but-invalid only when it is a
        // string. Absent / non-string -> None without logging here (claude only
        // logs when the *string* value is not in the valid set).
        return None;
    };
    match raw.as_str() {
        "user" => Some(AgentMemoryScope::User),
        "project" => Some(AgentMemoryScope::Project),
        "local" => Some(AgentMemoryScope::Local),
        _ => {
            tracing::debug!(
                path = %path.display(),
                "Agent file has invalid memory value. Valid options: {}",
                VALID_MEMORY_SCOPES.join(", ")
            );
            None
        }
    }
}

/// claude isolation coercion: valid set ['worktree'] (3P) / ['worktree','remote']
/// (ant); invalid -> log.
fn parse_isolation(value: Option<&serde_yaml::Value>, path: &Path) -> Option<AgentIsolation> {
    let Some(raw) = value.and_then(yaml_as_string) else {
        return None;
    };
    let ant = std::env::var("USER_TYPE").as_deref() == Ok("ant");
    match raw.as_str() {
        "worktree" => Some(AgentIsolation::Worktree),
        "remote" if ant => Some(AgentIsolation::Remote),
        _ => {
            let valid = if ant { "worktree, remote" } else { "worktree" };
            tracing::debug!(
                path = %path.display(),
                "Agent file has invalid isolation value. Valid options: {valid}"
            );
            None
        }
    }
}

/// claude permissionMode coercion: validate against PERMISSION_MODES; invalid
/// -> log. Map the VALID raw string onto the lossy [`AgentPermissionMode`]
/// enum — only `'plan'` has a behavioral analog (`Plan`); the other four
/// collapse to `Bubble` (a documented divergence: LingXi's enum lacks
/// `acceptEdits`/`bypassPermissions`/`default`/`dontAsk` variants).
fn parse_permission_mode(value: Option<&serde_yaml::Value>, path: &Path) -> AgentPermissionMode {
    let Some(raw) = value.and_then(yaml_as_string) else {
        return AgentPermissionMode::Bubble;
    };
    if !PERMISSION_MODES.contains(&raw.as_str()) {
        tracing::debug!(
            path = %path.display(),
            "Agent file has invalid permissionMode '{raw}'. Valid options: {}",
            PERMISSION_MODES.join(", ")
        );
        return AgentPermissionMode::Bubble;
    }
    match raw.as_str() {
        "plan" => AgentPermissionMode::Plan,
        // 'default' | 'acceptEdits' | 'bypassPermissions' | 'dontAsk'
        _ => AgentPermissionMode::Bubble,
    }
}

/// claude mcpServers coercion: each array item is either a string (ByName) or
/// an inline `{ name: config }` map (Record); invalid items are logged +
/// skipped. Non-array input contributes nothing.
fn parse_mcp_servers(value: Option<&serde_yaml::Value>, path: &Path) -> Vec<AgentMcpServerSpec> {
    let Some(serde_yaml::Value::Sequence(seq)) = value else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for item in seq {
        match parse_mcp_server_spec(item) {
            Some(spec) => out.push(spec),
            None => {
                tracing::debug!(
                    path = %path.display(),
                    "Agent file has invalid mcpServers item; skipping"
                );
            }
        }
    }
    out
}

/// Parse one `AgentMcpServerSpec` item: a string -> `ByName`; a mapping ->
/// `Record` (kept RAW — the body is validated and converted by
/// [`crate::mcp_servers::agent_mcp_specs_to_scoped_configs`], which also
/// rejects records with more than one key, exactly like claude `obs`).
fn parse_mcp_server_spec(item: &serde_yaml::Value) -> Option<AgentMcpServerSpec> {
    match item {
        serde_yaml::Value::String(s) => Some(AgentMcpServerSpec::ByName(s.clone())),
        serde_yaml::Value::Mapping(_) => {
            // YAML mapping → JSON record (non-string keys / YAML tags fail the
            // conversion → item skipped, like a zod safeParse failure).
            let json: serde_json::Value = serde_yaml::from_value(item.clone()).ok()?;
            let serde_json::Value::Object(map) = json else {
                return None;
            };
            Some(AgentMcpServerSpec::Record(map))
        }
        _ => None,
    }
}

/// claude `parseHooksFromFrontmatter`: validate `frontmatter.hooks` and on
/// failure log + drop (empty). Reuses the hooks crate's settings-JSON loader by
/// wrapping the value as `{"hooks": <value>}`.
fn parse_hooks_from_frontmatter(
    value: Option<&serde_yaml::Value>,
    agent_type: &str,
) -> Vec<hooks::HookDefinition> {
    let Some(hooks_value) = value else {
        return Vec::new();
    };
    // Convert the untyped YAML value to JSON (can fail on non-string map keys
    // or YAML tags — guard + drop like claude's safeParse).
    let json_value: serde_json::Value = match serde_yaml::from_value(hooks_value.clone()) {
        Ok(jv) => jv,
        Err(e) => {
            tracing::debug!("Invalid hooks in agent '{agent_type}': {e}");
            return Vec::new();
        }
    };
    let wrapper = serde_json::json!({ "hooks": json_value });
    let raw = match serde_json::to_string(&wrapper) {
        Ok(s) => s,
        Err(e) => {
            tracing::debug!("Invalid hooks in agent '{agent_type}': {e}");
            return Vec::new();
        }
    };
    match hooks::parse_hooks_from_settings_json(&raw, hooks::HookSource::FrontMatter) {
        Ok(defs) => defs,
        Err(e) => {
            tracing::debug!("Invalid hooks in agent '{agent_type}': {e}");
            Vec::new()
        }
    }
}

/// Parse an agent definition from JSON data (claude `parseAgentFromJson`,
/// loadAgentsDir.ts:445-516). JSON agents use `prompt` for the system-prompt
/// claude's `z.number().int()` (`Number.isInteger`): a JSON number is a valid
/// integer iff it has no fractional part. JS has no int/float split, so `7.0`
/// is accepted exactly like `7`; `7.5` and non-finite values are rejected.
/// serde_json *does* distinguish `7` from `7.0`, so we accept both an `i64` and
/// an integer-valued finite `f64`.
fn json_int_value(n: &serde_json::Number) -> Option<i64> {
    if let Some(i) = n.as_i64() {
        return Some(i);
    }
    n.as_f64()
        .filter(|f| f.is_finite() && f.fract() == 0.0)
        .map(|f| f as i64)
}

/// body (NOT markdown content). Returns `None` and logs on any validation
/// failure (required `description`/`prompt` non-empty). `source` defaults to
/// [`AgentSource::Flag`] in claude (`flagSettings`).
#[must_use]
pub fn parse_agent_from_json(
    name: &str,
    definition: &serde_json::Value,
    source: AgentSource,
) -> Option<AgentDefinition> {
    let obj = match definition.as_object() {
        Some(o) => o,
        None => {
            tracing::debug!("Error parsing agent '{name}' from JSON: definition is not an object");
            return None;
        }
    };

    // claude validates JSON agents with a THROWING zod schema
    // (`AgentJsonSchema().parse(definition)` in parseAgentFromJson): ANY invalid
    // field throws and the WHOLE agent is dropped (the catch returns null). So
    // unlike the lenient markdown path, every optional field below validates
    // STRICTLY and `return None` (= zod throw) on a bad value.

    // Required: description (`z.string().min(1)`).
    let when_to_use = match obj.get("description").and_then(json_as_string) {
        Some(d) if !d.is_empty() => d,
        _ => {
            tracing::debug!("Error parsing agent '{name}' from JSON: Description cannot be empty");
            return None;
        }
    };

    // Required: prompt (`z.string().min(1)`) -> system_prompt.
    let system_prompt = match obj.get("prompt").and_then(json_as_string) {
        Some(p) if !p.is_empty() => p,
        _ => {
            tracing::debug!("Error parsing agent '{name}' from JSON: Prompt cannot be empty");
            return None;
        }
    };

    // tools / disallowedTools / skills: `z.array(z.string()).optional()` —
    // absent is fine; otherwise it MUST be an array of strings. A non-array (or
    // an array with a non-string element) throws -> drop the whole agent.
    let tools_field = match json_string_array_strict(obj.get("tools")) {
        Ok(v) => v,
        Err(()) => {
            tracing::debug!(
                "Error parsing agent '{name}' from JSON: tools must be an array of strings"
            );
            return None;
        }
    };
    // The validated (string-array) value still flows through the same tool-list
    // coercion as the markdown path.
    let tools_yaml = tools_field.map(json_string_array_to_yaml);
    let tools_vec = parse_agent_tools_from_frontmatter(tools_yaml.as_ref());
    let (tools_policy, allowed_tools) = match &tools_vec {
        None => (
            AgentToolPolicy::All {
                use_exact_tools: false,
            },
            Vec::new(),
        ),
        Some(list) => (AgentToolPolicy::Explicit(list.clone()), list.clone()),
    };

    let disallowed_tools = match json_string_array_strict(obj.get("disallowedTools")) {
        Ok(Some(arr)) => {
            let yv = json_string_array_to_yaml(arr);
            parse_agent_tools_from_frontmatter(Some(&yv)).unwrap_or_default()
        }
        Ok(None) => Vec::new(),
        Err(()) => {
            tracing::debug!(
                "Error parsing agent '{name}' from JSON: disallowedTools must be an array of strings"
            );
            return None;
        }
    };

    let skills = match json_string_array_strict(obj.get("skills")) {
        Ok(Some(arr)) => {
            parse_slash_command_tools_from_frontmatter(Some(&json_string_array_to_yaml(arr)))
        }
        Ok(None) => Vec::new(),
        Err(()) => {
            tracing::debug!(
                "Error parsing agent '{name}' from JSON: skills must be an array of strings"
            );
            return None;
        }
    };

    // model: `z.string().trim().min(1).optional()` — absent is fine; present
    // must be a string whose TRIMMED form is non-empty (whitespace-only throws).
    let model = match obj.get("model") {
        None | Some(serde_json::Value::Null) => AgentModel::Inherit,
        Some(serde_json::Value::String(s)) if !s.trim().is_empty() => {
            let trimmed = s.trim();
            if trimmed.eq_ignore_ascii_case("inherit") {
                AgentModel::Inherit
            } else {
                AgentModel::Alias(trimmed.to_string())
            }
        }
        _ => {
            tracing::debug!("Error parsing agent '{name}' from JSON: Model cannot be empty");
            return None;
        }
    };

    // effort: `z.union([z.enum(EFFORT_LEVELS), z.number().int()])` — a STRICT
    // union. Only an exact level string (case-sensitive) or an integer number
    // is accepted; numeric strings like "7" and fractional floats throw. An
    // integer-valued float like `7.0` IS accepted (`Number.isInteger`).
    let effort = match obj.get("effort") {
        None | Some(serde_json::Value::Null) => None,
        Some(serde_json::Value::String(s)) if EFFORT_LEVELS.contains(&s.as_str()) => {
            Some(AgentEffort::Level(s.clone()))
        }
        Some(serde_json::Value::Number(n)) if json_int_value(n).is_some() => {
            Some(AgentEffort::Numeric(json_int_value(n).unwrap()))
        }
        _ => {
            tracing::debug!("Error parsing agent '{name}' from JSON: invalid effort");
            return None;
        }
    };

    // permissionMode: `z.enum(PERMISSION_MODES).optional()`.
    let permission_mode = match obj.get("permissionMode") {
        None | Some(serde_json::Value::Null) => AgentPermissionMode::Bubble,
        Some(serde_json::Value::String(raw)) if PERMISSION_MODES.contains(&raw.as_str()) => {
            if raw == "plan" {
                AgentPermissionMode::Plan
            } else {
                AgentPermissionMode::Bubble
            }
        }
        _ => {
            tracing::debug!("Error parsing agent '{name}' from JSON: invalid permissionMode");
            return None;
        }
    };

    // mcpServers: `z.array(AgentMcpServerSpecSchema()).optional()` — absent is
    // fine; otherwise an array where EVERY item is a valid spec (string, or a
    // record of name->McpServerConfig). Any invalid item throws -> drop agent.
    let mcp_servers = match parse_mcp_servers_json_strict(obj.get("mcpServers")) {
        Ok(v) => v,
        Err(()) => {
            tracing::debug!("Error parsing agent '{name}' from JSON: invalid mcpServers");
            return None;
        }
    };

    // hooks: `HooksSchema().optional()` — invalid hooks throw -> drop agent.
    let frontmatter_hooks = match obj.get("hooks") {
        None | Some(serde_json::Value::Null) => Vec::new(),
        Some(v) => match parse_hooks_from_frontmatter_strict(&json_to_yaml(v), name) {
            Some(h) => h,
            None => {
                tracing::debug!("Error parsing agent '{name}' from JSON: invalid hooks");
                return None;
            }
        },
    };

    // maxTurns: `z.number().int().positive().optional()` — absent is fine;
    // otherwise must be a positive integer NUMBER (numeric strings throw).
    let max_turns = match obj.get("maxTurns") {
        None | Some(serde_json::Value::Null) => 100,
        Some(serde_json::Value::Number(n)) => match json_int_value(n) {
            Some(i) if i > 0 => match u32::try_from(i) {
                Ok(v) => v,
                Err(_) => {
                    tracing::debug!("Error parsing agent '{name}' from JSON: invalid maxTurns");
                    return None;
                }
            },
            _ => {
                tracing::debug!("Error parsing agent '{name}' from JSON: invalid maxTurns");
                return None;
            }
        },
        Some(_) => {
            tracing::debug!("Error parsing agent '{name}' from JSON: invalid maxTurns");
            return None;
        }
    };

    // initialPrompt (raw, when trim non-empty). `z.string().optional()` — any
    // non-string throws.
    let initial_prompt = match obj.get("initialPrompt") {
        None | Some(serde_json::Value::Null) => None,
        Some(serde_json::Value::String(s)) => {
            if s.trim().is_empty() {
                None
            } else {
                Some(s.clone())
            }
        }
        Some(_) => {
            tracing::debug!("Error parsing agent '{name}' from JSON: invalid initialPrompt");
            return None;
        }
    };

    // memory: `z.enum(['user','project','local']).optional()`.
    let memory = match obj.get("memory") {
        None | Some(serde_json::Value::Null) => None,
        Some(serde_json::Value::String(s)) => match s.as_str() {
            "user" => Some(AgentMemoryScope::User),
            "project" => Some(AgentMemoryScope::Project),
            "local" => Some(AgentMemoryScope::Local),
            _ => {
                tracing::debug!("Error parsing agent '{name}' from JSON: invalid memory");
                return None;
            }
        },
        Some(_) => {
            tracing::debug!("Error parsing agent '{name}' from JSON: invalid memory");
            return None;
        }
    };

    // background: `z.boolean().optional()` — only a JSON boolean is accepted.
    let background = match obj.get("background") {
        None | Some(serde_json::Value::Null) => false,
        Some(serde_json::Value::Bool(b)) => *b,
        Some(_) => {
            tracing::debug!("Error parsing agent '{name}' from JSON: invalid background");
            return None;
        }
    };

    // isolation: `z.enum(['worktree'])` (3P) / `z.enum(['worktree','remote'])`
    // (ant). On non-ant, `'remote'` is NOT in the enum and therefore throws.
    let ant = std::env::var("USER_TYPE").as_deref() == Ok("ant");
    let isolation = match obj.get("isolation") {
        None | Some(serde_json::Value::Null) => None,
        Some(serde_json::Value::String(s)) => match s.as_str() {
            "worktree" => Some(AgentIsolation::Worktree),
            "remote" if ant => Some(AgentIsolation::Remote),
            _ => {
                tracing::debug!("Error parsing agent '{name}' from JSON: invalid isolation");
                return None;
            }
        },
        Some(_) => {
            tracing::debug!("Error parsing agent '{name}' from JSON: invalid isolation");
            return None;
        }
    };

    Some(AgentDefinition {
        agent_type: name.to_string(),
        when_to_use,
        tools: tools_policy,
        max_turns,
        model,
        permission_mode,
        source,
        base_dir: PathBuf::new(),
        system_prompt: Some(system_prompt),
        mcp_servers,
        frontmatter_hooks,
        icon: None,
        allowed_tools,
        worktree_requirement: None,
        disallowed_tools,
        skills,
        required_mcp_servers: Vec::new(),
        background,
        isolation,
        memory,
        effort,
        initial_prompt,
        color: None,
    })
}

/// Parse multiple agents from a `{ name: definition }` JSON object (claude
/// `parseAgentsFromJson`, loadAgentsDir.ts:521-536). Non-object input -> `[]`
/// (+ log). Entries that fail validation are filtered out.
#[must_use]
pub fn parse_agents_from_json(
    agents_json: &serde_json::Value,
    source: AgentSource,
) -> Vec<AgentDefinition> {
    let Some(obj) = agents_json.as_object() else {
        tracing::debug!("Error parsing agents from JSON: top-level value is not an object");
        return Vec::new();
    };
    obj.iter()
        .filter_map(|(name, def)| parse_agent_from_json(name, def, source))
        .collect()
}

/// Parse the `--agents <json>` CLI flag payload (claude 2.1.198 `QXt(e,
/// "flagSettings")` @223080769: `DBm().parse(e)` where `DBm = A.record(
/// A.string(), r2l())`, then per-entry `s2l(name, def, "flagSettings")`).
///
/// STRICTER than [`parse_agents_from_json`] (the `parseAgentsFromJson` file
/// loader, which filters bad entries): the flag path validates the WHOLE
/// record with a throwing zod schema, so ANY invalid agent definition drops
/// ALL flag agents (`catch` → `C(\`Error parsing agents from JSON: ${msg}\`,
/// {level:"error"})` → `[]`). The only per-entry drop that survives the
/// record parse is `s2l`'s leading-`-` name check (`Agent '${name}' has an
/// invalid name: names must not start with '-'` → that agent only).
///
/// A JSON *syntax* error is caught one frame up in the binary (`try{let g=
/// Ba(r); …}catch(g){De(g)}` — logged, non-fatal); mirrored here so callers
/// hand us the raw flag string. Every failure path returns `[]` and logs —
/// the flag NEVER aborts startup.
#[must_use]
pub fn parse_agents_from_flag_json(raw: &str) -> Vec<AgentDefinition> {
    // `Ba(r)` — JSON.parse of the flag string; a syntax error is logged
    // (`De(g)`) and yields no agents.
    let value: serde_json::Value = match serde_json::from_str(raw) {
        Ok(v) => v,
        Err(e) => {
            tracing::error!("Error parsing agents from JSON: {e}");
            return Vec::new();
        }
    };
    // `DBm().parse` — the top level must be a record (object).
    let Some(obj) = value.as_object() else {
        tracing::error!("Error parsing agents from JSON: expected an object of agent definitions");
        return Vec::new();
    };
    // All-or-nothing record validation: every entry must parse (zod record
    // schema throws on the first invalid definition → [] overall).
    let mut out = Vec::with_capacity(obj.len());
    for (name, def) in obj {
        // `s2l` name guard — drops ONLY this agent (post-record-parse check).
        if name.starts_with('-') {
            tracing::error!("Agent '{name}' has an invalid name: names must not start with '-'");
            continue;
        }
        match parse_agent_from_json(name, def, AgentSource::Flag) {
            Some(a) => out.push(a),
            None => {
                tracing::error!(
                    "Error parsing agents from JSON: invalid definition for agent '{name}'"
                );
                return Vec::new();
            }
        }
    }
    out
}

/// Read a JSON value as a string only when it is a JSON string.
fn json_as_string(v: &serde_json::Value) -> Option<String> {
    v.as_str().map(str::to_string)
}

/// Strictly validate `z.array(z.string()).optional()`: `None` field -> `Ok(None)`;
/// an array of strings -> `Ok(Some(arr))`; a JSON `null` -> `Ok(None)` (optional);
/// a non-array OR an array containing a non-string element -> `Err(())` (zod throw).
#[allow(clippy::result_unit_err)]
fn json_string_array_strict(value: Option<&serde_json::Value>) -> Result<Option<Vec<String>>, ()> {
    match value {
        None | Some(serde_json::Value::Null) => Ok(None),
        Some(serde_json::Value::Array(arr)) => {
            let mut out = Vec::with_capacity(arr.len());
            for item in arr {
                match item.as_str() {
                    Some(s) => out.push(s.to_string()),
                    None => return Err(()),
                }
            }
            Ok(Some(out))
        }
        Some(_) => Err(()),
    }
}

/// Lift a validated `Vec<String>` into a YAML sequence so the shared tool-list
/// coercion helpers (operating on `serde_yaml::Value`) can consume it.
fn json_string_array_to_yaml(arr: Vec<String>) -> serde_yaml::Value {
    serde_yaml::Value::Sequence(arr.into_iter().map(serde_yaml::Value::String).collect())
}

/// Strictly validate `z.array(AgentMcpServerSpecSchema()).optional()` for JSON
/// agents: `None`/`null` -> `Ok([])`; an array where EVERY item is a valid spec
/// (a string, or a record of name -> server-config body) -> `Ok(specs)`;
/// a non-array OR any invalid item -> `Err(())` (zod throw -> drop the agent).
///
/// A multi-key record `{ a: cfgA, b: cfgB }` is valid for zod's
/// `z.record(...)` and is kept as ONE [`AgentMcpServerSpec::Record`] — claude
/// `obs` later warns + skips it (`expected exactly one key`).
#[allow(clippy::result_unit_err)]
fn parse_mcp_servers_json_strict(
    value: Option<&serde_json::Value>,
) -> Result<Vec<AgentMcpServerSpec>, ()> {
    let arr = match value {
        None | Some(serde_json::Value::Null) => return Ok(Vec::new()),
        Some(serde_json::Value::Array(arr)) => arr,
        Some(_) => return Err(()),
    };
    let mut out = Vec::new();
    for item in arr {
        match item {
            serde_json::Value::String(s) => out.push(AgentMcpServerSpec::ByName(s.clone())),
            serde_json::Value::Object(map) if !map.is_empty() => {
                // zod validates each record VALUE against the server-config
                // union; any invalid body throws → drop the whole agent. The
                // record itself is kept RAW (multi-key included) — the
                // exactly-one-key rule is enforced later by claude `obs`
                // (`crate::mcp_servers::agent_mcp_specs_to_scoped_configs`).
                for v in map.values() {
                    if !mcp::server_entry_shape_is_valid(v) {
                        return Err(());
                    }
                }
                out.push(AgentMcpServerSpec::Record(map.clone()));
            }
            _ => return Err(()),
        }
    }
    Ok(out)
}

/// Strictly validate `HooksSchema().optional()` for JSON agents: returns the
/// parsed hooks on success, or `None` when the value is invalid (claude's zod
/// throws and the whole agent is dropped — caller maps `None` -> drop).
fn parse_hooks_from_frontmatter_strict(
    value: &serde_yaml::Value,
    agent_type: &str,
) -> Option<Vec<hooks::HookDefinition>> {
    let json_value: serde_json::Value = serde_yaml::from_value(value.clone()).ok()?;
    let wrapper = serde_json::json!({ "hooks": json_value });
    let raw = serde_json::to_string(&wrapper).ok()?;
    match hooks::parse_hooks_from_settings_json(&raw, hooks::HookSource::FrontMatter) {
        Ok(defs) => Some(defs),
        Err(e) => {
            tracing::debug!("Invalid hooks in agent '{agent_type}': {e}");
            None
        }
    }
}

/// Convert a JSON value into the YAML value space so the shared frontmatter
/// coercion helpers (which operate on `serde_yaml::Value`) can be reused for
/// JSON agents.
fn json_to_yaml(v: &serde_json::Value) -> serde_yaml::Value {
    serde_yaml::to_value(v).unwrap_or(serde_yaml::Value::Null)
}

/// Load every `*.md` agent file under each path in `paths`, in order.
///
/// Files with no frontmatter or invalid YAML are logged at `warn!` and
/// skipped. On `agent_type` collision, **later paths win** — pass the
/// global path FIRST and the project path SECOND so project agents
/// override user-globals.
///
/// The returned list is sorted alphabetically by `agent_type` for stable
/// display order in `/agents`.
pub async fn load_agents_from_dirs(paths: &[(PathBuf, AgentSource)]) -> Vec<AgentDefinition> {
    use std::collections::HashMap;
    let mut by_name: HashMap<String, AgentDefinition> = HashMap::new();
    for (dir, source) in paths {
        // missing dir = empty contribution
        let Ok(mut entries) = tokio::fs::read_dir(dir).await else {
            continue;
        };
        while let Ok(Some(entry)) = entries.next_entry().await {
            let p = entry.path();
            if p.extension().and_then(|s| s.to_str()) != Some("md") {
                continue;
            }
            let raw = match tokio::fs::read_to_string(&p).await {
                Ok(r) => r,
                Err(e) => {
                    tracing::warn!(
                        error = %e,
                        path = %p.display(),
                        "skipping unreadable agent file"
                    );
                    continue;
                }
            };
            match parse_agent_markdown(&raw, *source, dir.clone(), &p) {
                Ok(def) => {
                    by_name.insert(def.agent_type.clone(), def);
                }
                // claude SILENTLY skips files without a `name` field (likely
                // co-located reference docs, not agent attempts).
                Err(AgentLoadError::MissingName(_)) => {}
                Err(e) => {
                    tracing::warn!(
                        error = %e,
                        path = %p.display(),
                        "skipping malformed agent file"
                    );
                }
            }
        }
    }
    let mut out: Vec<AgentDefinition> = by_name.into_values().collect();
    out.sort_by(|a, b| a.agent_type.cmp(&b.agent_type));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::definition::AgentEffort;
    use tempfile::TempDir;

    #[test]
    fn parse_minimal_frontmatter() {
        let raw = "---\nname: reviewer\ndescription: review code\n---\nBody";
        let def = parse_agent_markdown(
            raw,
            AgentSource::UserDefined,
            PathBuf::from("/tmp"),
            Path::new("reviewer.md"),
        )
        .unwrap();
        assert_eq!(def.agent_type, "reviewer");
        assert_eq!(def.when_to_use, "review code");
        assert_eq!(def.system_prompt.as_deref(), Some("Body"));
    }

    #[test]
    fn missing_frontmatter_errors() {
        let raw = "no frontmatter here";
        let err = parse_agent_markdown(
            raw,
            AgentSource::UserDefined,
            PathBuf::from("/tmp"),
            Path::new("x.md"),
        )
        .unwrap_err();
        assert!(matches!(err, AgentLoadError::NoFrontmatter(_)));
    }

    #[test]
    fn tool_list_splits_on_space_and_comma_preserving_parens() {
        // claude parseToolListFromCLI splits on whitespace AND commas (outside
        // parens). A single space-separated string -> multiple tokens.
        assert_eq!(
            parse_tool_list_from_cli(&["Read Grep".to_string()]),
            vec!["Read".to_string(), "Grep".to_string()]
        );
        // Comma-separated.
        assert_eq!(
            parse_tool_list_from_cli(&["Read, Grep".to_string()]),
            vec!["Read".to_string(), "Grep".to_string()]
        );
        // Mixed space + comma, with a paren-containing entry kept whole.
        assert_eq!(
            parse_tool_list_from_cli(&["Read Grep, Bash(git:*) Write".to_string()]),
            vec![
                "Read".to_string(),
                "Grep".to_string(),
                "Bash(git:*)".to_string(),
                "Write".to_string(),
            ]
        );
        // Commas inside parens are preserved.
        assert_eq!(
            parse_tool_list_from_cli(&["Bash(git:*,npm:*)".to_string()]),
            vec!["Bash(git:*,npm:*)".to_string()]
        );
        // Leading/multiple spaces collapse; no empty tokens.
        assert_eq!(
            parse_tool_list_from_cli(&["   Read    Grep   ".to_string()]),
            vec!["Read".to_string(), "Grep".to_string()]
        );
    }

    #[test]
    fn space_separated_tools_in_frontmatter_string() {
        // A frontmatter `tools: "Read Grep"` string now yields two tools.
        let def = md("---\nname: a\ndescription: d\ntools: \"Read Grep\"\n---\n");
        match &def.tools {
            AgentToolPolicy::Explicit(v) => {
                assert_eq!(v, &vec!["Read".to_string(), "Grep".to_string()]);
            }
            other => panic!("expected Explicit, got {other:?}"),
        }
        // disallowedTools and skills share the same coercion.
        let def = md("---\nname: a\ndescription: d\ndisallowedTools: \"Read Bash\"\n---\n");
        assert_eq!(def.disallowed_tools, vec!["Read", "Bash"]);
        let def = md("---\nname: a\ndescription: d\nskills: \"alpha beta\"\n---\n");
        assert_eq!(def.skills, vec!["alpha", "beta"]);
    }

    #[test]
    fn empty_name_is_silent_skip() {
        // claude `!agentType` is falsy for "" -> dropped like an absent name.
        let raw = "---\nname: \"\"\ndescription: d\n---\nBody";
        let err = parse_agent_markdown(
            raw,
            AgentSource::UserDefined,
            PathBuf::from("/tmp"),
            Path::new("x.md"),
        )
        .unwrap_err();
        assert!(matches!(err, AgentLoadError::MissingName(_)));
    }

    #[test]
    fn empty_description_is_skip() {
        // claude `!whenToUse` is falsy for "" -> logged + dropped.
        let raw = "---\nname: a\ndescription: \"\"\n---\nBody";
        let err = parse_agent_markdown(
            raw,
            AgentSource::UserDefined,
            PathBuf::from("/tmp"),
            Path::new("x.md"),
        )
        .unwrap_err();
        assert!(matches!(err, AgentLoadError::MissingDescription(_)));
    }

    #[test]
    fn frontmatter_tools_become_explicit_policy() {
        let raw = "---\nname: r\ndescription: d\ntools: [Read, Grep]\n---\n";
        let def = parse_agent_markdown(
            raw,
            AgentSource::Project,
            PathBuf::from("/tmp"),
            Path::new("r.md"),
        )
        .unwrap();
        match &def.tools {
            AgentToolPolicy::Explicit(v) => {
                assert_eq!(v, &vec!["Read".to_string(), "Grep".to_string()]);
            }
            other => panic!("expected Explicit, got {other:?}"),
        }
        assert_eq!(
            def.allowed_tools,
            vec!["Read".to_string(), "Grep".to_string()]
        );
    }

    #[tokio::test]
    async fn load_agents_from_dirs_merges_user_and_project() {
        let dir = TempDir::new().unwrap();
        let user = dir.path().join("user");
        let project = dir.path().join("project");
        tokio::fs::create_dir_all(&user).await.unwrap();
        tokio::fs::create_dir_all(&project).await.unwrap();
        tokio::fs::write(
            user.join("alpha.md"),
            "---\nname: alpha\ndescription: from user\n---\n",
        )
        .await
        .unwrap();
        tokio::fs::write(
            project.join("alpha.md"),
            "---\nname: alpha\ndescription: from project\n---\n",
        )
        .await
        .unwrap();
        tokio::fs::write(
            project.join("beta.md"),
            "---\nname: beta\ndescription: project-only\n---\n",
        )
        .await
        .unwrap();

        let defs = load_agents_from_dirs(&[
            (user.clone(), AgentSource::UserDefined),
            (project.clone(), AgentSource::Project),
        ])
        .await;

        assert_eq!(defs.len(), 2);
        // Sorted alphabetically.
        assert_eq!(defs[0].agent_type, "alpha");
        assert_eq!(defs[0].when_to_use, "from project"); // project wins
        assert_eq!(defs[1].agent_type, "beta");
    }

    #[tokio::test]
    async fn load_agents_from_missing_dir_yields_empty() {
        let defs =
            load_agents_from_dirs(&[(PathBuf::from("/does/not/exist"), AgentSource::UserDefined)])
                .await;
        assert!(defs.is_empty());
    }

    // ── extended frontmatter field parsing ──

    fn md(body: &str) -> AgentDefinition {
        parse_agent_markdown(
            body,
            AgentSource::UserDefined,
            PathBuf::from("/tmp"),
            Path::new("a.md"),
        )
        .unwrap()
    }

    #[test]
    fn missing_name_is_silent_skip() {
        let raw = "---\ndescription: d\n---\nBody";
        let err = parse_agent_markdown(
            raw,
            AgentSource::UserDefined,
            PathBuf::from("/tmp"),
            Path::new("notes.md"),
        )
        .unwrap_err();
        assert!(matches!(err, AgentLoadError::MissingName(_)));
    }

    #[test]
    fn missing_description_errors() {
        let raw = "---\nname: x\n---\nBody";
        let err = parse_agent_markdown(
            raw,
            AgentSource::UserDefined,
            PathBuf::from("/tmp"),
            Path::new("x.md"),
        )
        .unwrap_err();
        assert!(matches!(err, AgentLoadError::MissingDescription(_)));
    }

    #[test]
    fn disallowed_tools_string_and_list() {
        let def = md("---\nname: a\ndescription: d\ndisallowedTools: \"Read, Bash\"\n---\n");
        assert_eq!(def.disallowed_tools, vec!["Read", "Bash"]);
        let def = md("---\nname: a\ndescription: d\ndisallowedTools: [Read, Bash]\n---\n");
        assert_eq!(def.disallowed_tools, vec!["Read", "Bash"]);
    }

    #[test]
    fn frontmatter_disallowed_tools_parsed() {
        // claude applies `tools` (allowlist → Explicit) and `disallowedTools`
        // (denylist) ORTHOGONALLY: an agent can carry both. The resolver
        // subtracts disallowed_tools from the Explicit-resolved pool.
        let def =
            md("---\nname: a\ndescription: d\ntools: [Read, Bash]\ndisallowedTools: [Bash]\n---\n");
        match &def.tools {
            AgentToolPolicy::Explicit(names) => {
                assert_eq!(names, &vec!["Read".to_string(), "Bash".to_string()]);
            }
            other => panic!("expected Explicit policy, got {other:?}"),
        }
        assert_eq!(def.disallowed_tools, vec!["Bash"]);
    }

    #[test]
    fn skills_comma_and_list() {
        let def = md("---\nname: a\ndescription: d\nskills: \"alpha, beta\"\n---\n");
        assert_eq!(def.skills, vec!["alpha", "beta"]);
        let def = md("---\nname: a\ndescription: d\nskills: [alpha, beta]\n---\n");
        assert_eq!(def.skills, vec!["alpha", "beta"]);
        // missing -> []
        let def = md("---\nname: a\ndescription: d\n---\n");
        assert!(def.skills.is_empty());
    }

    #[test]
    fn background_true_variants() {
        assert!(md("---\nname: a\ndescription: d\nbackground: true\n---\n").background);
        assert!(md("---\nname: a\ndescription: d\nbackground: \"true\"\n---\n").background);
        assert!(!md("---\nname: a\ndescription: d\nbackground: false\n---\n").background);
        // invalid value -> false (logged)
        assert!(!md("---\nname: a\ndescription: d\nbackground: yes\n---\n").background);
        // omitted -> false
        assert!(!md("---\nname: a\ndescription: d\n---\n").background);
    }

    #[test]
    fn memory_valid_and_invalid() {
        assert_eq!(
            md("---\nname: a\ndescription: d\nmemory: project\n---\n").memory,
            Some(AgentMemoryScope::Project)
        );
        assert_eq!(
            md("---\nname: a\ndescription: d\nmemory: bogus\n---\n").memory,
            None
        );
    }

    #[test]
    fn isolation_worktree_and_remote_rejected_on_non_ant() {
        // NOTE: relies on USER_TYPE != "ant" in the test env.
        assert_eq!(
            md("---\nname: a\ndescription: d\nisolation: worktree\n---\n").isolation,
            Some(AgentIsolation::Worktree)
        );
        if std::env::var("USER_TYPE").as_deref() != Ok("ant") {
            assert_eq!(
                md("---\nname: a\ndescription: d\nisolation: remote\n---\n").isolation,
                None
            );
        }
    }

    #[test]
    fn effort_level_and_integer_and_invalid() {
        assert_eq!(
            md("---\nname: a\ndescription: d\neffort: high\n---\n").effort,
            Some(AgentEffort::Level("high".into()))
        );
        assert_eq!(
            md("---\nname: a\ndescription: d\neffort: 7\n---\n").effort,
            Some(AgentEffort::Numeric(7))
        );
        assert_eq!(
            md("---\nname: a\ndescription: d\neffort: bogus\n---\n").effort,
            None
        );
    }

    #[test]
    fn color_valid_and_invalid() {
        assert_eq!(
            md("---\nname: a\ndescription: d\ncolor: blue\n---\n").color,
            Some("blue".to_string())
        );
        assert_eq!(
            md("---\nname: a\ndescription: d\ncolor: mauve\n---\n").color,
            None
        );
    }

    #[test]
    fn initial_prompt_keeps_raw_when_trim_nonempty() {
        // Leading space preserved; raw (untrimmed) value kept.
        let def = md("---\nname: a\ndescription: d\ninitialPrompt: \"  hello\"\n---\n");
        assert_eq!(def.initial_prompt.as_deref(), Some("  hello"));
        // whitespace-only -> None
        let def = md("---\nname: a\ndescription: d\ninitialPrompt: \"   \"\n---\n");
        assert_eq!(def.initial_prompt, None);
    }

    #[test]
    fn permission_mode_plan_and_default_and_invalid() {
        assert_eq!(
            md("---\nname: a\ndescription: d\npermissionMode: plan\n---\n").permission_mode,
            AgentPermissionMode::Plan
        );
        // 'default' is valid but collapses to Bubble (lossy enum).
        assert_eq!(
            md("---\nname: a\ndescription: d\npermissionMode: default\n---\n").permission_mode,
            AgentPermissionMode::Bubble
        );
        // 'acceptEdits' is valid but collapses to Bubble.
        assert_eq!(
            md("---\nname: a\ndescription: d\npermissionMode: acceptEdits\n---\n").permission_mode,
            AgentPermissionMode::Bubble
        );
        // invalid -> Bubble (logged)
        assert_eq!(
            md("---\nname: a\ndescription: d\npermissionMode: bogus\n---\n").permission_mode,
            AgentPermissionMode::Bubble
        );
    }

    #[test]
    fn model_inherit_coercion() {
        assert!(matches!(
            md("---\nname: a\ndescription: d\nmodel: inherit\n---\n").model,
            AgentModel::Inherit
        ));
        assert!(matches!(
            md("---\nname: a\ndescription: d\nmodel: INHERIT\n---\n").model,
            AgentModel::Inherit
        ));
        assert!(
            matches!(md("---\nname: a\ndescription: d\nmodel: sonnet\n---\n").model, AgentModel::Alias(m) if m == "sonnet")
        );
    }

    #[test]
    fn tools_star_is_all_and_empty_is_explicit_none() {
        // missing -> All
        assert!(matches!(
            md("---\nname: a\ndescription: d\n---\n").tools,
            AgentToolPolicy::All { .. }
        ));
        // '*' -> All
        assert!(matches!(
            md("---\nname: a\ndescription: d\ntools: \"*\"\n---\n").tools,
            AgentToolPolicy::All { .. }
        ));
        // present-but-empty -> Explicit([]) (no tools)
        match md("---\nname: a\ndescription: d\ntools: []\n---\n").tools {
            AgentToolPolicy::Explicit(v) => assert!(v.is_empty()),
            other => panic!("expected Explicit([]), got {other:?}"),
        }
    }

    #[test]
    fn hooks_frontmatter_parsed_with_frontmatter_source() {
        let raw = "---\nname: a\ndescription: d\nhooks:\n  PreToolUse:\n    - hooks:\n        - type: command\n          command: echo hi\n---\nBody";
        let def = md(raw);
        assert!(
            !def.frontmatter_hooks.is_empty(),
            "expected hooks parsed from frontmatter"
        );
        assert!(def
            .frontmatter_hooks
            .iter()
            .all(|h| h.source == hooks::HookSource::FrontMatter));
    }

    #[test]
    fn mcp_servers_by_name_and_skip_invalid() {
        let def = md("---\nname: a\ndescription: d\nmcpServers: [slack]\n---\n");
        assert!(
            matches!(def.mcp_servers.as_slice(), [AgentMcpServerSpec::ByName(n)] if n == "slack")
        );
    }

    /// (M7 cc2.1.220) A real-world inline server body (`{command, args}` — one
    /// `.mcp.json` entry, NOT a serialized `McpServerConfig`) parses to a raw
    /// `Record`; the scoped-config conversion (`obs`) validates it later.
    #[test]
    fn mcp_servers_inline_record_kept_raw() {
        let def = md(concat!(
            "---\nname: a\ndescription: d\nmcpServers:\n",
            "  - docs:\n      command: npx\n      args: [\"-y\", \"docs-mcp\"]\n",
            "---\n"
        ));
        match def.mcp_servers.as_slice() {
            [AgentMcpServerSpec::Record(map)] => {
                assert_eq!(map.len(), 1);
                let body = map.get("docs").expect("keyed by server name");
                assert_eq!(body.get("command").and_then(|v| v.as_str()), Some("npx"));
            }
            other => panic!("expected one Record spec, got {other:?}"),
        }
    }

    // ── JSON agents ──

    #[test]
    fn json_agent_full_field_set() {
        let json = serde_json::json!({
            "description": "does things",
            "prompt": "you are a helper",
            "tools": ["Read", "Grep"],
            "disallowedTools": ["Bash"],
            "model": "inherit",
            "effort": "high",
            "permissionMode": "plan",
            "maxTurns": 5,
            "skills": ["alpha"],
            "initialPrompt": "  start",
            "memory": "project",
            "background": true,
            "isolation": "worktree",
        });
        let def = parse_agent_from_json("myagent", &json, AgentSource::Flag).unwrap();
        assert_eq!(def.agent_type, "myagent");
        assert_eq!(def.when_to_use, "does things");
        assert_eq!(def.system_prompt.as_deref(), Some("you are a helper"));
        assert!(matches!(def.tools, AgentToolPolicy::Explicit(_)));
        assert_eq!(def.disallowed_tools, vec!["Bash"]);
        assert!(matches!(def.model, AgentModel::Inherit));
        assert_eq!(def.effort, Some(AgentEffort::Level("high".into())));
        assert_eq!(def.permission_mode, AgentPermissionMode::Plan);
        assert_eq!(def.max_turns, 5);
        assert_eq!(def.skills, vec!["alpha"]);
        assert_eq!(def.initial_prompt.as_deref(), Some("  start"));
        assert_eq!(def.memory, Some(AgentMemoryScope::Project));
        assert!(def.background);
        assert_eq!(def.isolation, Some(AgentIsolation::Worktree));
        assert_eq!(def.source, AgentSource::Flag);
    }

    #[test]
    fn json_agent_missing_description_is_none() {
        let json = serde_json::json!({ "prompt": "x" });
        assert!(parse_agent_from_json("a", &json, AgentSource::Flag).is_none());
        let json = serde_json::json!({ "description": "", "prompt": "x" });
        assert!(parse_agent_from_json("a", &json, AgentSource::Flag).is_none());
    }

    #[test]
    fn json_agent_missing_prompt_is_none() {
        let json = serde_json::json!({ "description": "d" });
        assert!(parse_agent_from_json("a", &json, AgentSource::Flag).is_none());
        let json = serde_json::json!({ "description": "d", "prompt": "" });
        assert!(parse_agent_from_json("a", &json, AgentSource::Flag).is_none());
    }

    #[test]
    fn json_agent_strict_invalid_fields_drop_whole_agent() {
        // Each of these is a single invalid field that claude's throwing zod
        // schema rejects -> the WHOLE agent is dropped (None).
        let base = |extra: serde_json::Value| {
            let mut obj = serde_json::Map::new();
            obj.insert("description".into(), serde_json::json!("d"));
            obj.insert("prompt".into(), serde_json::json!("p"));
            if let serde_json::Value::Object(m) = extra {
                for (k, v) in m {
                    obj.insert(k, v);
                }
            }
            serde_json::Value::Object(obj)
        };

        // bad effort: numeric string "7" is NOT a level and NOT a number.
        assert!(parse_agent_from_json(
            "a",
            &base(serde_json::json!({"effort": "7"})),
            AgentSource::Flag
        )
        .is_none());
        // bad effort: float is not an int.
        assert!(parse_agent_from_json(
            "a",
            &base(serde_json::json!({"effort": 1.5})),
            AgentSource::Flag
        )
        .is_none());
        // bad effort: wrong-case level (zod enum is case-sensitive).
        assert!(parse_agent_from_json(
            "a",
            &base(serde_json::json!({"effort": "HIGH"})),
            AgentSource::Flag
        )
        .is_none());
        // bad maxTurns: zero / negative / non-number string.
        assert!(parse_agent_from_json(
            "a",
            &base(serde_json::json!({"maxTurns": 0})),
            AgentSource::Flag
        )
        .is_none());
        assert!(parse_agent_from_json(
            "a",
            &base(serde_json::json!({"maxTurns": -3})),
            AgentSource::Flag
        )
        .is_none());
        assert!(parse_agent_from_json(
            "a",
            &base(serde_json::json!({"maxTurns": "5"})),
            AgentSource::Flag
        )
        .is_none());
        // bad permissionMode.
        assert!(parse_agent_from_json(
            "a",
            &base(serde_json::json!({"permissionMode": "bogus"})),
            AgentSource::Flag
        )
        .is_none());
        // bad memory.
        assert!(parse_agent_from_json(
            "a",
            &base(serde_json::json!({"memory": "bogus"})),
            AgentSource::Flag
        )
        .is_none());
        // bad isolation (non-ant: 'remote' not in enum; assumes test env != ant).
        if std::env::var("USER_TYPE").as_deref() != Ok("ant") {
            assert!(parse_agent_from_json(
                "a",
                &base(serde_json::json!({"isolation": "remote"})),
                AgentSource::Flag
            )
            .is_none());
        }
        assert!(parse_agent_from_json(
            "a",
            &base(serde_json::json!({"isolation": "bogus"})),
            AgentSource::Flag
        )
        .is_none());
        // non-array tools / disallowedTools / skills.
        assert!(parse_agent_from_json(
            "a",
            &base(serde_json::json!({"tools": "Read"})),
            AgentSource::Flag
        )
        .is_none());
        assert!(parse_agent_from_json(
            "a",
            &base(serde_json::json!({"tools": [1, 2]})),
            AgentSource::Flag
        )
        .is_none());
        assert!(parse_agent_from_json(
            "a",
            &base(serde_json::json!({"disallowedTools": "Bash"})),
            AgentSource::Flag
        )
        .is_none());
        assert!(parse_agent_from_json(
            "a",
            &base(serde_json::json!({"skills": "alpha"})),
            AgentSource::Flag
        )
        .is_none());
        // non-bool background.
        assert!(parse_agent_from_json(
            "a",
            &base(serde_json::json!({"background": "true"})),
            AgentSource::Flag
        )
        .is_none());
        // whitespace-only model.
        assert!(parse_agent_from_json(
            "a",
            &base(serde_json::json!({"model": "   "})),
            AgentSource::Flag
        )
        .is_none());
        // invalid mcpServers item (number is neither string nor record).
        assert!(parse_agent_from_json(
            "a",
            &base(serde_json::json!({"mcpServers": [123]})),
            AgentSource::Flag
        )
        .is_none());
        // mcpServers not an array.
        assert!(parse_agent_from_json(
            "a",
            &base(serde_json::json!({"mcpServers": "slack"})),
            AgentSource::Flag
        )
        .is_none());
    }

    #[test]
    fn json_agent_strict_valid_fields_kept() {
        // Sanity: the strict path STILL accepts well-formed values.
        let json = serde_json::json!({
            "description": "d",
            "prompt": "p",
            "effort": 7,
            "maxTurns": 3,
            "tools": ["Read"],
            "background": false,
            "memory": "user",
            "mcpServers": ["slack", "github"],
        });
        let def = parse_agent_from_json("a", &json, AgentSource::Flag).unwrap();
        assert_eq!(def.effort, Some(AgentEffort::Numeric(7)));
        assert_eq!(def.max_turns, 3);
        assert!(!def.background);
        assert_eq!(def.memory, Some(AgentMemoryScope::User));
        assert_eq!(def.mcp_servers.len(), 2);
        assert!(matches!(&def.mcp_servers[0], AgentMcpServerSpec::ByName(n) if n == "slack"));
        assert!(matches!(&def.mcp_servers[1], AgentMcpServerSpec::ByName(n) if n == "github"));
    }

    #[test]
    fn json_agents_object_filters_invalid() {
        let json = serde_json::json!({
            "good": { "description": "d", "prompt": "p" },
            "bad": { "description": "d" }, // missing prompt
        });
        let defs = parse_agents_from_json(&json, AgentSource::Flag);
        assert_eq!(defs.len(), 1);
        assert_eq!(defs[0].agent_type, "good");
    }

    #[test]
    fn json_agents_non_object_is_empty() {
        let json = serde_json::json!([1, 2, 3]);
        assert!(parse_agents_from_json(&json, AgentSource::Flag).is_empty());
    }

    #[test]
    fn agent_definition_legacy_json_roundtrip() {
        // A legacy serialized AgentDefinition missing the new keys must still
        // deserialize via #[serde(default)].
        let legacy = serde_json::json!({
            "agent_type": "legacy",
            "when_to_use": "w",
            "tools": { "All": { "use_exact_tools": false } },
            "max_turns": 10,
            "model": "Inherit",
            "permission_mode": "Bubble",
            "source": "BuiltIn",
            "base_dir": "/tmp",
            "system_prompt": null,
            "mcp_servers": [],
            "frontmatter_hooks": [],
            "icon": null,
            "allowed_tools": [],
            "worktree_requirement": null
        });
        let def: AgentDefinition = serde_json::from_value(legacy).unwrap();
        assert_eq!(def.agent_type, "legacy");
        assert!(def.disallowed_tools.is_empty());
        assert!(!def.background);
        assert_eq!(def.color, None);
        // Round-trips back out and back in.
        let s = serde_json::to_string(&def).unwrap();
        let def2: AgentDefinition = serde_json::from_str(&s).unwrap();
        assert_eq!(def2.agent_type, "legacy");
    }

    // ── `--agents <json>` flag parser (`QXt`, cc 2.1.198 M4) ────────────────

    #[test]
    fn flag_json_valid_agents_parse_with_flag_source() {
        // The binary's own help example.
        let raw =
            r#"{"reviewer": {"description": "Reviews code", "prompt": "You are a code reviewer"}}"#;
        let out = parse_agents_from_flag_json(raw);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].agent_type, "reviewer");
        assert_eq!(out[0].when_to_use, "Reviews code");
        assert_eq!(
            out[0].system_prompt.as_deref(),
            Some("You are a code reviewer")
        );
        assert_eq!(out[0].source, AgentSource::Flag);
    }

    #[test]
    fn flag_json_syntax_error_yields_no_agents() {
        // `Ba(r)` throws → `De(g)` (logged), no agents, no abort.
        assert!(parse_agents_from_flag_json("{not json").is_empty());
    }

    #[test]
    fn flag_json_non_object_yields_no_agents() {
        // `DBm()` is a record schema — arrays/scalars fail the record parse.
        assert!(parse_agents_from_flag_json("[1,2]").is_empty());
        assert!(parse_agents_from_flag_json("\"x\"").is_empty());
    }

    #[test]
    fn flag_json_one_invalid_agent_drops_all() {
        // Record-level zod parse is all-or-nothing (unlike the per-entry
        // filtering of `parseAgentsFromJson`): `good` is dropped too.
        let raw = r#"{"good": {"description": "d", "prompt": "p"},
                      "bad": {"description": "", "prompt": "p"}}"#;
        assert!(parse_agents_from_flag_json(raw).is_empty());
    }

    #[test]
    fn flag_json_dash_name_drops_only_that_agent() {
        // `s2l`'s leading-`-` name guard is per-entry (post-record-parse).
        let raw = r#"{"-bad": {"description": "d", "prompt": "p"},
                      "good": {"description": "d", "prompt": "p"}}"#;
        let out = parse_agents_from_flag_json(raw);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].agent_type, "good");
    }
}
