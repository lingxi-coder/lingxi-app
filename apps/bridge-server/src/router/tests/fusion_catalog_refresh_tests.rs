use super::{CommandRouter, EngineCommandRouter};
use client::adapter::ClientEventSink;
use client::protocol::commands::{ClientCommand, ProviderCredentialSecretDto};
use client::protocol::events::ClientEvent;
use lingxi_core::host::task_registry::{
    TaskCreateInput, TaskListFilter, TaskOutputChunk, TaskRecord, TaskRegistryError,
    TaskRegistryHandle, TaskUpdatePatch,
};
use lingxi_core::host::{AuthError, AuthHandle, LoginInfo};
use platform_posix::{PlainTextSecureStorage, PosixClock, PosixHttp};
use std::sync::Arc;

struct SilentSink;
#[async_trait::async_trait]
impl ClientEventSink for SilentSink {
    async fn emit(&self, _event: ClientEvent) {}
}

struct MockAuth;
#[async_trait::async_trait]
impl AuthHandle for MockAuth {
    async fn login(&self) -> Result<LoginInfo, AuthError> {
        Ok(LoginInfo {
            email: "u@x.com".into(),
            org_id: "org_1".into(),
        })
    }
    async fn logout(&self) -> Result<(), AuthError> {
        Ok(())
    }
    async fn current_user(&self) -> Option<LoginInfo> {
        None
    }
}

#[derive(Default)]
struct MockTaskRegistry {
    human_messages: std::sync::Mutex<Vec<(String, String)>>,
}
#[async_trait::async_trait]
impl TaskRegistryHandle for MockTaskRegistry {
    async fn send_human_task_message(
        &self,
        task_id: &str,
        message: &str,
    ) -> Result<(), TaskRegistryError> {
        self.human_messages
            .lock()
            .unwrap()
            .push((task_id.into(), message.into()));
        Ok(())
    }
    async fn create(&self, _input: TaskCreateInput) -> Result<TaskRecord, TaskRegistryError> {
        Err(TaskRegistryError::Internal("unused".into()))
    }
    async fn get(&self, _id: &str) -> Result<Option<TaskRecord>, TaskRegistryError> {
        Ok(None)
    }
    async fn list(&self, _filter: TaskListFilter) -> Result<Vec<TaskRecord>, TaskRegistryError> {
        Ok(Vec::new())
    }
    async fn update(
        &self,
        _id: &str,
        _patch: TaskUpdatePatch,
    ) -> Result<TaskRecord, TaskRegistryError> {
        Err(TaskRegistryError::Internal("unused".into()))
    }
    async fn set_status(&self, _id: &str, _status: &str) -> Result<TaskRecord, TaskRegistryError> {
        Err(TaskRegistryError::Internal("unused".into()))
    }
    async fn kill(&self, _id: &str) -> Result<TaskRecord, TaskRegistryError> {
        Err(TaskRegistryError::Internal("unused".into()))
    }
    async fn output(
        &self,
        _id: &str,
        _offset: Option<u64>,
    ) -> Result<TaskOutputChunk, TaskRegistryError> {
        Err(TaskRegistryError::Internal("unused".into()))
    }
}

#[derive(Default)]
struct TaskMessageSink(std::sync::Mutex<Vec<ClientEvent>>);
#[async_trait::async_trait]
impl ClientEventSink for TaskMessageSink {
    async fn emit(&self, event: ClientEvent) {
        self.0.lock().unwrap().push(event);
    }
}

