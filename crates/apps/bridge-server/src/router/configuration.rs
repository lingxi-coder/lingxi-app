use super::EngineCommandRouter;
use crate::hook_admin;
use crate::mcp_admin;
use crate::plugin_admin;
use crate::skills_admin;
use client::adapter::ClientEventSink;
use client::protocol::commands::HookAdminCommandDto;
use client::protocol::commands::ListingKindDto;
use client::protocol::commands::McpAdminCommandDto;
use client::protocol::commands::PluginAdminCommandDto;
use client::protocol::commands::SkillAdminCommandDto;
use client::protocol::commands::WritableScopeDto;
use client::protocol::events::ClientEvent;
use client::protocol::events::ErrorKindDto;
use client::protocol::listings::ConfigurationDomainDto;
use client::protocol::listings::ConfigurationEffectDto;
use client::protocol::listings::ConfigurationOperationStatusDto;
use std::collections::HashMap;

/// Decode a `ClientCommand::UpsertMcpServer.config_json` wire string into the
/// `serde_json::Value` [`crate::mcp_bridge::upsert_server`] expects. Mirrors
/// [`parse_settings_patch`]'s object-shape validation: `config_json` must be
/// a JSON object (a `.mcp.json` entry is always `{command: ...}` or
/// `{url: ...}` shaped — never a bare string/array/number).
///
/// # Errors
/// `config_json` is not valid JSON, or it parses to something other than a
/// JSON object.
pub(super) fn parse_mcp_config_json(config_json: &str) -> Result<serde_json::Value, String> {
    match serde_json::from_str::<serde_json::Value>(config_json) {
        Ok(value @ serde_json::Value::Object(_)) => Ok(value),
        Ok(other) => Err(format!(
            "MCP server config must be a JSON object, got: {other}"
        )),
        Err(e) => Err(format!("MCP server config is not valid JSON: {e}")),
    }
}

