use super::command_source_string;
use super::lower_auth_state;
use super::provider_model_catalog_from_listings;
use super::EngineCommandRouter;
use super::DEFAULT_SESSION_LIST_LIMIT;
use client::adapter::lowering::lower_agent_info;
use client::adapter::lowering::lower_doctor_report;
use client::adapter::lowering::lower_hook_info;
use client::adapter::lowering::lower_mcp_server_info;
use client::adapter::lowering::lower_skill_info;
use client::adapter::lowering::lower_status_snapshot;
use client::adapter::ClientEventSink;
use client::protocol::commands::ListingKindDto;
use client::protocol::events::ClientEvent;
use client::protocol::listings::AuthStateDto;
use client::protocol::listings::SlashCommandDto;
use command_api::builtin_support::names::core_description;
use command_api::builtin_support::names::is_palette_hidden;
use command_api::registry::CommandRegistry;
use lingxi_core::host::task_registry::TaskListFilter;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct SlashAuthoritySnapshot {
    pub(super) session_id: String,
    pub(super) model: String,
    pub(super) permission_mode: Option<String>,
    pub(super) auth: AuthStateDto,
    pub(super) catalog: Option<Vec<SlashCommandDto>>,
}

impl EngineCommandRouter {
    /// Snapshot the live slash-command catalog from the shared registry, adding
    /// the local `/reload-plugins` command when the registry itself does not
    /// carry it.
    pub(super) async fn slash_command_catalog(&self) -> Option<Vec<SlashCommandDto>> {
        let registry = self.slash_registry.as_ref()?;
        let reg = registry.read().await;
        Some(Self::slash_command_catalog_from_registry(&reg))
    }
    pub(super) fn slash_command_catalog_from_registry(
        reg: &CommandRegistry,
    ) -> Vec<SlashCommandDto> {
        let mut commands: Vec<SlashCommandDto> = reg
            .palette_commands()
            .into_iter()
            .map(|cmd| SlashCommandDto {
                hidden: is_palette_hidden(&cmd.name),
                source: command_source_string(cmd.source).to_string(),
                name: cmd.name,
                description: cmd.description,
                aliases: cmd.aliases,
                argument_hint: cmd.argument_hint,
                menu_description: cmd.menu_description,
            })
            .collect();
        if !commands.iter().any(|cmd| cmd.name == "reload-plugins")
            && !is_palette_hidden("reload-plugins")
        {
            commands.push(SlashCommandDto {
                name: "reload-plugins".to_string(),
                description: core_description("reload-plugins").to_string(),
                source: "builtin".to_string(),
                aliases: Vec::new(),
                argument_hint: None,
                menu_description: None,
                hidden: false,
            });
        }
        commands.sort_by(|a, b| a.name.cmp(&b.name));
        commands
    }
    pub(super) async fn capture_slash_authority(&self) -> SlashAuthoritySnapshot {
        let snapshot = self.handle.get_status_snapshot().await;
        SlashAuthoritySnapshot {
            session_id: self.handle.current_session_id().await.to_string(),
            model: lingxi_core::host::qualified_model_ref(
                &snapshot.model,
                snapshot.model_profile.as_deref(),
            ),
            permission_mode: self.handle.permission_mode().await,
            auth: lower_auth_state(self.auth.current_user().await),
            catalog: self.slash_command_catalog().await,
        }
    }
    pub(super) async fn emit_slash_authority_changes(
        &self,
        before: &SlashAuthoritySnapshot,
        after: &SlashAuthoritySnapshot,
        sink: &dyn ClientEventSink,
    ) {
        for event in Self::slash_authority_change_events(before, after) {
            sink.emit(event).await;
        }
    }
    pub(super) fn slash_authority_change_events(
        before: &SlashAuthoritySnapshot,
        after: &SlashAuthoritySnapshot,
    ) -> Vec<ClientEvent> {
        let mut events = Vec::new();
        if before.session_id != after.session_id {
            events.push(ClientEvent::SessionEnded);
        }
        if before.model != after.model {
            events.push(ClientEvent::ModelChanged {
                model: after.model.clone(),
            });
        }
        if before.permission_mode != after.permission_mode {
            if let Some(mode) = after.permission_mode.clone() {
                events.push(ClientEvent::PermissionModeChanged { mode });
            }
        }
        if before.auth != after.auth {
            events.push(ClientEvent::AuthState {
                state: after.auth.clone(),
            });
        }
        if before.catalog != after.catalog {
            if let Some(commands) = after.catalog.clone() {
                events.push(ClientEvent::CommandsChanged { commands });
            }
        }
        events
    }
    /// Pull + emit a single listing kind. Listing kinds with no engine handle in
    /// the foundation are skipped (see module docs).
    pub(super) async fn emit_listing(&self, kind: ListingKindDto, sink: &dyn ClientEventSink) {
        match kind {
            ListingKindDto::Models => {
                let available = self.handle.list_available_models().await;
                let listings = self.handle.list_model_listings().await;
                let snapshot = self.handle.get_status_snapshot().await;
                let provider_catalog = if self.provider_model_catalog_listings.is_empty() {
                    provider_model_catalog_from_listings(&listings)
                } else {
                    provider_model_catalog_from_listings(&self.provider_model_catalog_listings)
                };
                sink.emit(ClientEvent::ProviderModelCatalog {
                    providers: provider_catalog,
                })
                .await;
                let curated = lingxi_core::host::curated_model_listings(
                    &listings,
                    &snapshot.model,
                    snapshot.model_profile.as_deref(),
                );
                let models = lingxi_core::host::curated_model_refs(
                    &listings,
                    &available,
                    &snapshot.model,
                    snapshot.model_profile.as_deref(),
                );
                let current = lingxi_core::host::qualified_model_ref(
                    &snapshot.model,
                    snapshot.model_profile.as_deref(),
                );
                sink.emit(ClientEvent::ModelList {
                    models,
                    current,
                    details: curated
                        .iter()
                        .map(client::adapter::lowering::lower_model_details)
                        .collect(),
                })
                .await;
            }
            ListingKindDto::Mcp => {
                let servers = self
                    .handle
                    .list_mcp_servers()
                    .await
                    .iter()
                    .map(lower_mcp_server_info)
                    .collect();
                sink.emit(ClientEvent::McpServers { servers }).await;
            }
            ListingKindDto::Skills => {
                let skills = self
                    .handle
                    .list_skills()
                    .await
                    .iter()
                    .map(lower_skill_info)
                    .collect();
                sink.emit(ClientEvent::Skills { skills }).await;
            }
            ListingKindDto::Hooks => {
                let hooks = self
                    .handle
                    .list_hooks()
                    .await
                    .iter()
                    .map(lower_hook_info)
                    .collect();
                sink.emit(ClientEvent::Hooks { hooks }).await;
            }
            ListingKindDto::Agents => {
                let agents = self
                    .handle
                    .list_agents()
                    .await
                    .iter()
                    .map(lower_agent_info)
                    .collect();
                sink.emit(ClientEvent::Agents { agents }).await;
            }
            ListingKindDto::Status => {
                let snapshot = lower_status_snapshot(&self.handle.get_status_snapshot().await);
                sink.emit(ClientEvent::StatusSnapshot { snapshot }).await;
            }
            ListingKindDto::Doctor => {
                let report = lower_doctor_report(&self.handle.run_doctor_checks().await);
                sink.emit(ClientEvent::DoctorReport { report }).await;
            }
            ListingKindDto::Auth => {
                let state = lower_auth_state(self.auth.current_user().await);
                sink.emit(ClientEvent::AuthState { state }).await;
            }
            ListingKindDto::Tasks => {
                self.emit_task_list(TaskListFilter::default(), sink).await;
            }
            ListingKindDto::SlashCommands => {
                if let Some(commands) = self.slash_command_catalog().await {
                    sink.emit(ClientEvent::SlashCommandCatalog { commands })
                        .await;
                } else {
                    tracing::debug!(
                        "bridge-server: slash-command catalog unavailable (no shared registry)"
                    );
                }
            }
            // HOST/engine-tier reads the binary wires once it holds the desktop
            // runtime (plan §2). No engine handle for these in the foundation —
            // routing them is additive and does not change this seam's shape.
            ListingKindDto::Sessions => {
                self.emit_session_list(DEFAULT_SESSION_LIST_LIMIT, sink)
                    .await;
            }
            ListingKindDto::Settings => {
                self.emit_settings_snapshot(sink).await;
            }
            ListingKindDto::Memory => {
                tracing::debug!(
                    ?kind,
                    "bridge-server: listing kind has no engine handle in the foundation"
                );
            }
            // `#[non_exhaustive]` catch-all: a future listing kind is additive.
            _ => {
                tracing::debug!(?kind, "bridge-server: unhandled listing kind");
            }
        }
    }
}