#[tokio::test]
async fn task_message_uses_trusted_registry_route_and_preserves_workspace_gate() {
    let handle = Arc::new(orchestrator::test_support::MockOrchestratorHandle::new());
    let registry = Arc::new(MockTaskRegistry::default());
    let router = EngineCommandRouter::new(
        handle.clone(),
        Arc::new(MockAuth),
        registry.clone(),
        None,
        None,
    );
    let sink = Arc::new(TaskMessageSink::default());
    router
        .route(
            ClientCommand::TaskMessage {
                task_id: "a123".into(),
                message: "  continue\nnext".into(),
            },
            sink.clone(),
        )
        .await;
    assert_eq!(
        *registry.human_messages.lock().unwrap(),
        vec![("a123".into(), "  continue\nnext".into())]
    );
    assert!(sink.0.lock().unwrap().iter().any(|event| matches!(
        event,
        ClientEvent::SystemNotice {
            is_error: false,
            ..
        }
    )));
    handle.set_workspace_trusted(false);
    router
        .route(
            ClientCommand::TaskMessage {
                task_id: "a123".into(),
                message: "denied".into(),
            },
            sink.clone(),
        )
        .await;
    assert_eq!(registry.human_messages.lock().unwrap().len(), 1);
    assert!(sink.0.lock().unwrap().iter().any(|event| matches!(
        event,
        ClientEvent::Error {
            kind: client::protocol::events::ErrorKindDto::Rejected,
            ..
        }
    )));
}

struct CronPermissionSink;
#[async_trait::async_trait]
impl client::adapter::PermissionRequestSink for CronPermissionSink {
    async fn emit_request(&self, _request: client::protocol::permission::PermissionRequest) {}
}

fn cron_request(action: &str) -> client::protocol::commands::CronRequestDto {
    client::protocol::commands::CronRequestDto {
        automation: None,
        action: action.into(),
        id: None,
        cron: Some("0 9 1 1 *".into()),
        prompt: Some("Prepare the daily report".into()),
        recurring: Some(true),
        durable: Some(true),
        expires_at: None,
        no_expiry: Some(true),
    }
}

async fn route_cron(
    router: &EngineCommandRouter,
    request: client::protocol::commands::CronRequestDto,
) -> Result<Vec<client::protocol::events::CronJobDto>, String> {
    let sink = Arc::new(TaskMessageSink::default());
    router
        .route(
            ClientCommand::CronManage {
                request_id: "cron-trust-regression".into(),
                request,
            },
            sink.clone(),
        )
        .await;
    let mut events = sink.0.lock().unwrap();
    assert_eq!(events.len(), 1);
    match events.pop().unwrap() {
        ClientEvent::CronResult {
            request_id,
            jobs,
            error,
        } => {
            assert_eq!(request_id, "cron-trust-regression");
            match error {
                Some(error) => {
                    assert!(jobs.is_empty());
                    Err(error)
                }
                None => Ok(jobs),
            }
        }
        event => panic!("expected CronResult, got {event:?}"),
    }
}

