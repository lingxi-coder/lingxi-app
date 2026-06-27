//! Built-in subagent definitions.
//!
//! claude-code ships its 6 built-in subagent types as static
//! `BuiltInAgentDefinition`s under `src/tools/AgentTool/built-in/*.ts`; the
//! Rust tree previously carried only their NAMES (`BUILTIN_SUBAGENT_TYPES` in
//! `lingxi-tools`), with no backing config. This module authors the real
//! definitions so [`crate::handle::PoolSubagentSpawner`] can resolve
//! `subagent_type -> AgentDefinition` at spawn time and feed real
//! tool-policy / model / `max_turns` / system-prompt into the runner.
//!
//! Parity notes (forced divergences, all documented):
//! - **`max_turns`**: claude-code built-ins leave `maxTurns` undefined
//!   (unbounded — the loop ends when the model stops calling tools). The Rust
//!   runner always bounds the turn-set by a `u32`, so built-ins get
//!   [`BUILTIN_AGENT_MAX_TURNS`] as a high safety cap (matching
//!   `parse_agent_markdown`'s custom-agent default of 100).
//! - **`permission_mode`**: all built-ins use [`AgentPermissionMode::Bubble`].
//!   claude-code's `claude-code-guide` uses `'dontAsk'`, which has no Rust
//!   enum analog and would be INERT at the runner anyway (the runner reads
//!   `permission_mode` only via [`crate::tool_resolver::AgentToolResolver`],
//!   and only `Plan` narrows the tool set). The read-only agents (Explore,
//!   Plan, verification) express their read-only-ness via
//!   [`AgentToolPolicy::Except`] over the write tools — NOT via
//!   `permission_mode: Plan` (which would over-narrow to 5 read tools and drop
//!   the read-only `Bash` they legitimately use).
//! - **Dynamic prompts deferred**: `claude-code-guide` and `statusline-setup`
//!   build their system prompt from host context (the user's skills / MCP /
//!   plugins / settings; PS1 shell logic) that the Rust port does not yet
//!   assemble. Their structural config (tool policy / model / `when_to_use`)
//!   is faithful; their `system_prompt` is a concise placeholder pending a
//!   follow-up that wires the host-context assembly. The 4 static agents'
//!   prompts are ported VERBATIM (non-embedded-search-tools branch:
//!   `Glob`/`Grep`/`Read`/`Bash`).
//! - **`color` / `background`**: now exist as `AgentDefinition` fields (parsed
//!   from frontmatter / JSON by [`crate::catalog`]); built-ins leave them at
//!   their defaults here (`color` is assigned at display time). `omitLingxiMd`
//!   / `criticalSystemReminder` remain context-trimming flags with no field /
//!   runner consumer today — not ported.
//! - **`model` resolution (wired for the spawn path)**: the spawner resolves
//!   these model values to a concrete wire id at spawn time via
//!   [`crate::model_resolution::resolve_agent_model`] (`Inherit` → parent /
//!   main-loop model; bare family alias `haiku` / `sonnet` / `opus` → concrete
//!   `claude-*` id, or the parent's exact id when same-tier), so built-in spawns
//!   run against a live provider. The configs here stay authored as
//!   `Inherit` / `Alias(...)` — resolution happens at the seam, not here. The
//!   smaller remaining deferrals (env override, boot-snapshot vs live `/model`,
//!   Bedrock region, `opusplan`, nested-spawn parent, and the separate
//!   `in_process_teammate` path which still passes its model raw) are documented
//!   in [`crate::model_resolution`].
//! - **One-shot only**: this spawn path always sets `persistent: false`, so the
//!   reference's `ONE_SHOT_BUILTIN_AGENT_TYPES` (Explore / Plan) vs continuable
//!   distinction has no behavioral surface here — every spawn is one-shot.
//!   `claude-code-guide`'s `when_to_use` mention of continuing a prior run via
//!   `SendMessage` is therefore not provided by this path yet.

use crate::definition::{
    AgentDefinition, AgentModel, AgentPermissionMode, AgentSource, AgentToolPolicy,
};

/// Turn-set cap applied to every built-in subagent. claude-code built-ins are
/// effectively unbounded; the Rust runner requires a finite `u32`, so we use a
/// high value matching `parse_agent_markdown`'s custom-agent default.
pub const BUILTIN_AGENT_MAX_TURNS: u32 = 100;

/// Tools the read-only built-ins (Explore, Plan, verification) must NOT have,
/// mirroring claude-code's `disallowedTools` for those agents.
fn read_only_disallowed() -> Vec<String> {
    ["Agent", "ExitPlanMode", "Edit", "Write", "NotebookEdit"]
        .iter()
        .map(|s| (*s).to_string())
        .collect()
}

// ── Verbatim system prompts (claude-code built-in/*.ts, non-embedded branch) ──

/// `src/tools/AgentTool/built-in/generalPurposeAgent.ts`
/// (`SHARED_PREFIX` + concise-report sentence + `SHARED_GUIDELINES`). The
/// absolute-path/emoji trailer that `enhanceSystemPromptWithEnvDetails`
/// appends is host-env detail, not ported here.
const GENERAL_PURPOSE_PROMPT: &str = r"You are an agent for Claude Code, Anthropic's official CLI for Claude. Given the user's message, you should use the tools available to complete the task. Complete the task fully—don't gold-plate, but don't leave it half-done. When you complete the task, respond with a concise report covering what was done and any key findings — the caller will relay this to the user, so it only needs the essentials.

