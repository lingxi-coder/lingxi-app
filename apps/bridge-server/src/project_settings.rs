//! Directory-bound configuration administration for Desktop settings. This path does not boot an orchestrator.
use crate::{
    hook_admin, mcp_admin, mcp_bridge::McpPaths, plugin_admin, settings_bridge::*, skills_admin,
};
use client::protocol::{
    commands::{ClientCommand, ListingKindDto},
    events::ClientEvent,
    listings::{
        ConfigurationDomainDto as Domain, ConfigurationEffectDto as Effect,
        ConfigurationOperationStatusDto as Status,
    },
};
use std::{
    io::Read,
    path::PathBuf,
    sync::{Arc, RwLock},
};

pub async fn run(
    home: PathBuf,
    cwd: PathBuf,
    input: impl Read,
) -> Result<Vec<ClientEvent>, String> {
    let mut data = Vec::new();
    const MAX_INPUT: u64 = 4 * 1024 * 1024;
    input
        .take(MAX_INPUT + 1)
        .read_to_end(&mut data)
        .map_err(|e| e.to_string())?;
    if data.len() as u64 > MAX_INPUT {
        return Err("Settings command is too large".into());
    }
    let command: ClientCommand = serde_json::from_slice(&data).map_err(|e| e.to_string())?;
    let mut cfg = harness_runtime::desktop::DesktopConfig::default();
    cfg.cwd = cwd.clone();
    cfg.lingxi_home = home.clone();
    let managed = harness_runtime::desktop::managed_settings_overlay(&cfg)
        .await
        .map_err(|error| settings_error_message(&error))?
        .unwrap_or_else(empty_settings_source);
    let context = SettingsContext {
        paths: SettingsPaths {
            lingxi_home: home,
            project_dir: cwd,
        },
        managed,
        managed_diagnostics: None,
        active: Arc::new(RwLock::new(empty_settings_source())),
        plugin_archive_session: Arc::new(plugin::PluginArchiveSession::new()),
        fusion_layers: None,
    };
    execute(&context, command).await
}

fn snapshot(context: &SettingsContext) -> Result<ClientEvent, String> {
    let state = context
        .snapshot()
        .and_then(|snapshot| lower_snapshot(&snapshot))
        .map_err(|e| settings_error_message(&e))?;
    Ok(ClientEvent::SettingsSnapshot {
        effective_json: state.effective_json,
        provenance_json: state.provenance_json,
        files_json: Some(state.files_json),
        active_json: None,
        locked: Some(state.locked),
        layers_json: Some(state.layers_json),
        merged_keys: Some(state.merged_keys),
    })
}

fn operation(
    domain: Domain,
    id: Option<u64>,
    effect: Effect,
    details_json: Option<String>,
) -> Result<ClientEvent, String> {
    Ok(ClientEvent::ConfigurationOperation {
        domain,
        operation_id: id.ok_or("Missing operation ID")?,
        status: Status::Succeeded,
        effect,
        message: None,
        details_json,
    })
}

fn required<'a>(value: &'a Option<String>, name: &str) -> Result<&'a str, String> {
    value.as_deref().ok_or_else(|| format!("Missing {name}"))
}

