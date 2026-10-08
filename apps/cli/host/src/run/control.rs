use super::suggestions::StreamFileSuggestionIndex;
use crate::init::Runtime;
use crate::stream_json_input::ControlPlaneWriter;
use lingxi_core::host::{McpStatus, OrchestratorHandle};
use permission;
use serde_json::{json, Value};
use std::sync::Arc;

/// Resolution of a `set_model` control request's `model` field against the
/// session default, byte-faithful to claude-code 2.1.208's engine handler:
/// `if(fr!=null&&typeof fr!=="string"){…reject…} let or=model??"default",
/// Jr=or.trim().toLowerCase()==="default",vr=Jr?SE():or`.
pub(super) enum SetModelTarget {
    /// Apply this model: the raw requested string, or the session default when
    /// the request was absent / explicit `null` / case-insensitive `"default"`.
    Apply(String),
    /// The `model` field was present but neither a string nor null — reject.
    Reject,
}

pub(super) fn resolve_set_model_target(
    field: Option<&Value>,
    default_model: &str,
) -> SetModelTarget {
    // CC: `fr != null` — in JS `!= null` covers both `null` and `undefined`, so
    // an explicit JSON `null` is treated as absent (→ default), not a type
    // error. `model ?? "default"` collapses absent/null to `"default"`.
    let requested = match field {
        Some(Value::String(m)) => m.as_str(),
        None | Some(Value::Null) => "default",
        Some(_) => return SetModelTarget::Reject,
    };
    // CC: `or.trim().toLowerCase() === "default"` — trimmed, case-insensitive.
    if requested.trim().to_lowercase() == "default" {
        SetModelTarget::Apply(default_model.to_string())
    } else {
        // CC: `vr = Jr ? SE() : or` — the RAW requested string (untrimmed).
        SetModelTarget::Apply(requested.to_string())
    }
}

