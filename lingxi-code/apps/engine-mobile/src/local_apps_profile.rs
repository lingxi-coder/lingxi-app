//! Process-wide, profile-keyed local-app service registry.

use crate::local_apps_generation::{ClientGenerationJobObserver, MobileAppGenerationExecutor};
use crate::local_apps_host::LocalAppsHostBroker;
use async_trait::async_trait;
use client_adapter::ClientEventSink;
use client_protocol::events::ClientEvent;
use local_apps::{AppError, AppEventFanout, AppGenerationCoordinator, AppService};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, Weak};
use tokio::sync::OnceCell;
use traits::{Clock, MobileLinuxRuntime};

type ProfileCell = Arc<OnceCell<Arc<ProfileApps>>>;

fn registry() -> &'static Mutex<HashMap<PathBuf, ProfileCell>> {
    static REGISTRY: OnceLock<Mutex<HashMap<PathBuf, ProfileCell>>> = OnceLock::new();
    REGISTRY.get_or_init(|| Mutex::new(HashMap::new()))
}

/// The long-lived runtime every `ProfileApps`-owned task runs on.
///
/// `ProfileApps` is cached process-wide, but the tasks it owns are `tokio::spawn`ed
/// onto the AMBIENT runtime — the generation worker in `attach_service`, the
/// static server and the full-runtime exit watch in the broker. For the first
/// engine that ambient runtime is the `MobileEngineHandle`-owned one, which the
/// next reconnect or project switch drops while the cached profile survives:
/// generation silently retires for the rest of the process and a `Running` entry
/// outlives the socket it describes. Anchoring the load here anchors them all.
pub(crate) fn worker_runtime() -> &'static tokio::runtime::Handle {
    static RUNTIME: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
    RUNTIME
        .get_or_init(|| {
            tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .enable_all()
                .thread_name("lingxi-local-apps")
                .build()
                .expect("build the local-app worker runtime")
        })
        .handle()
}

pub(crate) struct ClientEventFanout {
    next_id: AtomicU64,
    sinks: Mutex<HashMap<u64, Weak<dyn ClientEventSink>>>,
}

impl ClientEventFanout {
    fn new() -> Self {
        Self {
            next_id: AtomicU64::new(1),
            sinks: Mutex::new(HashMap::new()),
        }
    }

    pub(crate) fn subscribe(&self, sink: Arc<dyn ClientEventSink>) -> u64 {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        self.sinks
            .lock()
            .expect("local-app client fanout poisoned")
            .insert(id, Arc::downgrade(&sink));
        id
    }

    pub(crate) fn unsubscribe(&self, id: u64) {
        self.sinks
            .lock()
            .expect("local-app client fanout poisoned")
            .remove(&id);
    }
}

#[async_trait]
impl ClientEventSink for ClientEventFanout {
    async fn emit(&self, event: ClientEvent) {
        let sinks = {
            let mut registrations = self.sinks.lock().expect("local-app client fanout poisoned");
            let mut live = Vec::with_capacity(registrations.len());
            registrations.retain(|_, sink| match sink.upgrade() {
                Some(sink) => {
                    live.push(sink);
                    true
                }
                None => false,
            });
            live
        };
        for sink in sinks {
            sink.emit(event.clone()).await;
        }
    }
}

pub(crate) struct ProfileApps {
    pub(crate) service: Arc<AppService>,
    pub(crate) generation: Arc<AppGenerationCoordinator>,
    pub(crate) host: Arc<LocalAppsHostBroker>,
    pub(crate) domain_events: Arc<AppEventFanout>,
    pub(crate) client_events: Arc<ClientEventFanout>,
}