impl EngineCommandRouter {
    /// Route [`ClientCommand::UpsertMcpServer`] to `mcp_bridge::upsert_server`.
    /// `config_json` is decoded and validated as a JSON object HERE (a
    /// malformed client payload is [`ErrorKindDto::Protocol`], matching
    /// [`apply_settings_patch`](Self::apply_settings_patch)'s
    /// `patch_json` handling); a write that fails once the shape is valid
    /// (broken destination file, non-object `mcpServers`, legacy bare-map
    /// `.mcp.json`, I/O failure) is [`ErrorKindDto::Internal`], matching every
    /// other write-failure path in this router. There is no dedicated success
    /// event — the desktop already has the wired `RefreshListings{Mcp}` path
    /// to observe the change (decision: adding a parallel listing here would
    /// duplicate that path, and the live `McpRegistry` snapshot it reads is
    /// not reloaded from disk by a bare file write, so re-emitting it here
    /// would not even show the new value).
    pub(super) async fn apply_mcp_upsert(
        &self,
        scope: WritableScopeDto,
        name: &str,
        config_json: &str,
        sink: &dyn ClientEventSink,
    ) {
        let Some(mcp) = self.mcp.as_ref() else {
            sink.emit(ClientEvent::Error {
                kind: ErrorKindDto::Internal,
                message: "MCP server update unavailable: this connection was built without an \
                          MCP context"
                    .to_string(),
            })
            .await;
            return;
        };

        if name.trim().is_empty() {
            // `mcp::json_config::build_servers_from_map` would turn a `""`
            // map key into a nameless server entry — reject at the wire
            // boundary rather than writing it. (Reserved-name collisions,
            // e.g. `computer-use` per `mcp/src/server_gate.rs`, are
            // deliberately NOT blocked here: a hardcoded name blocklist in
            // the writer would be a second source of truth that drifts from
            // `server_gate`'s own allowlist.)
            sink.emit(ClientEvent::Error {
                kind: ErrorKindDto::Protocol,
                message: "MCP server name must not be empty".to_string(),
            })
            .await;
            return;
        }

        let config = match parse_mcp_config_json(config_json) {
            Ok(config) => config,
            Err(message) => {
                sink.emit(ClientEvent::Error {
                    kind: ErrorKindDto::Protocol,
                    message,
                })
                .await;
                return;
            }
        };

        if let Err(message) = crate::mcp_bridge::upsert_server(mcp, scope, name, config) {
            sink.emit(ClientEvent::Error {
                kind: ErrorKindDto::Internal,
                message,
            })
            .await;
        }
    }
    /// Route [`ClientCommand::RemoveMcpServer`] to `mcp_bridge::remove_server`.
    /// Same context/error-kind shape as [`Self::apply_mcp_upsert`], minus the
    /// `config_json` decode (there is nothing to parse for a removal).
    pub(super) async fn apply_mcp_remove(
        &self,
        scope: WritableScopeDto,
        name: &str,
        sink: &dyn ClientEventSink,
    ) {
        let Some(mcp) = self.mcp.as_ref() else {
            sink.emit(ClientEvent::Error {
                kind: ErrorKindDto::Internal,
                message: "MCP server update unavailable: this connection was built without an \
                          MCP context"
                    .to_string(),
            })
            .await;
            return;
        };

        if let Err(message) = crate::mcp_bridge::remove_server(mcp, scope, name) {
            sink.emit(ClientEvent::Error {
                kind: ErrorKindDto::Internal,
                message,
            })
            .await;
        }
    }
    pub(super) async fn emit_configuration_operation(
        &self,
        sink: &dyn ClientEventSink,
        domain: ConfigurationDomainDto,
        operation_id: u64,
        status: ConfigurationOperationStatusDto,
        effect: ConfigurationEffectDto,
        message: Option<String>,
        details_json: Option<String>,
    ) {
        sink.emit(ClientEvent::ConfigurationOperation {
            domain,
            operation_id,
            status,
            effect,
            message,
            details_json,
        })
        .await;
    }
    pub(super) async fn emit_skill_catalog(&self, sink: &dyn ClientEventSink) {
        let Some(context) = self.skills_admin_context() else {
            sink.emit(ClientEvent::Error {
                kind: ErrorKindDto::Internal,
                message: "skill catalog unavailable: this connection was built without a settings context".to_string(),
            }).await;
            return;
        };
        match skills_admin::catalog_json(&context) {
            Ok(catalog_json) => {
                let catalog_json = match serde_json::from_str::<serde_json::Value>(&catalog_json) {
                    Ok(mut catalog) => {
                        let runtime_skills = self.handle.list_skills().await;
                        let entries = catalog
                            .get_mut("entries")
                            .and_then(serde_json::Value::as_array_mut);
                        if let Some(entries) = entries {
                            let known = entries
                                .iter()
                                .filter_map(|entry| {
                                    entry.get("directory").and_then(serde_json::Value::as_str)
                                })
                                .map(ToOwned::to_owned)
                                .collect::<std::collections::BTreeSet<_>>();
                            for skill in runtime_skills {
                                let directory = skill.source_dir.to_string_lossy().into_owned();
                                if known.contains(&directory) {
                                    continue;
                                }
                                let source = if directory.starts_with('<') {
                                    "mcp"
                                } else if directory.starts_with(
                                    &context
                                        .lingxi_home
                                        .join("plugins")
                                        .to_string_lossy()
                                        .into_owned(),
                                ) {
                                    "plugin"
                                } else {
                                    "runtime"
                                };
                                entries.push(serde_json::json!({
                                    "id": format!("runtime:{}", skill.name),
                                    "name": skill.name,
                                    "source": source,
                                    "rootDir": directory,
                                    "directory": directory,
                                    "writable": false,
                                    "readonlyReason": format!("{source} skills are runtime-derived and read-only."),
                                    "diagnosticsJson": "{\"status\":\"runtime\"}"
                                }));
                            }
                            if let Some(registry) = self.slash_registry.as_ref() {
                                let registry = registry.read().await;
                                for command in registry.list_all() {
                                    match &command.kind {
                                        command_api::SlashCommandKind::Plugin {
                                            plugin_id,
                                            file_path,
                                            ..
                                        } if command.skill_root.is_some() => {
                                            let directory = command
                                                .skill_root
                                                .as_deref()
                                                .or_else(|| file_path.parent())
                                                .unwrap_or(file_path)
                                                .to_string_lossy()
                                                .into_owned();
                                            let revision =
                                                std::fs::read(file_path).ok().map(|bytes| {
                                                    crate::config_admin::sha256_hex(&bytes)
                                                });
                                            entries.push(serde_json::json!({
                                                "id": format!("command:{}", command.name),
                                                "name": command.name,
                                                "source": "plugin",
                                                "pluginOwner": plugin_id.to_string(),
                                                "rootDir": directory,
                                                "directory": directory,
                                                "writable": false,
                                                "readonlyReason": "Plugin skills are managed by their owning plugin.",
                                                "revision": revision,
                                                "description": command.description,
                                                "whenToUse": command.when_to_use,
                                                "diagnosticsJson": "{\"ok\":true,\"issues\":[]}",
                                            }));
                                        }
                                        command_api::SlashCommandKind::Mcp {
                                            connection_id,
                                            prompt_name,
                                            ..
                                        } => {
                                            let directory =
                                                format!("<mcp:{connection_id}:{prompt_name}>");
                                            entries.push(serde_json::json!({
                                                "id": format!("command:{}", command.name),
                                                "name": command.name,
                                                "source": "mcp",
                                                "rootDir": directory,
                                                "directory": directory,
                                                "writable": false,
                                                "readonlyReason": "MCP skills are provided by the connected server.",
                                                "description": command.description,
                                                "whenToUse": command.when_to_use,
                                                "diagnosticsJson": "{\"status\":\"runtime\"}",
                                            }));
                                        }
                                        _ => {}
                                    }
                                }
                            }
                        }
                        catalog.to_string()
                    }
                    Err(_) => catalog_json,
                };
                sink.emit(ClientEvent::SkillCatalog { catalog_json }).await
            }
            Err(message) => {
                sink.emit(ClientEvent::Error {
                    kind: ErrorKindDto::Internal,
                    message,
                })
                .await
            }
        }
    }
    pub(super) async fn emit_skill_document(&self, skill_id: &str, sink: &dyn ClientEventSink) {
        let Some(context) = self.skills_admin_context() else {
            sink.emit(ClientEvent::Error {
                kind: ErrorKindDto::Internal,
                message: "skill document unavailable: this connection was built without a settings context".to_string(),
            }).await;
            return;
        };
        if let Some(name) = skill_id.strip_prefix("command:") {
            let command = match self.slash_registry.as_ref() {
                Some(registry) => registry.read().await.resolve(name).cloned(),
                None => None,
            };
            if let Some(command) = command {
                let document = match command.kind {
                    command_api::SlashCommandKind::Plugin {
                        plugin_id,
                        file_path,
                        ..
                    } if command.skill_root.is_some() => {
                        let markdown = std::fs::read_to_string(&file_path).unwrap_or_default();
                        let directory = command
                            .skill_root
                            .as_deref()
                            .or_else(|| file_path.parent())
                            .unwrap_or(&file_path)
                            .to_string_lossy()
                            .into_owned();
                        serde_json::json!({
                            "id": skill_id,
                            "name": command.name,
                            "source": "plugin",
                            "pluginOwner": plugin_id.to_string(),
                            "directory": directory,
                            "rootDir": directory,
                            "markdown": markdown,
                            "writable": false,
                            "revision": crate::config_admin::sha256_hex(markdown.as_bytes()),
                            "readonlyReason": "Plugin skills are managed by their owning plugin.",
                            "diagnosticsJson": "{\"ok\":true,\"issues\":[]}",
                        })
                    }
                    command_api::SlashCommandKind::Mcp {
                        connection_id,
                        prompt_name,
                        ..
                    } => {
                        let directory = format!("<mcp:{connection_id}:{prompt_name}>");
                        serde_json::json!({
                            "id": skill_id,
                            "name": command.name,
                            "source": "mcp",
                            "directory": directory,
                            "rootDir": directory,
                            "markdown": "",
                            "writable": false,
                            "revision": crate::config_admin::sha256_hex(&[]),
                            "readonlyReason": "MCP skills are provided by the connected server.",
                            "diagnosticsJson": "{\"status\":\"runtime\"}",
                        })
                    }
                    _ => serde_json::Value::Null,
                };
                if !document.is_null() {
                    sink.emit(ClientEvent::SkillDocument {
                        document_json: document.to_string(),
                    })
                    .await;
                    return;
                }
            }
        }
        if let Some(name) = skill_id.strip_prefix("runtime:") {
            if let Some(skill) = self
                .handle
                .list_skills()
                .await
                .into_iter()
                .find(|skill| skill.name == name)
            {
                let directory = skill.source_dir.to_string_lossy().into_owned();
                let source = if directory.starts_with('<') {
                    "mcp"
                } else if directory.starts_with(
                    &context
                        .lingxi_home
                        .join("plugins")
                        .to_string_lossy()
                        .into_owned(),
                ) {
                    "plugin"
                } else {
                    "runtime"
                };
                let document_json = serde_json::json!({
                    "id": skill_id,
                    "name": skill.name,
                    "source": source,
                    "directory": directory,
                    "rootDir": directory,
                    "markdown": "",
                    "writable": false,
                    "revision": crate::config_admin::sha256_hex(&[]),
                    "readonlyReason": format!("{source} skills are runtime-derived and read-only."),
                    "diagnosticsJson": "{\"status\":\"runtime\"}"
                })
                .to_string();
                sink.emit(ClientEvent::SkillDocument { document_json })
                    .await;
                return;
            }
        }
        match skills_admin::document_json(skill_id, &context) {
            Ok(document_json) => {
                sink.emit(ClientEvent::SkillDocument { document_json })
                    .await
            }
            Err(message) => {
                sink.emit(ClientEvent::Error {
                    kind: ErrorKindDto::Internal,
                    message,
                })
                .await
            }
        }
    }
    pub(super) fn skills_admin_context(&self) -> Option<skills_admin::SkillsAdminContext> {
        self.settings
            .as_ref()
            .map(|context| skills_admin::SkillsAdminContext {
                cwd: context.paths.project_dir.clone(),
                lingxi_home: context.paths.lingxi_home.clone(),
            })
    }
    pub(super) async fn refresh_skills_runtime(&self) -> Result<(), String> {
        let Some(reloader) = self.repo_root_reloader.as_ref() else {
            return Err("no skill catalog reloader wired".to_string());
        };
        let root = self
            .settings
            .as_ref()
            .map(|settings| settings.paths.project_dir.clone())
            .ok_or_else(|| "no skill settings context wired".to_string())?;
        let outcome = reloader
            .reload(platform_api::RepoRootReloadRequest {
                root,
                reload_skills: true,
                reload_plugins: false,
            })
            .await;
        if outcome.skills_reloaded && outcome.errors.is_empty() {
            Ok(())
        } else if outcome.errors.is_empty() {
            Err("skill catalog did not confirm a live reload".to_string())
        } else {
            Err(outcome.errors.join("; "))
        }
    }
    pub(super) async fn apply_skill_admin(
        &self,
        command: SkillAdminCommandDto,
        sink: &dyn ClientEventSink,
    ) {
        match command.action.as_str() {
            "get_catalog" => self.emit_skill_catalog(sink).await,
            "get_document" => {
                let Some(skill_id) = command.target.as_deref() else {
                    sink.emit(ClientEvent::Error {
                        kind: ErrorKindDto::Protocol,
                        message: "skill document target is required".to_string(),
                    })
                    .await;
                    return;
                };
                self.emit_skill_document(skill_id, sink).await;
            }
            "save_document" | "create_skill" | "move_skill" | "trash_skill" | "restore_skill"
            | "purge_trash_skill" => {
                let Some(operation_id) = command.operation_id else {
                    sink.emit(ClientEvent::Error {
                        kind: ErrorKindDto::Protocol,
                        message: "skill mutation operation_id is required".to_string(),
                    })
                    .await;
                    return;
                };
                self.emit_configuration_operation(
                    sink,
                    ConfigurationDomainDto::Skill,
                    operation_id,
                    ConfigurationOperationStatusDto::Started,
                    ConfigurationEffectDto::NotApplicable,
                    Some("Applying skill change…".to_string()),
                    None,
                )
                .await;
                let result = self
                    .skills_admin_context()
                    .ok_or_else(|| "skill admin unavailable: missing settings context".to_string())
                    .and_then(|context| skills_admin::handle(command.clone(), &context));
                match result {
                    Ok(skills_admin::SkillAdminOutcome::Changed {
                        catalog_json,
                        document_json,
                    }) => {
                        let effect = if self.is_turn_active() {
                            ConfigurationEffectDto::RestartRequired
                        } else {
                            self.refresh_skills_runtime()
                                .await
                                .map(|_| ConfigurationEffectDto::Applied)
                                .unwrap_or(ConfigurationEffectDto::RestartRequired)
                        };
                        self.emit_configuration_operation(
                            sink,
                            ConfigurationDomainDto::Skill,
                            operation_id,
                            ConfigurationOperationStatusDto::Succeeded,
                            effect,
                            Some("Applied skill change.".to_string()),
                            None,
                        )
                        .await;
                        sink.emit(ClientEvent::SkillCatalog { catalog_json }).await;
                        if let Some(document_json) = document_json {
                            sink.emit(ClientEvent::SkillDocument { document_json })
                                .await;
                        }
                        self.emit_listing(ListingKindDto::Skills, sink).await;
                    }
                    Ok(_) => {
                        self.emit_configuration_operation(
                            sink,
                            ConfigurationDomainDto::Skill,
                            operation_id,
                            ConfigurationOperationStatusDto::Failed,
                            ConfigurationEffectDto::NotApplicable,
                            Some("skill mutation returned no change payload".to_string()),
                            None,
                        )
                        .await;
                    }
                    Err(message) => {
                        self.emit_configuration_operation(
                            sink,
                            ConfigurationDomainDto::Skill,
                            operation_id,
                            ConfigurationOperationStatusDto::Failed,
                            ConfigurationEffectDto::NotApplicable,
                            Some(message),
                            None,
                        )
                        .await;
                    }
                }
            }
            action => {
                sink.emit(ClientEvent::Error {
                    kind: ErrorKindDto::Protocol,
                    message: format!("unsupported skill admin action: {action}"),
                })
                .await
            }
        }
    }
    pub(super) async fn emit_mcp_configuration_snapshot(&self, sink: &dyn ClientEventSink) {
        let (Some(context), Some(paths)) = (self.settings.as_ref(), self.mcp.as_ref()) else {
            sink.emit(ClientEvent::Error {
                kind: ErrorKindDto::Internal,
                message: "MCP configuration snapshot unavailable: missing settings or MCP context"
                    .to_string(),
            })
            .await;
            return;
        };
        let mut runtime_servers = Vec::new();
        for server in self.handle.list_mcp_servers().await {
            let config = match self.mcp_registry.as_ref() {
                Some(registry) => registry.get_config(&server.name).await,
                None => None,
            };
            let (source, writable, read_only_reason) = match config.as_ref() {
                Some(config)
                    if config.metadata.agent_source == Some(mcp::McpAgentSource::Plugin) =>
                {
                    ("plugin", false, Some("由插件注入；请在 Plugins 设置中管理"))
                }
                Some(config) => match config.scope {
                    mcp::ConfigScope::Settings(protocol::SettingsScope::User) => {
                        ("user", true, None)
                    }
                    mcp::ConfigScope::Settings(protocol::SettingsScope::Local) => {
                        ("local", true, None)
                    }
                    mcp::ConfigScope::Settings(protocol::SettingsScope::Project) => {
                        ("project", true, None)
                    }
                    mcp::ConfigScope::Dynamic => ("dynamic", false, Some("由当前会话动态注入")),
                    mcp::ConfigScope::Enterprise => ("enterprise", false, Some("由企业配置管理")),
                    mcp::ConfigScope::ClaudeAi => {
                        ("claude_ai", false, Some("由 Claude.ai 连接提供"))
                    }
                    mcp::ConfigScope::Settings(protocol::SettingsScope::Managed) => {
                        ("managed", false, Some("由管理员策略管理"))
                    }
                    mcp::ConfigScope::Agent => ("agent", false, Some("由 Agent frontmatter 注入")),
                },
                None => ("runtime", false, Some("仅存在于当前运行态")),
            };
            runtime_servers.push(serde_json::json!({
                "name": server.name,
                "status": match server.status {
                    platform_api::McpStatus::Connected => serde_json::json!("connected"),
                    platform_api::McpStatus::Disconnected => serde_json::json!("disconnected"),
                    platform_api::McpStatus::Error(reason) => serde_json::json!({ "type": "error", "reason": reason }),
                },
                "transport": server.transport,
                "source": source,
                "writable": writable,
                "read_only_reason": read_only_reason,
            }));
        }
        match mcp_admin::snapshot_json(context, paths, runtime_servers) {
            Ok(snapshot_json) => {
                sink.emit(ClientEvent::McpConfigurationSnapshot { snapshot_json })
                    .await
            }
            Err(message) => {
                sink.emit(ClientEvent::Error {
                    kind: ErrorKindDto::Internal,
                    message,
                })
                .await
            }
        }
    }
    pub(super) async fn apply_mcp_admin(
        &self,
        command: McpAdminCommandDto,
        sink: &dyn ClientEventSink,
    ) {
        match command.action.as_str() {
            "get_snapshot" => self.emit_mcp_configuration_snapshot(sink).await,
            "save_server" => {
                let Some(operation_id) = command.operation_id else {
                    sink.emit(ClientEvent::Error {
                        kind: ErrorKindDto::Protocol,
                        message: "MCP save operation_id is required".to_string(),
                    })
                    .await;
                    return;
                };
                let Some(revision_sha256) = command.revision.as_deref() else {
                    sink.emit(ClientEvent::Error {
                        kind: ErrorKindDto::Protocol,
                        message: "MCP save revision is required".to_string(),
                    })
                    .await;
                    return;
                };
                let Some(payload_json) = command.payload_json.as_deref() else {
                    sink.emit(ClientEvent::Error {
                        kind: ErrorKindDto::Protocol,
                        message: "MCP save payload is required".to_string(),
                    })
                    .await;
                    return;
                };
                self.emit_configuration_operation(
                    sink,
                    ConfigurationDomainDto::Mcp,
                    operation_id,
                    ConfigurationOperationStatusDto::Started,
                    ConfigurationEffectDto::NotApplicable,
                    Some("Saving MCP server…".to_string()),
                    None,
                )
                .await;
                let result = self
                    .mcp
                    .as_ref()
                    .ok_or_else(|| "MCP save unavailable: missing MCP context".to_string())
                    .and_then(|paths| {
                        mcp_admin::save_server_entry(paths, revision_sha256, payload_json)
                    });
                match result {
                    Ok(()) => {
                        let effect = if self.is_turn_active() {
                            ConfigurationEffectDto::RestartRequired
                        } else {
                            self.refresh_mcp_runtime()
                                .await
                                .map(|_| ConfigurationEffectDto::Applied)
                                .unwrap_or(ConfigurationEffectDto::RestartRequired)
                        };
                        self.emit_configuration_operation(
                            sink,
                            ConfigurationDomainDto::Mcp,
                            operation_id,
                            ConfigurationOperationStatusDto::Succeeded,
                            effect,
                            Some("Saved MCP server configuration.".to_string()),
                            None,
                        )
                        .await;
                        self.emit_mcp_configuration_snapshot(sink).await;
                        self.emit_listing(ListingKindDto::Mcp, sink).await;
                    }
                    Err(message) => {
                        self.emit_configuration_operation(
                            sink,
                            ConfigurationDomainDto::Mcp,
                            operation_id,
                            ConfigurationOperationStatusDto::Failed,
                            ConfigurationEffectDto::NotApplicable,
                            Some(message),
                            None,
                        )
                        .await
                    }
                }
            }
            "remove_server" => {
                let Some(operation_id) = command.operation_id else {
                    sink.emit(ClientEvent::Error {
                        kind: ErrorKindDto::Protocol,
                        message: "MCP remove operation_id is required".to_string(),
                    })
                    .await;
                    return;
                };
                let Some(revision_sha256) = command.revision.as_deref() else {
                    sink.emit(ClientEvent::Error {
                        kind: ErrorKindDto::Protocol,
                        message: "MCP remove revision is required".to_string(),
                    })
                    .await;
                    return;
                };
                let Some(payload_json) = command.payload_json.as_deref() else {
                    sink.emit(ClientEvent::Error {
                        kind: ErrorKindDto::Protocol,
                        message: "MCP remove payload is required".to_string(),
                    })
                    .await;
                    return;
                };
                self.emit_configuration_operation(
                    sink,
                    ConfigurationDomainDto::Mcp,
                    operation_id,
                    ConfigurationOperationStatusDto::Started,
                    ConfigurationEffectDto::NotApplicable,
                    Some("Removing MCP server…".to_string()),
                    None,
                )
                .await;
                let result = self
                    .mcp
                    .as_ref()
                    .ok_or_else(|| "MCP remove unavailable: missing MCP context".to_string())
                    .and_then(|paths| {
                        mcp_admin::remove_server_entry(paths, revision_sha256, payload_json)
                    });
                match result {
                    Ok(()) => {
                        let effect = if self.is_turn_active() {
                            ConfigurationEffectDto::RestartRequired
                        } else {
                            self.refresh_mcp_runtime()
                                .await
                                .map(|_| ConfigurationEffectDto::Applied)
                                .unwrap_or(ConfigurationEffectDto::RestartRequired)
                        };
                        self.emit_configuration_operation(
                            sink,
                            ConfigurationDomainDto::Mcp,
                            operation_id,
                            ConfigurationOperationStatusDto::Succeeded,
                            effect,
                            Some("Removed MCP server configuration.".to_string()),
                            None,
                        )
                        .await;
                        self.emit_mcp_configuration_snapshot(sink).await;
                        self.emit_listing(ListingKindDto::Mcp, sink).await;
                    }
                    Err(message) => {
                        self.emit_configuration_operation(
                            sink,
                            ConfigurationDomainDto::Mcp,
                            operation_id,
                            ConfigurationOperationStatusDto::Failed,
                            ConfigurationEffectDto::NotApplicable,
                            Some(message),
                            None,
                        )
                        .await
                    }
                }
            }
            "set_approval" => {
                let Some(operation_id) = command.operation_id else {
                    sink.emit(ClientEvent::Error {
                        kind: ErrorKindDto::Protocol,
                        message: "MCP approval operation_id is required".to_string(),
                    })
                    .await;
                    return;
                };
                let Some(revision_sha256) = command.revision.as_deref() else {
                    sink.emit(ClientEvent::Error {
                        kind: ErrorKindDto::Protocol,
                        message: "MCP approval revision is required".to_string(),
                    })
                    .await;
                    return;
                };
                let Some(payload_json) = command.payload_json.as_deref() else {
                    sink.emit(ClientEvent::Error {
                        kind: ErrorKindDto::Protocol,
                        message: "MCP approval payload is required".to_string(),
                    })
                    .await;
                    return;
                };
                self.emit_configuration_operation(
                    sink,
                    ConfigurationDomainDto::Mcp,
                    operation_id,
                    ConfigurationOperationStatusDto::Started,
                    ConfigurationEffectDto::NotApplicable,
                    Some("Saving MCP approval…".to_string()),
                    None,
                )
                .await;
                let result = self
                    .settings
                    .as_ref()
                    .ok_or_else(|| "MCP approval unavailable: missing settings context".to_string())
                    .and_then(|context| {
                        mcp_admin::set_project_approval(context, revision_sha256, payload_json)
                    });
                match result {
                    Ok(()) => {
                        let effect = if self.is_turn_active() {
                            ConfigurationEffectDto::RestartRequired
                        } else {
                            match self.refresh_mcp_runtime().await {
                                Ok(()) => {
                                    if let Some(settings) = self.settings.as_ref() {
                                        settings.mark_keys_applied(&[
                                            "enabledMcpjsonServers",
                                            "disabledMcpjsonServers",
                                            "enableAllProjectMcpServers",
                                        ]);
                                    }
                                    ConfigurationEffectDto::Applied
                                }
                                Err(_) => ConfigurationEffectDto::RestartRequired,
                            }
                        };
                        self.emit_configuration_operation(
                            sink,
                            ConfigurationDomainDto::Mcp,
                            operation_id,
                            ConfigurationOperationStatusDto::Succeeded,
                            effect,
                            Some("Saved project MCP approval.".to_string()),
                            None,
                        )
                        .await;
                        self.emit_mcp_configuration_snapshot(sink).await;
                        self.emit_settings_snapshot(sink).await;
                        self.emit_listing(ListingKindDto::Mcp, sink).await;
                    }
                    Err(message) => {
                        self.emit_configuration_operation(
                            sink,
                            ConfigurationDomainDto::Mcp,
                            operation_id,
                            ConfigurationOperationStatusDto::Failed,
                            ConfigurationEffectDto::NotApplicable,
                            Some(message),
                            None,
                        )
                        .await
                    }
                }
            }
            action => {
                sink.emit(ClientEvent::Error {
                    kind: ErrorKindDto::Protocol,
                    message: format!("unsupported MCP admin action: {action}"),
                })
                .await
            }
        }
    }
    pub(super) async fn emit_plugin_catalog(&self, sink: &dyn ClientEventSink) {
        let Some(context) = self.settings.as_ref() else {
            sink.emit(ClientEvent::Error {
                kind: ErrorKindDto::Internal,
                message: "plugin catalog unavailable: missing settings context".to_string(),
            })
            .await;
            return;
        };
        match plugin_admin::catalog_json(context, self.credentials.as_deref()).await {
            Ok(catalog_json) => sink.emit(ClientEvent::PluginCatalog { catalog_json }).await,
            Err(message) => {
                sink.emit(ClientEvent::Error {
                    kind: ErrorKindDto::Internal,
                    message,
                })
                .await
            }
        }
    }
    pub(super) async fn refresh_plugins_runtime(&self) -> Result<(), String> {
        let Some(runtime) = self.plugin_runtime.as_ref() else {
            return Err("plugin runtime is unavailable".to_string());
        };
        let counts = runtime.refresh().await;
        if counts.errors == 0 {
            Ok(())
        } else {
            Err(format!(
                "plugin runtime refresh reported {} component error(s)",
                counts.errors
            ))
        }
    }
    pub(super) async fn refresh_mcp_runtime(&self) -> Result<(), String> {
        let Some(registry) = self.mcp_registry.as_ref() else {
            return Err("MCP registry is unavailable".to_string());
        };
        let Some(paths) = self.mcp.as_ref() else {
            return Err("MCP paths are unavailable".to_string());
        };
        crate::mcp_bridge::reconcile_writable_servers(registry, paths).await
    }
    pub(super) async fn apply_plugin_admin(
        &self,
        command: PluginAdminCommandDto,
        sink: &dyn ClientEventSink,
    ) {
        match command.action.as_str() {
            "get_catalog" => self.emit_plugin_catalog(sink).await,
            "preview_operation" => {
                let Some(operation_id) = command.operation_id else {
                    sink.emit(ClientEvent::Error {
                        kind: ErrorKindDto::Protocol,
                        message: "plugin preview operation_id is required".to_string(),
                    })
                    .await;
                    return;
                };
                let Some(payload_json) = command.payload_json.as_deref() else {
                    sink.emit(ClientEvent::Error {
                        kind: ErrorKindDto::Protocol,
                        message: "plugin preview payload is required".to_string(),
                    })
                    .await;
                    return;
                };
                let details = plugin_admin::preview_operation(
                    self.settings.as_ref(),
                    command.revision.as_deref(),
                    payload_json,
                )
                .await;
                match details {
                    Ok(details_json) => {
                        self.emit_configuration_operation(
                            sink,
                            ConfigurationDomainDto::Plugin,
                            operation_id,
                            ConfigurationOperationStatusDto::Succeeded,
                            ConfigurationEffectDto::NotApplicable,
                            Some("Plugin operation preview ready.".to_string()),
                            Some(details_json),
                        )
                        .await
                    }
                    Err(message) => {
                        self.emit_configuration_operation(
                            sink,
                            ConfigurationDomainDto::Plugin,
                            operation_id,
                            ConfigurationOperationStatusDto::Failed,
                            ConfigurationEffectDto::NotApplicable,
                            Some(message),
                            None,
                        )
                        .await
                    }
                }
            }
            "apply_operation" => {
                let Some(operation_id) = command.operation_id else {
                    sink.emit(ClientEvent::Error {
                        kind: ErrorKindDto::Protocol,
                        message: "plugin apply operation_id is required".to_string(),
                    })
                    .await;
                    return;
                };
                let Some(revision_sha256) = command.revision.as_deref() else {
                    sink.emit(ClientEvent::Error {
                        kind: ErrorKindDto::Protocol,
                        message: "plugin apply revision is required".to_string(),
                    })
                    .await;
                    return;
                };
                let Some(payload_json) = command.payload_json.as_deref() else {
                    sink.emit(ClientEvent::Error {
                        kind: ErrorKindDto::Protocol,
                        message: "plugin apply payload is required".to_string(),
                    })
                    .await;
                    return;
                };
                self.emit_configuration_operation(
                    sink,
                    ConfigurationDomainDto::Plugin,
                    operation_id,
                    ConfigurationOperationStatusDto::Started,
                    ConfigurationEffectDto::NotApplicable,
                    Some("Applying plugin operation…".to_string()),
                    None,
                )
                .await;
                let result = match self.settings.as_ref() {
                    Some(context) => {
                        plugin_admin::apply_operation(context, revision_sha256, payload_json).await
                    }
                    None => Err("plugin apply unavailable: missing settings context".to_string()),
                };
                match result {
                    Ok(operation_message) => {
                        let (effect, reload_error) = if self.is_turn_active() {
                            (ConfigurationEffectDto::RestartRequired, None)
                        } else {
                            match self.refresh_plugins_runtime().await {
                                Ok(()) => {
                                    if let Some(settings) = self.settings.as_ref() {
                                        settings.mark_keys_applied(&[
                                            "enabledPlugins",
                                            "pluginConfigs",
                                            "extraKnownMarketplaces",
                                        ]);
                                    }
                                    (ConfigurationEffectDto::Applied, None)
                                }
                                Err(error) => {
                                    (ConfigurationEffectDto::RestartRequired, Some(error))
                                }
                            }
                        };
                        self.emit_configuration_operation(
                            sink,
                            ConfigurationDomainDto::Plugin,
                            operation_id,
                            ConfigurationOperationStatusDto::Succeeded,
                            effect,
                            Some(operation_message),
                            reload_error.map(|error| {
                                serde_json::json!({ "reloadError": error }).to_string()
                            }),
                        )
                        .await;
                        self.emit_settings_snapshot(sink).await;
                        self.emit_plugin_catalog(sink).await;
                    }
                    Err(message) => {
                        self.emit_configuration_operation(
                            sink,
                            ConfigurationDomainDto::Plugin,
                            operation_id,
                            ConfigurationOperationStatusDto::Failed,
                            ConfigurationEffectDto::NotApplicable,
                            Some(message),
                            None,
                        )
                        .await
                    }
                }
            }
            "save_config" => {
                let Some(operation_id) = command.operation_id else {
                    sink.emit(ClientEvent::Error {
                        kind: ErrorKindDto::Protocol,
                        message: "plugin save operation_id is required".to_string(),
                    })
                    .await;
                    return;
                };
                let Some(revision_sha256) = command.revision.as_deref() else {
                    sink.emit(ClientEvent::Error {
                        kind: ErrorKindDto::Protocol,
                        message: "plugin save revision is required".to_string(),
                    })
                    .await;
                    return;
                };
                let Some(payload_json) = command.payload_json.as_deref() else {
                    sink.emit(ClientEvent::Error {
                        kind: ErrorKindDto::Protocol,
                        message: "plugin save payload is required".to_string(),
                    })
                    .await;
                    return;
                };
                self.emit_configuration_operation(
                    sink,
                    ConfigurationDomainDto::Plugin,
                    operation_id,
                    ConfigurationOperationStatusDto::Started,
                    ConfigurationEffectDto::NotApplicable,
                    Some("Saving plugin settings…".to_string()),
                    None,
                )
                .await;
                let result = match self.settings.as_ref() {
                    Some(context) => {
                        plugin_admin::save_config(context, revision_sha256, payload_json).await
                    }
                    None => {
                        Err("plugin config save unavailable: missing settings context".to_string())
                    }
                };
                match result {
                    Ok(()) => {
                        let (effect, reload_error) = if self.is_turn_active() {
                            (ConfigurationEffectDto::RestartRequired, None)
                        } else {
                            match self.refresh_plugins_runtime().await {
                                Ok(()) => {
                                    if let Some(settings) = self.settings.as_ref() {
                                        settings.mark_keys_applied(&[
                                            "enabledPlugins",
                                            "pluginConfigs",
                                            "extraKnownMarketplaces",
                                        ]);
                                    }
                                    (ConfigurationEffectDto::Applied, None)
                                }
                                Err(error) => {
                                    (ConfigurationEffectDto::RestartRequired, Some(error))
                                }
                            }
                        };
                        self.emit_configuration_operation(
                            sink,
                            ConfigurationDomainDto::Plugin,
                            operation_id,
                            ConfigurationOperationStatusDto::Succeeded,
                            effect,
                            Some("Saved plugin settings.".to_string()),
                            reload_error.map(|error| {
                                serde_json::json!({ "reloadError": error }).to_string()
                            }),
                        )
                        .await;
                        self.emit_settings_snapshot(sink).await;
                        self.emit_plugin_catalog(sink).await;
                    }
                    Err(message) => {
                        self.emit_configuration_operation(
                            sink,
                            ConfigurationDomainDto::Plugin,
                            operation_id,
                            ConfigurationOperationStatusDto::Failed,
                            ConfigurationEffectDto::NotApplicable,
                            Some(message),
                            None,
                        )
                        .await
                    }
                }
            }
            action => {
                sink.emit(ClientEvent::Error {
                    kind: ErrorKindDto::Protocol,
                    message: format!("unsupported plugin admin action: {action}"),
                })
                .await
            }
        }
    }
    pub(super) async fn emit_hook_document(&self, scope: Option<&str>, sink: &dyn ClientEventSink) {
        let Some(context) = self.settings.as_ref() else {
            sink.emit(ClientEvent::Error {
                kind: ErrorKindDto::Internal,
                message: "hook document unavailable: missing settings context".to_string(),
            })
            .await;
            return;
        };
        match hook_admin::document_json(context, scope) {
            Ok(details_json) => {
                self.emit_configuration_operation(
                    sink,
                    ConfigurationDomainDto::Hook,
                    0,
                    ConfigurationOperationStatusDto::Succeeded,
                    ConfigurationEffectDto::NotApplicable,
                    Some("Hook document loaded.".to_string()),
                    Some(details_json),
                )
                .await;
            }
            Err(message) => {
                sink.emit(ClientEvent::Error {
                    kind: ErrorKindDto::Internal,
                    message,
                })
                .await
            }
        }
    }
    pub(super) async fn apply_hook_admin(
        &self,
        command: HookAdminCommandDto,
        sink: &dyn ClientEventSink,
    ) {
        match command.action.as_str() {
            "get_document" => {
                self.emit_hook_document(command.scope.as_deref(), sink)
                    .await
            }
            "validate_document" => {
                let Some(operation_id) = command.operation_id else {
                    sink.emit(ClientEvent::Error {
                        kind: ErrorKindDto::Protocol,
                        message: "hook validate operation_id is required".to_string(),
                    })
                    .await;
                    return;
                };
                let Some(payload_json) = command.payload_json.as_deref() else {
                    sink.emit(ClientEvent::Error {
                        kind: ErrorKindDto::Protocol,
                        message: "hook validate payload is required".to_string(),
                    })
                    .await;
                    return;
                };
                match hook_admin::validate_document(payload_json) {
                    Ok(()) => {
                        self.emit_configuration_operation(
                            sink,
                            ConfigurationDomainDto::Hook,
                            operation_id,
                            ConfigurationOperationStatusDto::Succeeded,
                            ConfigurationEffectDto::NotApplicable,
                            Some("Hook document is valid.".to_string()),
                            None,
                        )
                        .await
                    }
                    Err(message) => {
                        self.emit_configuration_operation(
                            sink,
                            ConfigurationDomainDto::Hook,
                            operation_id,
                            ConfigurationOperationStatusDto::Failed,
                            ConfigurationEffectDto::NotApplicable,
                            Some(message),
                            None,
                        )
                        .await
                    }
                }
            }
            "save_document" => {
                let Some(operation_id) = command.operation_id else {
                    sink.emit(ClientEvent::Error {
                        kind: ErrorKindDto::Protocol,
                        message: "hook save operation_id is required".to_string(),
                    })
                    .await;
                    return;
                };
                let Some(revision_sha256) = command.revision.as_deref() else {
                    sink.emit(ClientEvent::Error {
                        kind: ErrorKindDto::Protocol,
                        message: "hook save revision is required".to_string(),
                    })
                    .await;
                    return;
                };
                let Some(payload_json) = command.payload_json.as_deref() else {
                    sink.emit(ClientEvent::Error {
                        kind: ErrorKindDto::Protocol,
                        message: "hook save payload is required".to_string(),
                    })
                    .await;
                    return;
                };
                self.emit_configuration_operation(
                    sink,
                    ConfigurationDomainDto::Hook,
                    operation_id,
                    ConfigurationOperationStatusDto::Started,
                    ConfigurationEffectDto::NotApplicable,
                    Some("Saving hooks…".to_string()),
                    None,
                )
                .await;
                let candidate = hook_admin::runtime_candidate(payload_json);
                let result = candidate.and_then(|candidate| {
                    self.settings
                        .as_ref()
                        .ok_or_else(|| {
                            "hook save unavailable: missing settings context".to_string()
                        })
                        .and_then(|context| {
                            hook_admin::save_document(context, revision_sha256, payload_json)
                        })
                        .map(|_| candidate)
                });
                match result {
                    Ok(candidate) => {
                        let effect = if self.is_turn_active() {
                            ConfigurationEffectDto::RestartRequired
                        } else if let Some(registry) = self.hook_registry.as_ref() {
                            let matchers = {
                                let mut registry = registry.write().await;
                                registry.replace_source_hooks(candidate.source, candidate.hooks);
                                registry.file_changed_matchers()
                            };
                            if let Some(watcher) = self.file_changed_watcher.as_ref() {
                                if let Some(settings) = self.settings.as_ref() {
                                    watcher.replace_matchers(
                                        matchers,
                                        settings.paths.project_dir.clone(),
                                    );
                                }
                            }
                            if let Some(settings) = self.settings.as_ref() {
                                settings.mark_keys_applied(&["hooks"]);
                            }
                            ConfigurationEffectDto::Applied
                        } else {
                            ConfigurationEffectDto::RestartRequired
                        };
                        self.emit_configuration_operation(
                            sink,
                            ConfigurationDomainDto::Hook,
                            operation_id,
                            ConfigurationOperationStatusDto::Succeeded,
                            effect,
                            Some("Saved hooks configuration.".to_string()),
                            None,
                        )
                        .await;
                        self.emit_settings_snapshot(sink).await;
                        self.emit_listing(ListingKindDto::Hooks, sink).await;
                    }
                    Err(message) => {
                        self.emit_configuration_operation(
                            sink,
                            ConfigurationDomainDto::Hook,
                            operation_id,
                            ConfigurationOperationStatusDto::Failed,
                            ConfigurationEffectDto::NotApplicable,
                            Some(message),
                            None,
                        )
                        .await
                    }
                }
            }
            action => {
                sink.emit(ClientEvent::Error {
                    kind: ErrorKindDto::Protocol,
                    message: format!("unsupported hook admin action: {action}"),
                })
                .await
            }
        }
    }
    pub(super) async fn emit_provider_credential_status(
        &self,
        operation_id: u64,
        provider_ids: &[String],
        preview_provider_ids: &[String],
        operation_error: Option<String>,
        sink: &dyn ClientEventSink,
    ) {
        let Some(credentials) = self.credentials.as_ref() else {
            sink.emit(ClientEvent::ProviderCredentialStatus {
                operation_id,
                configured_provider_ids: Vec::new(),
                unavailable_provider_ids: provider_ids.to_vec(),
                storage_encrypted: false,
                credential_previews: HashMap::new(),
                error: Some("provider credential storage is unavailable".to_string()),
            })
            .await;
            return;
        };

        if let Some(error) = operation_error {
            sink.emit(ClientEvent::ProviderCredentialStatus {
                operation_id,
                configured_provider_ids: Vec::new(),
                unavailable_provider_ids: provider_ids.to_vec(),
                storage_encrypted: credentials.provider_key_storage_is_encrypted(),
                credential_previews: HashMap::new(),
                error: Some(error),
            })
            .await;
            return;
        }

        let mut configured_provider_ids = Vec::new();
        let mut unavailable_provider_ids = Vec::new();
        let mut credential_previews = HashMap::new();
        let mut failures = Vec::new();
        for provider_id in provider_ids {
            match credentials.has_provider_key(provider_id).await {
                Ok(true) => {
                    configured_provider_ids.push(provider_id.clone());
                    if preview_provider_ids.contains(provider_id) {
                        match credentials.get_provider_key(provider_id).await {
                            Ok(Some(secret)) => {
                                credential_previews.insert(
                                    provider_id.clone(),
                                    secret::masked_credential_preview(secret.expose_secret()),
                                );
                            }
                            Ok(None) => {}
                            Err(failure) => {
                                failures.push(format!("{provider_id} preview: {failure}"))
                            }
                        }
                    }
                }
                Ok(false) => {}
                Err(failure) => {
                    unavailable_provider_ids.push(provider_id.clone());
                    failures.push(format!("{provider_id}: {failure}"));
                }
            }
        }
        let error = (!failures.is_empty()).then(|| {
            format!(
                "provider credential storage is unavailable ({})",
                failures.join("; ")
            )
        });

        sink.emit(ClientEvent::ProviderCredentialStatus {
            operation_id,
            configured_provider_ids,
            unavailable_provider_ids,
            storage_encrypted: credentials.provider_key_storage_is_encrypted(),
            credential_previews,
            error,
        })
        .await;
    }
}