#[tokio::test]
async fn cron_host_trust_allows_persisted_crud_and_untrusted_mutations_are_rejected() {
    use lingxi_core::host::OrchestratorHandle;

    let temp = tempfile::tempdir().unwrap();
    let cwd = temp.path().join("project");
    let home = temp.path().join("home");
    std::fs::create_dir_all(&cwd).unwrap();
    let runtime = Box::pin(harness_runtime::desktop::build(
        harness_runtime::desktop::DesktopConfig {
            cwd: cwd.clone(),
            lingxi_home: home.clone(),
            isolated_credential_storage: true,
            credential_storage_policy: lingxi_core::host::CredentialStoragePolicy::PlainTextFixture,
            host_workspace_trusted: Some(true),
            restricted: true,
            strict_mcp_config: true,
            default_model_explicit: true,
            ..Default::default()
        },
        Arc::new(orchestrator::test_support::MockOutputStream::new()),
        Arc::new(CronPermissionSink),
    ))
    .await
    .unwrap();
    assert!(runtime.orchestrator.workspace_trusted().await);
    let trusted_router = || {
        EngineCommandRouter::new(
            runtime.orchestrator.clone(),
            runtime.auth.clone(),
            runtime.task_registry.clone(),
            None,
            None,
        )
        .with_session_store(super::SessionStoreContext::new(
            home.clone(),
            cwd.to_string_lossy().into_owned(),
            Arc::new(platform_posix::PosixFileSystem::new(cwd.clone())),
        ))
    };
    let router = trusted_router();
    let created = route_cron(&router, cron_request("create")).await.unwrap();
    assert_eq!(created.len(), 1);
    assert_eq!(created[0].prompt, "Prepare the daily report");
    assert!(created[0].durable);
    let status = runtime.orchestrator.get_status_snapshot().await;
    assert_eq!(
        created[0].session_id.as_deref(),
        Some(status.session_id.trim_start_matches("sess:"))
    );
    // A fresh router reloads the durable file rather than a UI-local draft.
    assert_eq!(
        route_cron(&trusted_router(), cron_request("list"))
            .await
            .unwrap(),
        created
    );

    let untrusted_handle = Arc::new(orchestrator::test_support::MockOrchestratorHandle::new());
    untrusted_handle.set_status_snapshot(status);
    untrusted_handle.set_workspace_trusted(false);
    let untrusted = EngineCommandRouter::new(
        untrusted_handle,
        runtime.auth.clone(),
        runtime.task_registry.clone(),
        None,
        None,
    );
    for action in ["create", "update", "delete"] {
        let mut request = cron_request(action);
        request.id = Some(created[0].id.clone());
        assert_eq!(
            route_cron(&untrusted, request).await.unwrap_err(),
            "Trust this workspace before changing scheduled tasks"
        );
        assert_eq!(
            route_cron(&untrusted, cron_request("list")).await.unwrap(),
            created,
            "{action} must leave persisted jobs unchanged and list must remain available"
        );
    }

    let mut update = cron_request("update");
    update.id = Some(created[0].id.clone());
    update.prompt = Some("Prepare the revised report".into());
    let updated = route_cron(&router, update).await.unwrap();
    assert_eq!(updated.len(), 1);
    assert_eq!(updated[0].id, created[0].id);
    assert_eq!(updated[0].created_at, created[0].created_at);
    assert_eq!(updated[0].prompt, "Prepare the revised report");
    assert_eq!(
        route_cron(&trusted_router(), cron_request("list"))
            .await
            .unwrap(),
        updated
    );
    let mut delete = cron_request("delete");
    delete.id = Some(created[0].id.clone());
    assert!(route_cron(&router, delete).await.unwrap().is_empty());
    assert!(route_cron(&trusted_router(), cron_request("list"))
        .await
        .unwrap()
        .is_empty());
    let shutdown = runtime.session_lifecycle.shutdown_and_drain().await;
    assert!(shutdown.complete, "shutdown errors: {:?}", shutdown.errors);
}

async fn router_with_credentials(
    credentials: Arc<secret::CredentialManager>,
    ephemeral: bool,
    catalog_registry: harness_runtime::desktop::FusionCatalogRegistry,
) -> EngineCommandRouter {
    EngineCommandRouter::new(
        Arc::new(orchestrator::test_support::MockOrchestratorHandle::new())
            as Arc<dyn lingxi_core::host::orchestrator::OrchestratorHandle>,
        Arc::new(MockAuth) as Arc<dyn AuthHandle>,
        Arc::new(MockTaskRegistry::default()) as Arc<dyn TaskRegistryHandle>,
        None,
        None,
    )
    .with_credentials(credentials)
    .with_ephemeral_provider_credentials(ephemeral)
    .with_catalog_registry(catalog_registry)
}

