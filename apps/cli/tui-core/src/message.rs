//! Backend-neutral message model for TUI scrollback rendering.

/// A rendered message in the scrollback buffer.
#[derive(Debug, Clone)]
pub enum RenderedMessage {
    /// User-submitted prompt (rendered with `> ` prefix, default fg color).
    UserText {
        /// The text body the user submitted.
        body: String,
        /// Unix-seconds timestamp at the moment of render.
        timestamp: i64,
    },
    /// Assistant response (rendered with `● ` prefix, cyan).
    AssistantText {
        /// The assistant body text.
        body: String,
        /// Unix-seconds timestamp.
        timestamp: i64,
    },
    /// System message (slash command output, hints, errors). Rendered in
    /// dim grey or red depending on `is_error`.
    SystemText {
        /// The system message body.
        body: String,
        /// Unix-seconds timestamp.
        timestamp: i64,
        /// `true` → render red; `false` → render dim grey.
        is_error: bool,
    },
    /// Assistant tool-use block (M6-04). Renders as `● Tool({input_preview})`
    /// when collapsed, or as `● Tool(...)\n<pretty json>` when expanded.
    /// Per-id expanded state lives in `AppState.expanded`.
    AssistantToolUse {
        /// Correlator (model-supplied `tool_use_id`).
        id: lingxi_core::types::ToolUseId,
        /// Tool name (e.g. `"Read"`, `"Bash"`).
        tool: String,
        /// JSON input the tool was invoked with.
        input: serde_json::Value,
    },
    /// User-side tool result (M6-04). Renders with `└ ` prefix and
    /// dim-colored body. Line/byte truncation (100 lines / 4000 bytes)
    /// kicks in when expanded; the collapsed form shows only the first
    /// line plus a `(+N lines)` suffix.
    UserToolResult {
        /// Correlator matching the paired `AssistantToolUse.id`.
        id: lingxi_core::types::ToolUseId,
        /// Tool name (used to gate Bash → ANSI parser).
        tool: String,
        /// JSON result payload.
        result: serde_json::Value,
        /// (M7-02) Pre-edit text for diff tools (`old_string` for Edit; `None`
        /// for Write/pure-add). Populated from the paired `ToolUseStart` input
        /// at construction time; `None` for non-diff tools.
        old_string: Option<String>,
        /// (M7-02) Post-edit text for diff tools (`new_string` for Edit;
        /// `content` for Write). `None` for non-diff tools.
        new_string: Option<String>,
        /// (M7-02) Edited file path (drives diff syntax language). `None` for
        /// non-diff tools.
        file_path: Option<String>,
        /// The originating call's input, retained so the result renderer can
        /// derive the SAME headline `client-adapter` ships to the mobile and
        /// desktop clients. `None` when the pairing call was lost (a torn or
        /// compacted transcript).
        input: Option<serde_json::Value>,
    },
    /// (M7-04) Assistant thinking block. Collapsed → `∴ Thinking` + expand hint;
    /// expanded → `∴ Thinking…` + markdown body. `expanded` mirrors
    /// `AppState.expanded`-style state (default false → collapsed).
    AssistantThinking {
        /// The thinking text (markdown when expanded).
        thinking: String,
        /// `true` → render the full markdown body; `false` → header + hint only.
        expanded: bool,
    },
    /// (M7-04) Redacted thinking. Single dim+italic line `✻ Thinking…`.
    AssistantRedactedThinking,
    /// (M7-04) Compaction boundary. REPLACES M6-08's `[Compacted …]` `SystemText`.
    /// Renders the compact hint in normal mode and its full summary under
    /// Ctrl-O. Counts are retained for telemetry/debug parity but not rendered.
    CompactBoundary {
        /// Message count before compaction (debug/telemetry parity; not rendered).
        messages_before: u32,
        /// Message count after compaction (debug/telemetry parity; not rendered).
        messages_after: u32,
        /// Full compact summary. Hidden in normal mode and revealed by Ctrl-O.
        summary: String,
    },
    /// (M7-04) Level-aware system text. info → plain dim body; warning/error →
    /// `●` marker + colored body.
    SystemTextRich {
        /// Message body.
        body: String,
        /// Severity → marker/color.
        level: SystemLevel,
    },
    /// (M7-04) API error with retry countdown footer.
    SystemApiError {
        /// Formatted API error text.
        error: String,
        /// 1-based retry attempt.
        retry_attempt: u32,
        /// Seconds until the next retry.
        retry_in_seconds: u32,
        /// Max retry attempts.
        max_retries: u32,
        /// `true` → error body was clipped; append `…` + expand hint.
        truncated: bool,
    },
    /// (M7-04) Rate-limit notice (error text + optional dim upsell line).
    RateLimit {
        /// The rate-limit notice text (error-colored).
        text: String,
        /// Optional dim upsell line.
        upsell: Option<String>,
    },
    /// (M7-04) Teammate shutdown request/rejected notice.
    Shutdown {
        /// Originating teammate id.
        from: String,
        /// Optional reason.
        reason: Option<String>,
        /// `true` → rejected response; `false` → request.
        rejected: bool,
    },
    /// (M9-03) Task assignment notice — claude-code `TaskAssignmentMessage`.
    TaskAssignment {
        /// Task id, rendered as `#{task_id}`.
        task_id: String,
        /// Assigning agent's name.
        assigned_by: String,
        /// Task subject / title.
        subject: String,
        /// Optional task description.
        description: Option<String>,
    },
    /// (M9-03) Background-agent notification — claude-code
    /// `UserAgentNotificationMessage`. `status`: completed/failed/killed/other
    /// → marker color.
    AgentNotification {
        /// Summary line (empty → renders nothing).
        summary: String,
        /// Optional status string.
        status: Option<String>,
    },
    /// (M9-03) Inbound channel message — claude-code `UserChannelMessage`.
    ChannelMessage {
        /// Source server (raw; renderer takes the leaf after the last `:`).
        server: String,
        /// Optional sender user.
        user: Option<String>,
        /// Message content (renderer collapses whitespace + truncates to 60).
        content: String,
    },
    /// (M9-03) Teammate message — claude-code `UserTeammateMessage`
    /// (task-completed + plain-note sub-types).
    UserTeammate {
        /// Display name (`leader` or teammate id).
        display_name: String,
        /// Optional agent color name (→ `agent_color_from_name`).
        color: Option<String>,
        /// Sub-type payload.
        kind: UserTeammateKind,
    },
    /// (M7-04) Advisor block.
    Advisor {
        /// Advisor block content kind.
        kind: AdvisorKind,
        /// `true` → render the full result text (markdown).
        verbose: bool,
    },
    /// (M7-04) Hook-progress line.
    HookProgress {
        /// Hook event name (e.g. `"PreToolUse"`).
        event: String,
        /// In-progress hook count for this event.
        count: u32,
        /// `true` → static transcript summary; `false` → live running line.
        transcript_summary: bool,
    },
    /// One nested execution line from a running subagent (a tool call it made),
    /// rendered as an indented `⎿` line under its `Task` cell. Surfaces the
    /// otherwise-invisible inner work of a `Task`/Agent invocation.
    SubagentActivity {
        /// Pre-formatted one-line summary, e.g. `"Read(src/main.rs)"`.
        text: String,
    },
    /// (M7-04) Plan approval request/response.
    PlanApproval {
        /// Request/approved/rejected content.
        kind: PlanApprovalKind,
    },
    /// (M7-05) User bash-mode command line (`!` prefix). Body is the
    /// command text already extracted from the `<bash-input>` engine tag.
    UserBashInput {
        /// The command line the user typed in `!` bash mode.
        command: String,
    },
    /// (M7-05) Bash tool output. stdout/stderr already extracted from the
    /// `<bash-stdout>`/`<bash-stderr>` engine tags. Body carries ANSI SGR
    /// codes and is parsed through the ANSI parser at render time.
    UserBashOutput {
        /// Standard output (ANSI-coded).
        stdout: String,
        /// Standard error (ANSI-coded).
        stderr: String,
    },
    /// (M7-05) Slash-command echo. `❯ /{command} {args}` or `❯ Skill(name)`.
    UserCommand {
        /// Command name (without leading slash).
        command: String,
        /// Argument string (may be empty).
        args: String,
        /// `true` → render `Skill(name)` form instead of `/name args`.
        is_skill: bool,
    },
    /// (M7-05) Output of a local (slash) command. stdout/stderr already
    /// extracted; body rendered as markdown under a `  ⎿  ` gutter.
    UserLocalCommandOutput {
        /// Local-command stdout.
        stdout: String,
        /// Local-command stderr.
        stderr: String,
    },
    /// (M7-05) Memory write (`# {input}`) + a saving acknowledgement line.
    UserMemoryInput {
        /// Text the user added to memory (from `<user-memory-input>`).
        input: String,
    },
    /// (M7-05) Plan-mode plan body, rendered as bordered markdown under a
    /// "Plan to implement" header.
    UserPlan {
        /// Markdown plan content.
        plan_content: String,
    },
    /// (M7-05) A user prompt echoed into scrollback. Long bodies are
    /// head+tail truncated (claude-code parity, 10k char cap).
    UserPrompt {
        /// The prompt body text.
        text: String,
    },
    /// (M7-05) MCP resource/polling update lines (`↻ server: target · reason`).
    UserResourceUpdate {
        /// Parsed update triples (server, target, optional reason).
        updates: Vec<(String, String, Option<String>)>,
    },
    /// (M7-05) Image attachment. Renders `[Image #N]`/`[Image]` + optional
    /// metadata as text; a ratatui backend with `source_path` set + a graphics-
    /// capable terminal can display the real image (via `ratatui-image`).
    UserImage {
        /// Stored image id, if any (drives `#N` suffix).
        image_id: Option<u64>,
        /// Optional metadata suffix (dims/name) shown in parens.
        metadata: Option<String>,
        /// Absolute path to the image file on disk, when known. Enables real
        /// inline display in a graphics-capable terminal; `None` → text
        /// placeholder only.
        source_path: Option<String>,
    },
    /// (M7-05) A non-tool attachment summary line (directory listing, file
    /// read, PDF/resource reference, etc.). Carries the parsed attachment
    /// kind; team/swarm/hook kinds defer to M8.
    Attachment {
        /// The parsed attachment kind.
        attachment: Attachment,
    },
    /// (M7-05) A fold of consecutive same-tool tool-use blocks. Collapsed →
    /// `● {tool} (×N)`; expanded → header + each child input/result pair.
    /// `group_id` (the first child's id) keys `AppState.expanded`.
    GroupedToolUse {
        /// Shared tool name for the group.
        tool: String,
        /// First child's id — the per-group expanded-map key.
        group_id: lingxi_core::types::ToolUseId,
        /// `(input, result)` pairs in group order.
        entries: Vec<(serde_json::Value, serde_json::Value)>,
    },
    /// (M7-05) A fold of Read/Search/List tool runs into one count summary.
    /// Fullscreen also folds MCP, Bash, and auto-managed memory writes.
    CollapsedReadSearch {
        /// Number of search (Grep/Glob) tool uses.
        search_count: u64,
        /// Number of file reads.
        read_count: u64,
        /// Number of directory listings.
        list_count: u64,
        /// `true` while the group is still streaming (present-tense verbs).
        is_active: bool,
        /// Expanded-map key (first child's id).
        group_id: lingxi_core::types::ToolUseId,
        /// Per-entry display lines, shown when expanded.
        entries: Vec<String>,
        /// Number of REPL invocations folded (present-tense `REPL'ing`).
        repl_count: u64,
        /// Number of MCP tool calls folded in fullscreen.
        mcp_call_count: u64,
        /// Distinct MCP server names in stable display order.
        mcp_server_names: Vec<String>,
        /// Number of non-search/read Bash commands folded in fullscreen.
        bash_count: u64,
        /// The latest read target — the dim `⎿` hint shown ONLY while active.
        latest_hint: Option<String>,
        /// Team memories recalled (M9-03; data feed wired later).
        mem_read: u64,
        /// Team-memory searches (M9-03; data feed wired later).
        mem_search: u64,
        /// Team memories written (M9-03; data feed wired later).
        mem_write: u64,
    },
}