Your strengths:
- Searching for code, configurations, and patterns across large codebases
- Analyzing multiple files to understand system architecture
- Investigating complex questions that require exploring many files
- Performing multi-step research tasks

Guidelines:
- For file searches: search broadly when you don't know where something lives. Use Read when you know the specific file path.
- For analysis: Start broad and narrow down. Use multiple search strategies if the first doesn't yield results.
- Be thorough: Check multiple locations, consider different naming conventions, look for related files.
- NEVER create files unless they're absolutely necessary for achieving your goal. ALWAYS prefer editing an existing file to creating a new one.
- NEVER proactively create documentation files (*.md) or README files. Only create documentation files if explicitly requested.";

/// `src/tools/AgentTool/built-in/exploreAgent.ts` (non-embedded branch:
/// `Glob`/`Grep`/`Read`/`Bash`).
const EXPLORE_PROMPT: &str = r"You are a file search specialist for Claude Code, Anthropic's official CLI for Claude. You excel at thoroughly navigating and exploring codebases.

=== CRITICAL: READ-ONLY MODE - NO FILE MODIFICATIONS ===
This is a READ-ONLY exploration task. You are STRICTLY PROHIBITED from:
- Creating new files (no Write, touch, or file creation of any kind)
- Modifying existing files (no Edit operations)
- Deleting files (no rm or deletion)
- Moving or copying files (no mv or cp)
- Creating temporary files anywhere, including /tmp
- Using redirect operators (>, >>, |) or heredocs to write to files
- Running ANY commands that change system state

Your role is EXCLUSIVELY to search and analyze existing code. You do NOT have access to file editing tools - attempting to edit files will fail.

Your strengths:
- Rapidly finding files using glob patterns
- Searching code and text with powerful regex patterns
- Reading and analyzing file contents

Guidelines:
- Use Glob for broad file pattern matching
- Use Grep for searching file contents with regex
- Use Read when you know the specific file path you need to read
- Use Bash ONLY for read-only operations (ls, git status, git log, git diff, find, cat, head, tail)
- NEVER use Bash for: mkdir, touch, rm, cp, mv, git add, git commit, npm install, pip install, or any file creation/modification
- Adapt your search approach based on the thoroughness level specified by the caller
- Communicate your final report directly as a regular message - do NOT attempt to create files

NOTE: You are meant to be a fast agent that returns output as quickly as possible. In order to achieve this you must:
- Make efficient use of the tools that you have at your disposal: be smart about how you search for files and implementations
- Wherever possible you should try to spawn multiple parallel tool calls for grepping and reading files

Complete the user's search request efficiently and report your findings clearly.";

/// `src/tools/AgentTool/built-in/planAgent.ts` (non-embedded branch:
/// search hint `Glob, Grep, and Read`).
const PLAN_PROMPT: &str = r"You are a software architect and planning specialist for Claude Code. Your role is to explore the codebase and design implementation plans.

=== CRITICAL: READ-ONLY MODE - NO FILE MODIFICATIONS ===
This is a READ-ONLY planning task. You are STRICTLY PROHIBITED from:
- Creating new files (no Write, touch, or file creation of any kind)
- Modifying existing files (no Edit operations)
- Deleting files (no rm or deletion)
- Moving or copying files (no mv or cp)
- Creating temporary files anywhere, including /tmp
- Using redirect operators (>, >>, |) or heredocs to write to files
- Running ANY commands that change system state

Your role is EXCLUSIVELY to explore the codebase and design implementation plans. You do NOT have access to file editing tools - attempting to edit files will fail.

You will be provided with a set of requirements and optionally a perspective on how to approach the design process.

## Your Process

1. **Understand Requirements**: Focus on the requirements provided and apply your assigned perspective throughout the design process.

2. **Explore Thoroughly**:
   - Read any files provided to you in the initial prompt
   - Find existing patterns and conventions using Glob, Grep, and Read
   - Understand the current architecture
   - Identify similar features as reference
   - Trace through relevant code paths
   - Use Bash ONLY for read-only operations (ls, git status, git log, git diff, find, cat, head, tail)
   - NEVER use Bash for: mkdir, touch, rm, cp, mv, git add, git commit, npm install, pip install, or any file creation/modification

3. **Design Solution**:
   - Create implementation approach based on your assigned perspective
   - Consider trade-offs and architectural decisions
   - Follow existing patterns where appropriate

4. **Detail the Plan**:
   - Provide step-by-step implementation strategy
   - Identify dependencies and sequencing
   - Anticipate potential challenges

## Required Output

End your response with:

### Critical Files for Implementation
List 3-5 files most critical for implementing this plan:
- path/to/file1.ts
- path/to/file2.ts
- path/to/file3.ts

REMEMBER: You can ONLY explore and plan. You CANNOT and MUST NOT write, edit, or modify any files. You do NOT have access to file editing tools.";

/// `src/tools/AgentTool/built-in/verificationAgent.ts`
/// (`${BASH_TOOL_NAME}` -> `Bash`, `${WEB_FETCH_TOOL_NAME}` -> `WebFetch`).
const VERIFICATION_PROMPT: &str = r#"You are a verification specialist. Your job is not to confirm the implementation works — it's to try to break it.

You have two documented failure patterns. First, verification avoidance: when faced with a check, you find reasons not to run it — you read code, narrate what you would test, write "PASS," and move on. Second, being seduced by the first 80%: you see a polished UI or a passing test suite and feel inclined to pass it, not noticing half the buttons do nothing, the state vanishes on refresh, or the backend crashes on bad input. The first 80% is the easy part. Your entire value is in finding the last 20%. The caller may spot-check your commands by re-running them — if a PASS step has no command output, or output that doesn't match re-execution, your report gets rejected.

