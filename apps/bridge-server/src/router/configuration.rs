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
use client::protocol::text::ClientTextDto;
use lingxi_core::types::utf16_json::Utf16JsonProjection;
use std::collections::HashMap;

fn source_string(value: impl Into<Utf16JsonProjection>) -> Utf16JsonProjection {
    value.into()
}

fn mark_settings_keys_applied(
    settings: Option<&crate::settings_bridge::SettingsContext>,
    keys: &[&str],
    operation: &'static str,
) -> Result<(), Utf16JsonProjection> {
    let result = match settings {
        Some(settings) => settings
            .mark_keys_applied(keys)
            .map_err(|error| error.diagnostic_projection()),
        None => Ok(()),
    };
    if result.is_err() {
        tracing::warn!(
            operation = operation,
            "hot reload completed but active settings baseline could not be updated"
        );
    }
    result
}

fn settings_reload_error_details(diagnostic: &Utf16JsonProjection) -> String {
    let message = diagnostic
        .subprojection("/message")
        .unwrap_or_else(|_| diagnostic.clone());
    Utf16JsonProjection::object(vec![
        ("reloadError".to_owned(), message),
        ("reloadErrorDiagnostic".to_owned(), diagnostic.clone()),
    ])
    .expect("reload error details are a valid JSON object")
    .to_json_string()
    .expect("reload error details preserve valid source JSON")
}

#[cfg(test)]
mod settings_reload_error_source_tests {
    use super::*;
    use std::sync::{Arc, RwLock};

    #[test]
    fn plain_refresh_error_retains_its_source_string_in_operation_details() {
        let diagnostic = Utf16JsonProjection::parse(r#""refresh \ud800 failed""#).unwrap();
        let details =
            Utf16JsonProjection::parse(&settings_reload_error_details(&diagnostic)).unwrap();
        assert_eq!(
            details.string_units("/reloadError"),
            diagnostic.string_units("")
        );
        assert_eq!(
            details.string_units("/reloadErrorDiagnostic"),
            diagnostic.string_units("")
        );
    }

    #[test]
    fn failed_settings_baseline_update_retains_lone_utf16_in_operation_details() {
        let temporary = tempfile::tempdir().unwrap();
        let home = temporary.path().join("home");
        let project = temporary.path().join("project");
        let project_settings = project.join(".lingxi");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::create_dir_all(&project_settings).unwrap();
        std::fs::write(
            home.join("settings.json"),
            r#"{"policyHelpers":{"defaultSettings":{"custom":[]}}}"#,
        )
        .unwrap();
        std::fs::write(
            project_settings.join("settings.json"),
            r#"{"policyHelpers":{"defaultSettings":{"custom":{"length":{"toString":"\ud800"}}}}}"#,
        )
        .unwrap();

        let settings = crate::settings_bridge::SettingsContext {
            fusion_layers: None,
            paths: crate::settings_bridge::SettingsPaths {
                lingxi_home: home,
                project_dir: project,
            },
            active: Arc::new(RwLock::new(crate::settings_bridge::empty_settings_source())),
            managed: crate::settings_bridge::empty_settings_source(),
            managed_diagnostics: None,
        };
        let diagnostic = mark_settings_keys_applied(
            Some(&settings),
            &["policyHelpers"],
            "source-fidelity regression",
        )
        .expect_err("cross-layer Native array length coercion must fail");

        assert_eq!(diagnostic.string_at("/kind"), Some("settings_projection"));
        assert_eq!(
            diagnostic.string_units("/sourceValue/toString"),
            Some(vec![0xd800]),
            "the settings diagnostic must retain the raw lone UTF-16 unit"
        );
        let details_json = settings_reload_error_details(&diagnostic);
        let details = Utf16JsonProjection::parse(&details_json).unwrap();
        assert_eq!(
            details.string_at("/reloadError"),
            diagnostic.string_at("/message"),
            "the existing reloadError remains readable"
        );
        assert_eq!(
            details.string_units("/reloadErrorDiagnostic/sourceValue/toString"),
            Some(vec![0xd800]),
            "the operation details must preserve sourceValue through JSON serialization"
        );
    }
}

#[cfg(test)]
mod skill_identity_source_tests {
    use super::*;
    use async_trait::async_trait;
    use lingxi_core::host::auth::{AuthError, AuthHandle, LoginInfo};
    use lingxi_core::host::task_registry::{
        TaskCreateInput, TaskListFilter, TaskOutputChunk, TaskRecord, TaskRegistryError,
        TaskRegistryHandle, TaskUpdatePatch,
    };
    use std::sync::{Arc, Mutex};