/// (M7-04) System message severity → marker/color mapping.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SystemLevel {
    /// Plain dim body, no marker.
    Info,
    /// `●` marker + yellow body.
    Warning,
    /// `●` marker + red body.
    Error,
}

impl Default for SystemLevel {
    fn default() -> Self {
        Self::Info
    }
}

/// (M7-04) Advisor block content kinds (claude-code `AdvisorMessage` subtypes).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AdvisorKind {
    /// `Advising` header (+ optional model / input descriptor).
    ServerToolUse {
        /// Optional advising model name.
        model: Option<String>,
        /// Optional input descriptor.
        input: Option<String>,
    },
    /// Advisor reviewed-and-applied result; `text` is the full feedback.
    Result {
        /// The advisor feedback text (markdown when verbose).
        text: String,
    },
    /// Redacted result — no expandable body.
    RedactedResult,
    /// `Advisor unavailable ({error_code})`.
    Error {
        /// The error code reported by the advisor service.
        error_code: String,
    },
}

impl Default for AdvisorKind {
    fn default() -> Self {
        Self::RedactedResult
    }
}

/// (M9-03) `UserTeammate` sub-type payloads (claude-code
/// `UserTeammateMessage`). plan-approval/shutdown reuse the existing
/// `PlanApproval`/`Shutdown` variants; `idle_notification` is suppressed at
/// the drain (M9-06), so it has no payload here.
#[derive(Debug, Clone)]
pub enum UserTeammateKind {
    /// `✓ Completed task #{task_id}` + optional ` ({task_subject})`.
    TaskCompleted {
        /// Completed task id.
        task_id: String,
        /// Optional task subject.
        task_subject: Option<String>,
    },
    /// Plain teammate note: optional summary + optional full content
    /// (shown indented when `is_transcript_mode`).
    Note {
        /// Optional one-line summary.
        summary: Option<String>,
        /// Optional full content.
        content: Option<String>,
        /// `true` → show full content (transcript mode).
        is_transcript_mode: bool,
    },
}

