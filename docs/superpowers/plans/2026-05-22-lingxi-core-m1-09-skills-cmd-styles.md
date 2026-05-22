# LingXi Core M1 · Plan 09 · Skills + SlashCommands + OutputStyles

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development or superpowers:executing-plans.

**Goal:** Three small but user-visible crates: `lingxi-skills` (Skill model + registry + discovery prefetch + SkillTool), `lingxi-commands` (80+ builtin slash command handlers + markdown loader + argument substitution), `lingxi-outputstyles` (style registry + system-prompt addendum).

**Architecture:** SkillTool dispatches via `lingxi-agent::StateMachinePool` (per §10.0 visibility contract — skill execution is model-visible, so it's a Subagent slot, not a ForkedAgent). SlashCommand parser emits a `CommandResult::EmitEffects` so the run loop can route to the right subsystem.

**Depends on:** Plans 01-08.

---

## File Structure

```
crates/skills/
├── Cargo.toml
└── src/{lib, model, registry, frontmatter, mcp_builders, prefetch, skill_tool}.rs

crates/commands/
├── Cargo.toml
└── src/{lib, model, parser, argument_substitution, registry, builtin/mod.rs, builtin/{help, login, logout, mcp, memory, compact, resume, permissions, plugin, agents, hooks, skills, output_style, cost, tasks, ide, model, plan}.rs, markdown_loader}.rs

crates/outputstyles/
├── Cargo.toml
└── src/{lib, model, registry}.rs
```

---

## Task 1: lingxi-skills

**Files:** Create the 7 files above for skills.

- [ ] **Step 1: Cargo.toml**

```toml
[package]
name = "lingxi-skills"
version = "0.1.0"
edition.workspace = true
license.workspace = true

[dependencies]
lingxi-protocol = { path = "../protocol" }
lingxi-traits = { path = "../traits" }
lingxi-tools = { path = "../tools" }
lingxi-agent = { path = "../agent" }
lingxi-sidequery = { path = "../sidequery" }
lingxi-mcp = { path = "../mcp" }
serde.workspace = true
serde_json.workspace = true
serde_yaml = "0.9"
thiserror.workspace = true
async-trait.workspace = true
tokio = { version = "1", features = ["sync"] }
tracing.workspace = true

[lints]
workspace = true
```

- [ ] **Step 2: model.rs**

```rust
use lingxi_protocol::PluginId;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Skill {
    pub name: String,
    pub description: String,
    pub frontmatter: SkillFrontmatter,
    pub content: String,
    pub source: SkillSource,
    pub loaded_from: LoadedFrom,
    pub plugin_id: Option<PluginId>,
    pub file_path: PathBuf,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct SkillFrontmatter {
    pub name: String,
    pub description: String,
    pub when_to_use: Option<String>,
    /// Field name aligned with AgentDefinition::allowed_tools (§10.2) and
    /// CommandFrontmatter::allowed_tools (§19.1). Accepts legacy tools_allowed alias.
    #[serde(alias = "tools_allowed")]
    pub allowed_tools: Option<Vec<String>>,
    #[serde(default = "default_true")]
    pub auto_search: bool,
    pub triggers: Vec<String>,
}

fn default_true() -> bool { true }

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SkillSource {
    Bundled, User, Project, Plugin, Managed,
    Mcp { },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum LoadedFrom { Bundled, Skills, Plugin, Managed, Mcp, CommandsDeprecated }
```

- [ ] **Step 3: registry.rs**

```rust
use crate::model::Skill;
use lingxi_protocol::{McpConnectionId, PluginId};
use std::collections::HashMap;

pub struct SkillRegistry {
    skills: HashMap<String, Skill>,
    trigger_index: HashMap<String, Vec<String>>,
    mcp_skills: HashMap<McpConnectionId, Vec<String>>,
    plugin_skills: HashMap<PluginId, Vec<String>>,
}

impl SkillRegistry {
    pub fn new() -> Self {
        Self { skills: HashMap::new(), trigger_index: HashMap::new(), mcp_skills: HashMap::new(), plugin_skills: HashMap::new() }
    }

    pub fn register(&mut self, skill: Skill) {
        for trig in &skill.frontmatter.triggers {
            self.trigger_index.entry(trig.to_lowercase()).or_default().push(skill.name.clone());
        }
        self.skills.insert(skill.name.clone(), skill);
    }

    pub fn get(&self, name: &str) -> Option<&Skill> { self.skills.get(name) }

    pub fn discover(&self, query: &str) -> Vec<&Skill> {
        let q = query.to_lowercase();
        let mut names: Vec<&String> = Vec::new();
        for (trig, skill_names) in &self.trigger_index {
            if q.contains(trig) { names.extend(skill_names); }
        }
        names.sort(); names.dedup();
        names.into_iter().filter_map(|n| self.skills.get(n)).collect()
    }

    pub fn register_plugin_skills(&mut self, plugin_id: PluginId, skills: Vec<Skill>) {
        let names: Vec<String> = skills.iter().map(|s| s.name.clone()).collect();
        for s in skills { self.register(s); }
        self.plugin_skills.insert(plugin_id, names);
    }

    pub fn unregister_plugin(&mut self, plugin_id: &PluginId) {
        if let Some(names) = self.plugin_skills.remove(plugin_id) {
            for n in &names { self.skills.remove(n); }
        }
    }
}

impl Default for SkillRegistry { fn default() -> Self { Self::new() } }

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{SkillFrontmatter, SkillSource, LoadedFrom};

    fn mk_skill(name: &str, triggers: &[&str]) -> Skill {
        Skill {
            name: name.into(),
            description: "".into(),
            frontmatter: SkillFrontmatter { name: name.into(), description: "".into(), triggers: triggers.iter().map(|s| (*s).into()).collect(), ..Default::default() },
            content: "".into(),
            source: SkillSource::Bundled,
            loaded_from: LoadedFrom::Bundled,
            plugin_id: None,
            file_path: "/tmp".into(),
        }
    }

    #[test]
    fn discover_matches_trigger() {
        let mut r = SkillRegistry::new();
        r.register(mk_skill("git-commit", &["commit", "git"]));
        let hits = r.discover("please commit my changes");
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].name, "git-commit");
    }
}
```

- [ ] **Step 4: frontmatter.rs (reuse memory's parser)**

```rust
use crate::model::{Skill, SkillFrontmatter, SkillSource, LoadedFrom};
use std::path::PathBuf;
use thiserror::Error;

#[derive(Debug, Clone, Error)]
pub enum SkillLoadError {
    #[error("parse failed: {0}")]
    Parse(String),
}

pub fn parse_skill_markdown(
    raw: &str,
    file_path: PathBuf,
    source: SkillSource,
    loaded_from: LoadedFrom,
) -> Result<Skill, SkillLoadError> {
    let (fm, body): (SkillFrontmatter, String) = if raw.starts_with("---") {
        let rest = &raw[3..];
        let end = rest.find("\n---\n").ok_or(SkillLoadError::Parse("unterminated".into()))?;
        let yaml = &rest[..end];
        let body = &rest[end + 5..];
        let fm: SkillFrontmatter = serde_yaml::from_str(yaml).map_err(|e| SkillLoadError::Parse(e.to_string()))?;
        (fm, body.trim_start().to_string())
    } else {
        (SkillFrontmatter::default(), raw.to_string())
    };

    Ok(Skill {
        name: fm.name.clone(),
        description: fm.description.clone(),
        frontmatter: fm,
        content: body,
        source,
        loaded_from,
        plugin_id: None,
        file_path,
    })
}
```

- [ ] **Step 5: skill_tool.rs (dispatches via StateMachinePool — D5)**

```rust
use crate::registry::SkillRegistry;
use async_trait::async_trait;
use lingxi_agent::{StateMachinePool, SubagentContext};
use lingxi_permission::{PermissionDecisionReason, PermissionMetadata, PermissionResult};
use lingxi_tools::{DescriptionOptions, PromptOptions, Tool, ToolCallResult, ToolError, ToolStaticContext, ToolUseContext};
use serde_json::Value;
use std::sync::Arc;
use tokio::sync::RwLock;

pub struct SkillTool {
    registry: Arc<RwLock<SkillRegistry>>,
    pool: Arc<StateMachinePool>,
}

impl SkillTool {
    pub fn new(registry: Arc<RwLock<SkillRegistry>>, pool: Arc<StateMachinePool>) -> Self {
        Self { registry, pool }
    }
}

#[async_trait]
impl Tool for SkillTool {
    fn name(&self) -> &str { "Skill" }

    fn input_schema(&self) -> &Value {
        static SCHEMA: once_cell::sync::Lazy<Value> = once_cell::sync::Lazy::new(|| {
            serde_json::json!({"type":"object","properties":{"name":{"type":"string"}},"required":["name"]})
        });
        &SCHEMA
    }

    fn is_enabled(&self, _: &ToolStaticContext) -> bool { true }
    fn max_result_size_chars(&self) -> usize { 32_768 }
    fn is_concurrency_safe(&self, _: &Value) -> bool { false /* mutates parent context via subagent */ }
    fn is_read_only(&self, _: &Value) -> bool { false }

    async fn check_permissions(&self, _: &Value, _: &ToolUseContext) -> PermissionResult {
        PermissionResult::Allow {
            reason: PermissionDecisionReason::Other { reason: "skill tool".into() },
            updated_input: None, update_destination: None,
            metadata: PermissionMetadata::default(),
        }
    }

    async fn description(&self, _: &Value, _: &DescriptionOptions) -> String {
        "Invoke a registered skill by name.".into()
    }
    async fn prompt(&self, _: &PromptOptions) -> String { "Use this when a skill matches your task.".into() }

    async fn call(&self, input: Value, _ctx: ToolUseContext, _: lingxi_tools::ToolProgressSender)
        -> Result<ToolCallResult, ToolError>
    {
        let name = input.get("name").and_then(|v| v.as_str())
            .ok_or(ToolError::InvalidInput("name required".into()))?;
        let _skill = {
            let reg = self.registry.read().await;
            reg.get(name).cloned().ok_or_else(|| ToolError::NotFound(name.into()))?
        };
        // Build a Subagent context — model-visible execution. Full subagent
        // spawn happens in Plan 15 cli-demo wiring; M1.15 ships the registry
        // resolution + result shape.
        Ok(ToolCallResult {
            data: serde_json::json!({"skill": name, "status": "stub"}),
            new_messages: vec![],
            context_modifier: None,
            mcp_meta: None,
        })
    }
}
```

- [ ] **Step 6: prefetch.rs + mcp_builders.rs (small stubs)**

```rust
// prefetch.rs — scaffold; real prefetch in Plan 10 Session work
pub struct SkillDiscoveryPrefetch;
```

```rust
// mcp_builders.rs — derive skills from MCP server tool descriptions
use crate::model::{Skill, SkillFrontmatter, SkillSource, LoadedFrom};
use lingxi_traits::McpToolDto;

pub fn skill_from_mcp_tool(tool: &McpToolDto) -> Skill {
    Skill {
        name: tool.full_name.clone(),
        description: tool.description.clone(),
        frontmatter: SkillFrontmatter {
            name: tool.full_name.clone(),
            description: tool.description.clone(),
            triggers: vec![tool.tool_name.to_lowercase()],
            ..Default::default()
        },
        content: tool.description.clone(),
        source: SkillSource::Mcp {},
        loaded_from: LoadedFrom::Mcp,
        plugin_id: None,
        file_path: "<mcp>".into(),
    }
}
```

- [ ] **Step 7: lib.rs + once_cell dep**

```rust
#![forbid(unsafe_code)]
pub mod frontmatter;
pub mod mcp_builders;
pub mod model;
pub mod prefetch;
pub mod registry;
pub mod skill_tool;

pub use model::*;
pub use registry::SkillRegistry;
pub use skill_tool::SkillTool;
```

Add `once_cell = "1"` to Cargo.toml deps.

- [ ] **Step 8: Run + commit**

```bash
cargo test -p lingxi-skills
git add crates/skills
git commit -m "feat(skills): registry + frontmatter loader + SkillTool via StateMachinePool"
```

---

## Task 2: lingxi-commands

**Files:** Create scaffold + parser + argument substitution + a few representative builtin handlers.

- [ ] **Step 1: Cargo.toml**

```toml
[package]
name = "lingxi-commands"
version = "0.1.0"
edition.workspace = true
license.workspace = true

[dependencies]
lingxi-protocol = { path = "../protocol" }
lingxi-traits = { path = "../traits" }
lingxi-core = { path = "../core" }
lingxi-tools = { path = "../tools" }
serde.workspace = true
serde_json.workspace = true
serde_yaml = "0.9"
thiserror.workspace = true
async-trait.workspace = true
tracing.workspace = true

[lints]
workspace = true
```

- [ ] **Step 2: parser.rs**

```rust
#[derive(Debug, Clone)]
pub struct ParsedSlashCommand {
    pub name: String,
    pub raw_args: String,
    pub positional_args: Vec<String>,
}

pub fn parse_slash_command(input: &str) -> Option<ParsedSlashCommand> {
    if !input.starts_with('/') { return None; }
    let trimmed = &input[1..];
    let (name, args_str) = match trimmed.find(char::is_whitespace) {
        Some(i) => (&trimmed[..i], trimmed[i + 1..].trim_start()),
        None => (trimmed, ""),
    };
    let positional = tokenize_args(args_str);
    Some(ParsedSlashCommand { name: name.to_string(), raw_args: args_str.to_string(), positional_args: positional })
}

fn tokenize_args(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut buf = String::new();
    let mut in_quote: Option<char> = None;
    for c in s.chars() {
        match (in_quote, c) {
            (Some(q), c) if c == q => { in_quote = None; out.push(std::mem::take(&mut buf)); }
            (Some(_), c) => buf.push(c),
            (None, '"') | (None, '\'') => { in_quote = Some(c); }
            (None, c) if c.is_whitespace() => { if !buf.is_empty() { out.push(std::mem::take(&mut buf)); } }
            (None, c) => buf.push(c),
        }
    }
    if !buf.is_empty() { out.push(buf); }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_name_and_args() {
        let p = parse_slash_command("/memory add foo bar").unwrap();
        assert_eq!(p.name, "memory");
        assert_eq!(p.positional_args, vec!["add", "foo", "bar"]);
        assert_eq!(p.raw_args, "add foo bar");
    }

    #[test]
    fn handles_quoted_args() {
        let p = parse_slash_command("/skill run \"git commit\"").unwrap();
        assert_eq!(p.positional_args, vec!["run", "git commit"]);
    }
}
```

- [ ] **Step 3: argument_substitution.rs**

```rust
use crate::parser::ParsedSlashCommand;

pub fn substitute_arguments(template: &str, args: &ParsedSlashCommand) -> String {
    let mut out = template.to_string();
    out = out.replace("$ARGUMENTS", &args.raw_args);
    out = out.replace("$@", &args.positional_args.join(" "));
    for (i, a) in args.positional_args.iter().enumerate() {
        out = out.replace(&format!("${}", i + 1), a);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn substitutes_positional_and_arguments() {
        let p = crate::parser::parse_slash_command("/foo a b c").unwrap();
        assert_eq!(substitute_arguments("first=$1 all=$ARGUMENTS", &p), "first=a all=a b c");
    }
}
```

- [ ] **Step 4: model.rs**

```rust
use crate::parser::ParsedSlashCommand;
use lingxi_protocol::{Effect, PluginId, McpConnectionId};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SlashCommand {
    pub name: String,
    pub description: String,
    pub source: CommandSource,
    pub kind: SlashCommandKind,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SlashCommandKind {
    Builtin { handler_id: String },
    Markdown { file_path: PathBuf, frontmatter: CommandFrontmatter, prompt_template: String },
    Plugin { plugin_id: PluginId, file_path: PathBuf, frontmatter: CommandFrontmatter, prompt_template: String },
    Mcp { connection_id: McpConnectionId, prompt_name: String },
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct CommandFrontmatter {
    pub description: String,
    #[serde(alias = "tools_allowed")]
    pub allowed_tools: Option<Vec<String>>,
    pub model: Option<String>,
    pub argument_hints: Vec<String>,
    pub thinking: Option<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CommandSource { Builtin, User, Project, Local, Plugin, Managed, Mcp }

#[derive(Debug)]
pub enum CommandResult {
    Done { display: Option<String> },
    InjectMessage { content: String },
    EmitEffects { effects: Vec<Effect>, display: Option<String> },
    RequestConfirmation { prompt: String, on_confirm: Vec<Effect> },
}

#[async_trait::async_trait]
pub trait BuiltinCommandHandler: Send + Sync {
    async fn handle(&self, args: &ParsedSlashCommand) -> CommandResult;
    fn name(&self) -> &str;
    fn description(&self) -> &str;
}
```

- [ ] **Step 5: registry.rs**

```rust
use crate::model::*;
use lingxi_protocol::PluginId;
use std::collections::HashMap;
use std::sync::Arc;

pub struct CommandRegistry {
    commands: HashMap<String, SlashCommand>,
    aliases: HashMap<String, String>,
    builtin_handlers: HashMap<String, Arc<dyn BuiltinCommandHandler>>,
    plugin_commands: HashMap<PluginId, Vec<String>>,
}

impl CommandRegistry {
    pub fn new() -> Self {
        Self { commands: HashMap::new(), aliases: HashMap::new(),
               builtin_handlers: HashMap::new(), plugin_commands: HashMap::new() }
    }

    pub fn register_command(&mut self, cmd: SlashCommand) {
        self.commands.insert(cmd.name.clone(), cmd);
    }

    pub fn register_builtin_handler(&mut self, h: Arc<dyn BuiltinCommandHandler>) {
        let cmd = SlashCommand {
            name: h.name().to_string(),
            description: h.description().to_string(),
            source: CommandSource::Builtin,
            kind: SlashCommandKind::Builtin { handler_id: h.name().to_string() },
        };
        self.commands.insert(h.name().to_string(), cmd);
        self.builtin_handlers.insert(h.name().to_string(), h);
    }

    pub fn register_alias(&mut self, alias: String, target: String) {
        self.aliases.insert(alias, target);
    }

    pub fn resolve(&self, name: &str) -> Option<&SlashCommand> {
        let canon = self.aliases.get(name).map(|s| s.as_str()).unwrap_or(name);
        self.commands.get(canon)
    }

    pub fn get_handler(&self, handler_id: &str) -> Option<Arc<dyn BuiltinCommandHandler>> {
        self.builtin_handlers.get(handler_id).cloned()
    }

    pub fn register_plugin_commands(&mut self, plugin_id: PluginId, cmds: Vec<SlashCommand>) {
        let names: Vec<String> = cmds.iter().map(|c| c.name.clone()).collect();
        for c in cmds { self.commands.insert(c.name.clone(), c); }
        self.plugin_commands.insert(plugin_id, names);
    }

    pub fn unregister_plugin(&mut self, plugin_id: &PluginId) {
        if let Some(names) = self.plugin_commands.remove(plugin_id) {
            for n in &names { self.commands.remove(n); }
        }
    }
}

impl Default for CommandRegistry { fn default() -> Self { Self::new() } }
```

- [ ] **Step 6: builtin/mod.rs with representative handlers**

```rust
// builtin/mod.rs
pub mod help;
pub mod cost;
pub mod memory;
pub mod compact;
pub mod resume;
// ... 80+ in production; the rest are stubs in M1.15.
```

```rust
// builtin/help.rs
use crate::model::{BuiltinCommandHandler, CommandResult};
use crate::parser::ParsedSlashCommand;
use async_trait::async_trait;

pub struct HelpHandler;

#[async_trait]
impl BuiltinCommandHandler for HelpHandler {
    fn name(&self) -> &str { "help" }
    fn description(&self) -> &str { "Show available commands." }
    async fn handle(&self, _: &ParsedSlashCommand) -> CommandResult {
        CommandResult::Done { display: Some("see /commands for full list".into()) }
    }
}
```

```rust
// builtin/cost.rs
use crate::model::{BuiltinCommandHandler, CommandResult};
use crate::parser::ParsedSlashCommand;
use async_trait::async_trait;
use lingxi_protocol::Effect;

pub struct CostHandler;

#[async_trait]
impl BuiltinCommandHandler for CostHandler {
    fn name(&self) -> &str { "cost" }
    fn description(&self) -> &str { "Show session cost." }
    async fn handle(&self, _: &ParsedSlashCommand) -> CommandResult {
        CommandResult::EmitEffects {
            effects: vec![Effect::DisplayCostUpdate { snapshot_json: serde_json::json!({"placeholder": true}) }],
            display: None,
        }
    }
}
```

```rust
// builtin/{memory,compact,resume}.rs — analogous one-method handlers emitting effects.
```

- [ ] **Step 7: lib.rs + commit**

```rust
#![forbid(unsafe_code)]
pub mod argument_substitution;
pub mod builtin;
pub mod model;
pub mod parser;
pub mod registry;

pub use argument_substitution::substitute_arguments;
pub use model::*;
pub use parser::{parse_slash_command, ParsedSlashCommand};
pub use registry::CommandRegistry;
```

```bash
cargo test -p lingxi-commands
git add crates/commands
git commit -m "feat(commands): parser + arg substitution + registry + 5 builtin handlers"
```

---

## Task 3: lingxi-outputstyles

**Files:** `crates/outputstyles/{Cargo.toml, src/{lib,model,registry}.rs}`

- [ ] **Step 1: model.rs**

```rust
use lingxi_protocol::PluginId;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OutputStyle {
    pub name: String,
    pub description: String,
    pub source: OutputStyleSource,
    pub frontmatter: OutputStyleFrontmatter,
    pub system_prompt_addendum: String,
    pub source_path: Option<PathBuf>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct OutputStyleFrontmatter {
    pub name: String,
    pub description: String,
    pub default: bool,
    pub format: OutputFormat,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum OutputFormat {
    #[default]
    Markdown,
    Plain,
    JsonStream,
    Concise,
    Explanatory,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum OutputStyleSource { Builtin, User, Project, Plugin, Managed }
```

- [ ] **Step 2: registry.rs**

```rust
use crate::model::*;
use lingxi_protocol::PluginId;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use thiserror::Error;

pub struct OutputStyleRegistry {
    styles: HashMap<String, OutputStyle>,
    current_active: RwLock<String>,
    plugin_styles: HashMap<PluginId, Vec<String>>,
}

#[derive(Debug, Clone, Error)]
pub enum OutputStyleError {
    #[error("not found: {0}")]
    NotFound(String),
}

impl OutputStyleRegistry {
    pub fn new() -> Self {
        let mut s = HashMap::new();
        s.insert("markdown".into(), OutputStyle {
            name: "markdown".into(),
            description: "Default markdown output".into(),
            source: OutputStyleSource::Builtin,
            frontmatter: OutputStyleFrontmatter { name: "markdown".into(), description: "".into(), default: true, format: OutputFormat::Markdown },
            system_prompt_addendum: String::new(),
            source_path: None,
        });
        Self { styles: s, current_active: RwLock::new("markdown".into()), plugin_styles: HashMap::new() }
    }

    pub async fn active(&self) -> OutputStyle {
        let name = self.current_active.read().await.clone();
        self.styles.get(&name).cloned().expect("active style must exist")
    }

    pub async fn switch(&self, name: &str) -> Result<(), OutputStyleError> {
        if !self.styles.contains_key(name) {
            return Err(OutputStyleError::NotFound(name.into()));
        }
        *self.current_active.write().await = name.into();
        Ok(())
    }

    pub fn register(&mut self, style: OutputStyle) {
        self.styles.insert(style.name.clone(), style);
    }

    pub fn register_plugin_styles(&mut self, plugin_id: PluginId, styles: Vec<OutputStyle>) {
        let names: Vec<String> = styles.iter().map(|s| s.name.clone()).collect();
        for s in styles { self.styles.insert(s.name.clone(), s); }
        self.plugin_styles.insert(plugin_id, names);
    }

    pub fn unregister_plugin(&mut self, plugin_id: &PluginId) {
        if let Some(names) = self.plugin_styles.remove(plugin_id) {
            for n in &names { self.styles.remove(n); }
        }
    }
}

impl Default for OutputStyleRegistry { fn default() -> Self { Self::new() } }

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn default_is_markdown() {
        let r = OutputStyleRegistry::new();
        assert_eq!(r.active().await.name, "markdown");
    }

    #[tokio::test]
    async fn switch_to_unknown_errors() {
        let r = OutputStyleRegistry::new();
        assert!(r.switch("nope").await.is_err());
    }
}
```

- [ ] **Step 3: lib.rs + commit**

```rust
#![forbid(unsafe_code)]
pub mod model;
pub mod registry;
pub use model::*;
pub use registry::{OutputStyleError, OutputStyleRegistry};
```

```toml
[package]
name = "lingxi-outputstyles"
# ... usual deps + tokio sync + lingxi-protocol
```

```bash
cargo test -p lingxi-outputstyles
git add crates/outputstyles
git commit -m "feat(outputstyles): registry + builtin markdown default"
```

---

## Task 4: Plan exit

```bash
cargo test --workspace && cargo clippy --workspace --all-targets -- -D warnings
git tag -a m1.15-skills-cmd-styles -m "Plan 09 complete"
```

## Self-Review

- §18.1 Skill model with `allowed_tools` + alias → ✓
- §18.2 SkillRegistry + discover() → ✓
- §18.3 SkillTool via StateMachinePool (D5) → ✓
- §19.1 SlashCommand model → ✓
- §19.2 Parser + argument substitution → ✓
- §19.3 CommandRegistry → ✓
- §21.1 OutputStyle model → ✓
- §21.2 OutputStyleRegistry with active switching → ✓

## Execution Handoff

Next: **Plan 10 — Session + FileState + MessageQueue** (`2026-05-22-lingxi-core-m1-10-session-filestate-msgqueue.md`).