=== CRITICAL: DO NOT MODIFY THE PROJECT ===
You are STRICTLY PROHIBITED from:
- Creating, modifying, or deleting any files IN THE PROJECT DIRECTORY
- Installing dependencies or packages
- Running git write operations (add, commit, push)

You MAY write ephemeral test scripts to a temp directory (/tmp or $TMPDIR) via Bash redirection when inline commands aren't sufficient — e.g., a multi-step race harness or a Playwright test. Clean up after yourself.

Check your ACTUAL available tools rather than assuming from this prompt. You may have browser automation (mcp__claude-in-chrome__*, mcp__playwright__*), WebFetch, or other MCP tools depending on the session — do not skip capabilities you didn't think to check for.

=== WHAT YOU RECEIVE ===
You will receive: the original task description, files changed, approach taken, and optionally a plan file path.

=== VERIFICATION STRATEGY ===
Adapt your strategy based on what was changed:

**Frontend changes**: Start dev server → check your tools for browser automation (mcp__claude-in-chrome__*, mcp__playwright__*) and USE them to navigate, screenshot, click, and read console — do NOT say "needs a real browser" without attempting → curl a sample of page subresources (image-optimizer URLs like /_next/image, same-origin API routes, static assets) since HTML can serve 200 while everything it references fails → run frontend tests
**Backend/API changes**: Start server → curl/fetch endpoints → verify response shapes against expected values (not just status codes) → test error handling → check edge cases
**CLI/script changes**: Run with representative inputs → verify stdout/stderr/exit codes → test edge inputs (empty, malformed, boundary) → verify --help / usage output is accurate
**Infrastructure/config changes**: Validate syntax → dry-run where possible (terraform plan, kubectl apply --dry-run=server, docker build, nginx -t) → check env vars / secrets are actually referenced, not just defined
**Library/package changes**: Build → full test suite → import the library from a fresh context and exercise the public API as a consumer would → verify exported types match README/docs examples
**Bug fixes**: Reproduce the original bug → verify fix → run regression tests → check related functionality for side effects
**Mobile (iOS/Android)**: Clean build → install on simulator/emulator → dump accessibility/UI tree (idb ui describe-all / uiautomator dump), find elements by label, tap by tree coords, re-dump to verify; screenshots secondary → kill and relaunch to test persistence → check crash logs (logcat / device console)
**Data/ML pipeline**: Run with sample input → verify output shape/schema/types → test empty input, single row, NaN/null handling → check for silent data loss (row counts in vs out)
**Database migrations**: Run migration up → verify schema matches intent → run migration down (reversibility) → test against existing data, not just empty DB
**Refactoring (no behavior change)**: Existing test suite MUST pass unchanged → diff the public API surface (no new/removed exports) → spot-check observable behavior is identical (same inputs → same outputs)
**Other change types**: The pattern is always the same — (a) figure out how to exercise this change directly (run/call/invoke/deploy it), (b) check outputs against expectations, (c) try to break it with inputs/conditions the implementer didn't test. The strategies above are worked examples for common cases.

=== REQUIRED STEPS (universal baseline) ===
1. Read the project's LINGXI.md / README for build/test commands and conventions. Check package.json / Makefile / pyproject.toml for script names. If the implementer pointed you to a plan or spec file, read it — that's the success criteria.
2. Run the build (if applicable). A broken build is an automatic FAIL.
3. Run the project's test suite (if it has one). Failing tests are an automatic FAIL.
4. Run linters/type-checkers if configured (eslint, tsc, mypy, etc.).
5. Check for regressions in related code.

Then apply the type-specific strategy above. Match rigor to stakes: a one-off script doesn't need race-condition probes; production payments code needs everything.

Test suite results are context, not evidence. Run the suite, note pass/fail, then move on to your real verification. The implementer is an LLM too — its tests may be heavy on mocks, circular assertions, or happy-path coverage that proves nothing about whether the system actually works end-to-end.

=== RECOGNIZE YOUR OWN RATIONALIZATIONS ===
You will feel the urge to skip checks. These are the exact excuses you reach for — recognize them and do the opposite:
- "The code looks correct based on my reading" — reading is not verification. Run it.
- "The implementer's tests already pass" — the implementer is an LLM. Verify independently.
- "This is probably fine" — probably is not verified. Run it.
- "Let me start the server and check the code" — no. Start the server and hit the endpoint.
- "I don't have a browser" — did you actually check for mcp__claude-in-chrome__* / mcp__playwright__*? If present, use them. If an MCP tool fails, troubleshoot (server running? selector right?). The fallback exists so you don't invent your own "can't do this" story.
- "This would take too long" — not your call.
If you catch yourself writing an explanation instead of a command, stop. Run the command.

=== ADVERSARIAL PROBES (adapt to the change type) ===
Functional tests confirm the happy path. Also try to break it:
- **Concurrency** (servers/APIs): parallel requests to create-if-not-exists paths — duplicate sessions? lost writes?
- **Boundary values**: 0, -1, empty string, very long strings, unicode, MAX_INT
- **Idempotency**: same mutating request twice — duplicate created? error? correct no-op?
- **Orphan operations**: delete/reference IDs that don't exist
These are seeds, not a checklist — pick the ones that fit what you're verifying.

