//! Per-hook-run transcript `attachment` records.
//!
//! claude-code persists exactly ONE `attachment` transcript line for every
//! hook run. The attachment payload's `type` discriminates the outcome; this
//! module builds the three run-outcome payloads the port was missing —
//! `hook_success`, `hook_non_blocking_error`, `hook_cancelled` — with
//! claude's EXACT key ORDER and conditional-field presence.
//!
//! # Oracle evidence
//!
//! Key ORDER is byte-locked against BOTH sources:
//!
//! * **Real 2.1.220 transcripts** (`~/.claude/projects/**/*.jsonl`, 26 048
//!   hook attachment records mined): every one of the 25 901 `hook_success`
//!   records has the key tuple
//!   `(type, hookName, toolUseID, hookEvent, content, stdout, stderr,
//!   exitCode, command, durationMs)`; `hook_non_blocking_error` has
//!   `(type, hookName, toolUseID, hookEvent, stderr, stdout, exitCode,
//!   command, durationMs)`; `hook_cancelled` has
//!   `(type, hookName, toolUseID, hookEvent, command, durationMs, timedOut,
//!   timeoutMs)`.
//! * **The 2.1.220 binary** (`~/.local/share/claude/versions/2.1.220`), the
//!   command-hook runner at BIN off **237798900–237806040**:
//!   - exit-0 success — `Va({type:"hook_success",hookName:f,toolUseID:r,
//!     hookEvent:p,content:Ae,stdout:Ce.stdout,stderr:Ce.stderr,
//!     exitCode:Ce.status,command:ee,durationMs:Ee})`
//!   - non-blocking error — `Va({type:"hook_non_blocking_error",hookName:f,
//!     toolUseID:r,hookEvent:p,stderr:…,stdout:…,exitCode:…,command:ee,
//!     durationMs:Ee})`
//!   - aborted — `Va({type:"hook_cancelled",hookName:f,toolUseID:r,
//!     hookEvent:p,command:ee,durationMs:Ee,timedOut:!o?.aborted,
//!     timeoutMs:re})`
//!
//! `command` is `ee = qq(hook)` (BIN off **230279724**): the hook's
//! `statusMessage` when set, else `iSe(hook)` (BIN off **230279432**) — the
//! per-arm rendering `command [args…]` / `prompt` / `url` /
//! `server/tool` / `"callback"` / `"function"`.
//!
//! Conditional presence (keys must be ABSENT, never `null`):
//! * `command` / `durationMs` on `hook_non_blocking_error` are omitted by the
//!   HTTP and `mcp_tool` arms (BIN off 237799357 / 237800962), which yield the
//!   payload without them.
//! * `timedOut` / `timeoutMs` on `hook_cancelled` travel as a PAIR: the
//!   command / HTTP / `mcp_tool` cancel arms emit both, the prompt / agent
//!   early-return arm (BIN off 237798423) and the `PostToolUse` re-yield
//!   (BIN off 234725805) emit NEITHER — hence [`CancellationTimeout`] models
//!   them as one optional unit rather than two independent options.

use crate::definition::{HookDefinition, HookExecutor};
use crate::events::HookEvent;
use async_trait::async_trait;
use serde_json::{Map, Value};

/// Sink the executor publishes each hook-run attachment to.
///
/// The engine wires an implementation that appends one `type:"attachment"`
/// line to the session transcript. Without a sink, synchronous attachments are
/// still carried on [`crate::AggregateHookResult::hook_attachments`].
/// Detached completions cannot be added to an aggregate that has already
/// returned, so they are retained only when this sink is wired.
#[async_trait]
pub trait HookAttachmentSink: Send + Sync {
    /// Persist one hook-run attachment payload.
    async fn record(&self, attachment: Value);
}

/// claude's inline cap for hook output spliced into an attachment's `content`
/// (`P0u = 1e4`, BIN off **230268805**; `jKe(e,t,r,n=P0u)` returns `e`
/// unchanged when `e.length <= n`).
pub const HOOK_OUTPUT_INLINE_LIMIT: usize = 10_000;