/// Dispatch a single `control_request` frame using initialization data collected
/// before the asynchronous dispatcher task starts.
#[allow(clippy::too_many_arguments)]
pub(super) async fn dispatch_control_request(
    subtype: &str,
    request_id: &str,
    frame: &serde_json::Value,
    writer: &ControlPlaneWriter,
    cancel_tx: &tokio::sync::watch::Sender<bool>,
    lifecycle: &crate::queued_commands::QueueLifecycle,
    orchestrator: &Arc<orchestrator::ConversationOrchestrator>,
    task_registry: &Arc<tasks::registry::TaskRegistry>,
    session_cwd: &Arc<tool_api::SessionCwd>,
    control_plane: &Arc<crate::control_plane::StdioControlPlane>,
    end_notify: &Arc<tokio::sync::Notify>,
    init_commands: &[serde_json::Value],
    init_agents: &[serde_json::Value],
    init_models: &[serde_json::Value],
    init_account: &serde_json::Value,
    init_fast_mode_state: &'static str,
    init_fast_mode_disabled_reason: Option<&'static str>,
    file_suggestions: &StreamFileSuggestionIndex,
) {
    // Request body fields live at `frame.request.<field>` (already key-normalized).
    let field = |k: &str| frame.get("request").and_then(|r| r.get(k));

    match subtype {
        "initialize" => {
            let payload = initialize_response_payload(
                init_commands,
                init_agents,
                init_models,
                init_account,
                std::process::id(),
                init_fast_mode_state,
                init_fast_mode_disabled_reason,
            );
            writer.reply_success(request_id, Some(payload));
        }
        "interrupt" => {
            // §2.2 #1: cancel the per-turn token, then send the interrupt
            // RECEIPT (2.1.220 `interrupt_receipt_v1`, advertised on
            // system/init). Live-captured contract:
            //   plain            → `{"still_queued":[uuids…]}` — queued async
            //                      user messages SURVIVE the interrupt;
            //   cancel_queued:true → sweep them too (2.1.219
            //                      `interrupt_cancel_queued_v1`): a terminal
            //                      `command_lifecycle`/`cancelled` frame per
            //                      uuid, then `{"still_queued":[],
            //                      "cancelled":[uuids…]}`. Idempotent — a
            //                      repeat interrupt lists nothing twice.
            // Cancel the token owned by the turn that is live NOW. Do not leave
            // a sticky watch value behind when the CLI is between turns: that
            // would cancel the next queued user message instead of the turn the
            // host intended to interrupt.
            if control_plane.cancel_active_turn().await {
                let _ = cancel_tx.send(true);
            } else {
                let _ = cancel_tx.send(false);
            }
            if field("cancel_queued").and_then(Value::as_bool) == Some(true) {
                let cancelled = lifecycle.queued.cancel_all_queued();
                for uuid in &cancelled {
                    lifecycle.emit(uuid, crate::queued_commands::LIFECYCLE_CANCELLED);
                }
                writer.reply_success(
                    request_id,
                    Some(json!({"still_queued": [], "cancelled": cancelled})),
                );
            } else {
                writer.reply_success(
                    request_id,
                    Some(json!({"still_queued": lifecycle.queued.still_queued()})),
                );
            }
        }
        "set_model" => {
            // §2.2 #5: `"default"` (or an absent model) resolves to the session
            // default model and APPLIES it — so a client can revert a prior
            // `set_model` override (claude-code re-resolves via
            // getDefaultMainLoopModel() and calls setMainLoopModelOverride).
            let default_model = orchestrator.default_model();
            let target = match resolve_set_model_target(field("model"), default_model.as_str()) {
                SetModelTarget::Apply(t) => t,
                SetModelTarget::Reject => {
                    // CC 2.1.208: `set_model: model must be a string`.
                    writer.reply_error(request_id, "set_model: model must be a string");
                    return;
                }
            };
            match orchestrator
                .switch_model_with_source(&target, None, "sdk")
                .await
            {
                Ok(()) => writer.reply_success(request_id, None),
                Err(e) => writer.reply_error(request_id, &e.to_string()),
            }
        }
        "set_max_thinking_tokens" => {
            let max_tokens = match field("max_thinking_tokens") {
                Some(Value::Null) => None,
                Some(Value::Number(value)) => value.as_u64().and_then(|v| u32::try_from(v).ok()),
                None | Some(_) => None,
            };
            let max_tokens_valid =
                matches!(field("max_thinking_tokens"), Some(Value::Null)) || max_tokens.is_some();
            let display_valid = match field("thinking_display") {
                None | Some(Value::Null) => true,
                Some(Value::String(value)) => value == "summarized" || value == "omitted",
                Some(_) => false,
            };
            if !max_tokens_valid || !display_valid {
                writer.reply_error(
                    request_id,
                    "set_max_thinking_tokens: max_thinking_tokens must be an integer or null and thinking_display must be \"summarized\", \"omitted\", or null",
                );
                return;
            }
            let thinking = match max_tokens {
                Some(0) => llm_runtime::model::thinking::ThinkingConfig::Disabled,
                Some(budget_tokens) => {
                    llm_runtime::model::thinking::ThinkingConfig::Enabled { budget_tokens }
                }
                None => llm_runtime::model::thinking::ThinkingConfig::Adaptive,
            };
            orchestrator.set_thinking_config(thinking);
            orchestrator.set_thinking_display(field("thinking_display").and_then(Value::as_str));
            writer.reply_success(request_id, None);
        }
        "rename_session" => {
            let title = field("title").and_then(Value::as_str).unwrap_or("");
            if title.trim().is_empty() {
                writer.reply_error(request_id, "title must be non-empty");
                return;
            }
            match orchestrator.rename_session(title.to_string()).await {
                Ok(()) => writer.reply_success(request_id, None),
                Err(err) => writer.reply_error(request_id, &format!("rename_session: {err}")),
            }
        }
        "mcp_status" => {
            // §2.2 #7: `{mcpServers: [...]}`.
            let servers: Vec<serde_json::Value> = orchestrator
                .list_mcp_servers()
                .await
                .into_iter()
                .map(|s| {
                    let status = match s.status {
                        McpStatus::Connected => "connected",
                        McpStatus::Disconnected => "disconnected",
                        McpStatus::Error(_) => "error",
                    };
                    json!({"name": s.name, "status": status})
                })
                .collect();
            writer.reply_success(request_id, Some(json!({"mcpServers": servers})));
        }
        "get_context_usage" => {
            // §2.2 #9: token-budget breakdown (shape inferred — not byte-dumped).
            let (used, total) = orchestrator.context_window_usage().await;
            writer.reply_success(
                request_id,
                Some(json!({
                    "usedTokens": used,
                    "maxTokens": total
                })),
            );
        }
        "get_session_cost" => {
            // §2.2 #10: `{text}` (format inferred — not byte-dumped).
            let cost = orchestrator.snapshot_cost().await;
            writer.reply_success(
                request_id,
                Some(json!({"text": format!("Total cost: ${:.4}", cost.total_usd)})),
            );
        }
        "get_usage" => {
            // §2.2 #11: usage snapshot (shape inferred — not byte-dumped).
            let cost = orchestrator.snapshot_cost().await;
            writer.reply_success(
                request_id,
                Some(json!({
                    "input_tokens": cost.input_tokens,
                    "output_tokens": cost.output_tokens,
                    "cache_read_tokens": cost.cache_read_tokens,
                    "cache_creation_tokens": cost.cache_creation_tokens,
                    "total_tokens": cost.total_tokens
                })),
            );
        }
        "stop_task" => {
            // §2.2 #38: best-effort kill; not_found/not_running ⇒ success `{}`.
            if let Some(task_id) = field("task_id").and_then(|v| v.as_str()) {
                // The control-channel `stopTask` — claude-code's `source:"user"`
                // caller, which inherits `killedBy = "user"`.
                let _ = task_registry.kill_with_reason(task_id, "user").await;
            }
            writer.reply_success(request_id, Some(json!({})));
        }
        "background_tasks" => {
            // claude-code's SDK/bridge `background_tasks` request:
            // `if(D.toolUseId!==void 0){let ue=Ode(D.toolUseId,F);Xe(r,{backgrounded:ue})}
            //  else zM(F),Xe(r,{})`.
            //
            // `K4t` validation runs FIRST, before the disabled gate: absent,
            // null or empty means "background everything", a string means
            // "background that one tool call", and any other JSON type is a
            // hard error.
            enum Target {
                All,
                One(String),
                Invalid,
            }
            let target = match field("tool_use_id") {
                None | Some(Value::Null) => Target::All,
                Some(Value::String(id)) if id.is_empty() => Target::All,
                Some(Value::String(id)) => Target::One(id.clone()),
                Some(_) => Target::Invalid,
            };
            match target {
                Target::Invalid => {
                    writer.reply_error(request_id, "background_tasks: tool_use_id must be a string")
                }
                _ if lingxi_core::host::env::background_tasks_disabled() => {
                    writer.reply_error(request_id, "Background tasks are disabled in this session.")
                }
                Target::One(id) => {
                    let backgrounded = task_registry.background_task_for_tool_use(&id).await;
                    writer.reply_success(request_id, Some(json!({"backgrounded": backgrounded})));
                }
                Target::All => {
                    task_registry.background_all_tasks().await;
                    writer.reply_success(request_id, Some(json!({})));
                }
            }
        }
        "register_repo_root" => {
            let request_value = frame.get("request").cloned().unwrap_or_else(|| json!({}));
            match serde_json::from_value::<lingxi_core::host::RegisterRepoRootRequest>(
                request_value,
            ) {
                Ok(request) if !request.path.trim().is_empty() => {
                    match orchestrator.register_repo_root(request).await {
                        Ok(outcome) => match serde_json::to_value(outcome) {
                            Ok(value) => writer.reply_success(request_id, Some(value)),
                            Err(error) => writer.reply_error(
                                request_id,
                                &format!("register_repo_root: failed to encode response: {error}"),
                            ),
                        },
                        Err(error) => {
                            writer.reply_error(request_id, &format!("register_repo_root: {error}"));
                        }
                    }
                }
                Ok(_) => {
                    writer.reply_error(request_id, "register_repo_root: path must not be empty")
                }
                Err(error) => writer.reply_error(
                    request_id,
                    &format!("register_repo_root: invalid request: {error}"),
                ),
            }
        }
        "set_cwd" => {
            // Move the live session to another directory. This crosses the
            // TRUST boundary — the target's files become readable and writable
            // under the session's rules — so an untrusted directory is
            // confirmed by the client before the move, via the
            // `needs_trust` → `trust_accepted` + `trusted_directory` echo
            // handshake. The decision (and every byte-exact rejection) lives in
            // `permission::set_cwd`; this arm only gathers the facts and
            // performs the move.
            let request = permission::set_cwd::SetCwdRequest {
                path: field("path")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_string(),
                trust_accepted: field("trust_accepted").and_then(serde_json::Value::as_bool),
                trusted_directory: field("trusted_directory")
                    .and_then(|v| v.as_str())
                    .map(str::to_string),
            };
            let trimmed = request.path.trim().to_string();
            let raw = std::path::PathBuf::from(&trimmed);
            let target = if raw.is_absolute() {
                raw
            } else {
                session_cwd.cwd().join(raw)
            };
            // Canonicalise when we can; a path that cannot be canonicalised is
            // reported at the path the user typed, not at a half-resolved one.
            let display = std::fs::canonicalize(&target).unwrap_or(target.clone());
            let display_str = display.to_string_lossy().into_owned();
            let resolved = if !display.exists() {
                permission::set_cwd::ResolvedPath::NotFound(display_str.clone())
            } else if display.is_dir() {
                permission::set_cwd::ResolvedPath::Directory(display_str.clone())
            } else {
                permission::set_cwd::ResolvedPath::NotADirectory(display_str.clone())
            };
            // A relocation must own the same lease as admitted operations,
            // not just sample the busy token before an async transcript move.
            let _cwd_operation = control_plane.try_lock_operation();
            let ctx = permission::set_cwd::SetCwdContext {
                resolved,
                current_cwd: session_cwd.cwd().to_string_lossy().into_owned(),
                // `Cd(…)` rules have no port representation yet, so no rule can
                // block. Left explicit rather than implied: when Cd rules land,
                // this is the one line that has to change.
                blocking_cd_rule: None,
                // No config path (no resolvable home) ⇒ nothing can be
                // recorded as trusted, so the handshake runs — fail closed.
                trusted: migrations::global_config::global_config_path().is_some_and(|p| {
                    migrations::global_config::check_has_trust_dialog_accepted(&p, &display)
                }),
                project_root: permission::set_cwd::project_root_of(&display)
                    .map(|p| p.to_string_lossy().into_owned()),
                // The control channel is served off the turn loop, so a
                // concurrently running turn is exactly what this guards.
                busy: _cwd_operation.is_none() || control_plane.is_busy().await,
            };
            match permission::set_cwd::decide_set_cwd(&request, &ctx) {
                permission::set_cwd::SetCwdDecision::Respond(
                    permission::set_cwd::SetCwdResponse::Invalid(message),
                ) => writer.reply_error(request_id, &message),
                permission::set_cwd::SetCwdDecision::Respond(
                    permission::set_cwd::SetCwdResponse::Rejected { reason, message },
                ) => writer.reply_success(
                    request_id,
                    Some(json!({
                        "status": "rejected",
                        "reason": reason.as_str(),
                        "message": message,
                    })),
                ),
                permission::set_cwd::SetCwdDecision::Respond(
                    permission::set_cwd::SetCwdResponse::NeedsTrust {
                        directory,
                        trust_root,
                    },
                ) => {
                    let mut payload = serde_json::Map::new();
                    payload.insert("status".into(), json!("needs_trust"));
                    payload.insert("directory".into(), json!(directory));
                    // Omitted, not null, when there is nothing useful to offer.
                    if let Some(root) = trust_root {
                        payload.insert("trust_root".into(), json!(root));
                    }
                    writer.reply_success(request_id, Some(Value::Object(payload)));
                }
                permission::set_cwd::SetCwdDecision::Respond(
                    permission::set_cwd::SetCwdResponse::AlreadyThere { cwd },
                ) => writer.reply_success(
                    request_id,
                    Some(json!({
                        "status": "ok",
                        "cwd": cwd,
                        "changed": false,
                        "transcript_relocated": true,
                    })),
                ),
                permission::set_cwd::SetCwdDecision::Proceed {
                    directory,
                    mark_trusted,
                } => {
                    // Record the trust BEFORE the move, so a crash in between
                    // leaves a trusted directory the user did approve rather
                    // than a session sitting in one it never confirmed.
                    if mark_trusted {
                        if let Some(cfg) = migrations::global_config::global_config_path() {
                            migrations::global_config::record_trust_accept(&cfg, &display);
                        }
                    }
                    let dir = std::path::PathBuf::from(&directory);
                    let previous = session_cwd.snapshot();
                    // The new cwd becomes the SOLE trusted directory, matching
                    // what `EnterWorktree` and the worktree restore already do.
                    // Any `--add-dir` extras are dropped rather than carried
                    // across: narrowing a trust boundary on a move is the safe
                    // direction, and inheriting the old session's extras into a
                    // directory the user has just been asked to trust would
                    // grant more than the prompt described.
                    let trusted = vec![dir.clone()];
                    session_cwd.swap(dir.clone(), trusted);
                    let transcript_path = match orchestrator.retarget_transcript_for_cwd(&dir).await
                    {
                        Ok(path) => path,
                        Err(error) => {
                            // Do not acknowledge a cwd move if its transcript could
                            // not be rehomed. Restore both cwd and trusted roots so
                            // a subsequent request cannot run against a different
                            // directory while still reading the old session file.
                            session_cwd.swap(previous.0, previous.1);
                            tracing::warn!(%error, "failed to retarget transcript after set_cwd; rolled back cwd");
                            writer.reply_error(
                                request_id,
                                &format!("Could not change directory: {error}"),
                            );
                            return;
                        }
                    };
                    if let Some(path) = transcript_path {
                        if let Err(error) =
                            crate::background_launch::refresh_current_background_launch_identity(
                                &dir, &path,
                            )
                        {
                            let body = crate::mode::rollback_cd_after_launch_identity_failure(
                                orchestrator,
                                session_cwd,
                                previous,
                                &error.to_string(),
                            )
                            .await;
                            tracing::warn!(
                                %error,
                                "rejected set_cwd because background launch identity is stale"
                            );
                            writer.reply_error(request_id, &body);
                            return;
                        }
                    }
                    writer.reply_success(
                        request_id,
                        Some(json!({
                            "status": "ok",
                            "cwd": directory,
                            "changed": true,
                            "transcript_relocated": true,
                        })),
                    );
                }
            }
        }
        "set_permission_mode" => {
            // §2.2 #4: the net-new runtime mode-mutation surface. The gate
            // parses + validates the wire mode and applies it live; success
            // echoes `{mode}`, an invalid/disallowed mode returns an error frame.
            let mode = field("mode").and_then(|v| v.as_str()).unwrap_or("default");
            match orchestrator.set_permission_mode(mode).await {
                Ok(()) => {
                    crate::permission_mode_preference::remember(mode);
                    writer.reply_success(request_id, Some(json!({"mode": mode})));
                }
                Err(e) => writer.reply_error(request_id, &e),
            }
        }
        "set_mcp_permission_mode_override" => {
            let server_name = field("serverName")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let mode = match field("mode") {
                None | Some(Value::Null) => None,
                Some(Value::String(s)) if s == "default" || s == "auto" => Some(s.as_str()),
                Some(Value::String(s))
                    if matches!(
                        s.as_str(),
                        "acceptEdits"
                            | "auto"
                            | "bypassPermissions"
                            | "default"
                            | "dontAsk"
                            | "plan"
                    ) =>
                {
                    writer.reply_error(
                        request_id,
                        &format!(
                            "Permission mode override over the control channel is tighten-only ('default', 'auto', or null); rejected '{s}'"
                        ),
                    );
                    return;
                }
                _ => {
                    writer.reply_error(
                        request_id,
                        "Cannot set permission mode: must be one of acceptEdits, auto, bypassPermissions, default, dontAsk, plan",
                    );
                    return;
                }
            };
            let known = orchestrator
                .list_mcp_servers()
                .await
                .into_iter()
                .any(|s| s.name == server_name);
            if !known {
                if let Err(e) = orchestrator
                    .set_mcp_permission_mode_override(&server_name, mode)
                    .await
                {
                    writer.reply_error(request_id, &e);
                    return;
                }
                let warning = match mode {
                    Some(_) => format!(
                        "MCP server '{server_name}' is not yet known; override stored but will not apply until a server with that exact name connects."
                    ),
                    None => format!(
                        "MCP server '{server_name}' is not known; no override was present to clear."
                    ),
                };
                writer.reply_success(request_id, Some(json!({"warning": warning})));
                return;
            }

            match orchestrator
                .set_mcp_permission_mode_override(&server_name, mode)
                .await
            {
                Ok(()) => writer.reply_success(request_id, None),
                Err(e) => writer.reply_error(request_id, &e),
            }
        }
        "end_session" => {
            // Cancel the actual owner too: idle notification turns do not use
            // the normal input turn's watch bridge.
            control_plane.shutdown("Session ended").await;
            let _ = cancel_tx.send(true);
            writer.reply_success(request_id, None);
            end_notify.notify_one();
        }
        "file_suggestions" => {
            let query = field("query").and_then(Value::as_str).unwrap_or("");
            let suggestions = file_suggestions
                .suggestions(&session_cwd.cwd(), query)
                .await
                .into_iter()
                .map(|path| json!({"path": path}))
                .collect::<Vec<_>>();
            writer.reply_success(request_id, Some(json!({"suggestions": suggestions})));
        }
        "seed_read_state" => {
            if let (Some(path), Some(mtime)) = (
                field("path").and_then(Value::as_str),
                field("mtime").and_then(Value::as_f64),
            ) {
                let _ = orchestrator.seed_read_state_from_host(path, mtime).await;
            }
            writer.reply_success(request_id, None);
        }
        "mcp_authenticate" | "mcp_reconnect" => {
            // ORACLE (2.1.201 `-p` handler): both branches first resolve the
            // MCP server config by `serverName`; when no server matches, they
            // reply `error: "Server not found: {serverName}"` (verified live —
            // `mcp_authenticate`/`mcp_reconnect` for an unknown server both
            // return that exact string). A fresh `-p` session has no MCP
            // servers, so this is the dominant observable path.
            //
            // DEFERRED (found-server path): the live handler then starts an
            // OAuth flow (mcp_authenticate → `{authUrl, requiresUserAction,…}`)
            // or tears down + reconnects the transport (mcp_reconnect → bare
            // success ack). The port's stream-json server has no live OAuth /
            // reconnect seam wired here, so a matched server is acked
            // best-effort: `mcp_reconnect` → bare success (mirrors the binary's
            // `Ur(_t)`), `mcp_authenticate` → success `{}`. Full flows tracked
            // as a follow-up.
            let server_name = field("serverName")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let known: Vec<String> = orchestrator
                .list_mcp_servers()
                .await
                .into_iter()
                .map(|s| s.name)
                .collect();
            if !known.iter().any(|n| n == &server_name) {
                writer.reply_error(request_id, &format!("Server not found: {server_name}"));
            } else if subtype == "mcp_reconnect" {
                writer.reply_success(request_id, None);
            } else {
                writer.reply_success(request_id, Some(json!({})));
            }
        }
        // The orchestrator-free arms (get_binary_version, message_rated,
        // mcp_oauth_callback_url), the CLI-originated guard subtypes (no-reply),
        // and the byte-exact `Unsupported control request subtype` fallthrough
        // are pure — classified by `pure_control_response` so the wire shapes
        // are unit-testable without a live orchestrator.
        other => match pure_control_response(other, frame) {
            PureControlReply::Success(payload) => writer.reply_success(request_id, payload),
            PureControlReply::Error(msg) => writer.reply_error(request_id, &msg),
            // CLI-originated subtype seen inbound — no control_response (see below).
            PureControlReply::Ignore => {}
        },
    }
}