    struct TestAuth;

    #[async_trait]
    impl AuthHandle for TestAuth {
        fn register_account_change_observer(
            &self,
            _observer: std::sync::Weak<dyn lingxi_core::host::auth::AccountChangeObserver>,
        ) {
        }

        async fn login(&self) -> Result<LoginInfo, AuthError> {
            unreachable!("Skill Admin identity test does not log in")
        }

        async fn logout(&self) -> Result<(), AuthError> {
            Ok(())
        }

        async fn current_user(&self) -> Option<LoginInfo> {
            None
        }
    }

    struct TestTasks;

    #[async_trait]
    impl TaskRegistryHandle for TestTasks {
        async fn create(&self, _input: TaskCreateInput) -> Result<TaskRecord, TaskRegistryError> {
            unreachable!("Skill Admin identity test does not create tasks")
        }

        async fn get(&self, _id: &str) -> Result<Option<TaskRecord>, TaskRegistryError> {
            unreachable!("Skill Admin identity test does not get tasks")
        }

        async fn list(
            &self,
            _filter: TaskListFilter,
        ) -> Result<Vec<TaskRecord>, TaskRegistryError> {
            Ok(Vec::new())
        }

        async fn update(
            &self,
            _id: &str,
            _patch: TaskUpdatePatch,
        ) -> Result<TaskRecord, TaskRegistryError> {
            unreachable!("Skill Admin identity test does not update tasks")
        }

        async fn set_status(
            &self,
            _id: &str,
            _status: &str,
        ) -> Result<TaskRecord, TaskRegistryError> {
            unreachable!("Skill Admin identity test does not change task status")
        }

        async fn kill(&self, _id: &str) -> Result<TaskRecord, TaskRegistryError> {
            unreachable!("Skill Admin identity test does not stop tasks")
        }

        async fn output(
            &self,
            _id: &str,
            _offset: Option<u64>,
        ) -> Result<TaskOutputChunk, TaskRegistryError> {
            unreachable!("Skill Admin identity test does not read task output")
        }
    }

    #[derive(Default)]
    struct CapturingSink(Mutex<Vec<ClientEvent>>);

    #[async_trait]
    impl ClientEventSink for CapturingSink {
        async fn emit(&self, event: ClientEvent) {
            self.0.lock().unwrap().push(event);
        }
    }

    impl CapturingSink {
        fn take_one(&self) -> ClientEvent {
            let mut events = self.0.lock().unwrap();
            assert_eq!(events.len(), 1);
            events.pop().unwrap()
        }
    }

    #[test]
    fn command_skill_ids_keep_source_name_units_through_catalog_and_load_prefix() {
        let lone = vec![u16::from(b'r'), u16::from(b'e'), 0xd800, u16::from(b'v')];
        let replacement = vec![u16::from(b'r'), u16::from(b'e'), 0xfffd, u16::from(b'v')];
        let name = |units: Vec<u16>| {
            Utf16JsonProjection::root_string(String::from_utf16_lossy(&units), units)
                .expect("valid exact source name")
        };
        let lone_name = name(lone.clone());
        let replacement_name = name(replacement.clone());
        let lone_id = source_with_prefix("command:", &lone_name);
        let replacement_id = source_with_prefix("command:", &replacement_name);

        let entry = source_object(vec![
            ("id".into(), lone_id.clone()),
            ("name".into(), lone_name),
        ]);
        let catalog_json = entry.to_json_string().unwrap();
        let catalog = Utf16JsonProjection::parse(&catalog_json).unwrap();
        let id_units = catalog.string_units("/id").unwrap();
        assert_eq!(catalog.string_units("/name"), Some(lone));
        assert_ne!(
            id_units,
            replacement_id.string_units("").unwrap(),
            "display-colliding names must retain distinct catalog ids"
        );

        let target = ClientTextDto::from_code_units(id_units);
        let loaded_name = strip_source_prefix(&target.code_units, "command:")
            .expect("catalog command id has a source prefix");
        assert_eq!(
            loaded_name,
            [u16::from(b'r'), u16::from(b'e'), 0xd800, u16::from(b'v')]
        );
    }