/// Both branches of the arm — the persistent keychain write and the
/// packaged/brokered ephemeral one — must reach the refresher. Asserting on
/// the SHARED availability map a registered `FusionCatalogRefresher` owns
/// (the very map `FusionCatalogModelSource::list()` re-filters against)
/// pins the wiring end to end, not just that some function was called.
#[tokio::test]
async fn setting_a_provider_credential_refreshes_the_fusion_catalog() {
    let temp = tempfile::tempdir().expect("tempdir");
    let storage = Arc::new(
        PlainTextSecureStorage::new(temp.path().join("credentials"))
            .await
            .expect("storage"),
    );
    let credentials = Arc::new(secret::CredentialManager::new(
        storage,
        Arc::new(PosixClock::new()),
        Arc::new(PosixHttp::new()),
    ));

    // The boot availability map of a session that had neither provider
    // credentialed — what Fusion's catalog filter enforces until a
    // credential write refreshes it.
    let availability = Arc::new(std::sync::RwLock::new(
        [
            ("openrouter".to_string(), false),
            ("deepseek".to_string(), false),
        ]
        .into_iter()
        .collect::<std::collections::BTreeMap<String, bool>>(),
    ));
    let catalog_registry = harness_runtime::desktop::FusionCatalogRegistry::default();
    harness_runtime::desktop::register_fusion_catalog_refresher(
        &catalog_registry,
        harness_runtime::desktop::FusionCatalogRefresher::for_keychain_profiles(
            availability.clone(),
            credentials.clone(),
            &["openrouter", "deepseek"],
        ),
    );

    let sink: Arc<dyn ClientEventSink> = Arc::new(SilentSink);
    router_with_credentials(credentials.clone(), false, catalog_registry.clone())
        .await
        .route(
            ClientCommand::SetProviderCredential {
                operation_id: 1,
                provider_id: "openrouter".into(),
                credential: ProviderCredentialSecretDto::new("sk-or-round9".into()),
            },
            sink.clone(),
        )
        .await;
    router_with_credentials(credentials, true, catalog_registry.clone())
        .await
        .route(
            ClientCommand::SetProviderCredential {
                operation_id: 2,
                provider_id: "deepseek".into(),
                credential: ProviderCredentialSecretDto::new("sk-ds-round9".into()),
            },
            sink,
        )
        .await;

    let published = availability.read().expect("availability lock").clone();
    assert_eq!(
        published.get("openrouter"),
        Some(&true),
        "the persistent Settings credential write must reach Fusion's \
catalog refresher; a stale `false` here is what silently drops every \
OpenRouter row from /fusion for the rest of the process: {published:?}"
    );
    assert_eq!(
        published.get("deepseek"),
        Some(&true),
        "the packaged/brokered EPHEMERAL branch of the same arm must \
refresh too — its key is never persisted, so only the write-side \
notification can make it visible to Fusion: {published:?}"
    );
}

/// Round-10 finding N3's class sweep: the SAME unbounded keychain re-probe
/// the parent-supplied-keys path had (`boot::seed_parent_supplied_provider_keys`)
/// also sat on this arm — and this one is awaited straight from the
/// connection's read loop (`server.rs`'s `on_frame`, contract: "return
/// promptly"). A contended macOS credential broker would stop the
/// connection from READING anything at all — interrupts and permission
/// replies included — for as long as the broker stalled.
///
/// Virtual time (`start_paused`): the assertion is on the DEADLINE.
#[tokio::test(start_paused = true)]
async fn a_stalled_credential_backend_never_parks_the_router_arm() {
    // "never-answers" has no credential of any kind, so the re-probe this
    // write triggers reaches the never-answering backend.
    let (credentials, _availability, reads, catalog_registry) =
        crate::boot::fusion_refresh_test_support::register_stalling_refresher(&[
            "openrouter",
            "never-answers",
        ]);

    let sink: Arc<dyn ClientEventSink> = Arc::new(SilentSink);
    let router = router_with_credentials(credentials, true, catalog_registry.clone()).await;
    let began = tokio::time::Instant::now();
    let arm_returned = tokio::time::timeout(
        crate::boot::FUSION_CATALOG_REFRESH_BUDGET * 3,
        router.route(
            ClientCommand::SetProviderCredential {
                operation_id: 7,
                provider_id: "openrouter".into(),
                credential: ProviderCredentialSecretDto::new("sk-or-round10".into()),
            },
            sink,
        ),
    )
    .await;
    assert!(
        arm_returned.is_ok(),
        "`SetProviderCredential` must return even when the credential backend \
never answers: this arm is awaited from the connection read loop, so a stall \
here stops the client's interrupts and permission replies from being read"
    );
    assert!(
        began.elapsed() < crate::boot::FUSION_CATALOG_REFRESH_BUDGET * 2,
        "the arm must return within one {:?} refresh budget, waited {:?}",
        crate::boot::FUSION_CATALOG_REFRESH_BUDGET,
        began.elapsed()
    );
    crate::boot::fusion_refresh_test_support::wait_for_refresh_start(&reads).await;
    assert!(
        reads.load(std::sync::atomic::Ordering::SeqCst) >= 1,
        "the refresh must actually have reached the credential backend — with \
zero reads this test would pass without exercising the stall at all"
    );
}