/// (M7-04) Plan-approval request/response kinds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlanApprovalKind {
    /// Approval request from a teammate.
    Request {
        /// Originating teammate id.
        from: String,
        /// Markdown plan content.
        plan_content: String,
        /// Optional plan file path.
        plan_file_path: Option<String>,
    },
    /// Approved by `{name}`.
    Approved {
        /// Approver display name.
        name: String,
    },
    /// Rejected by `{name}` with optional feedback.
    Rejected {
        /// Rejector display name.
        name: String,
        /// Optional feedback text.
        feedback: Option<String>,
    },
}

impl Default for PlanApprovalKind {
    fn default() -> Self {
        Self::Approved {
            name: String::new(),
        }
    }
}

/// A solo-user attachment kind.
#[derive(Debug, Clone)]
pub enum Attachment {
    /// Directory listing.
    Directory {
        /// Display path of the listed directory.
        display_path: String,
    },
    /// File read.
    File {
        /// Display path of the read file.
        display_path: String,
        /// Number of lines read.
        num_lines: u64,
        /// `true` → file was truncated (append `+`).
        truncated: bool,
    },
    /// Compact file reference.
    CompactFileReference {
        /// Display path of the referenced file.
        display_path: String,
    },
    /// PDF reference.
    PdfReference {
        /// Display path of the referenced PDF.
        display_path: String,
        /// Number of pages.
        page_count: u64,
    },
    /// IDE-selected lines.
    SelectedLines {
        /// Number of selected lines.
        count: u64,
        /// Display path of the file.
        display_path: String,
        /// IDE name.
        ide_name: String,
    },
    /// Nested memory file loaded.
    NestedMemory {
        /// Display path of the loaded memory file.
        display_path: String,
    },
    /// MCP resource read.
    McpResource {
        /// Resource name.
        name: String,
        /// MCP server name.
        server: String,
    },
    /// Plan file referenced.
    PlanFileReference {
        /// Plan file path.
        plan_file_path: String,
    },
    /// Skills restored.
    InvokedSkills {
        /// Comma-joined skill names.
        skill_names: Vec<String>,
    },
}