    #[tokio::test]
    async fn catalog_command_id_loads_the_exact_colliding_plugin_document() {
        use command_api::{
            CommandFrontmatter, CommandRegistry, CommandSource, SlashCommand, SlashCommandKind,
        };

        let temp = tempfile::tempdir().unwrap();
        let project_dir = temp.path().join("project");
        let home_dir = temp.path().join("home");
        std::fs::create_dir_all(&project_dir).unwrap();
        std::fs::create_dir_all(&home_dir).unwrap();

        let lone_units = [u16::from(b'r'), u16::from(b'e'), 0xd800, u16::from(b'v')];
        let replacement_units = [u16::from(b'r'), u16::from(b'e'), 0xfffd, u16::from(b'v')];
        let name = |units: &[u16]| {
            Utf16JsonProjection::root_string(String::from_utf16_lossy(units), units.to_vec())
                .unwrap()
        };
        let make_plugin_command = |units: &[u16], directory: &std::path::Path, markdown: &str| {
            std::fs::create_dir_all(directory).unwrap();
            let file_path = directory.join("SKILL.md");
            std::fs::write(&file_path, markdown).unwrap();
            SlashCommand {
                name: name(units),
                description: "collision fixture".into(),
                source: CommandSource::Plugin,
                kind: SlashCommandKind::Plugin {
                    plugin_id: lingxi_core::types::PluginId::new(),
                    file_path,
                    frontmatter: CommandFrontmatter::default(),
                    prompt_template: markdown.into(),
                },
                skill_root: Some(directory.to_path_buf()),
                ..SlashCommand::default()
            }
        };
        let lone_markdown = "document selected by lone surrogate";
        let replacement_markdown = "document selected by literal replacement character";
        let lone_dir = temp.path().join("lone-plugin-skill");
        let replacement_dir = temp.path().join("replacement-plugin-skill");
        let mut registry = CommandRegistry::new();
        registry.register_command(make_plugin_command(&lone_units, &lone_dir, lone_markdown));
        registry.register_command(make_plugin_command(
            &replacement_units,
            &replacement_dir,
            replacement_markdown,
        ));

        let settings = crate::settings_bridge::SettingsContext {
            fusion_layers: None,
            paths: crate::settings_bridge::SettingsPaths {
                lingxi_home: home_dir,
                project_dir,
            },
            active: Arc::new(std::sync::RwLock::new(
                crate::settings_bridge::empty_settings_source(),
            )),
            managed: crate::settings_bridge::empty_settings_source(),
            managed_diagnostics: None,
        };
        let router = EngineCommandRouter::new(
            Arc::new(orchestrator::test_support::MockOrchestratorHandle::new()),
            Arc::new(TestAuth),
            Arc::new(TestTasks) as Arc<dyn TaskRegistryHandle>,
            None,
            Some(Arc::new(tokio::sync::RwLock::new(registry))),
        )
        .with_settings_context(settings);
        let sink = CapturingSink::default();

        router.emit_skill_catalog(&sink).await;
        let ClientEvent::SkillCatalog { catalog_json } = sink.take_one() else {
            panic!("catalog request must emit the production SkillCatalog event");
        };
        let catalog = Utf16JsonProjection::parse(&catalog_json).unwrap();
        let entries = catalog.array_len("/entries").unwrap();
        let catalog_id_units = (0..entries)
            .find_map(|index| {
                let name_units = catalog.string_units(&format!("/entries/{index}/name"))?;
                (name_units == lone_units.to_vec()).then(|| {
                    catalog
                        .string_units(&format!("/entries/{index}/id"))
                        .unwrap()
                })
            })
            .expect("catalog must contain the lone-surrogate plugin skill");
        assert_eq!(
            catalog_id_units,
            [
                "command:".encode_utf16().collect::<Vec<_>>(),
                lone_units.to_vec()
            ]
            .concat()
        );

        router
            .apply_skill_admin(
                SkillAdminCommandDto {
                    action: "get_document".to_string(),
                    operation_id: None,
                    target: Some(ClientTextDto::from_code_units(catalog_id_units.clone())),
                    scope: None,
                    revision: None,
                    payload_json: None,
                },
                &sink,
            )
            .await;
        let ClientEvent::SkillDocument { document_json } = sink.take_one() else {
            panic!("catalog-selected ID must load a production SkillDocument");
        };
        let document = Utf16JsonProjection::parse(&document_json).unwrap();
        assert_eq!(document.string_units("/id"), Some(catalog_id_units));
        assert_eq!(document.string_units("/name"), Some(lone_units.to_vec()));
        assert_eq!(document.string_at("/markdown"), Some(lone_markdown));
    }
}

fn source_projection(value: &ClientTextDto) -> Utf16JsonProjection {
    let units = value.code_units.clone();
    Utf16JsonProjection::root_string(String::from_utf16_lossy(&units), units)
        .expect("ClientTextDto display is derived from its exact units")
}

fn source_with_affixes(
    prefix: &str,
    value: &Utf16JsonProjection,
    suffix: &str,
) -> Utf16JsonProjection {
    let mut units = prefix.encode_utf16().collect::<Vec<_>>();
    units.extend(
        value
            .string_units("")
            .expect("catalog source text fields are root strings"),
    );
    units.extend(suffix.encode_utf16());
    Utf16JsonProjection::root_string(String::from_utf16_lossy(&units), units)
        .expect("affixing text preserves the source string")
}

fn source_with_prefix(prefix: &str, value: &Utf16JsonProjection) -> Utf16JsonProjection {
    source_with_affixes(prefix, value, "")
}

fn source_object(fields: Vec<(String, Utf16JsonProjection)>) -> Utf16JsonProjection {
    Utf16JsonProjection::object(fields).expect("generated skill objects have unique fields")
}

fn strip_source_prefix<'a>(units: &'a [u16], prefix: &str) -> Option<&'a [u16]> {
    let prefix_units = prefix.encode_utf16().collect::<Vec<_>>();
    units.strip_prefix(prefix_units.as_slice())
}