/// Reply for a pure (orchestrator-free) control arm.
#[derive(Debug, PartialEq)]
pub(super) enum PureControlReply {
    /// `control_response` success; `None` ⇒ inner `response` key omitted.
    Success(Option<serde_json::Value>),
    /// `control_response` error with this message.
    Error(String),
    /// No `control_response` at all — a CLI-originated subtype seen inbound that
    /// the binary handles as a top-of-chain guard, never in the server switch.
    Ignore,
}

/// Classify the control arms that need no async orchestrator/registry access,
/// including the byte-exact `Unsupported control request subtype` fallthrough.
///
pub(super) fn pure_control_response(subtype: &str, frame: &serde_json::Value) -> PureControlReply {
    let field = |k: &str| frame.get("request").and_then(|r| r.get(k));
    match subtype {
        // §2.2 #8: `{version, buildTime}`.
        "get_binary_version" => PureControlReply::Success(Some(json!({
            "version": lingxi_core::host::CLAUDE_CODE_VERSION,
            "buildTime": ""
        }))),
        // §2.2 #45: telemetry-only; ack with `{}`.
        "message_rated" => PureControlReply::Success(Some(json!({}))),
        // ORACLE (2.1.201 `-p` handler): `mcp_oauth_callback_url` looks up the
        // in-flight OAuth flow for `serverName`; with no active flow it replies
        // `error: "No active OAuth flow for server: {serverName}"` (verified
        // live). The port keeps no active-flow registry in the stream-json
        // server, so this is always the faithful reply.
        "mcp_oauth_callback_url" => {
            let server_name = field("serverName").and_then(|v| v.as_str()).unwrap_or("");
            PureControlReply::Error(format!("No active OAuth flow for server: {server_name}"))
        }
        // CLI-ORIGINATED subtypes: `can_use_tool` / `request_user_dialog` /
        // `elicitation` are CLIENT→SERVER frames the CLI itself SENDS (their
        // `control_response` is handled by the resolver task). The binary checks
        // them as top-of-chain GUARDS routed to the StructuredIO pending-request
        // path — they NEVER enter this server switch nor reach the Unsupported
        // fallthrough. A well-behaved host never sends them inbound as a
        // control_request, so we emit NO control_response rather than erroring.
        "can_use_tool" | "request_user_dialog" | "elicitation" => PureControlReply::Ignore,
        // The binary fallthrough for every unhandled / deep [D] subtype.
        _ => PureControlReply::Error(format!("Unsupported control request subtype: {subtype}")),
    }
}