impl ProfileApps {
    async fn load(
        root: PathBuf,
        clock: Arc<dyn Clock>,
        mobile_linux: Option<Arc<dyn MobileLinuxRuntime>>,
        full_runtime: bool,
        runtime_root: Option<PathBuf>,
    ) -> Result<Arc<Self>, AppError> {
        let client_events = Arc::new(ClientEventFanout::new());
        let host = LocalAppsHostBroker::new(
            root.clone(),
            client_events.clone(),
            mobile_linux.clone(),
            full_runtime,
            runtime_root,
        );
        let executor = MobileAppGenerationExecutor::new(mobile_linux, host.clone());
        let generation = AppGenerationCoordinator::new_with_observer(
            root.clone(),
            clock.clone(),
            executor.clone(),
            ClientGenerationJobObserver::new(root.clone(), client_events.clone()),
        );
        let domain_events = Arc::new(AppEventFanout::new());
        let service = Arc::new(
            AppService::load(root, clock, generation.clone(), domain_events.clone()).await?,
        );
        executor
            .attach_service(service.clone())
            .map_err(|_| AppError::Io("generation executor was already attached".into()))?;
        generation.attach_service(service.clone()).await?;
        host.attach_generation(generation.clone())
            .map_err(|_| AppError::Io("local-app generation host was already attached".into()))?;
        host.attach_service(service.clone())
            .map_err(|_| AppError::Io("local-app host was already attached".into()))?;
        Ok(Arc::new(Self {
            service,
            generation,
            host,
            domain_events,
            client_events,
        }))
    }
}

pub(crate) async fn profile_apps(
    root: PathBuf,
    clock: Arc<dyn Clock>,
    mobile_linux: Option<Arc<dyn MobileLinuxRuntime>>,
    full_runtime: bool,
    runtime_root: Option<PathBuf>,
) -> Result<Arc<ProfileApps>, AppError> {
    let cell = {
        let mut profiles = registry()
            .lock()
            .expect("local-app profile registry poisoned");
        profiles
            .entry(root.clone())
            .or_insert_with(|| Arc::new(OnceCell::new()))
            .clone()
    };
    cell.get_or_try_init(|| async move {
        worker_runtime()
            .spawn(ProfileApps::load(
                root,
                clock,
                mobile_linux,
                full_runtime,
                runtime_root,
            ))
            .await
            .map_err(|error| AppError::Io(format!("local-app profile load failed: {error}")))?
    })
    .await
    .cloned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use local_apps::test_support::FixedClock;

    #[tokio::test]
    async fn same_profile_root_reuses_one_app_service() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path().join("profile");
        let first = profile_apps(
            root.clone(),
            Arc::new(FixedClock::new(1_000)),
            None,
            false,
            None,
        )
        .await
        .expect("first profile");
        let second = profile_apps(root, Arc::new(FixedClock::new(2_000)), None, false, None)
            .await
            .expect("second profile");

        assert!(Arc::ptr_eq(&first, &second));
        assert!(Arc::ptr_eq(&first.service, &second.service));
        assert!(Arc::ptr_eq(&first.generation, &second.generation));
    }

    #[test]
    fn generation_worker_survives_the_engine_runtime_that_loaded_the_profile() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path().join("profile");
        let engine_runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .expect("engine runtime");
        let (profile, app_id) = engine_runtime.block_on(async {
            let profile = profile_apps(root, Arc::new(FixedClock::new(1_000)), None, false, None)
                .await
                .expect("profile");
            let record = profile
                .service
                .create_app("Survivor", "a test app", None)
                .await
                .expect("create app");
            local_apps::test_support::advance_to_collecting_spec(&profile.service, &record.id)
                .await;
            (profile, record.id)
        });
        // The reconnect / project switch: engine #1's runtime goes away while
        // the process-wide profile — and its sole generation worker — stay.
        drop(engine_runtime);

        let next_engine = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("next engine runtime");
        next_engine.block_on(async {
            let interaction = profile
                .service
                .open_designer(&app_id)
                .await
                .expect("open designer");
            local_apps::test_support::stamp_fresh_plan(&profile.service, &app_id).await;
            let revision = profile.service.draft(&app_id).await.expect("draft").revision;
            profile
                .service
                .confirm_design(&app_id, &interaction.interaction_id, revision)
                .await
                .expect("confirm design");

            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
            loop {
                let picked_up = profile
                    .generation
                    .jobs_for_app(&app_id)
                    .await
                    .expect("jobs")
                    .iter()
                    .any(|job| job.status != local_apps::GenerationJobStatus::Queued);
                if picked_up {
                    break;
                }
                assert!(
                    std::time::Instant::now() < deadline,
                    "the generation worker died with the engine runtime that loaded the profile"
                );
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            }
        });
    }
}
