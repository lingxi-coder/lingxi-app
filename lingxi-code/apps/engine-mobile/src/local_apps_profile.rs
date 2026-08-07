//! Process-wide, profile-keyed local-app service registry.

use crate::local_apps_generation::{ClientGenerationJobObserver, MobileAppGenerationExecutor};
use crate::local_apps_host::LocalAppsHostBroker;
use crate::local_apps_llm::LocalAppsLlm;
use async_trait::async_trait;
use client_adapter::ClientEventSink;
use client_protocol::events::ClientEvent;
use local_apps::{AppError, AppEventFanout, AppGenerationCoordinator, AppService};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, RwLock, Weak};
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

/// Swappable holder for a profile's [`LocalAppsLlm`].
///
/// `ProfileApps` is cached process-wide (see [`profile_apps`]'s `OnceCell`),
/// but its `llm` is NOT — every consumer that runs the three LLM round
/// trips (`host.rs`'s authoring/planning triggers, and
/// `MobileAppGenerationExecutor`'s source generation) reads through this
/// cell instead of holding its own `Arc<LocalAppsLlm>`. `clock` /
/// `mobile_linux` stay genuinely pinned to whichever connection first loaded
/// the profile (their doc above explains why); the model is different: it
/// carries auth, and a RECONNECT (a fresh `MobileEngineHandle` — possibly
/// rotated credentials, possibly a different `ApiService`) hits this exact
/// cache-hit path with a brand-new `LocalAppsLlm`. Pinning that silently
/// would mean generation keeps authenticating as a rotated-out credential
/// with no error and no log — so [`profile_apps`] refreshes this cell on
/// every call, cached hit or not.
///
/// This does NOT cover a live `/model` switch — `ClientCommand::SetModel`
/// never rebuilds the engine, so it never reaches `profile_apps` at all.
/// That path is fixed separately and more narrowly: `ApiServiceModel`
/// (`local_apps_llm.rs`) holds its own model/profile behind a lock and
/// `SetModel`'s handler mutates it in place via
/// [`crate::local_apps_llm::LocalAppsModel::set_model`] — the SAME
/// `Arc<LocalAppsLlm>` this cell holds for the connection's lifetime, no
/// swap needed.
pub(crate) struct SharedLlm(RwLock<Arc<LocalAppsLlm>>);

impl SharedLlm {
    pub(crate) fn new(llm: Arc<LocalAppsLlm>) -> Self {
        Self(RwLock::new(llm))
    }

    /// The current model. Read fresh on every use (not cached by the
    /// caller) so a swap takes effect for the very next LLM call, including
    /// one already in flight when the swap lands but that has not yet
    /// reached `structured()`.
    pub(crate) fn current(&self) -> Arc<LocalAppsLlm> {
        self.0.read().expect("shared llm poisoned").clone()
    }

    /// Swap in a new model — called by [`profile_apps`] with the calling
    /// connection's own `ApiService`-backed model.
    pub(crate) fn replace(&self, llm: Arc<LocalAppsLlm>) {
        *self.0.write().expect("shared llm poisoned") = llm;
    }
}

/// Where a background authoring/planning task ([`spawn_authoring`] /
/// [`spawn_planning`]) reports the ONE thing it might need to tell a client
/// directly: a synthesized `AppOperationFailed` for an error that never
/// reaches the domain-event pipeline (e.g. the app record itself could not
/// be read before authoring even started). Every OTHER outcome —
/// `questionnaire_ready`, `questionnaire_failed`, `plan_ready`,
/// `plan_failed` — is a normal service mutation and already reaches every
/// subscribed client through the service's own `AppEventObserver` fanout
/// regardless of who triggered it (the wire client via `host.rs`, or the MCP
/// `create` tool via `LocalAppsHostBroker`) — this trait is NOT that path,
/// only the engine-synthesized-failure one.
#[async_trait]
pub(crate) trait AppFailureNotifier: Send + Sync {
    async fn notify_failure(
        &self,
        service: Option<&AppService>,
        app_id: Option<String>,
        error: &AppError,
    );
}