/// Build the `initialize` control_response `response` payload.
///
/// ORACLE (2.1.201, verified live via
/// `{"subtype":"initialize"} | claude -p --input-format stream-json \
///   --output-format stream-json --verbose`): the `-p` handler replies with
/// `{commands, agents, output_style, available_output_styles, models, account,
/// pid}` where `output_style` is `"default"` and `available_output_styles` is
/// the 4-item list `["default","Proactive","Explanatory","Learning"]`.
/// 2.1.220 (live re-capture) extends the tail with the remote-control gate
/// booleans and `fast_mode_state` / `fast_mode_disabled_reason`.
/// (NOTE: the separate REPL-bridge handler defaults these to `"normal"` /
/// `["normal"]`, but that bridge is NOT the `-p --input-format stream-json`
/// role this dispatcher models — the observable `-p` truth is the 4-item list.)
pub(super) fn initialize_response_payload(
    commands: &[serde_json::Value],
    agents: &[serde_json::Value],
    models: &[serde_json::Value],
    account: &serde_json::Value,
    pid: u32,
    fast_mode_state: &str,
    fast_mode_disabled_reason: Option<&str>,
) -> serde_json::Value {
    // 2.1.220 live capture appends five keys after `pid`:
    // `remote_control_auto_enable`, `remote_control_auto_on_by_default`,
    // `ide_rc_auto_enable_gate` (all `false` in a clean sandbox — LingXi has
    // no remote-control feature, an accepted divergence, so `false` is always
    // truthful), then `fast_mode_state` + optional `fast_mode_disabled_reason`.
    let mut payload = json!({
        "commands": commands,
        "agents": agents,
        "output_style": "default",
        "available_output_styles": ["default", "Proactive", "Explanatory", "Learning"],
        "models": models,
        "account": account,
        "pid": pid,
        "remote_control_auto_enable": false,
        "remote_control_auto_on_by_default": false,
        "ide_rc_auto_enable_gate": false,
        "fast_mode_state": fast_mode_state,
    });
    if let Some(reason) = fast_mode_disabled_reason {
        payload["fast_mode_disabled_reason"] = json!(reason);
    }
    payload
}