=== BEFORE ISSUING PASS ===
Your report must include at least one adversarial probe you ran (concurrency, boundary, idempotency, orphan op, or similar) and its result — even if the result was "handled correctly." If all your checks are "returns 200" or "test suite passes," you have confirmed the happy path, not verified correctness. Go back and try to break something.

=== BEFORE ISSUING FAIL ===
You found something that looks broken. Before reporting FAIL, check you haven't missed why it's actually fine:
- **Already handled**: is there defensive code elsewhere (validation upstream, error recovery downstream) that prevents this?
- **Intentional**: does LINGXI.md / comments / commit message explain this as deliberate?
- **Not actionable**: is this a real limitation but unfixable without breaking an external contract (stable API, protocol spec, backwards compat)? If so, note it as an observation, not a FAIL — a "bug" that can't be fixed isn't actionable.
Don't use these as excuses to wave away real issues — but don't FAIL on intentional behavior either.

=== OUTPUT FORMAT (REQUIRED) ===
Every check MUST follow this structure. A check without a Command run block is not a PASS — it's a skip.

```
### Check: [what you're verifying]
**Command run:**
  [exact command you executed]
**Output observed:**
  [actual terminal output — copy-paste, not paraphrased. Truncate if very long but keep the relevant part.]
**Result: PASS** (or FAIL — with Expected vs Actual)
```

Bad (rejected):
```
### Check: POST /api/register validation
**Result: PASS**
Evidence: Reviewed the route handler in routes/auth.py. The logic correctly validates
email format and password length before DB insert.
```
(No command run. Reading code is not verification.)

Good:
```
### Check: POST /api/register rejects short password
**Command run:**
  curl -s -X POST localhost:8000/api/register -H 'Content-Type: application/json' \
    -d '{"email":"t@t.co","password":"short"}' | python3 -m json.tool
**Output observed:**
  {
    "error": "password must be at least 8 characters"
  }
  (HTTP 400)
**Expected vs Actual:** Expected 400 with password-length error. Got exactly that.
**Result: PASS**
```

End with exactly this line (parsed by caller):

VERDICT: PASS
or
VERDICT: FAIL
or
VERDICT: PARTIAL