/// Best-effort log for a failed FAIL-CLOSE. `questionnaire_ready`/
/// `plan_ready` failing is expected (a stale or malformed LLM answer); the
/// FOLLOW-UP `questionnaire_failed`/`plan_failed` call ALSO failing means the
/// app's workflow state already moved out from under this task before it
/// could record the failure — e.g. a second authoring attempt (the
/// `retry_questionnaire`-from-`authoring_questionnaire` escape hatch) beat
/// this one to a terminal state. Nothing is silently lost: SOME transition
/// already committed (that's how the state moved), and
/// [`AppFailureNotifier::notify_failure`] below still reports the ORIGINAL
/// error — this is a defense-in-depth log for an edge case, not the only
/// signal a caller gets.
fn log_failed_fail_close(
    app_id: &str,
    stage: &'static str,
    original: &AppError,
    fail_close_error: AppError,
) {
    tracing::warn!(
        app_id,
        stage,
        original_error = %original,
        fail_close_error = %fail_close_error,
        "fail-closed transition itself failed — the app's workflow state moved out from \
         under this task before it could record the failure",
    );
}

/// 出题跑在后台：创建命令立刻返回，界面进「出题中」，模型往返
/// 落定后再推状态。失败落 questionnaire_failed，绝不静默降级
/// —— 模版已经删了，没有可退的默认问卷。
///
/// A free function (not a `MobileEngineHandle` method) so the wire-client
/// trigger (`host.rs`'s `handle_create_app`/`handle_update_app_brief`/
/// `handle_retry_app_questionnaire`) and the MCP `create` tool
/// (`LocalAppsHostBroker::trigger_authoring`) share ONE fail-closed
/// implementation. Two drifting copies of exactly this logic is the shape of
/// gap this whole task exists to close — the fabrication tripwire was about
/// wiring ONE entry point and silently leaving a second one behind.
///
/// Spawned on [`worker_runtime`], NOT the caller's own connection-owned
/// runtime: `worker_runtime`'s own doc explains why — a `MobileEngineHandle`
/// dies on reconnect/project-switch while the process-wide profile survives,
/// so a task anchored to the connection's runtime would be silently aborted
/// mid round trip by an ordinary reconnect, not just a crash. Anchoring here
/// is what the generation worker already does for the identical reason.
///
/// `epoch` MUST be the `llm_round` value the caller captured synchronously
/// when it started THIS round (`create_app`'s returned record, or the `u64`
/// `retry_questionnaire`/`update_brief` return) — never re-read later, or a
/// third round starting between the capture and the re-read could be
/// mis-attributed to this task. Passed straight through to
/// `questionnaire_ready`/`questionnaire_failed`, which reject a stale epoch
/// as a no-op: see [`local_apps::AppRecord::llm_round`]'s doc for why this
/// exists — admitting `authoring_questionnaire` as its own
/// `retry_questionnaire` source (the manual escape for a STUCK app) also
/// means a manual retry can race a round that was only SLOW, not dead,
/// spawning two tasks for the same app. Without the epoch check the loser
/// could silently commit its (stale-brief) questionnaire as the winner's.
pub(crate) fn spawn_authoring(
    service: Arc<AppService>,
    llm: Arc<LocalAppsLlm>,
    notifier: Arc<dyn AppFailureNotifier>,
    app_id: String,
    epoch: u64,
) -> tokio::task::JoinHandle<()> {
    worker_runtime().spawn(async move {
        let brief = match service.record(&app_id).await {
            Ok(record) => record.brief,
            Err(error) => {
                notifier
                    .notify_failure(Some(&service), Some(app_id), &error)
                    .await;
                return;
            }
        };
        match llm.author_questionnaire(&brief).await {
            Ok((name, steps)) => {
                if let Err(error) = service
                    .questionnaire_ready(&app_id, steps, name, epoch)
                    .await
                {
                    let fail_close = service
                        .questionnaire_failed(&app_id, &format!("{error}"), epoch)
                        .await;
                    report_llm_failure(
                        &service,
                        notifier.as_ref(),
                        &app_id,
                        "questionnaire_ready",
                        error,
                        fail_close,
                    )
                    .await;
                }
            }
            Err(error) => {
                let fail_close = service
                    .questionnaire_failed(&app_id, &format!("{error}"), epoch)
                    .await;
                report_llm_failure(
                    &service,
                    notifier.as_ref(),
                    &app_id,
                    "author_questionnaire",
                    error,
                    fail_close,
                )
                .await;
            }
        }
        service.announce_apps().await;
    })
}