/// Map the raw inner `control_response.response` permission payload onto a
/// [`permission::gate::PermissionOutcome`] for orphaned-tool recovery.
///
/// Mirrors the allow/deny shape of `StdioControlPermissionGate::map_payload`
/// but is deliberately LENIENT where the live gate is strict: an `allow`
/// WITHOUT `updatedInput` is honoured (falling back to the original tool input)
/// rather than rejected — matching claude-code's `handleOrphanedPermission`,
/// which logs a warning and uses the original input when `updatedInput` is
/// `undefined` (queryHelpers.ts:262-272), instead of `map_payload`'s strict
/// §3.3 "missing updatedInput" deny used for live responses.
pub(super) fn orphan_decision_from_payload(
    payload: &serde_json::Value,
) -> permission::gate::PermissionOutcome {
    use permission::gate::PermissionOutcome;
    match payload.get("behavior").and_then(serde_json::Value::as_str) {
        Some("allow") => {
            // Carry `updatedInput` only when it is a non-empty object (claude-code
            // applies it "when it has keys"); otherwise fall back to the original.
            let updated_input = match payload.get("updatedInput") {
                Some(serde_json::Value::Object(m)) if !m.is_empty() => {
                    Some(lingxi_core::types::utf16_json::Utf16JsonProjection::plain(serde_json::Value::Object(m.clone())))
                }
                _ => None,
            };
            let permission_updates = payload
                .get("updatedPermissions")
                .and_then(serde_json::Value::as_array)
                .cloned()
                .unwrap_or_default();
            let decision_classification = payload
                .get("decisionClassification")
                .and_then(serde_json::Value::as_str)
                .and_then(|value| match value {
                    "user_temporary" => Some(
                        lingxi_core::host::permission_gate::ToolDecisionClassification::UserTemporary,
                    ),
                    "user_permanent" => Some(
                        lingxi_core::host::permission_gate::ToolDecisionClassification::UserPermanent,
                    ),
                    "user_reject" => {
                        Some(lingxi_core::host::permission_gate::ToolDecisionClassification::UserReject)
                    }
                    _ => None,
                });
            PermissionOutcome::Allow {
                updated_input,
                permission_updates,
                decision_classification,
            }
        }
        Some("deny") => PermissionOutcome::Deny {
            reason: payload
                .get("message")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("Tool permission denied")
                .to_string(),
        },
        // Any non-allow/deny behaviour is a schema-invalid result; deny safely
        // rather than execute on a malformed recovered decision.
        _ => PermissionOutcome::Deny {
            reason: "Tool permission request failed: malformed orphaned control_response"
                .to_string(),
        },
    }
}