/// Apply claude's `jKe` inline-vs-persist threshold to hook output.
///
/// At or under [`HOOK_OUTPUT_INLINE_LIMIT`] chars the text is returned
/// verbatim — the byte-exact common case. RESIDUAL: over the limit claude
/// persists the full output to `~/.claude/…/tool-results` and substitutes a
/// `(Full output saved to: …)` reference (BIN off 233100900); the port has no
/// hook-output persist seam, so it truncates at the same boundary instead.
#[must_use]
pub fn inline_hook_output(text: &str) -> String {
    if text.chars().count() <= HOOK_OUTPUT_INLINE_LIMIT {
        return text.to_string();
    }
    text.chars().take(HOOK_OUTPUT_INLINE_LIMIT).collect()
}

/// The four identity fields every hook-run attachment leads with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HookAttachmentIdentity {
    /// claude's `hookName` — see [`hook_name_for_event`].
    pub hook_name: String,
    /// claude's `hookEvent` — the bare event name (`"PostToolUse"`, `"Stop"`).
    pub hook_event: String,
    /// claude's `toolUseID` — the provider tool-use id for tool events, else a
    /// freshly minted uuid (see [`tool_use_id_for_event`]).
    pub tool_use_id: String,
}

/// The `timedOut` / `timeoutMs` pair carried by a `hook_cancelled` attachment.
///
/// Modeled as one unit because claude emits both keys or neither — never one
/// (BIN off 237798900: `timedOut:!o?.aborted,timeoutMs:re`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CancellationTimeout {
    /// `!outerSignal?.aborted` — `true` when the abort came from the hook's own
    /// execution timeout, `false` when the caller (user Esc / turn teardown)
    /// aborted it. NOTE: a caller abort still emits the pair with `false`; it
    /// is the prompt/agent early-return arm that omits both keys.
    pub timed_out: bool,
    /// The deadline that was in force — `hook.timeout * 1000` when the hook
    /// declares one, else the runner's default (`re=q.timeout?q.timeout*1000:i`).
    pub timeout_ms: u64,
}

/// Seed the four leading identity keys in claude's order.
fn identity_head(kind: &str, id: &HookAttachmentIdentity) -> Map<String, Value> {
    let mut m = Map::new();
    m.insert("type".into(), Value::String(kind.into()));
    m.insert("hookName".into(), Value::String(id.hook_name.clone()));
    m.insert("toolUseID".into(), Value::String(id.tool_use_id.clone()));
    m.insert("hookEvent".into(), Value::String(id.hook_event.clone()));
    m
}

/// Build a `hook_success` attachment payload.
///
/// Key order `type, hookName, toolUseID, hookEvent, content, stdout, stderr,
/// exitCode, command, durationMs` (25 901 real 2.1.220 transcript records;
/// BIN off 237803277).
#[must_use]
pub fn success_attachment(
    id: &HookAttachmentIdentity,
    content: &str,
    stdout: &str,
    stderr: &str,
    exit_code: i32,
    command: &str,
    duration_ms: u64,
) -> Value {
    let mut m = identity_head("hook_success", id);
    m.insert("content".into(), Value::String(content.into()));
    m.insert("stdout".into(), Value::String(stdout.into()));
    m.insert("stderr".into(), Value::String(stderr.into()));
    m.insert("exitCode".into(), Value::from(exit_code));
    m.insert("command".into(), Value::String(command.into()));
    m.insert("durationMs".into(), Value::from(duration_ms));
    Value::Object(m)
}

/// Build a `hook_non_blocking_error` attachment payload.
///
/// Key order `type, hookName, toolUseID, hookEvent, stderr, stdout, exitCode[,
/// command, durationMs]`. `command` / `duration_ms` are `None` for the HTTP and
/// `mcp_tool` arms, which omit both keys entirely (BIN off 237799357).
#[must_use]
pub fn non_blocking_error_attachment(
    id: &HookAttachmentIdentity,
    stderr: &str,
    stdout: &str,
    exit_code: i32,
    command: Option<&str>,
    duration_ms: Option<u64>,
) -> Value {
    let mut m = identity_head("hook_non_blocking_error", id);
    m.insert("stderr".into(), Value::String(stderr.into()));
    m.insert("stdout".into(), Value::String(stdout.into()));
    m.insert("exitCode".into(), Value::from(exit_code));
    if let Some(c) = command {
        m.insert("command".into(), Value::String(c.into()));
    }
    if let Some(d) = duration_ms {
        m.insert("durationMs".into(), Value::from(d));
    }
    Value::Object(m)
}