async fn execute(
    context: &SettingsContext,
    command: ClientCommand,
) -> Result<Vec<ClientEvent>, String> {
    let paths = &context.paths;
    let mut events = Vec::new();
    match command {
        ClientCommand::SkillAdmin { command } => {
            let id = command.operation_id;
            if !matches!(command.action.as_str(), "get_catalog" | "get_document") && id.is_none() {
                return Err("Missing skill operation ID".into());
            }
            let scope = skills_admin::SkillsAdminContext {
                cwd: paths.project_dir.clone(),
                lingxi_home: paths.lingxi_home.clone(),
            };
            match skills_admin::handle(command, &scope)? {
                skills_admin::SkillAdminOutcome::Catalog(catalog_json) => {
                    events.push(ClientEvent::SkillCatalog { catalog_json })
                }
                skills_admin::SkillAdminOutcome::Document(document_json) => {
                    events.push(ClientEvent::SkillDocument { document_json })
                }
                skills_admin::SkillAdminOutcome::Changed {
                    catalog_json,
                    document_json,
                } => {
                    events.push(ClientEvent::SkillCatalog { catalog_json });
                    if let Some(document_json) = document_json {
                        events.push(ClientEvent::SkillDocument { document_json });
                    }
                    events.push(operation(Domain::Skill, id, Effect::RestartRequired, None)?);
                }
            }
            return Ok(events);
        }
        ClientCommand::McpAdmin { command } => {
            let mcp = McpPaths {
                project_dir: paths.project_dir.clone(),
                global_config_path: migrations::global_config::global_config_path()
                    .ok_or("MCP configuration home unavailable")?,
            };
            if command.action != "get_snapshot" {
                let revision = required(&command.revision, "MCP revision")?;
                let payload = required(&command.payload_json, "MCP configuration")?;
                let receipt = operation(
                    Domain::Mcp,
                    command.operation_id,
                    Effect::RestartRequired,
                    None,
                )?;
                match command.action.as_str() {
                    "save_server" => mcp_admin::save_server_entry(&mcp, revision, payload)?,
                    "remove_server" => mcp_admin::remove_server_entry(&mcp, revision, payload)?,
                    "set_approval" => mcp_admin::set_project_approval(context, revision, payload)?,
                    _ => return Err("Unsupported MCP settings action".into()),
                }
                events.push(receipt);
            }
            events.push(ClientEvent::McpConfigurationSnapshot {
                snapshot_json: mcp_admin::snapshot_json(context, &mcp, vec![])?,
            });
            return Ok(events);
        }
        ClientCommand::PluginAdmin { command } => {
            match command.action.as_str() {
                "get_catalog" => {}
                "preview_operation" => {
                    let preview = plugin_admin::preview_operation(
                        Some(context),
                        command.revision.as_deref(),
                        required(&command.payload_json, "plugin payload")?,
                    )
                    .await?;
                    return Ok(vec![operation(
                        Domain::Plugin,
                        command.operation_id,
                        Effect::NotApplicable,
                        Some(preview),
                    )?]);
                }
                "apply_operation" | "save_config" => {
                    let id = command.operation_id.ok_or("Missing plugin operation ID")?;
                    let revision = required(&command.revision, "plugin revision")?;
                    let payload = required(&command.payload_json, "plugin payload")?;
                    let details = if command.action == "apply_operation" {
                        let result =
                            plugin_admin::apply_operation(context, revision, payload, None).await?;
                        result.credential_cleanup.map(|cleanup| {
                            serde_json::json!({"credentialCleanup": cleanup}).to_string()
                        })
                    } else {
                        plugin_admin::save_config(context, revision, payload).await?;
                        None
                    };
                    events.push(operation(
                        Domain::Plugin,
                        Some(id),
                        Effect::RestartRequired,
                        details,
                    )?);
                    events.push(snapshot(context)?);
                }
                _ => return Err("Unsupported plugin settings action".into()),
            }
            events.push(ClientEvent::PluginCatalog {
                catalog_json: plugin_admin::catalog_json(context, None).await?,
            });
            return Ok(events);
        }
        ClientCommand::HookAdmin { command } => {
            let (effect, id) = match command.action.as_str() {
                "get_document" => (Effect::NotApplicable, Some(0)),
                "validate_document" => {
                    hook_admin::validate_document(required(
                        &command.payload_json,
                        "hook document",
                    )?)?;
                    return Ok(vec![operation(
                        Domain::Hook,
                        command.operation_id,
                        Effect::NotApplicable,
                        None,
                    )?]);
                }
                "save_document" => {
                    let id = command.operation_id.ok_or("Missing hook operation ID")?;
                    hook_admin::save_document(
                        context,
                        required(&command.revision, "hook revision")?,
                        required(&command.payload_json, "hook document")?,
                    )?;
                    events.push(snapshot(context)?);
                    (Effect::RestartRequired, Some(id))
                }
                _ => return Err("Unsupported hook settings action".into()),
            };
            let document = hook_admin::document_json(context, command.scope.as_deref())?;
            events.push(operation(Domain::Hook, id, effect, Some(document))?);
            return Ok(events);
        }
        ClientCommand::RefreshListings { which }
            if which.as_slice() == [ListingKindDto::Settings] => {}
        ClientCommand::UpdateSettings {
            destination,
            patch_json,
        } => {
            let patch = crate::router::parse_settings_patch(&patch_json)?;
            if patch
                .iter()
                .any(|(key, _)| matches!(key.as_str(), "providers" | "routing" | "providerRegion"))
            {
                return Err("Provider settings belong to the host".into());
            }
            apply_patch(paths, destination, patch)?;
        }
        ClientCommand::UpdatePermissionRules {
            destination,
            behavior,
            add,
            remove,
        } => {
            let files = permission_paths(paths);
            for (raw, add) in [(add, true), (remove, false)] {
                if raw.is_empty() {
                    continue;
                }
                let rules: Vec<_> = raw
                    .iter()
                    .map(|rule| permission_rule_from_wire(rule, behavior, destination))
                    .collect();
                permission::persist_permission_rule_set(
                    &rules,
                    add,
                    permission_destination(destination),
                    &files,
                )
                .await
                .map_err(|e| e.to_string())?;
            }
        }
        ClientCommand::SetDefaultPermissionMode { destination, mode } => {
            if mode == "bypassPermissions" {
                return Err("This permission mode is session scoped".into());
            }
            permission::persist_permission_mode(
                &mode,
                permission_destination(destination),
                &permission_paths(paths),
            )
            .await
            .map_err(|e| e.to_string())?;
        }
        ClientCommand::UpdateWorkspaceDirectories {
            destination,
            add,
            remove,
        } => {
            for (dirs, add) in [(add, true), (remove, false)] {
                if !dirs.is_empty() {
                    permission::persist_workspace_directories(
                        &dirs,
                        add,
                        permission_destination(destination),
                        &permission_paths(paths),
                    )
                    .await
                    .map_err(|e| e.to_string())?;
                }
            }
        }
        _ => return Err("Unsupported project settings command".into()),
    }
    events.push(snapshot(context)?);
    Ok(events)
}