/// Re-run a single ORPHANED tool: dequeued between turns, this looks the
/// unresolved `tool_use` up in the (resumed) session history and executes it
/// with the recovered permission decision. 1:1 with claude-code's
/// `handleOrphanedPermission` (queryHelpers.ts:224-343). Deduped per-toolUseID
/// via `handled_orphans` (twin of `handledOrphanedToolUseIds`, print.ts:2766):
/// a given id recovers once, but DISTINCT orphans each recover. An id is marked
/// handled ONLY on a real recovery (`Ok(true)`, which also covers the
/// unknown-tool case where the gate is consumed but nothing runs), so a
/// not-found orphan (`Ok(false)`) leaves a later same-id delivery able to
/// recover — matching claude-code, which adds to the Set only when
/// `findUnresolvedToolUse` succeeds.
pub(super) async fn recover_orphaned_permission(
    runtime: &Runtime,
    cmd: msgqueue::QueuedCommand,
    handled_orphans: &mut std::collections::HashSet<lingxi_core::types::ToolUseId>,
    cancel: tokio_util::sync::CancellationToken,
) {
    let msgqueue::QueuedCommandContent::OrphanedPermission {
        tool_use_id,
        permission_decision_json,
        ..
    } = cmd.content
    else {
        return;
    };
    if handled_orphans.contains(&tool_use_id) {
        tracing::debug!(
            "ignoring duplicate orphaned permission for toolUseID={} (already handled)",
            tool_use_id.as_str()
        );
        return;
    }
    let decision = orphan_decision_from_payload(&permission_decision_json);
    match runtime
        .orchestrator
        .run_orphaned_permission_with_cancel(&tool_use_id, decision, cancel)
        .await
    {
        Ok(true) => {
            handled_orphans.insert(tool_use_id.clone());
            tracing::info!(
                "recovered orphaned permission for toolUseID={}",
                tool_use_id.as_str()
            );
        }
        Ok(false) => {
            tracing::debug!(
                "orphaned permission toolUseID={} had no unresolved tool_use; skipped",
                tool_use_id.as_str()
            );
        }
        Err(e) => {
            tracing::warn!(
                "orphaned permission recovery failed for toolUseID={}: {e}",
                tool_use_id.as_str()
            );
        }
    }
}