/// Build a `hook_cancelled` attachment payload.
///
/// Key order `type, hookName, toolUseID, hookEvent[, command, durationMs][,
/// timedOut, timeoutMs]`. All four trailing keys are omitted by the
/// prompt / agent early-return arm (BIN off 237798423).
#[must_use]
pub fn cancelled_attachment(
    id: &HookAttachmentIdentity,
    command: Option<&str>,
    duration_ms: Option<u64>,
    timeout: Option<CancellationTimeout>,
) -> Value {
    let mut m = identity_head("hook_cancelled", id);
    if let Some(c) = command {
        m.insert("command".into(), Value::String(c.into()));
    }
    if let Some(d) = duration_ms {
        m.insert("durationMs".into(), Value::from(d));
    }
    if let Some(t) = timeout {
        m.insert("timedOut".into(), Value::Bool(t.timed_out));
        m.insert("timeoutMs".into(), Value::from(t.timeout_ms));
    }
    Value::Object(m)
}

/// claude's `hookName` for an event.
///
/// Tool-scoped events qualify the event name with the tool
/// (`` `PostToolUse:${t.name}` ``, BIN off 234725805) and `SessionStart`
/// qualifies with its source (`` `SessionStart:${e}` ``, BIN off 232675014);
/// every other event uses the bare event name. Verified against the real
/// transcript census: `PostToolUse:Bash` (15 059), `PostToolUse:Edit` (4 623),
/// `Stop` (1 241), `SessionStart:startup` (108), `SessionStart:compact` (55),
/// `SessionStart:clear` (13), `UserPromptSubmit` (1).
#[must_use]
pub fn hook_name_for_event(event: &HookEvent) -> String {
    let base = format!("{:?}", event.event_type());
    match event {
        HookEvent::PreToolUse { tool_name, .. }
        | HookEvent::PostToolUse { tool_name, .. }
        | HookEvent::PostToolUseFailure { tool_name, .. } => format!("{base}:{tool_name}"),
        HookEvent::SessionStart { source, .. } => format!("{base}:{source}"),
        _ => base,
    }
}

/// claude's `toolUseID` for an event, when the event carries one.
///
/// Tool events reuse the provider tool-use id verbatim (`toolu_…` — 24 497 of
/// the mined records). Every other event gets a freshly minted uuid at the
/// runner (`` let l=a||`hook-${randomUUID()}` ``); the caller mints it, so this
/// returns `None` rather than inventing one here.
#[must_use]
pub fn tool_use_id_for_event(event: &HookEvent) -> Option<String> {
    match event {
        HookEvent::PreToolUse { tool_use_id, .. }
        | HookEvent::PostToolUse { tool_use_id, .. }
        | HookEvent::PostToolUseFailure { tool_use_id, .. } => {
            Some(tool_use_id.as_str().to_string())
        }
        _ => None,
    }
}