/// 出方案跑在后台，形状同 [`spawn_authoring`]：读 brief + 问卷 + 答案，
/// 调模型出方案；成功落 `plan_ready`（打开确认门），失败落
/// `plan_failed`，绝不静默降级。See [`spawn_authoring`]'s doc for why this
/// is a shared free function, why it runs on [`worker_runtime`], and what
/// `epoch` must be.
pub(crate) fn spawn_planning(
    service: Arc<AppService>,
    llm: Arc<LocalAppsLlm>,
    notifier: Arc<dyn AppFailureNotifier>,
    app_id: String,
    epoch: u64,
) -> tokio::task::JoinHandle<()> {
    worker_runtime().spawn(async move {
        let loaded = async {
            let record = service.record(&app_id).await?;
            let draft = service.draft(&app_id).await?;
            Ok::<_, AppError>((record, draft))
        }
        .await;
        let (record, draft) = match loaded {
            Ok(pair) => pair,
            Err(error) => {
                notifier
                    .notify_failure(Some(&service), Some(app_id), &error)
                    .await;
                return;
            }
        };
        match llm
            .plan(&record.brief, &draft.questionnaire, &draft.fields)
            .await
        {
            Ok(plan) => {
                if let Err(error) = service.plan_ready(&app_id, plan, epoch).await {
                    let fail_close = service
                        .plan_failed(&app_id, &format!("{error}"), epoch)
                        .await;
                    report_llm_failure(
                        &service,
                        notifier.as_ref(),
                        &app_id,
                        "plan_ready",
                        error,
                        fail_close,
                    )
                    .await;
                }
            }
            Err(error) => {
                let fail_close = service
                    .plan_failed(&app_id, &format!("{error}"), epoch)
                    .await;
                report_llm_failure(
                    &service,
                    notifier.as_ref(),
                    &app_id,
                    "plan",
                    error,
                    fail_close,
                )
                .await;
            }
        }
        service.announce_apps().await;
    })
}

/// Report an LLM-round failure to the client — UNLESS the matching
/// fail-close (`questionnaire_failed`/`plan_failed`) turned out to be a
/// stale no-op (`Ok(false)`), meaning a FRESHER round already superseded
/// this one and owns whatever the app's current state says. Reporting THIS
/// round's failure anyway would show the user a confusing error on an app
/// that may already look successful (the review's exact scenario). A
/// genuine fail-close ERROR (a state-guard violation for some other
/// reason — not staleness) still logs via [`log_failed_fail_close`] AND
/// notifies with the ORIGINAL llm/service error, since that remains the
/// most useful thing to show even when the bookkeeping itself hit a snag.
async fn report_llm_failure(
    service: &AppService,
    notifier: &dyn AppFailureNotifier,
    app_id: &str,
    stage: &'static str,
    original: AppError,
    fail_close: Result<bool, AppError>,
) {
    match fail_close {
        Ok(true) => {
            notifier
                .notify_failure(Some(service), Some(app_id.to_string()), &original)
                .await;
        }
        Ok(false) => {
            // A fresher round already owns this app; this round's opinion
            // about its own failure is no longer relevant to show anyone.
        }
        Err(fail_error) => {
            log_failed_fail_close(app_id, stage, &original, fail_error);
            notifier
                .notify_failure(Some(service), Some(app_id.to_string()), &original)
                .await;
        }
    }
}