/// Port of the 2.1.219 fast-mode reason resolver `JW()` (binary @227890982),
/// narrowed to the inputs reachable on the print/stream-json surface:
///
/// ```js
/// function JW(e){
///   if(!El())return xn()!=="firstParty"?"not_first_party":"disabled_by_env";
///   if(Ke("tengu_penguins_off",null)!==null)return"unknown";
///   if(!Hl(jkt())){…}                                    // model_not_allowed
///   let t=Hr("flagSettings")?.fastMode===!0;
///   if(_n()&&LVt()&&!t)return"sdk_opt_in_required";
///   if(mB.status==="pending"&&…)return"pending";
///   if(mB.status==="disabled"&&…)return mB.reason;       // free|preference|…
///   return null}
/// ```
///
/// * `El()` = firstParty provider && `!CLAUDE_CODE_DISABLE_FAST_MODE` (raw JS
///   truthiness — any non-empty value disables).
/// * `tengu_penguins_off` is a dynamic-config STRING read (`Ke(key,null)`);
///   with no fetcher wired the shipped binary resolves `null` there too, so
///   the port's flag-absent default falls through identically.
/// * `Hl` (org allowed-models policy) has no port surface — managed
///   `allowedModels` is unported, so `model_not_allowed` is unreachable.
/// * `_n()&&LVt()` — the SDK/non-interactive entrypoint check — is
///   constitutively TRUE here: this resolver only runs on the `-p`
///   stream-json/json paths, which ARE the Agent-SDK surface.
/// * The availability prober (`mB`) is unported; its `pending` and
///   `free|preference|extra_usage_disabled|network_error|unknown` arms are
///   unreachable, matching the fall-through `null` of an active status.
pub(super) fn resolve_fast_mode_disabled_reason(
    first_party: bool,
    sdk_fast_mode_opt_in: bool,
) -> Option<&'static str> {
    if !first_party {
        return Some("not_first_party");
    }
    if std::env::var(branding::DISABLE_FAST_MODE_ENV).is_ok_and(|v| !v.is_empty()) {
        return Some("disabled_by_env");
    }
    if !sdk_fast_mode_opt_in {
        return Some("sdk_opt_in_required");
    }
    None
}