/// Currently active todo summary used by the status spinner.
///
/// Mirrors claude-code's current todo enough for presentation purposes
/// — only the two fields the spinner reads — so the TUI stays decoupled from
/// the `task` crate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CurrentTodo {
    /// The todo's imperative content (claude-code `subject`/`content`). Used
    /// as the spinner verb when `active_form` is absent.
    pub subject: String,
    /// The present-continuous form shown in the spinner while the todo is
    /// active (claude-code `activeForm`, e.g. "Running tests"). Preferred over
    /// `subject`.
    pub active_form: Option<String>,
}

impl RenderedMessage {
    pub fn tool_id(&self) -> Option<&lingxi_core::types::ToolUseId> {
        match self {
            RenderedMessage::AssistantToolUse { id, .. }
            | RenderedMessage::UserToolResult { id, .. } => Some(id),
            _ => None,
        }
    }

    /// Whether this message can be converted to immutable native terminal
    /// scrollback without losing later interaction state. Messages whose
    /// rendering depends on focus, expansion, retry countdowns, or other live
    /// state must stay in the iocraft tree.
    #[must_use]
    pub fn native_scrollback_safe(&self) -> bool {
        match self {
            RenderedMessage::AssistantToolUse { .. }
            | RenderedMessage::UserToolResult { .. }
            | RenderedMessage::AssistantThinking { .. }
            | RenderedMessage::AssistantRedactedThinking
            | RenderedMessage::SystemApiError { .. }
            | RenderedMessage::Advisor { .. }
            | RenderedMessage::PlanApproval { .. }
            | RenderedMessage::GroupedToolUse { .. }
            | RenderedMessage::CollapsedReadSearch { .. } => false,
            RenderedMessage::HookProgress {
                transcript_summary, ..
            } => *transcript_summary,
            RenderedMessage::UserText { .. }
            | RenderedMessage::AssistantText { .. }
            | RenderedMessage::SystemText { .. }
            | RenderedMessage::SubagentActivity { .. }
            | RenderedMessage::CompactBoundary { .. }
            | RenderedMessage::SystemTextRich { .. }
            | RenderedMessage::RateLimit { .. }
            | RenderedMessage::Shutdown { .. }
            | RenderedMessage::TaskAssignment { .. }
            | RenderedMessage::AgentNotification { .. }
            | RenderedMessage::ChannelMessage { .. }
            | RenderedMessage::UserTeammate { .. }
            | RenderedMessage::UserCommand { .. }
            | RenderedMessage::UserBashInput { .. }
            | RenderedMessage::UserBashOutput { .. }
            | RenderedMessage::UserLocalCommandOutput { .. }
            | RenderedMessage::UserMemoryInput { .. }
            | RenderedMessage::UserPlan { .. }
            | RenderedMessage::UserPrompt { .. }
            | RenderedMessage::UserResourceUpdate { .. }
            | RenderedMessage::UserImage { .. }
            | RenderedMessage::Attachment { .. } => true,
        }
    }
}

#[cfg(test)]
mod rendered_message_tests {
    use super::*;

    #[test]
    fn native_scrollback_safe_keeps_expandable_tool_messages_live() {
        let id = lingxi_core::types::ToolUseId::new();
        let tool_use = RenderedMessage::AssistantToolUse {
            id: id.clone(),
            tool: "Read".to_string(),
            input: serde_json::json!({"file_path": "src/lib.rs"}),
        };
        let tool_result = RenderedMessage::UserToolResult {
            id,
            tool: "Read".to_string(),
            result: serde_json::json!({"content": "hello"}),
            old_string: None,
            new_string: None,
            file_path: None,
            input: None,
        };
        let plain = RenderedMessage::AssistantText {
            body: "hello".to_string(),
            timestamp: 0,
        };

        assert!(!tool_use.native_scrollback_safe());
        assert!(!tool_result.native_scrollback_safe());
        assert!(plain.native_scrollback_safe());
    }
}