fn source_text_from_units(units: &[u16]) -> Utf16JsonProjection {
    let units = units.to_vec();
    Utf16JsonProjection::root_string(String::from_utf16_lossy(&units), units)
        .expect("display is derived from exact source units")
}

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
                    .to_string()
                    .into(),
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
                message: "MCP server name must not be empty".to_string().into(),
            })
            .await;
            return;
        }

        let config = match parse_mcp_config_json(config_json) {
            Ok(config) => config,
            Err(message) => {
                sink.emit(ClientEvent::Error {
                    kind: ErrorKindDto::Protocol,
                    message: message.into(),
                })
                .await;
                return;
            }
        };

        if let Err(message) = crate::mcp_bridge::upsert_server(mcp, scope, name, config) {
            sink.emit(ClientEvent::Error {
                kind: ErrorKindDto::Internal,
                message: message.into(),
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
                    .to_string()
                    .into(),
            })
            .await;
            return;
        };

        if let Err(message) = crate::mcp_bridge::remove_server(mcp, scope, name) {
            sink.emit(ClientEvent::Error {
                kind: ErrorKindDto::Internal,
                message: message.into(),
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
                message: "skill catalog unavailable: this connection was built without a settings context".to_string().into(),
            }).await;
            return;
        };
        match skills_admin::catalog_json(&context) {
            Ok(catalog_json) => {
                let mut source_catalog = match Utf16JsonProjection::parse(&catalog_json) {
                    Ok(catalog) => catalog,
                    Err(_) => {
                        sink.emit(ClientEvent::SkillCatalog { catalog_json }).await;
                        return;
                    }
                };
                let runtime_skills = self.handle.list_skills().await;
                let entry_count = source_catalog.array_len("/entries").unwrap_or_default();
                let mut entries = (0..entry_count)
                    .filter_map(|index| {
                        source_catalog
                            .subprojection(&format!("/entries/{index}"))
                            .ok()
                    })
                    .collect::<Vec<_>>();
                let known = entries
                    .iter()
                    .filter_map(|entry| entry.string_at("/directory").map(ToOwned::to_owned))
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
                    entries.push(source_object(vec![
                        (
                            "id".into(),
                            source_string(format!("runtime:{}", skill.name)),
                        ),
                        ("name".into(), source_string(skill.name)),
                        ("source".into(), source_string(source)),
                        ("rootDir".into(), source_string(directory.clone())),
                        ("directory".into(), source_string(directory)),
                        (
                            "writable".into(),
                            Utf16JsonProjection::parse("false").expect("boolean JSON"),
                        ),
                        (
                            "readonlyReason".into(),
                            source_string(format!(
                                "{source} skills are runtime-derived and read-only."
                            )),
                        ),
                        (
                            "diagnosticsJson".into(),
                            source_string("{\"status\":\"runtime\"}"),
                        ),
                    ]));
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
                                let revision = std::fs::read(file_path)
                                    .ok()
                                    .map(|bytes| crate::config_admin::sha256_hex(&bytes));
                                entries.push(source_object(vec![
                                    ("id".into(), source_with_prefix("command:", &command.name)),
                                    ("name".into(), command.name.clone()),
                                    ("source".into(), source_string("plugin")),
                                    ("pluginOwner".into(), source_string(plugin_id.to_string())),
                                    ("rootDir".into(), source_string(directory.clone())),
                                    ("directory".into(), source_string(directory)),
                                    (
                                        "writable".into(),
                                        Utf16JsonProjection::parse("false").expect("boolean JSON"),
                                    ),
                                    (
                                        "readonlyReason".into(),
                                        source_string(
                                            "Plugin skills are managed by their owning plugin.",
                                        ),
                                    ),
                                    (
                                        "revision".into(),
                                        revision.map(|value| source_string(value)).unwrap_or_else(
                                            || {
                                                Utf16JsonProjection::parse("null")
                                                    .expect("null JSON")
                                            },
                                        ),
                                    ),
                                    ("description".into(), command.description.clone()),
                                    (
                                        "whenToUse".into(),
                                        command
                                            .when_to_use
                                            .clone()
                                            .map(|value| source_string(value))
                                            .unwrap_or_else(|| {
                                                Utf16JsonProjection::parse("null")
                                                    .expect("null JSON")
                                            }),
                                    ),
                                    (
                                        "diagnosticsJson".into(),
                                        source_string("{\"ok\":true,\"issues\":[]}"),
                                    ),
                                ]));
                            }
                            command_api::SlashCommandKind::Mcp {
                                connection_id,
                                prompt_name,
                                ..
                            } => {
                                let directory = source_with_affixes(
                                    &format!("<mcp:{connection_id}:"),
                                    prompt_name,
                                    ">",
                                );
                                entries.push(source_object(vec![
                                    ("id".into(), source_with_prefix("command:", &command.name)),
                                    ("name".into(), command.name.clone()),
                                    ("source".into(), source_string("mcp")),
                                    ("rootDir".into(), directory.clone()),
                                    ("directory".into(), directory),
                                    (
                                        "writable".into(),
                                        Utf16JsonProjection::parse("false").expect("boolean JSON"),
                                    ),
                                    (
                                        "readonlyReason".into(),
                                        source_string(
                                            "MCP skills are provided by the connected server.",
                                        ),
                                    ),
                                    ("description".into(), command.description.clone()),
                                    (
                                        "whenToUse".into(),
                                        command
                                            .when_to_use
                                            .clone()
                                            .map(|value| source_string(value))
                                            .unwrap_or_else(|| {
                                                Utf16JsonProjection::parse("null")
                                                    .expect("null JSON")
                                            }),
                                    ),
                                    (
                                        "diagnosticsJson".into(),
                                        source_string("{\"status\":\"runtime\"}"),
                                    ),
                                ]));
                            }
                            _ => {}
                        }
                    }
                }
                if let Ok(entries) = Utf16JsonProjection::array(entries) {
                    let _ = source_catalog.set_field("entries", entries);
                }
                let catalog_json = source_catalog.to_json_string().unwrap_or(catalog_json);
                sink.emit(ClientEvent::SkillCatalog { catalog_json }).await
            }
            Err(message) => {
                sink.emit(ClientEvent::Error {
                    kind: ErrorKindDto::Internal,
                    message: message.into(),
                })
                .await
            }
        }
    }
    pub(super) async fn emit_skill_document(
        &self,
        skill_id: &ClientTextDto,
        sink: &dyn ClientEventSink,
    ) {
        let Some(context) = self.skills_admin_context() else {
            sink.emit(ClientEvent::Error {
                kind: ErrorKindDto::Internal,
                message: "skill document unavailable: this connection was built without a settings context".to_string().into(),
            }).await;
            return;
        };
        let target_units = skill_id.code_units.as_slice();
        if let Some(name_units) = strip_source_prefix(target_units, "command:") {
            let name = source_text_from_units(name_units);
            let command = match self.slash_registry.as_ref() {
                Some(registry) => registry.read().await.resolve(&name).cloned(),
                None => None,
            };
            if let Some(command) = command {
                let document = match command.kind {
                    command_api::SlashCommandKind::Plugin {
                        plugin_id,
                        file_path,
                        ..
                    } if command.skill_root.is_some() => Some({
                        let markdown = std::fs::read_to_string(&file_path).unwrap_or_default();
                        let directory = command
                            .skill_root
                            .as_deref()
                            .or_else(|| file_path.parent())
                            .unwrap_or(&file_path)
                            .to_string_lossy()
                            .into_owned();
                        let revision = crate::config_admin::sha256_hex(markdown.as_bytes());
                        source_object(vec![
                            ("id".into(), source_projection(skill_id)),
                            ("name".into(), command.name.clone()),
                            ("source".into(), source_string("plugin")),
                            ("pluginOwner".into(), source_string(plugin_id.to_string())),
                            ("directory".into(), source_string(directory.clone())),
                            ("rootDir".into(), source_string(directory)),
                            ("markdown".into(), source_string(markdown)),
                            (
                                "writable".into(),
                                Utf16JsonProjection::parse("false").expect("boolean JSON"),
                            ),
                            ("revision".into(), source_string(revision)),
                            (
                                "readonlyReason".into(),
                                source_string("Plugin skills are managed by their owning plugin."),
                            ),
                            (
                                "diagnosticsJson".into(),
                                source_string("{\"ok\":true,\"issues\":[]}"),
                            ),
                        ])
                    }),
                    command_api::SlashCommandKind::Mcp {
                        connection_id,
                        prompt_name,
                        ..
                    } => Some({
                        let directory = source_with_affixes(
                            &format!("<mcp:{connection_id}:"),
                            &prompt_name,
                            ">",
                        );
                        source_object(vec![
                            ("id".into(), source_projection(skill_id)),
                            ("name".into(), command.name.clone()),
                            ("source".into(), source_string("mcp")),
                            ("directory".into(), directory.clone()),
                            ("rootDir".into(), directory),
                            ("markdown".into(), source_string("")),
                            (
                                "writable".into(),
                                Utf16JsonProjection::parse("false").expect("boolean JSON"),
                            ),
                            (
                                "revision".into(),
                                source_string(crate::config_admin::sha256_hex(&[])),
                            ),
                            (
                                "readonlyReason".into(),
                                source_string("MCP skills are provided by the connected server."),
                            ),
                            (
                                "diagnosticsJson".into(),
                                source_string("{\"status\":\"runtime\"}"),
                            ),
                        ])
                    }),
                    _ => None,
                };
                if let Some(document) = document {
                    if let Ok(document_json) = document.to_json_string() {
                        sink.emit(ClientEvent::SkillDocument { document_json })
                            .await;
                        return;
                    }
                }
            }
        }
        if let Some(name_units) = strip_source_prefix(target_units, "runtime:") {
            if let Some(skill) = self
                .handle
                .list_skills()
                .await
                .into_iter()
                .find(|skill| skill.name.encode_utf16().eq(name_units.iter().copied()))
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
                let document = source_object(vec![
                    ("id".into(), source_projection(skill_id)),
                    ("name".into(), source_string(skill.name)),
                    ("source".into(), source_string(source)),
                    ("directory".into(), source_string(directory.clone())),
                    ("rootDir".into(), source_string(directory)),
                    ("markdown".into(), source_string("")),
                    (
                        "writable".into(),
                        Utf16JsonProjection::parse("false").expect("boolean JSON"),
                    ),
                    (
                        "revision".into(),
                        source_string(crate::config_admin::sha256_hex(&[])),
                    ),
                    (
                        "readonlyReason".into(),
                        source_string(format!(
                            "{source} skills are runtime-derived and read-only."
                        )),
                    ),
                    (
                        "diagnosticsJson".into(),
                        source_string("{\"status\":\"runtime\"}"),
                    ),
                ]);
                if let Ok(document_json) = document.to_json_string() {
                    sink.emit(ClientEvent::SkillDocument { document_json })
                        .await;
                    return;
                }
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
                    message: message.into(),
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
            .reload(lingxi_core::host::RepoRootReloadRequest {
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
                let Some(skill_id) = command.target.as_ref() else {
                    sink.emit(ClientEvent::Error {
                        kind: ErrorKindDto::Protocol,
                        message: "skill document target is required".to_string().into(),
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
                        message: "skill mutation operation_id is required".to_string().into(),
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
                    message: format!("unsupported skill admin action: {action}").into(),
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
                    .to_string()
                    .into(),
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
                    mcp::ConfigScope::Settings(lingxi_core::types::SettingsScope::User) => {
                        ("user", true, None)
                    }
                    mcp::ConfigScope::Settings(lingxi_core::types::SettingsScope::Local) => {
                        ("local", true, None)
                    }
                    mcp::ConfigScope::Settings(lingxi_core::types::SettingsScope::Project) => {
                        ("project", true, None)
                    }
                    mcp::ConfigScope::Dynamic => ("dynamic", false, Some("由当前会话动态注入")),
                    mcp::ConfigScope::Enterprise => ("enterprise", false, Some("由企业配置管理")),
                    mcp::ConfigScope::ClaudeAi => {
                        ("claude_ai", false, Some("由 Claude.ai 连接提供"))
                    }
                    mcp::ConfigScope::Settings(lingxi_core::types::SettingsScope::Managed) => {
                        ("managed", false, Some("由管理员策略管理"))
                    }
                    mcp::ConfigScope::Agent => ("agent", false, Some("由 Agent frontmatter 注入")),
                },
                None => ("runtime", false, Some("仅存在于当前运行态")),
            };
            runtime_servers.push(serde_json::json!({
                "name": server.name,
                "status": match server.status {
                    lingxi_core::host::McpStatus::Connected => serde_json::json!("connected"),
                    lingxi_core::host::McpStatus::Disconnected => serde_json::json!("disconnected"),
                    lingxi_core::host::McpStatus::Error(reason) => serde_json::json!({ "type": "error", "reason": reason }),
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
                    message: message.into(),
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
                        message: "MCP save operation_id is required".to_string().into(),
                    })
                    .await;
                    return;
                };
                let Some(revision_sha256) = command.revision.as_deref() else {
                    sink.emit(ClientEvent::Error {
                        kind: ErrorKindDto::Protocol,
                        message: "MCP save revision is required".to_string().into(),
                    })
                    .await;
                    return;
                };
                let Some(payload_json) = command.payload_json.as_deref() else {
                    sink.emit(ClientEvent::Error {
                        kind: ErrorKindDto::Protocol,
                        message: "MCP save payload is required".to_string().into(),
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
                        message: "MCP remove operation_id is required".to_string().into(),
                    })
                    .await;
                    return;
                };
                let Some(revision_sha256) = command.revision.as_deref() else {
                    sink.emit(ClientEvent::Error {
                        kind: ErrorKindDto::Protocol,
                        message: "MCP remove revision is required".to_string().into(),
                    })
                    .await;
                    return;
                };
                let Some(payload_json) = command.payload_json.as_deref() else {
                    sink.emit(ClientEvent::Error {
                        kind: ErrorKindDto::Protocol,
                        message: "MCP remove payload is required".to_string().into(),
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
                        message: "MCP approval operation_id is required".to_string().into(),
                    })
                    .await;
                    return;
                };
                let Some(revision_sha256) = command.revision.as_deref() else {
                    sink.emit(ClientEvent::Error {
                        kind: ErrorKindDto::Protocol,
                        message: "MCP approval revision is required".to_string().into(),
                    })
                    .await;
                    return;
                };
                let Some(payload_json) = command.payload_json.as_deref() else {
                    sink.emit(ClientEvent::Error {
                        kind: ErrorKindDto::Protocol,
                        message: "MCP approval payload is required".to_string().into(),
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
                        let (effect, details_json) = if self.is_turn_active() {
                            (ConfigurationEffectDto::RestartRequired, None)
                        } else {
                            match self.refresh_mcp_runtime().await {
                                Ok(()) => match mark_settings_keys_applied(
                                    self.settings.as_ref(),
                                    &[
                                        "enabledMcpjsonServers",
                                        "disabledMcpjsonServers",
                                        "enableAllProjectMcpServers",
                                    ],
                                    "project MCP approval",
                                ) {
                                    Ok(()) => (ConfigurationEffectDto::Applied, None),
                                    Err(error) => (
                                        ConfigurationEffectDto::RestartRequired,
                                        Some(settings_reload_error_details(&error)),
                                    ),
                                },
                                Err(_) => (ConfigurationEffectDto::RestartRequired, None),
                            }
                        };
                        self.emit_configuration_operation(
                            sink,
                            ConfigurationDomainDto::Mcp,
                            operation_id,
                            ConfigurationOperationStatusDto::Succeeded,
                            effect,
                            Some("Saved project MCP approval.".to_string()),
                            details_json,
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
                    message: format!("unsupported MCP admin action: {action}").into(),
                })
                .await
            }
        }
    }
    pub(super) async fn emit_plugin_catalog(&self, sink: &dyn ClientEventSink) {
        let Some(context) = self.settings.as_ref() else {
            sink.emit(ClientEvent::Error {
                kind: ErrorKindDto::Internal,
                message: "plugin catalog unavailable: missing settings context"
                    .to_string()
                    .into(),
            })
            .await;
            return;
        };
        match plugin_admin::catalog_json(context, self.credentials.as_deref()).await {
            Ok(catalog_json) => sink.emit(ClientEvent::PluginCatalog { catalog_json }).await,
            Err(message) => {
                sink.emit(ClientEvent::Error {
                    kind: ErrorKindDto::Internal,
                    message: message.into(),
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
                        message: "plugin preview operation_id is required".to_string().into(),
                    })
                    .await;
                    return;
                };
                let Some(payload_json) = command.payload_json.as_deref() else {
                    sink.emit(ClientEvent::Error {
                        kind: ErrorKindDto::Protocol,
                        message: "plugin preview payload is required".to_string().into(),
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
                        message: "plugin apply operation_id is required".to_string().into(),
                    })
                    .await;
                    return;
                };
                let Some(revision_sha256) = command.revision.as_deref() else {
                    sink.emit(ClientEvent::Error {
                        kind: ErrorKindDto::Protocol,
                        message: "plugin apply revision is required".to_string().into(),
                    })
                    .await;
                    return;
                };
                let Some(payload_json) = command.payload_json.as_deref() else {
                    sink.emit(ClientEvent::Error {
                        kind: ErrorKindDto::Protocol,
                        message: "plugin apply payload is required".to_string().into(),
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
                                    match mark_settings_keys_applied(
                                        self.settings.as_ref(),
                                        &[
                                            "enabledPlugins",
                                            "pluginConfigs",
                                            "extraKnownMarketplaces",
                                        ],
                                        "plugin operation",
                                    ) {
                                        Ok(()) => (ConfigurationEffectDto::Applied, None),
                                        Err(error) => {
                                            (ConfigurationEffectDto::RestartRequired, Some(error))
                                        }
                                    }
                                }
                                Err(error) => {
                                    (ConfigurationEffectDto::RestartRequired, Some(error.into()))
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
                            reload_error.map(|error| settings_reload_error_details(&error)),
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
                        message: "plugin save operation_id is required".to_string().into(),
                    })
                    .await;
                    return;
                };
                let Some(revision_sha256) = command.revision.as_deref() else {
                    sink.emit(ClientEvent::Error {
                        kind: ErrorKindDto::Protocol,
                        message: "plugin save revision is required".to_string().into(),
                    })
                    .await;
                    return;
                };
                let Some(payload_json) = command.payload_json.as_deref() else {
                    sink.emit(ClientEvent::Error {
                        kind: ErrorKindDto::Protocol,
                        message: "plugin save payload is required".to_string().into(),
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
                                    match mark_settings_keys_applied(
                                        self.settings.as_ref(),
                                        &[
                                            "enabledPlugins",
                                            "pluginConfigs",
                                            "extraKnownMarketplaces",
                                        ],
                                        "plugin settings save",
                                    ) {
                                        Ok(()) => (ConfigurationEffectDto::Applied, None),
                                        Err(error) => {
                                            (ConfigurationEffectDto::RestartRequired, Some(error))
                                        }
                                    }
                                }
                                Err(error) => {
                                    (ConfigurationEffectDto::RestartRequired, Some(error.into()))
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
                            reload_error.map(|error| settings_reload_error_details(&error)),
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
                    message: format!("unsupported plugin admin action: {action}").into(),
                })
                .await
            }
        }
    }
    pub(super) async fn emit_hook_document(&self, scope: Option<&str>, sink: &dyn ClientEventSink) {
        let Some(context) = self.settings.as_ref() else {
            sink.emit(ClientEvent::Error {
                kind: ErrorKindDto::Internal,
                message: "hook document unavailable: missing settings context"
                    .to_string()
                    .into(),
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
                    message: message.into(),
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
                        message: "hook validate operation_id is required".to_string().into(),
                    })
                    .await;
                    return;
                };
                let Some(payload_json) = command.payload_json.as_deref() else {
                    sink.emit(ClientEvent::Error {
                        kind: ErrorKindDto::Protocol,
                        message: "hook validate payload is required".to_string().into(),
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
                        message: "hook save operation_id is required".to_string().into(),
                    })
                    .await;
                    return;
                };
                let Some(revision_sha256) = command.revision.as_deref() else {
                    sink.emit(ClientEvent::Error {
                        kind: ErrorKindDto::Protocol,
                        message: "hook save revision is required".to_string().into(),
                    })
                    .await;
                    return;
                };
                let Some(payload_json) = command.payload_json.as_deref() else {
                    sink.emit(ClientEvent::Error {
                        kind: ErrorKindDto::Protocol,
                        message: "hook save payload is required".to_string().into(),
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
                        let (effect, details_json) = if self.is_turn_active() {
                            (ConfigurationEffectDto::RestartRequired, None)
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
                            match mark_settings_keys_applied(
                                self.settings.as_ref(),
                                &["hooks"],
                                "hook save",
                            ) {
                                Ok(()) => (ConfigurationEffectDto::Applied, None),
                                Err(error) => (
                                    ConfigurationEffectDto::RestartRequired,
                                    Some(settings_reload_error_details(&error)),
                                ),
                            }
                        } else {
                            (ConfigurationEffectDto::RestartRequired, None)
                        };
                        self.emit_configuration_operation(
                            sink,
                            ConfigurationDomainDto::Hook,
                            operation_id,
                            ConfigurationOperationStatusDto::Succeeded,
                            effect,
                            Some("Saved hooks configuration.".to_string()),
                            details_json,
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
                    message: format!("unsupported hook admin action: {action}").into(),
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