/// Port of `cK(model, fastModeOptIn)` (binary @227895153) — the
/// `fast_mode_state` carried by `system/init`, the `initialize`
/// control_response and every `result` frame:
///
/// ```js
/// function cK(e,t){let r=El()&&QN()&&!!t&&fE(e);
///   if(r&&z0e())return"cooldown";if(r)return"on";return"off"}
/// ```
///
/// * `QN()` is `El()&&fde(undefined)===null` i.e. `El()&&JW()===null`, so
///   `El()&&QN()` collapses to "the disabled reason resolved to null" — the
///   value this function is handed.
/// * `fE(model)` (@227892311) is the canonical registry's `fast_mode`
///   capability. The UI state, initialize response, and request path all
///   consume the same table.
/// * `z0e()` (`"cooldown"`) rides the unported availability prober `mB` — the
///   same dead arm as `JW`'s `pending` / `disabled` branches.
pub(super) fn resolve_fast_mode_state(
    model: &str,
    fast_mode_disabled_reason: Option<&str>,
    sdk_fast_mode_opt_in: bool,
) -> &'static str {
    let model_supports_fast_mode = lingxi_core::host::model_capabilities::has_capability(
        model,
        lingxi_core::host::model_capabilities::ModelCapability::FastMode,
    );
    if fast_mode_disabled_reason.is_none() && sdk_fast_mode_opt_in && model_supports_fast_mode {
        "on"
    } else {
        "off"
    }
}

/// `Hr("flagSettings")?.fastMode===!0` — the Agent-SDK fast-mode opt-in
/// carried by `--settings` (inline JSON or a settings-file path). Strictly
/// boolean `true`, like the oracle's `===!0`.
pub(super) fn flag_settings_fast_mode_opt_in(settings: Option<&str>) -> bool {
    let Some(raw) = settings else { return false };
    let trimmed = raw.trim();
    let text = if trimmed.starts_with('{') {
        trimmed.to_string()
    } else {
        match std::fs::read_to_string(trimmed) {
            Ok(t) => t,
            Err(_) => return false,
        }
    };
    serde_json::from_str::<serde_json::Value>(&text)
        .ok()
        .and_then(|v| v.get("fastMode").and_then(serde_json::Value::as_bool))
        == Some(true)
}

/// Map a model's `request_model` string to its capability flags.
///
/// Returns `(supportsEffort, supportedEffortLevels, supportsAdaptiveThinking,
///           supportsFastMode, supportsAutoMode)`.
///
/// Refreshed to the 2.1.220 registry truth. The binary's initialize models
/// builder (print.ts @223434963) computes per-row: effort = `iw` (registry
/// "effort" capability), levels = `UR = [low,medium,high,xhigh,max]` filtered
/// by `BIe` ("max_effort") and `Zne` ("xhigh_effort", which additionally
/// EXCLUDES opus-4-6/sonnet-4-6 by name), adaptive = `Vit`
/// ("adaptive_thinking"), fast = `_h` (registry "fast_mode" or the
/// opus-4-7/opus-4-8 pair, gated on firstParty via `lc()`), auto = `mTe`
/// (true on firstParty for every non-legacy model). Capability sets come from
/// the baked-in catalog (binary blob @207769000..207775500). Legacy Claude
/// ids (claude-3-*, opus-4-0/4-1/4-5, sonnet-4-0/4-5, haiku-4-5) are the
/// shared exclusion list in all four predicates → all-false. Unknown /
/// non-Anthropic models keep all-false / empty defaults (multi-provider
/// divergence: the binary's `RN(Fh(e))` non-1P fallback has no lingxi seam).
pub(super) fn model_capabilities(
    request_model: &str,
) -> (bool, Vec<&'static str>, bool, bool, bool) {
    // The "default" pseudo-model: the binary computes capabilities on the
    // RESOLVED model (`r = R_()` for the Default row); lingxi's default
    // resolves to claude-sonnet-5 (2.1.197/198, M1).
    if request_model.eq_ignore_ascii_case("default") {
        return model_capabilities("claude-sonnet-5");
    }
    let capabilities =
        lingxi_core::host::model_capabilities::initialization_capabilities_for(request_model);
    (
        capabilities.supports_effort,
        capabilities.supported_effort_levels.to_vec(),
        capabilities.supports_adaptive_thinking,
        capabilities.supports_fast_mode,
        capabilities.supports_auto_mode,
    )
}