pub(crate) struct ProfileApps {
    pub(crate) service: Arc<AppService>,
    pub(crate) generation: Arc<AppGenerationCoordinator>,
    pub(crate) host: Arc<LocalAppsHostBroker>,
    pub(crate) domain_events: Arc<AppEventFanout>,
    pub(crate) client_events: Arc<ClientEventFanout>,
    pub(crate) llm: Arc<SharedLlm>,
}

impl ProfileApps {
    async fn load(
        root: PathBuf,
        clock: Arc<dyn Clock>,
        mobile_linux: Option<Arc<dyn MobileLinuxRuntime>>,
        full_runtime: bool,
        runtime_root: Option<PathBuf>,
        llm: Arc<LocalAppsLlm>,
    ) -> Result<Arc<Self>, AppError> {
        let llm = Arc::new(SharedLlm::new(llm));
        let client_events = Arc::new(ClientEventFanout::new());
        let host = LocalAppsHostBroker::new(
            root.clone(),
            client_events.clone(),
            mobile_linux.clone(),
            full_runtime,
            runtime_root,
        );
        let executor = MobileAppGenerationExecutor::new(mobile_linux, host.clone(), llm.clone());
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
        host.attach_llm(llm.clone())
            .map_err(|_| AppError::Io("local-app host llm was already attached".into()))?;
        Ok(Arc::new(Self {
            service,
            generation,
            host,
            domain_events,
            client_events,
            llm,
        }))
    }
}

pub(crate) async fn profile_apps(
    root: PathBuf,
    clock: Arc<dyn Clock>,
    mobile_linux: Option<Arc<dyn MobileLinuxRuntime>>,
    full_runtime: bool,
    runtime_root: Option<PathBuf>,
    llm: Arc<LocalAppsLlm>,
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
    // Cloned BEFORE `llm` moves into the (maybe-never-run) init closure below,
    // so it survives to refresh a CACHED profile too — see `SharedLlm`'s doc.
    let refresh_llm = llm.clone();
    let profile = cell
        .get_or_try_init(|| async move {
            worker_runtime()
                .spawn(ProfileApps::load(
                    root,
                    clock,
                    mobile_linux,
                    full_runtime,
                    runtime_root,
                    llm,
                ))
                .await
                .map_err(|error| AppError::Io(format!("local-app profile load failed: {error}")))?
        })
        .await
        .cloned()?;
    profile.llm.replace(refresh_llm);
    Ok(profile)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::local_apps_llm::test_support::ScriptedModel;
    use local_apps::test_support::FixedClock;

    /// Neither test below drives generation far enough to reach the LLM call
    /// (see the comment on `generation_worker_survives_the_engine_runtime_
    /// that_loaded_the_profile`), so an empty script is enough — any call
    /// would fail loudly with "ran out of responses" rather than silently
    /// returning something plausible.
    fn no_op_llm() -> Arc<LocalAppsLlm> {
        Arc::new(LocalAppsLlm::new(ScriptedModel::new(Vec::new())))
    }

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
            no_op_llm(),
        )
        .await
        .expect("first profile");
        let second = profile_apps(
            root,
            Arc::new(FixedClock::new(2_000)),
            None,
            false,
            None,
            no_op_llm(),
        )
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
            let profile = profile_apps(
                root,
                Arc::new(FixedClock::new(1_000)),
                None,
                false,
                None,
                no_op_llm(),
            )
            .await
            .expect("profile");
            let record = profile
                .service
                .create_app(Some("Survivor"), "a test app", None)
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