/// claude's `command` field for a hook — `qq(hook)` (BIN off 230279724):
/// `statusMessage` when set, else `iSe(hook)` (BIN off 230279432):
///
/// ```text
/// function iSe(e){switch(e.type){
///   case"command": return e.args?[e.command,...e.args].join(" "):e.command;
///   case"prompt":  return e.prompt;
///   case"agent":   return e.prompt;
///   case"http":    return e.url;
///   case"mcp_tool":return `${e.server}/${e.tool}`;
///   case"callback":return "callback";
///   case"function":return "function"}}
/// function qq(e){if("statusMessage"in e&&e.statusMessage)return e.statusMessage;
///                return iSe(e)}
/// ```
///
/// The port has no `mcp_tool` / `callback` / `function` executor arms; its
/// `Builtin` arm is the nearest analogue of claude's in-process `callback`
/// hook, so it renders as the handler id.
#[must_use]
pub fn attachment_command(hook: &HookDefinition) -> String {
    if let Some(status) = hook.status_message.as_ref() {
        if !status.is_empty() {
            return status.clone();
        }
    }
    match &hook.executor {
        HookExecutor::Command { command, args, .. } => {
            if args.is_empty() {
                command.clone()
            } else {
                let mut parts = Vec::with_capacity(args.len() + 1);
                parts.push(command.as_str());
                parts.extend(args.iter().map(String::as_str));
                parts.join(" ")
            }
        }
        HookExecutor::Http { url, .. } => url.clone(),
        HookExecutor::Agent { prompt, .. } | HookExecutor::Prompt { prompt, .. } => prompt.clone(),
        HookExecutor::Builtin { handler_id } => handler_id.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ident() -> HookAttachmentIdentity {
        HookAttachmentIdentity {
            hook_name: "PostToolUse:Bash".into(),
            hook_event: "PostToolUse".into(),
            tool_use_id: "toolu_01ApkBwAZMCAza47B5nAWiGS".into(),
        }
    }

    #[test]
    fn success_matches_oracle_key_order() {
        let v = success_attachment(&ident(), "ok", "out\n", "", 0, "./hooks/fmt.sh", 37);
        assert_eq!(
            serde_json::to_string(&v).unwrap(),
            r#"{"type":"hook_success","hookName":"PostToolUse:Bash","toolUseID":"toolu_01ApkBwAZMCAza47B5nAWiGS","hookEvent":"PostToolUse","content":"ok","stdout":"out\n","stderr":"","exitCode":0,"command":"./hooks/fmt.sh","durationMs":37}"#
        );
    }

    #[test]
    fn non_blocking_error_matches_oracle_key_order() {
        let id = HookAttachmentIdentity {
            hook_name: "UserPromptSubmit".into(),
            hook_event: "UserPromptSubmit".into(),
            tool_use_id: "9b7a1c2f-2212-4d41-a588-38d55c6a13cd".into(),
        };
        let v = non_blocking_error_attachment(
            &id,
            "Failed with non-blocking status code: boom",
            "",
            1,
            Some("${LINGXI_PLUGIN_ROOT}/scripts/on-prompt-submit.sh"),
            Some(2),
        );
        assert_eq!(
            serde_json::to_string(&v).unwrap(),
            r#"{"type":"hook_non_blocking_error","hookName":"UserPromptSubmit","toolUseID":"9b7a1c2f-2212-4d41-a588-38d55c6a13cd","hookEvent":"UserPromptSubmit","stderr":"Failed with non-blocking status code: boom","stdout":"","exitCode":1,"command":"${LINGXI_PLUGIN_ROOT}/scripts/on-prompt-submit.sh","durationMs":2}"#
        );
    }

    #[test]
    fn non_blocking_error_omits_command_and_duration_for_transport_arms() {
        // HTTP / mcp_tool arms yield the payload WITHOUT `command`/`durationMs`
        // (BIN off 237799357 / 237800962) — the keys must be ABSENT, not null.
        let v = non_blocking_error_attachment(
            &ident(),
            "HTTP 500 from https://h/",
            "",
            500,
            None,
            None,
        );
        assert_eq!(
            serde_json::to_string(&v).unwrap(),
            r#"{"type":"hook_non_blocking_error","hookName":"PostToolUse:Bash","toolUseID":"toolu_01ApkBwAZMCAza47B5nAWiGS","hookEvent":"PostToolUse","stderr":"HTTP 500 from https://h/","stdout":"","exitCode":500}"#
        );
    }

    #[test]
    fn cancelled_with_timeout_matches_oracle_key_order() {
        let id = HookAttachmentIdentity {
            hook_name: "Stop".into(),
            hook_event: "Stop".into(),
            tool_use_id: "68c53d21-9374-46a9-b5a0-0ccf3659e9bb".into(),
        };
        let v = cancelled_attachment(
            &id,
            Some("${LINGXI_PLUGIN_ROOT}/scripts/on-stop.sh"),
            Some(591),
            Some(CancellationTimeout {
                timed_out: false,
                timeout_ms: 600_000,
            }),
        );
        assert_eq!(
            serde_json::to_string(&v).unwrap(),
            r#"{"type":"hook_cancelled","hookName":"Stop","toolUseID":"68c53d21-9374-46a9-b5a0-0ccf3659e9bb","hookEvent":"Stop","command":"${LINGXI_PLUGIN_ROOT}/scripts/on-stop.sh","durationMs":591,"timedOut":false,"timeoutMs":600000}"#
        );
    }

    #[test]
    fn cancelled_without_timeout_omits_all_four_optional_keys() {
        // prompt/agent-arm early return (BIN off 237798423) yields ONLY the four
        // identity keys — `timedOut`/`timeoutMs` must be ABSENT, not null.
        let v = cancelled_attachment(&ident(), None, None, None);
        assert_eq!(
            serde_json::to_string(&v).unwrap(),
            r#"{"type":"hook_cancelled","hookName":"PostToolUse:Bash","toolUseID":"toolu_01ApkBwAZMCAza47B5nAWiGS","hookEvent":"PostToolUse"}"#
        );
    }

    #[test]
    fn hook_name_is_tool_qualified_for_tool_events_and_source_qualified_for_session_start() {
        use crate::events::HookEvent;
        use protocol::{SessionId, ToolUseId};

        let post = HookEvent::PostToolUse {
            tool_name: "Bash".into(),
            tool_input: serde_json::json!({}),
            tool_output: serde_json::json!({}),
            tool_use_id: ToolUseId::from("toolu_x".to_string()),
            duration_ms: None,
        };
        assert_eq!(hook_name_for_event(&post), "PostToolUse:Bash");
        assert_eq!(
            tool_use_id_for_event(&post).as_deref(),
            Some("toolu_x"),
            "tool events reuse the tool_use id verbatim"
        );

        let start = HookEvent::SessionStart {
            session_id: SessionId::new(),
            source: "startup".into(),
        };
        assert_eq!(hook_name_for_event(&start), "SessionStart:startup");
        assert!(
            tool_use_id_for_event(&start).is_none(),
            "non-tool events mint a fresh uuid instead"
        );

        let stop = HookEvent::Stop {
            reason: "end_turn".into(),
        };
        assert_eq!(hook_name_for_event(&stop), "Stop");
    }

    #[test]
    fn command_string_prefers_status_message_then_renders_per_executor_arm() {
        use crate::definition::{HookDefinition, HookExecutor, HookSource};
        use crate::events::HookEventType;
        use std::collections::HashMap;

        let base = |executor: HookExecutor| HookDefinition {
            id: protocol::HookId::new(),
            name: "h".into(),
            events: vec![HookEventType::Stop],
            if_condition: None,
            executor,
            source: HookSource::User,
            blocking: true,
            timeout: None,
            priority: 0,
            once: false,
            status_message: None,
        };

        let cmd = base(HookExecutor::Command {
            command: "./x.sh".into(),
            args: vec!["-a".into(), "b".into()],
            env: HashMap::new(),
            cwd: None,
        });
        assert_eq!(attachment_command(&cmd), "./x.sh -a b");

        let http = base(HookExecutor::Http {
            url: "https://h/hook".into(),
            method: "POST".into(),
            headers: HashMap::new(),
            allowed_env_vars: vec![],
            timeout: std::time::Duration::from_secs(1),
        });
        assert_eq!(attachment_command(&http), "https://h/hook");

        let mut with_status = base(HookExecutor::Command {
            command: "./x.sh".into(),
            args: vec![],
            env: HashMap::new(),
            cwd: None,
        });
        with_status.status_message = Some("Formatting".into());
        assert_eq!(
            attachment_command(&with_status),
            "Formatting",
            "`qq` returns statusMessage when set, else `iSe`"
        );
    }
}