/// Round-12 finding [2]: the DELETE half of the same Settings surface.
///
/// `MultiCredentialProvider` reads `CredentialManager` per call, so the
/// ordinary turn loop stops routing a deleted provider immediately — but
/// `FusionCatalogModelSource::list()` keeps re-filtering against the
/// shared availability map, which nothing ever LOWERS. A provider whose
/// key was deleted mid-session therefore keeps every one of its catalog
/// rows, gets auto-selected as a `/fusion` panel, has budget reserved for
/// it, and dies on `LlmError::Authentication` at request time instead of
/// being excluded by the §4 preflight.
///
/// Both branches of the arm — persistent keychain delete and the
/// packaged/brokered ephemeral one — must clear the entry.
#[tokio::test]
async fn deleting_a_provider_credential_clears_it_from_the_fusion_catalog() {
    let temp = tempfile::tempdir().expect("tempdir");
    let storage = Arc::new(
        PlainTextSecureStorage::new(temp.path().join("credentials"))
            .await
            .expect("storage"),
    );
    let credentials = Arc::new(secret::CredentialManager::new(
        storage,
        Arc::new(PosixClock::new()),
        Arc::new(PosixHttp::new()),
    ));

    let availability = Arc::new(std::sync::RwLock::new(
        [
            ("openrouter".to_string(), false),
            ("deepseek".to_string(), false),
        ]
        .into_iter()
        .collect::<std::collections::BTreeMap<String, bool>>(),
    ));
    let catalog_registry = harness_runtime::desktop::FusionCatalogRegistry::default();
    harness_runtime::desktop::register_fusion_catalog_refresher(
        &catalog_registry,
        harness_runtime::desktop::FusionCatalogRefresher::for_keychain_profiles(
            availability.clone(),
            credentials.clone(),
            &["openrouter", "deepseek"],
        ),
    );

    let sink: Arc<dyn ClientEventSink> = Arc::new(SilentSink);
    // Boot state: both providers credentialed and published as available.
    router_with_credentials(credentials.clone(), false, catalog_registry.clone())
        .await
        .route(
            ClientCommand::SetProviderCredential {
                operation_id: 1,
                provider_id: "openrouter".into(),
                credential: ProviderCredentialSecretDto::new("sk-or-round12".into()),
            },
            sink.clone(),
        )
        .await;
    router_with_credentials(credentials.clone(), true, catalog_registry.clone())
        .await
        .route(
            ClientCommand::SetProviderCredential {
                operation_id: 2,
                provider_id: "deepseek".into(),
                credential: ProviderCredentialSecretDto::new("sk-ds-round12".into()),
            },
            sink.clone(),
        )
        .await;
    let seeded = availability.read().expect("availability lock").clone();
    assert_eq!(
        (seeded.get("openrouter"), seeded.get("deepseek")),
        (Some(&true), Some(&true)),
        "precondition: both writes must have published `true`, otherwise the \
delete assertions below would pass without exercising anything: {seeded:?}"
    );

    router_with_credentials(credentials.clone(), false, catalog_registry.clone())
        .await
        .route(
            ClientCommand::DeleteProviderCredential {
                operation_id: 3,
                provider_id: "openrouter".into(),
            },
            sink.clone(),
        )
        .await;
    router_with_credentials(credentials, true, catalog_registry.clone())
        .await
        .route(
            ClientCommand::DeleteProviderCredential {
                operation_id: 4,
                provider_id: "deepseek".into(),
            },
            sink,
        )
        .await;

    let published = availability.read().expect("availability lock").clone();
    assert_eq!(
        published.get("openrouter"),
        Some(&false),
        "the persistent Settings credential DELETE must clear Fusion's \
availability entry; a stale `true` here is what lets /fusion auto-select \
openrouter and burn a panel slot on LlmError::Authentication: {published:?}"
    );
    assert_eq!(
        published.get("deepseek"),
        Some(&false),
        "the packaged/brokered EPHEMERAL branch of the same arm must clear \
too — its key never touched the keychain, so only the delete-side \
notification can lower the entry: {published:?}"
    );
}