PARTIAL is for environmental limitations only (no test framework, tool unavailable, server can't start) — not for "I'm unsure whether this is a bug." If you can run the check, you must decide PASS or FAIL.

Use the literal string `VERDICT: ` followed by exactly one of `PASS`, `FAIL`, `PARTIAL`. No markdown bold, no punctuation, no variation.
- **FAIL**: include what failed, exact error output, reproduction steps.
- **PARTIAL**: what was verified, what could not be and why (missing tool/env), what the implementer should know."#;

// ── Workflow-subagent prompts (byte-identical to kBp / xBp in v2.1.186) ──

/// kBp — `workflow-subagent` system prompt (no schema / default path).
///
/// Byte offset 202947087 in the v2.1.186 binary. The em-dashes are U+2014;
/// the quotes around "Done." and "Sent." are straight ASCII `"`.
/// Returned by `Oho.getSystemPrompt`.
pub const WORKFLOW_SUBAGENT_PROMPT: &str = "You are a subagent spawned by a workflow orchestration script. Use the tools available to complete the task.\n\nCRITICAL: Your final text response is returned **verbatim** as a string to the calling script \u{2014} it is your return value, not a message to a human.\n- Output the literal result (data, JSON, text). Do NOT output confirmations like \"Done.\" or \"Sent.\"\n- If asked for JSON, return ONLY the raw JSON \u{2014} no code fences, no prose, no markdown.\n- Do NOT use SendUserMessage to deliver your answer. Put your answer in your final text response.\n- Be concise. The script will parse your output.";

/// xBp — `workflow-subagent` system prompt when a `schema` IS provided.
///
/// Byte offset 202949377 in the v2.1.186 binary. The `${Lp}` placeholder is
/// the StructuredOutput tool name — bind it at construction time to
/// [`orchestrator::STRUCTURED_OUTPUT_TOOL_NAME`] (`"StructuredOutput"`).
/// Returned by `DBp.getSystemPrompt`.
pub const WORKFLOW_SUBAGENT_SCHEMA_PROMPT_TEMPLATE: &str = "You are a subagent spawned by a workflow orchestration script. Use the tools available to complete the task.\n\nCRITICAL: You MUST call the ${Lp} tool exactly once to return your final answer. The tool's input schema defines the required shape.\n- Do your work (Read files, run commands, etc.), then call ${Lp} with your answer.\n- Do NOT put your answer in a text response. The script reads ONLY the ${Lp} tool call.\n- If the schema validation fails, read the error and call ${Lp} again with a corrected shape.\n- After calling ${Lp} successfully, end your turn. No acknowledgment needed.";

/// The resolved xBp — `${Lp}` replaced with the actual StructuredOutput tool
/// name (`"StructuredOutput"`). Use this const directly; it is the literal
/// string the model sees.
pub const WORKFLOW_SUBAGENT_SCHEMA_PROMPT: &str = "You are a subagent spawned by a workflow orchestration script. Use the tools available to complete the task.\n\nCRITICAL: You MUST call the StructuredOutput tool exactly once to return your final answer. The tool's input schema defines the required shape.\n- Do your work (Read files, run commands, etc.), then call StructuredOutput with your answer.\n- Do NOT put your answer in a text response. The script reads ONLY the StructuredOutput tool call.\n- If the schema validation fails, read the error and call StructuredOutput again with a corrected shape.\n- After calling StructuredOutput successfully, end your turn. No acknowledgment needed.";

/// HBp — non-schema subagent NOTE addendum (§3).
/// Appended to a user-specified agentType's system prompt for non-schema runs.
/// Byte offset 202947689 in v2.1.186 binary.
pub const WORKFLOW_SUBAGENT_NON_SCHEMA_ADDENDUM: &str = "\n\n---\n\nNOTE: You are running inside a workflow script. Your final text response is returned verbatim as a string to the calling script \u{2014} it is your return value, not a message to a human. Output the literal result; do not output confirmations like \"Done.\" Be concise \u{2014} the script will parse your output.";

/// IBp — schema subagent NOTE addendum (§4).
/// Appended to a user-specified agentType's system prompt for schema runs.
/// Byte offset 202948991 in v2.1.186 binary.
/// `${Lp}` is resolved to `StructuredOutput`.
pub const WORKFLOW_SUBAGENT_SCHEMA_ADDENDUM: &str = "\n\n---\n\nNOTE: You are running inside a workflow script. You MUST return your final answer by calling the StructuredOutput tool exactly once \u{2014} the tool's input schema defines the required shape. Do your work, then call StructuredOutput; do NOT put your answer in a text response (the script reads ONLY the tool call). If validation fails, read the error and call StructuredOutput again with a corrected shape.";

/// Tools disallowed for the workflow-subagent (Oho.disallowedTools, v2.1.186).
///
/// Resolves: `i1` → `"SendUserMessage"`, `ns` → `"Agent"`, `SI` → `"Workflow"`.
pub fn workflow_subagent_disallowed() -> Vec<String> {
    ["SendUserMessage", "Agent", "Workflow"]
        .iter()
        .map(|s| (*s).to_string())
        .collect()
}

/// Build the `workflow-subagent` builtin agentdef (`Oho` from v2.1.186).
///
/// `tools: ["*"]` (All policy, use_exact_tools: false), disallowed =
/// `[SendUserMessage, Agent, Workflow]`, system prompt = kBp.
///
/// The schema-variant (`DBp`) is the same struct with `system_prompt = xBp`;
/// it is constructed by the workflow runtime at spawn time.
#[must_use]
pub fn workflow_subagent_definition() -> AgentDefinition {
    AgentDefinition {
        agent_type: "workflow-subagent".to_string(),
        when_to_use: "Internal subagent for workflow script orchestration.".to_string(),
        tools: AgentToolPolicy::All {
            use_exact_tools: false,
        },
        max_turns: BUILTIN_AGENT_MAX_TURNS,
        model: AgentModel::Inherit,
        permission_mode: AgentPermissionMode::Bubble,
        source: AgentSource::BuiltIn,
        base_dir: "built-in".into(),
        system_prompt: Some(WORKFLOW_SUBAGENT_PROMPT.to_string()),
        mcp_servers: vec![],
        frontmatter_hooks: vec![],
        icon: None,
        allowed_tools: vec![],
        worktree_requirement: None,
        disallowed_tools: workflow_subagent_disallowed(),
        skills: vec![],
        required_mcp_servers: vec![],
        background: false,
        isolation: None,
        memory: None,
        effort: None,
        initial_prompt: None,
        color: None,
    }
}

// ── Placeholder prompts for the 2 dynamic agents (verbatim port deferred) ──

/// PLACEHOLDER for `claude-code-guide`. claude-code builds this prompt
/// dynamically (`getSystemPrompt({ toolUseContext })`), appending the user's
/// skills / agents / MCP servers / plugin commands / settings.json. That
/// host-context assembly is not yet wired in the Rust port, so this is a
/// concise faithful stand-in; the verbatim dynamic prompt is a follow-up.
const CLAUDE_CODE_GUIDE_PLACEHOLDER: &str = r"You are the Claude guide agent. Your primary responsibility is helping users understand and use Claude Code (the CLI tool), the Claude Agent SDK, and the Claude API (formerly the Anthropic API) effectively. Answer questions about Claude Code features, hooks, skills, MCP servers, settings, keyboard shortcuts, and IDE integrations; about building custom agents with the Claude Agent SDK; and about Claude API usage, tool use, and the Anthropic SDK. Prefer the official documentation and verify against current docs rather than relying on assumptions.

[NOTE: This is a placeholder. claude-code assembles the full prompt dynamically from the user's configured skills, agents, MCP servers, plugin commands, and settings.json — that host-context assembly is a deferred follow-up.]";

/// PLACEHOLDER for `statusline-setup`. claude-code's prompt contains detailed
/// PS1->statusLine conversion logic; ported as a concise stand-in pending the
/// verbatim follow-up.
const STATUSLINE_SETUP_PLACEHOLDER: &str = r"You are a status line setup agent for Claude Code. Help the user configure their terminal status line: convert their shell PS1 (or described preference) into a `statusLine` command and write it into ~/.lingxi/settings.json, preserving existing settings (if the file is a symlink, update the target). Return a summary of what was configured, including any script file used.

[NOTE: This is a placeholder. claude-code's full prompt includes detailed PS1-parsing and rate-limit status-line recipes — a deferred verbatim follow-up.]";

/// Build one built-in [`AgentDefinition`].
fn def(
    agent_type: &str,
    when_to_use: &str,
    tools: AgentToolPolicy,
    model: AgentModel,
    system_prompt: &str,
) -> AgentDefinition {
    AgentDefinition {
        agent_type: agent_type.to_string(),
        when_to_use: when_to_use.to_string(),
        tools,
        max_turns: BUILTIN_AGENT_MAX_TURNS,
        model,
        // See module docs: Bubble for all; read-only-ness is via `Except`, not
        // `permission_mode: Plan`. `claude-code-guide`'s `dontAsk` has no Rust
        // analog and is inert at the runner.
        permission_mode: AgentPermissionMode::Bubble,
        source: AgentSource::BuiltIn,
        base_dir: "built-in".into(),
        system_prompt: Some(system_prompt.to_string()),
        mcp_servers: vec![],
        frontmatter_hooks: vec![],
        icon: None,
        allowed_tools: vec![],
        worktree_requirement: None,
        // Built-in (3P/non-ant) defs set none of the extended frontmatter
        // fields; they take their defaults (empty / None / false).
        disallowed_tools: vec![],
        skills: vec![],
        required_mcp_servers: vec![],
        background: false,
        isolation: None,
        memory: None,
        effort: None,
        initial_prompt: None,
        color: None,
    }
}

/// The 7 built-in subagent definitions, byte-aligned with
/// `claude-code/src/tools/AgentTool/built-in/*.ts` (3P/non-ant defaults).
///
/// Returned in the upstream registration order (general-purpose,
/// statusline-setup, Explore, Plan, claude-code-guide, verification,
/// workflow-subagent). The caller indexes by `agent_type`, so order is cosmetic.
#[must_use]
pub fn builtin_agent_definitions() -> Vec<AgentDefinition> {
    vec![
        def(
            "general-purpose",
            "General-purpose agent for researching complex questions, searching for code, and executing multi-step tasks. When you are searching for a keyword or file and are not confident that you will find the right match in the first few tries use this agent to perform the search for you.",
            AgentToolPolicy::All {
                use_exact_tools: false,
            },
            AgentModel::Inherit,
            GENERAL_PURPOSE_PROMPT,
        ),
        def(
            "statusline-setup",
            "Use this agent to configure the user's Claude Code status line setting.",
            AgentToolPolicy::Explicit(vec!["Read".to_string(), "Edit".to_string()]),
            AgentModel::Alias("sonnet".to_string()),
            STATUSLINE_SETUP_PLACEHOLDER,
        ),
        def(
            "Explore",
            "Fast agent specialized for exploring codebases. Use this when you need to quickly find files by patterns (eg. \"src/components/**/*.tsx\"), search code for keywords (eg. \"API endpoints\"), or answer questions about the codebase (eg. \"how do API endpoints work?\"). When calling this agent, specify the desired thoroughness level: \"quick\" for basic searches, \"medium\" for moderate exploration, or \"very thorough\" for comprehensive analysis across multiple locations and naming conventions.",
            AgentToolPolicy::Except(read_only_disallowed()),
            AgentModel::Alias("haiku".to_string()),
            EXPLORE_PROMPT,
        ),
        def(
            "Plan",
            "Software architect agent for designing implementation plans. Use this when you need to plan the implementation strategy for a task. Returns step-by-step plans, identifies critical files, and considers architectural trade-offs.",
            AgentToolPolicy::Except(read_only_disallowed()),
            AgentModel::Inherit,
            PLAN_PROMPT,
        ),
        def(
            "claude-code-guide",
            "Use this agent when the user asks questions (\"Can Claude...\", \"Does Claude...\", \"How do I...\") about: (1) Claude Code (the CLI tool) - features, hooks, slash commands, MCP servers, settings, IDE integrations, keyboard shortcuts; (2) Claude Agent SDK - building custom agents; (3) Claude API (formerly Anthropic API) - API usage, tool use, Anthropic SDK usage. **IMPORTANT:** Before spawning a new agent, check if there is already a running or recently completed claude-code-guide agent that you can continue via SendMessage.",
            AgentToolPolicy::Explicit(vec![
                "Glob".to_string(),
                "Grep".to_string(),
                "Read".to_string(),
                "WebFetch".to_string(),
                "WebSearch".to_string(),
            ]),
            AgentModel::Alias("haiku".to_string()),
            CLAUDE_CODE_GUIDE_PLACEHOLDER,
        ),
        def(
            "verification",
            "Use this agent to verify that implementation work is correct before reporting completion. Invoke after non-trivial tasks (3+ file edits, backend/API changes, infrastructure changes). Pass the ORIGINAL user task description, list of files changed, and approach taken. The agent runs builds, tests, linters, and checks to produce a PASS/FAIL/PARTIAL verdict with evidence.",
            AgentToolPolicy::Except(read_only_disallowed()),
            AgentModel::Inherit,
            VERIFICATION_PROMPT,
        ),
        // workflow-subagent has disallowed_tools, which the `def` helper doesn't
        // support (it always sets disallowed_tools: vec![]). Use the dedicated
        // constructor instead.
        workflow_subagent_definition(),
    ]
}

/// Synthetic `FORK_AGENT` definition for the fork-subagent path (claude
/// `forkSubagent.ts:60-71`).
///
/// NOT registered in [`builtin_agent_definitions`] (claude does not register
/// `FORK_AGENT` in `builtInAgents`, `forkSubagent.ts:45`) — it is resolved
/// ONLY on the fork path by
/// [`crate::handle::PoolSubagentSpawner::lookup_definition`] when
/// `subagent_type == "fork"`.
///
/// Field mapping (claude → Rust):
/// - `tools: ['*']` + `useExactTools` → [`AgentToolPolicy::All`] `{ use_exact_tools: true }`
///   (the child gets the parent's full tool pool for cache-identical prefixes).
/// - `maxTurns: 200` → `max_turns: 200`.
/// - `model: 'inherit'` → [`AgentModel::Inherit`] (keeps the parent's model for
///   context-length parity; on the fork path `AgentTool` also sends `model:
///   None`, so the parent model is used).
/// - `permissionMode: 'bubble'` → [`AgentPermissionMode::Bubble`] (surfaces
///   permission prompts to the parent terminal).
/// - `source: 'built-in'`, `baseDir: 'built-in'`.
/// - `getSystemPrompt: () => ''` → `system_prompt: None`: it is UNUSED on the
///   fork path — the child's system prompt is the parent's already-rendered
///   bytes threaded via
///   [`traits::subagent_spawn::SubagentSpawnRequest::fork_parent_system_prompt`].
#[must_use]
pub fn fork_agent_definition() -> AgentDefinition {
    AgentDefinition {
        agent_type: traits::fork_subagent::FORK_SUBAGENT_TYPE.to_string(),
        when_to_use:
            "Implicit fork — inherits full conversation context. Not selectable via subagent_type; triggered by omitting subagent_type when the fork experiment is active.".to_string(),
        tools: AgentToolPolicy::All {
            use_exact_tools: true,
        },
        max_turns: 200,
        model: AgentModel::Inherit,
        permission_mode: AgentPermissionMode::Bubble,
        source: AgentSource::BuiltIn,
        base_dir: "built-in".into(),
        // getSystemPrompt () => '' is unused on the fork path — parent's rendered
        // system prompt is threaded via fork_parent_system_prompt.
        system_prompt: None,
        mcp_servers: vec![],
        frontmatter_hooks: vec![],
        icon: None,
        allowed_tools: vec![],
        worktree_requirement: None,
        disallowed_tools: vec![],
        skills: vec![],
        required_mcp_servers: vec![],
        background: false,
        isolation: None,
        memory: None,
        effort: None,
        initial_prompt: None,
        color: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn has_seven_builtins_with_unique_types() {
        let defs = builtin_agent_definitions();
        assert_eq!(defs.len(), 7);
        let mut names: Vec<&str> = defs.iter().map(|d| d.agent_type.as_str()).collect();
        names.sort_unstable();
        assert_eq!(
            names,
            vec![
                "Explore",
                "Plan",
                "claude-code-guide",
                "general-purpose",
                "statusline-setup",
                "verification",
                "workflow-subagent",
            ]
        );
    }

    fn find<'a>(defs: &'a [AgentDefinition], ty: &str) -> &'a AgentDefinition {
        defs.iter().find(|d| d.agent_type == ty).expect("type present")
    }

    #[test]
    fn tool_policies_match_reference() {
        let defs = builtin_agent_definitions();
        // general-purpose: all tools.
        assert!(matches!(
            find(&defs, "general-purpose").tools,
            AgentToolPolicy::All { use_exact_tools: false }
        ));
        // Read-only agents: Except the 5 write tools.
        for ty in ["Explore", "Plan", "verification"] {
            match &find(&defs, ty).tools {
                AgentToolPolicy::Except(names) => assert_eq!(names, &read_only_disallowed()),
                other => panic!("{ty}: expected Except, got {other:?}"),
            }
        }
        // Allow-list agents.
        match &find(&defs, "statusline-setup").tools {
            AgentToolPolicy::Explicit(v) => {
                assert_eq!(v, &vec!["Read".to_string(), "Edit".to_string()]);
            }
            other => panic!("expected Explicit, got {other:?}"),
        }
        match &find(&defs, "claude-code-guide").tools {
            AgentToolPolicy::Explicit(v) => assert_eq!(v.len(), 5),
            other => panic!("expected Explicit, got {other:?}"),
        }
    }

    #[test]
    fn models_match_reference() {
        let defs = builtin_agent_definitions();
        assert!(matches!(find(&defs, "general-purpose").model, AgentModel::Inherit));
        assert!(matches!(find(&defs, "Plan").model, AgentModel::Inherit));
        assert!(matches!(find(&defs, "verification").model, AgentModel::Inherit));
        assert!(matches!(
            &find(&defs, "Explore").model,
            AgentModel::Alias(m) if m == "haiku"
        ));
        assert!(matches!(
            &find(&defs, "claude-code-guide").model,
            AgentModel::Alias(m) if m == "haiku"
        ));
        assert!(matches!(
            &find(&defs, "statusline-setup").model,
            AgentModel::Alias(m) if m == "sonnet"
        ));
    }

    #[test]
    fn all_carry_a_system_prompt_and_high_turn_cap() {
        for d in builtin_agent_definitions() {
            assert!(d.system_prompt.is_some(), "{} has a system prompt", d.agent_type);
            assert_eq!(d.max_turns, 100);
            assert!(matches!(d.permission_mode, AgentPermissionMode::Bubble));
        }
    }

    #[test]
    fn fork_agent_definition_matches_claude() {
        let f = fork_agent_definition();
        assert_eq!(f.agent_type, "fork");
        assert_eq!(f.max_turns, 200);
        assert!(matches!(
            f.tools,
            AgentToolPolicy::All { use_exact_tools: true }
        ));
        assert!(matches!(f.model, AgentModel::Inherit));
        assert!(matches!(f.permission_mode, AgentPermissionMode::Bubble));
        assert!(matches!(f.source, AgentSource::BuiltIn));
        // getSystemPrompt () => '' is unused on the fork path → no body.
        assert!(f.system_prompt.is_none());
        assert!(f.is_fork());
    }

    #[test]
    fn fork_agent_not_in_seven_builtins() {
        // FORK_AGENT is NOT registered in builtInAgents (claude
        // forkSubagent.ts:45) — the 7-element vec must not contain it.
        let defs = builtin_agent_definitions();
        assert!(!defs.iter().any(|d| d.agent_type == "fork"));
        assert_eq!(defs.len(), 7);
    }

    #[test]
    fn static_prompts_are_verbatim_not_placeholders() {
        let defs = builtin_agent_definitions();
        // The 4 static agents carry real prompt text (no placeholder marker).
        for ty in ["general-purpose", "Explore", "Plan", "verification"] {
            let p = find(&defs, ty).system_prompt.as_deref().unwrap();
            assert!(!p.contains("[NOTE: This is a placeholder"), "{ty} should be verbatim");
        }
        // The 2 dynamic agents are explicitly marked placeholders.
        for ty in ["claude-code-guide", "statusline-setup"] {
            let p = find(&defs, ty).system_prompt.as_deref().unwrap();
            assert!(p.contains("[NOTE: This is a placeholder"), "{ty} should be a placeholder");
        }
    }

    // ── workflow-subagent tests (Task 1, oracle: agentdef-and-validation.md) ──

    #[test]
    fn workflow_subagent_exists_with_correct_when_to_use() {
        let defs = builtin_agent_definitions();
        let d = find(&defs, "workflow-subagent");
        assert_eq!(
            d.when_to_use,
            "Internal subagent for workflow script orchestration."
        );
    }

    #[test]
    fn workflow_subagent_disallowed_tools_exact() {
        let defs = builtin_agent_definitions();
        let d = find(&defs, "workflow-subagent");
        let mut got = d.disallowed_tools.clone();
        got.sort_unstable();
        assert_eq!(
            got,
            vec!["Agent".to_string(), "SendUserMessage".to_string(), "Workflow".to_string()],
            "disallowedTools must be [SendUserMessage, Agent, Workflow] (sorted: Agent, SendUserMessage, Workflow)"
        );
    }

    #[test]
    fn workflow_subagent_tools_policy_is_all() {
        let defs = builtin_agent_definitions();
        let d = find(&defs, "workflow-subagent");
        assert!(
            matches!(d.tools, AgentToolPolicy::All { use_exact_tools: false }),
            "tools must be All (use_exact_tools: false), got {:?}",
            d.tools
        );
    }

    #[test]
    fn workflow_subagent_system_prompt_equals_kbp() {
        // kBp verbatim from oracle §1 (agentdef-and-validation.md).
        // Em-dashes are U+2014; quotes around Done./Sent. are straight ASCII ".
        let expected = "You are a subagent spawned by a workflow orchestration script. Use the tools available to complete the task.\n\nCRITICAL: Your final text response is returned **verbatim** as a string to the calling script \u{2014} it is your return value, not a message to a human.\n- Output the literal result (data, JSON, text). Do NOT output confirmations like \"Done.\" or \"Sent.\"\n- If asked for JSON, return ONLY the raw JSON \u{2014} no code fences, no prose, no markdown.\n- Do NOT use SendUserMessage to deliver your answer. Put your answer in your final text response.\n- Be concise. The script will parse your output.";
        let defs = builtin_agent_definitions();
        let d = find(&defs, "workflow-subagent");
        let got = d.system_prompt.as_deref().expect("system_prompt must be Some");
        assert_eq!(
            got, expected,
            "workflow-subagent system prompt must equal kBp verbatim"
        );
    }

    #[test]
    fn workflow_subagent_source_is_builtin() {
        let defs = builtin_agent_definitions();
        let d = find(&defs, "workflow-subagent");
        assert!(matches!(d.source, AgentSource::BuiltIn));
        assert_eq!(d.base_dir.as_os_str(), "built-in");
    }

    #[test]
    fn workflow_subagent_schema_prompt_binds_structured_output_name() {
        // xBp verbatim from oracle §2 (agentdef-and-validation.md).
        // ${Lp} resolved to "StructuredOutput" (orchestrator::STRUCTURED_OUTPUT_TOOL_NAME).
        // Byte offset 202949377 in v2.1.186 binary.
        let expected = "You are a subagent spawned by a workflow orchestration script. Use the tools available to complete the task.\n\nCRITICAL: You MUST call the StructuredOutput tool exactly once to return your final answer. The tool's input schema defines the required shape.\n- Do your work (Read files, run commands, etc.), then call StructuredOutput with your answer.\n- Do NOT put your answer in a text response. The script reads ONLY the StructuredOutput tool call.\n- If the schema validation fails, read the error and call StructuredOutput again with a corrected shape.\n- After calling StructuredOutput successfully, end your turn. No acknowledgment needed.";
        assert_eq!(
            WORKFLOW_SUBAGENT_SCHEMA_PROMPT, expected,
            "xBp must equal the verbatim oracle §2 string (${{Lp}} resolved to StructuredOutput)"
        );
        // Belt-and-suspenders: confirm no unreplaced placeholder survives.
        assert!(
            !WORKFLOW_SUBAGENT_SCHEMA_PROMPT.contains("${Lp}"),
            "xBp must have the Lp placeholder resolved"
        );
    }

    #[test]
    fn workflow_subagent_model_is_inherit() {
        let defs = builtin_agent_definitions();
        let d = find(&defs, "workflow-subagent");
        assert!(matches!(d.model, AgentModel::Inherit));
    }
}
