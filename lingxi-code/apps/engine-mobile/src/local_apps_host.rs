//! Host-owned runtime, data, approval and WebView broker for local apps.
//!
//! The MCP provider deliberately has no direct filesystem, SQLite, process or
//! WebView handles.  This broker is the single trust boundary for those
//! operations and is also used by the native client command surface.

use crate::host::LocalAppBackgroundRunDto;
use crate::local_apps_mcp::LocalAppsMcpHost;
use async_trait::async_trait;
use client_adapter::ClientEventSink;
use client_protocol::events::ClientEvent;
use client_protocol::local_apps::{
    AppAuthorizationDecisionDto, AppBridgeOperationDto, AppBridgeRequestDto, AppBridgeResponseDto,
    AppCapabilityKindDto, AppCapabilityRequestDto, AppEventDto, AppUiActionKindDto,
    AppUiRequestDto, AppUiTargetDto,
};
use futures_util::StreamExt;
use local_apps::{
    load_manifest, load_permissions, save_permissions, AppCapability, AppDataStore,
    AppDependencyState, AppLayout, AppPermissions, AppRuntimeMode, AppRuntimeState, AppService,
    BackgroundTaskStatus, DataMigrationPreview, DataMutation, DataQuery, DataSortDirection,
    DataSortKey, PermissionDecision, SessionPermissions,
};
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::io::{self, Read};
use std::net::{IpAddr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock, RwLock};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{oneshot, watch, Mutex, Semaphore};
use tokio::time::{sleep, timeout, Duration};
use traits::mobile_linux::guest_paths;
use traits::{
    LinuxCommandRequest, MobileLinuxRuntime, MountPurpose, MountSpec, NetworkPolicy, ResourceLimits,
};

const APPROVAL_TIMEOUT: Duration = Duration::from_secs(5 * 60);
const UI_TIMEOUT: Duration = Duration::from_secs(2 * 60);
const MAX_HTTP_REQUEST_BYTES: usize = 16 * 1024;
const MAX_STATIC_ASSET_BYTES: u64 = 32 * 1024 * 1024;
const STATIC_REQUEST_CONCURRENCY: usize = 8;
const STATIC_ASSET_CHUNK_BYTES: usize = 256 * 1024;
const MAX_NETWORK_RESPONSE_BYTES: usize = 2 * 1024 * 1024;
const STATIC_ACCEPT_RETRY: Duration = Duration::from_millis(50);
const RUNTIME_SEED_POLL_INTERVAL: Duration = Duration::from_millis(100);
const DEPENDENCY_INSTALL_POLL_INTERVAL: Duration = Duration::from_millis(100);
const DEPENDENCY_INSTALL_TIMEOUT: Duration = Duration::from_secs(10 * 60);
const PNPM_TOOLCHAIN_KEY: &str = "pnpm@11.22.0/node@24.18.1";
const DEPENDENCY_SNAPSHOT_VERSION: u8 = 1;
const DEPENDENCY_SNAPSHOT_READY_FILE: &str = ".lingxi-dependency-ready";
const WORKSPACE_DEPENDENCY_ATTESTATION_FILE: &str = ".lingxi-build-state/dependency-attestation";
/// Consecutive `accept()` failures that retire the static server.  A burst of
/// ECONNABORTED/EMFILE must not, so the cap is deliberately generous
/// (100 * 50 ms ~= 5 s of an unbroken failure); a listener whose I/O driver is
/// gone fails EVERY poll and reaches it immediately.
const STATIC_ACCEPT_ERROR_LIMIT: u32 = 100;
/// First port of the window an app's PERMANENT loopback port is drawn from, and
/// the window's length — see `bind_stable_loopback` for why it has to sit below
/// every shipped platform's ephemeral floor.
const APP_PORT_WINDOW_FIRST: u16 = 20_000;
const APP_PORT_WINDOW_LEN: u16 = 12_000;
const LOCAL_APP_BRIDGE_CONTROL_BYTES: usize = 64 * 1024;
const LOCAL_APP_BRIDGE_LLM_BYTES: usize = 8 * 1024 * 1024;
/// File writes travel as JSON with a base64 body. The decoded file cap is
/// [`files_ops::MAX_APP_FILE_BYTES`]; the wire envelope must be large enough
/// for the 4/3 expansion plus a small JSON wrapper.
const LOCAL_APP_BRIDGE_FILE_BYTES: usize = files_ops::MAX_APP_FILE_BYTES.div_ceil(3) * 4 + 1024;
const FLOW_EXECUTION_TIMEOUT: Duration = Duration::from_secs(15 * 60);
const FLOW_STEP_TIMEOUT: Duration = Duration::from_secs(60);
static LOCAL_APP_BUILD_LOCK: OnceLock<Arc<Mutex<()>>> = OnceLock::new();
static DEPENDENCY_SNAPSHOT_DIGESTS: OnceLock<std::sync::Mutex<HashMap<PathBuf, String>>> =
    OnceLock::new();
const LOCAL_APP_CONTENT_SECURITY_POLICY: &str = "default-src 'self'; script-src 'self' 'unsafe-inline'; style-src 'self' 'unsafe-inline'; img-src 'self' data: blob:; font-src 'self' data:; connect-src 'self'; media-src 'self' data: blob:; worker-src 'none'; object-src 'none'; base-uri 'none'; frame-ancestors 'none'; form-action 'self'";

#[derive(Debug)]
struct UiResolution {
    decision: AppAuthorizationDecisionDto,
    result_json: Option<String>,
    error: Option<String>,
}

enum RuntimeHandle {
    Static { shutdown: oneshot::Sender<()> },
}

pub(super) struct PendingAppProfileProposal {
    pub(super) proposal: local_apps::AppAgentProfileProposal,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum RuntimeStartStatus {
    Pending,
    Running,
    Failed(String),
}

enum RuntimeEntryState {
    Starting {
        gate: watch::Sender<RuntimeStartStatus>,
    },
    Running {
        handle: RuntimeHandle,
    },
}

struct RuntimeEntry {
    state: RuntimeEntryState,
    last_used: u64,
    generation: u64,
}

/// Releases a `Starting` reservation whose owner never resolved it.
///
/// `start_reserved_runtime` can leave through `?` on a persist error and can be
/// dropped outright when the foreign caller cancels `submit`.  Without this the
/// entry — and the gate every later `StartApp` subscribes to — stays in the map
/// for the process's life, hanging every subsequent start; on iOS, where the
/// instance quota is 1, that bricks the whole local-app surface.
struct RuntimeReservation {
    runtimes: Arc<Mutex<HashMap<String, RuntimeEntry>>>,
    app_id: String,
    generation: u64,
}

impl RuntimeReservation {
    fn abandon(runtimes: &mut HashMap<String, RuntimeEntry>, app_id: &str, generation: u64) {
        let still_reserved = runtimes.get(app_id).is_some_and(|entry| {
            entry.generation == generation
                && matches!(entry.state, RuntimeEntryState::Starting { .. })
        });
        if !still_reserved {
            return;
        }
        if let Some(RuntimeEntry {
            state: RuntimeEntryState::Starting { gate },
            ..
        }) = runtimes.remove(app_id)
        {
            let _ = gate.send(RuntimeStartStatus::Failed(
                "runtime start was abandoned".into(),
            ));
        }
    }
}

impl Drop for RuntimeReservation {
    fn drop(&mut self) {
        // Only fires while the entry is STILL our `Starting` reservation, so the
        // success path (state replaced with `Running`) and every
        // `fail_reserved_runtime_start` path are no-ops — there is nothing to
        // commit explicitly.
        if let Ok(mut runtimes) = self.runtimes.try_lock() {
            Self::abandon(&mut runtimes, &self.app_id, self.generation);
            return;
        }
        let runtimes = Arc::clone(&self.runtimes);
        let app_id = std::mem::take(&mut self.app_id);
        let generation = self.generation;
        crate::local_apps_profile::worker_runtime().spawn(async move {
            Self::abandon(&mut *runtimes.lock().await, &app_id, generation);
        });
    }
}

/// Loopback ports an in-flight start has CHOSEN but has not yet persisted as
/// its app's pin, each paired with the app holding it.
///
/// `sibling_pinned_ports` reads the RECORDS, and a record only learns its port
/// when `update_runtime_record` writes it — two persists and, on the full
/// runtime, a deliberate ~11 ms after `bind_stable_loopback` picked it.  (That
/// distance is what keeps the next binder out of the kernel's 1.2-2.8 ms
/// refusal window after the probe listener closes; it must not be shortened.)
/// For that whole stretch the port sits in NO snapshot a sibling can read: a
/// concurrently-starting app derives or scans to the same port, binds it
/// cleanly because the probe is already gone, and pins it too.  `set_runtime`
/// then refuses to move either pin, so neither app can run while the other
/// does — and on Android the two share one `http://127.0.0.1:<port>` origin's
/// `localStorage` / `IndexedDB`.
///
/// A lease closes that stretch without closing the window: it is taken at the
/// instant a candidate is chosen and released only once the pin is durable, so
/// at any single INSTANT "persisted pins UNION live leases" names every port an
/// in-flight start owns.
///
/// Reading that union is NOT one instant, and the difference is the whole of
/// the subtlety here.  An allocator reads the pins first and takes its lease
/// second, so a sibling can persist its pin and release its lease entirely
/// between those two steps: the sibling's port is missing from the pin half
/// (read too early) and missing from the lease half (sampled too late), even
/// though neither half was ever wrong on its own.  A sample of a union is not
/// a sample of an instant.
///
/// What makes the sample sound is the ORDER those halves are consulted in,
/// plus a SECOND pin read taken after the lease (`bind_stable_loopback`).  A
/// lease is released only once the pin it covers is durable, so once we hold
/// the lease on a candidate, any sibling that could have chosen it either
/// still holds its own lease — in which case our take already failed — or has
/// already made its pin visible to that second read.  There is no third state,
/// and no sibling can newly choose the port while we hold it.  It needs no new
/// on-disk format — the records stay the registry, and this covers only the
/// gap before a record has the answer.
type PortLeases = Arc<std::sync::Mutex<HashMap<u16, String>>>;

/// A panic while choosing a port must not brick every later start, so the
/// poison is discarded rather than propagated: the map is a set of live
/// reservations, and a half-written insert cannot corrupt it.
fn lock_port_leases(leases: &PortLeases) -> std::sync::MutexGuard<'_, HashMap<u16, String>> {
    leases
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Releases a leased port whose start never persisted it.
///
/// Same lifetime shape as [`RuntimeReservation`], for the same reason: the
/// stretch it covers is crossed by `?` on two persist failures, by every
/// `fail_reserved_runtime_start` bail-out, by a panic, and by the foreign
/// caller cancelling `submit` outright.  A port leaked on any of those is a
/// port no app in the profile can ever use again for the life of the process.
///
/// Unlike `RuntimeReservation` this holds a std mutex, so `drop` completes
/// synchronously on whichever runtime the guard dies on — the guard is minted
/// on the worker runtime and dropped on the ambient one.
#[derive(Debug)]
struct PortLease {
    leases: PortLeases,
    app_id: String,
    port: u16,
    released: bool,
}

impl PortLease {
    /// Takes `port` for `app_id`, or `None` when another in-flight start
    /// already holds it.  Test-and-insert under one lock: two allocators
    /// racing on the same candidate cannot both come away with it.
    fn take(leases: &PortLeases, app_id: &str, port: u16) -> Option<Self> {
        {
            let mut held = lock_port_leases(leases);
            if held.contains_key(&port) {
                return None;
            }
            held.insert(port, app_id.to_string());
        }
        Some(Self {
            leases: Arc::clone(leases),
            app_id: app_id.to_string(),
            port,
            released: false,
        })
    }

    /// Hand-off point: the pin is now in the app's record, so
    /// `sibling_pinned_ports` sees the port and the lease is redundant.
    ///
    /// Releasing is the same operation `drop` performs — what `commit` buys is
    /// the ORDER.  It must be called after the persist and nowhere else: a
    /// release taken before it re-opens exactly the stretch this type exists
    /// to cover.
    fn commit(mut self) {
        self.release();
    }

    fn release(&mut self) {
        if self.released {
            return;
        }
        self.released = true;
        let mut held = lock_port_leases(&self.leases);
        // Only while the entry is STILL ours, mirroring
        // `RuntimeReservation::abandon`'s generation check: a late drop must
        // never hand away a port some other start has since leased.
        if held
            .get(&self.port)
            .is_some_and(|owner| owner == &self.app_id)
        {
            held.remove(&self.port);
        }
    }
}

impl Drop for PortLease {
    fn drop(&mut self) {
        self.release();
    }
}

// The `device.*` operations of the bridge — capture / pick / record / locate
// / notify. A CHILD module (not a sibling) so it reaches the broker's private
// fields and `authorize_declared_capability` without widening their
// visibility; split out purely for size. The dispatch match stays here.
#[path = "local_apps_host_device.rs"]
mod device_ops;

// `files.read` / `files.write` — app-private file store operations.
#[path = "local_apps_host_files.rs"]
mod files_ops;

// `llm.chat` — the app-initiated model call. A child module for the same
// reason as `device_ops`.
#[path = "local_apps_host_llm.rs"]
mod llm_ops;

// `agent.post` — the app-to-conversation mailbox write.
#[path = "local_apps_host_agent.rs"]
mod agent_ops;
pub(crate) use agent_ops::{
    AgentOutputRouter, AgentOutputStream, AgentTurnControl, AgentTurnUsageState,
    LocalAppsAgentExecutor,
};

#[path = "local_apps_host_background.rs"]
mod background_ops;

/// A bridge failure: human-readable message plus an optional stable machine
/// code the page can branch on (`AppBridgeResponseDto::error_code`). Every
/// legacy `Result<_, String>` site lowers through `From<String>` into a
/// code-less failure; only paths that deliberately publish a contract code
/// construct one with [`BridgeFailure::coded`].
#[derive(Debug)]
pub(crate) struct BridgeFailure {
    code: Option<&'static str>,
    message: String,
}

impl BridgeFailure {
    pub(crate) fn coded(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code: Some(code),
            message: message.into(),
        }
    }
}

impl From<String> for BridgeFailure {
    fn from(message: String) -> Self {
        Self {
            code: None,
            message,
        }
    }
}

impl From<&str> for BridgeFailure {
    fn from(message: &str) -> Self {
        Self::from(message.to_string())
    }
}

/// ONE canonical spelling for a session-catalog cwd key. `canonicalize`
/// collapses the platform's symlink split (`/var` vs `/private/var` on
/// iOS/macOS), so mint, listing, resume and the cwd gates all derive the SAME
/// sanitized `projects/` directory. Falls back to the raw string when the path
/// does not exist.
pub(crate) fn canonical_cwd_string(path: &std::path::Path) -> String {
    std::fs::canonicalize(path)
        .unwrap_or_else(|_| path.to_path_buf())
        .to_string_lossy()
        .to_string()
}

/// Delete a session file this host minted into an app's workspace catalog.
/// Used by both create paths when `set_init_session` refuses their id — the
/// set-once pin is the arbiter, and the loser's file would otherwise linger as
/// a phantom conversation row in the app's session list.
pub(crate) fn remove_app_session_file(
    lingxi_home: &std::path::Path,
    data_root: &std::path::Path,
    record: &local_apps::AppRecord,
    session_id: &str,
) -> bool {
    let workspace_cwd = canonical_cwd_string(&data_root.join(&record.workspace_rel));
    let path = lingxi_home
        .join("projects")
        .join(session::jsonl::path::project_dir_name(&workspace_cwd))
        .join(format!("{session_id}.jsonl"));
    std::fs::remove_file(path).is_ok()
}

pub(crate) fn create_next_step_guidance() -> String {
    "Run local-app-build: the workspace already contains the repository-verified Vite + Tailwind + shadcn/ui foundation. Edit app screens, app/globals.css, src/, components/, public/, and non-host-managed lib/style files. A host-owned `pnpm install` prepares workspace-local dependencies in the background; check `LocalAppInstallDeps` or `LocalAppGet` if you need its status. Do not recreate the app scaffold or run a package-manager scaffold command. Then call LocalAppBuild and preview via LocalAppRuntime.".into()
}

struct LocalAppsRuntimeConfiguration {
    mobile_linux: Option<Arc<dyn MobileLinuxRuntime>>,
    physical_memory_bytes: u64,
    runtime_root: Option<PathBuf>,
}

/// Profile-scoped broker.  The service is attached after its durable load has
/// completed, while command/capability resolution can be wired immediately.
pub(crate) struct LocalAppsHostBroker {
    root: PathBuf,
    event_sink: Arc<dyn ClientEventSink>,
    runtime_configuration: RwLock<LocalAppsRuntimeConfiguration>,
    service: OnceLock<Arc<AppService>>,
    /// Set once at profile load (same call site as `attach_service`), so the
    /// broker's `llm.chat` bridge operation reaches the live model.
    llm: OnceLock<Arc<crate::local_apps_profile::SharedLlm>>,
    /// Set at the same profile-load site as `llm` — live per-connection
    /// device handles behind a swap cell (see `local_apps_device`).
    device: OnceLock<Arc<crate::local_apps_device::SharedDeviceCapabilities>>,
    /// The single active `device.recordAudio*` session (one per broker — the
    /// platform has ONE audio session). Arc'd like `runtimes` so the duration
    /// watchdog task can reach it. See `device_ops`.
    recording: Arc<Mutex<Option<device_ops::ActiveRecording>>>,
    /// Captures retained so `llm.chat` can attach them by handle instead of
    /// copying base64 through every WebView/FFI layer. See
    /// [`crate::local_apps_device::MediaCache`].
    media: crate::local_apps_device::MediaCache,
    /// Apps with an `llm.chat` call in flight. One per app: an app-initiated
    /// call spends the user's quota, so a page cannot fan out.
    ///
    /// A std mutex behind an `Arc` on purpose: the slot is released by
    /// `LlmInflightGuard::drop`, which cannot await, and the set is only ever
    /// insert/remove — no lock is ever held across an await.
    pub(super) llm_inflight: Arc<std::sync::Mutex<std::collections::HashSet<String>>>,
    /// Serializes mailbox read-modify-writes. Held across the file update
    /// and NOTHING else — never across an emit, never across a client call.
    mailbox_writes: Mutex<()>,
    /// Serializes Agent session catalog read-modify-writes. Atomic file
    /// replacement alone cannot prevent concurrent creates/updates from
    /// overwriting a stale catalog snapshot.
    agent_session_writes: Mutex<()>,
    /// Host-owned app Agent execution seam, attached by the mobile composition
    /// root after the app service and MCP host are ready.
    agent_executor: OnceLock<Arc<dyn LocalAppsAgentExecutor>>,
    /// Active app Agent turns keyed by host-minted turn id.
    agent_turns: Arc<Mutex<HashMap<String, Arc<AgentTurnControl>>>>,
    /// One-time Profile proposals awaiting an explicit trusted-client decision.
    pending_profile_proposals: Mutex<HashMap<String, PendingAppProfileProposal>>,
    /// Serializes background task claims and journal transitions within one
    /// profile. The native scheduler may deliver duplicate wake-ups.
    background_task_writes: Mutex<()>,
    /// In-memory duplicate-delivery guard; persisted `Running` state handles
    /// process death, while this set handles concurrent WorkManager/BGTask
    /// deliveries in one process.
    background_inflight: Mutex<std::collections::HashSet<String>>,
    /// Serializes `device.recordAudioStart` — and ONLY starts.
    ///
    /// Separate from `recording` because a start crosses into Swift and the
    /// first mic use shows an OS permission alert with unbounded think time.
    /// Stops and reclaims take `recording` alone, so they can never be
    /// blocked behind that alert. Taken with `try_lock`: a second start
    /// answers `audio_session_busy` rather than queueing behind it.
    recording_start: Mutex<()>,
    /// Weak self-reference handed to the runtime-exit watchers, which are
    /// spawned onto the profile worker and outlive the call that started
    /// them. Weak so a watcher can never be what keeps the broker alive.
    self_ref: OnceLock<std::sync::Weak<LocalAppsHostBroker>>,
    pending_capabilities: Mutex<HashMap<String, oneshot::Sender<AppAuthorizationDecisionDto>>>,
    pending_ui: Mutex<HashMap<String, oneshot::Sender<UiResolution>>>,
    session_permissions: Mutex<SessionPermissions>,
    runtimes: Arc<Mutex<HashMap<String, RuntimeEntry>>>,
    /// See [`PortLeases`].  Broker-scoped because a profile's apps are what
    /// collide with each other, and one broker is exactly one profile.
    port_leases: PortLeases,
    /// Serializes port ALLOCATION — the sibling-pin read plus the choice —
    /// across this broker's starts.
    ///
    /// What it buys: the pin snapshot goes stale the instant another start
    /// persists one, and a lease is only taken AFTER the snapshot is read.
    /// Without this gate an allocator can read the pins, lose the scheduler for
    /// the length of another app's entire lease, and then choose from a set
    /// that never contained that app's port at all — so it wastes the whole
    /// scan re-deriving candidates it has no reason to reject.
    ///
    /// It does NOT buy mutual exclusion on a candidate: two ungated allocators
    /// sitting between the same pair of steps still cannot both come away with
    /// one port, because `PortLease::take` is a test-and-insert under a single
    /// mutex and the loser scans on.  Claiming otherwise here was the ninth
    /// false comment this module has shipped; the guarantee lives in `take`.
    ///
    /// What it does NOT buy, because this was mis-stated here once already: it
    /// does not make one start's snapshot fresh.  A sibling's persist and its
    /// `PortLease::commit` both run AFTER that sibling has left this gate, so
    /// they land freely inside the window a later start holds it — pins read at
    /// the top of a gated allocation can be stale by the bottom of the very
    /// same allocation.  The gate narrows the staleness to "no OTHER allocation
    /// is in progress"; what closes it is the second pin read
    /// `bind_stable_loopback` takes after leasing its candidate.
    ///
    /// Extending the gate over the persist instead would close the same hole
    /// and is deliberately not done: `AppService::with_app` holds its state
    /// lock across a blocking disk write, so that shape would hold this mutex
    /// across another subsystem's lock — the ordering hazard, and the
    /// held-across-blocking-work hazard, both at once.  Held across service
    /// reads and the bind hop only — never across a call into client or
    /// listener code, which is the rule `AppEmissionQueue` exists to keep.
    port_allocation: Mutex<()>,
    /// Serializes dependency snapshot publication/materialization per lock
    /// digest so concurrent app creates do not run the same install twice.
    dependency_snapshot_locks: Mutex<HashMap<String, Arc<Mutex<()>>>>,
    next_request_id: AtomicU64,
}

impl LocalAppsHostBroker {
    /// Test-convenience constructor (production goes through
    /// [`Self::new_with_physical_memory`], which every test root that needs a
    /// memory figure also uses).
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn new(
        root: PathBuf,
        event_sink: Arc<dyn ClientEventSink>,
        mobile_linux: Option<Arc<dyn MobileLinuxRuntime>>,
        _full_runtime: bool,
        runtime_root: Option<PathBuf>,
    ) -> Arc<Self> {
        Self::new_with_physical_memory(root, event_sink, mobile_linux, false, runtime_root, 0)
    }

    pub(crate) fn new_with_physical_memory(
        root: PathBuf,
        event_sink: Arc<dyn ClientEventSink>,
        mobile_linux: Option<Arc<dyn MobileLinuxRuntime>>,
        _full_runtime: bool,
        runtime_root: Option<PathBuf>,
        physical_memory_bytes: u64,
    ) -> Arc<Self> {
        let broker = Arc::new(Self {
            root,
            event_sink,
            runtime_configuration: RwLock::new(LocalAppsRuntimeConfiguration {
                mobile_linux,
                physical_memory_bytes,
                runtime_root,
            }),
            service: OnceLock::new(),
            llm: OnceLock::new(),
            device: OnceLock::new(),
            recording: Arc::new(Mutex::new(None)),
            media: crate::local_apps_device::MediaCache::default(),
            llm_inflight: Arc::new(std::sync::Mutex::new(std::collections::HashSet::new())),
            mailbox_writes: Mutex::new(()),
            agent_session_writes: Mutex::new(()),
            agent_executor: OnceLock::new(),
            agent_turns: Arc::new(Mutex::new(HashMap::new())),
            pending_profile_proposals: Mutex::new(HashMap::new()),
            background_task_writes: Mutex::new(()),
            background_inflight: Mutex::new(std::collections::HashSet::new()),
            recording_start: Mutex::new(()),
            self_ref: OnceLock::new(),
            pending_capabilities: Mutex::new(HashMap::new()),
            pending_ui: Mutex::new(HashMap::new()),
            session_permissions: Mutex::new(SessionPermissions::default()),
            runtimes: Arc::new(Mutex::new(HashMap::new())),
            port_leases: Arc::new(std::sync::Mutex::new(HashMap::new())),
            port_allocation: Mutex::new(()),
            dependency_snapshot_locks: Mutex::new(HashMap::new()),
            next_request_id: AtomicU64::new(1),
        });
        // The one place an `Arc<Self>` exists; the exit watchers downgrade
        // from it rather than being handed a strong clone.
        let _ = broker.self_ref.set(Arc::downgrade(&broker));
        broker
    }

    /// Weak handle for tasks that outlive the call that spawned them.
    fn weak_self(&self) -> std::sync::Weak<LocalAppsHostBroker> {
        self.self_ref.get().cloned().unwrap_or_default()
    }

    pub(crate) fn attach_service(&self, service: Arc<AppService>) -> Result<(), Arc<AppService>> {
        self.service.set(service)
    }

    pub(crate) fn refresh_runtime_configuration(
        &self,
        mobile_linux: Option<Arc<dyn MobileLinuxRuntime>>,
        runtime_root: Option<PathBuf>,
        physical_memory_bytes: u64,
    ) {
        *self
            .runtime_configuration
            .write()
            .expect("local-app runtime configuration poisoned") = LocalAppsRuntimeConfiguration {
            mobile_linux,
            physical_memory_bytes,
            runtime_root,
        };
    }

    fn mobile_linux(&self) -> Option<Arc<dyn MobileLinuxRuntime>> {
        self.runtime_configuration
            .read()
            .expect("local-app runtime configuration poisoned")
            .mobile_linux
            .clone()
    }

    pub(crate) fn build_lock(&self) -> Arc<Mutex<()>> {
        LOCAL_APP_BUILD_LOCK
            .get_or_init(|| Arc::new(Mutex::new(())))
            .clone()
    }

    pub(crate) async fn has_active_runtimes(&self) -> bool {
        !self.runtimes.lock().await.is_empty()
    }

    pub(crate) fn attach_llm(
        &self,
        llm: Arc<crate::local_apps_profile::SharedLlm>,
    ) -> Result<(), Arc<crate::local_apps_profile::SharedLlm>> {
        self.llm.set(llm)
    }

    pub(crate) fn attach_agent_executor(
        &self,
        executor: Arc<dyn LocalAppsAgentExecutor>,
    ) -> Result<(), Arc<dyn LocalAppsAgentExecutor>> {
        self.agent_executor.set(executor)
    }

    pub(crate) fn attach_device(
        &self,
        device: Arc<crate::local_apps_device::SharedDeviceCapabilities>,
    ) -> Result<(), Arc<crate::local_apps_device::SharedDeviceCapabilities>> {
        self.device.set(device)
    }

    pub(crate) async fn reset_permissions(&self, app_id: &str) -> Result<(), String> {
        self.service()?
            .record(app_id)
            .await
            .map_err(|error| error.to_string())?;
        let layout = self.layout(app_id)?;
        let current = load_permissions(&layout).map_err(|error| error.to_string())?;
        let mut reset = AppPermissions::default();
        reset.grant_epoch = current.grant_epoch.saturating_add(1).max(1);
        save_permissions(&layout, &reset).map_err(|error| error.to_string())?;
        self.session_permissions.lock().await.revoke_app(app_id);
        for outcome in self
            .cancel_background_tasks_for_revoked_schedule(
                app_id,
                "background scheduling permission was revoked",
            )
            .await?
        {
            self.emit_background_task_changed(&outcome).await;
        }
        Ok(())
    }

    fn service(&self) -> Result<Arc<AppService>, String> {
        self.service
            .get()
            .cloned()
            .ok_or_else(|| "local apps service is still starting; retry shortly".into())
    }

    fn request_id(&self, prefix: &str) -> String {
        let id = self.next_request_id.fetch_add(1, Ordering::Relaxed);
        format!("{prefix}-{id}")
    }

    fn layout(&self, app_id: &str) -> Result<AppLayout, String> {
        AppLayout::new(&self.root, app_id).map_err(|error| error.to_string())
    }

    fn app_dependency_marker(layout: &AppLayout) -> PathBuf {
        layout
            .root()
            .join(layout.workspace_rel())
            .join("node_modules/vite/bin/vite.js")
    }

    fn dependency_store_root(&self) -> PathBuf {
        self.root
            .join("dependency-cache")
            .join("pnpm")
            .join("11.22.0")
    }

    fn dependency_snapshot_root(&self, lock_digest: &str) -> PathBuf {
        self.dependency_store_root()
            .join("snapshots")
            .join(lock_digest)
    }

    async fn dependency_snapshot_lock(&self, lock_digest: &str) -> Arc<Mutex<()>> {
        let mut locks = self.dependency_snapshot_locks.lock().await;
        locks
            .entry(lock_digest.to_string())
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone()
    }

    fn workspace_dependencies_ready(layout: &AppLayout) -> Result<bool, String> {
        Self::workspace_dependencies_ready_path(&layout.root().join(layout.workspace_rel()))
    }

    fn dependency_lock_digest(layout: &AppLayout) -> Result<String, String> {
        let workspace = layout.root().join(layout.workspace_rel());
        let path = workspace.join("pnpm-lock.yaml");
        let bytes = std::fs::read(&path)
            .map_err(|error| format!("read pnpm-lock.yaml {}: {error}", path.display()))?;
        Ok(format!("{:x}", Sha256::digest(bytes)))
    }

    fn dependency_inputs_match(workspace: &Path) -> Result<bool, String> {
        for (relative, expected) in
            crate::local_apps_build::VITE_LOCKED_FILES
                .iter()
                .filter(|(relative, _)| {
                    matches!(
                        *relative,
                        "package.json" | "pnpm-lock.yaml" | "pnpm-workspace.yaml"
                    )
                })
        {
            let path = workspace.join(relative);
            let metadata = match std::fs::symlink_metadata(&path) {
                Ok(metadata) => metadata,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
                Err(error) => {
                    return Err(format!(
                        "inspect dependency input {}: {error}",
                        path.display()
                    ))
                }
            };
            if !metadata.is_file() || metadata.file_type().is_symlink() {
                return Ok(false);
            }
            if std::fs::read(&path)
                .map_err(|error| format!("read dependency input {}: {error}", path.display()))?
                != *expected
            {
                return Ok(false);
            }
        }
        Ok(true)
    }

    fn prepare_dependency_staging(layout: &AppLayout) -> Result<PathBuf, String> {
        let workspace = layout.root().join(layout.workspace_rel());
        let state_root = workspace.join(".lingxi-build-state");
        let staging = state_root.join("dependency-staging");
        if let Ok(entries) = std::fs::read_dir(&state_root) {
            for entry in entries.flatten() {
                if entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with("node_modules.previous-")
                {
                    Self::remove_owned_path(&entry.path())?;
                }
            }
        }
        Self::remove_owned_path(&staging)?;
        std::fs::create_dir_all(&staging)
            .map_err(|error| format!("create dependency staging directory: {error}"))?;
        for file in ["package.json", "pnpm-lock.yaml", "pnpm-workspace.yaml"] {
            let source = workspace.join(file);
            let metadata = std::fs::symlink_metadata(&source).map_err(|error| {
                format!("inspect dependency input {}: {error}", source.display())
            })?;
            if !metadata.is_file() || metadata.file_type().is_symlink() {
                return Err(format!(
                    "dependency input must be a regular file: {}",
                    source.display()
                ));
            }
            std::fs::copy(&source, staging.join(file))
                .map_err(|error| format!("stage dependency input {}: {error}", source.display()))?;
        }
        Ok(staging)
    }

    fn remove_owned_path(path: &Path) -> Result<(), String> {
        let metadata = match std::fs::symlink_metadata(path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(format!("inspect owned path {}: {error}", path.display())),
        };
        if metadata.is_dir() && !metadata.file_type().is_symlink() {
            std::fs::remove_dir_all(path)
                .map_err(|error| format!("remove owned directory {}: {error}", path.display()))
        } else {
            std::fs::remove_file(path)
                .map_err(|error| format!("remove owned file {}: {error}", path.display()))
        }
    }

    fn dependency_snapshot_is_ready(
        snapshot_root: &Path,
        lock_digest: &str,
    ) -> Result<bool, String> {
        let root_metadata = match std::fs::symlink_metadata(snapshot_root) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
            Err(error) => {
                return Err(format!(
                    "inspect dependency snapshot {}: {error}",
                    snapshot_root.display()
                ))
            }
        };
        if !root_metadata.is_dir() || root_metadata.file_type().is_symlink() {
            return Ok(false);
        }
        let marker = snapshot_root.join(DEPENDENCY_SNAPSHOT_READY_FILE);
        let marker_metadata = match std::fs::symlink_metadata(&marker) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
            Err(error) => {
                return Err(format!(
                    "inspect dependency snapshot marker {}: {error}",
                    marker.display()
                ))
            }
        };
        if !marker_metadata.is_file() || marker_metadata.file_type().is_symlink() {
            return Ok(false);
        }
        let marker_contents = std::fs::read_to_string(&marker)
            .map_err(|error| format!("read dependency snapshot marker: {error}"))?;
        let mut marker_lines = marker_contents.lines();
        let expected_version = DEPENDENCY_SNAPSHOT_VERSION.to_string();
        if marker_lines.next() != Some(expected_version.as_str())
            || marker_lines.next() != Some(lock_digest)
            || marker_lines.next() != Some(PNPM_TOOLCHAIN_KEY)
        {
            return Ok(false);
        }
        let Some(expected_tree_digest) = marker_lines.next() else {
            return Ok(false);
        };
        if marker_lines.next().is_some() || expected_tree_digest.is_empty() {
            return Ok(false);
        }
        let node_modules = snapshot_root.join("node_modules");
        let node_modules_metadata = match std::fs::symlink_metadata(&node_modules) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
            Err(error) => return Err(format!("inspect dependency snapshot node_modules: {error}")),
        };
        if !node_modules_metadata.is_dir() || node_modules_metadata.file_type().is_symlink() {
            return Ok(false);
        }
        let digest_cached = DEPENDENCY_SNAPSHOT_DIGESTS
            .get_or_init(|| std::sync::Mutex::new(HashMap::new()))
            .lock()
            .expect("dependency snapshot digest cache poisoned")
            .get(snapshot_root)
            .is_some_and(|digest| digest == expected_tree_digest);
        if !digest_cached {
            validate_dependency_tree(&node_modules)?;
            let actual_tree_digest = dependency_tree_digest(&node_modules)?;
            if actual_tree_digest != expected_tree_digest {
                return Ok(false);
            }
            DEPENDENCY_SNAPSHOT_DIGESTS
                .get_or_init(|| std::sync::Mutex::new(HashMap::new()))
                .lock()
                .expect("dependency snapshot digest cache poisoned")
                .insert(snapshot_root.to_path_buf(), actual_tree_digest);
        }
        let vite = node_modules.join("vite/bin/vite.js");
        let vite_metadata = match std::fs::symlink_metadata(&vite) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
            Err(error) => {
                return Err(format!(
                    "inspect dependency snapshot Vite executable: {error}"
                ))
            }
        };
        Ok(vite_metadata.is_file() && !vite_metadata.file_type().is_symlink())
    }

    fn workspace_dependencies_match_snapshot(
        workspace: &Path,
        snapshot_root: &Path,
        lock_digest: &str,
    ) -> Result<bool, String> {
        if !Self::workspace_dependencies_ready_path(workspace)?
            || !Self::dependency_snapshot_is_ready(snapshot_root, lock_digest)?
        {
            return Ok(false);
        }
        let marker = snapshot_root.join(DEPENDENCY_SNAPSHOT_READY_FILE);
        let expected_tree_digest = dependency_tree_digest_from_marker(&marker)?;
        let Some(expected_tree_digest) = expected_tree_digest else {
            return Ok(false);
        };
        let expected_attestation = dependency_attestation(lock_digest, &expected_tree_digest);
        let attestation = workspace.join(WORKSPACE_DEPENDENCY_ATTESTATION_FILE);
        if std::fs::read_to_string(&attestation).ok().as_deref()
            == Some(expected_attestation.as_str())
        {
            return Ok(true);
        }
        let workspace_node_modules = workspace.join("node_modules");
        validate_dependency_tree(&workspace_node_modules)?;
        if dependency_tree_digest(&workspace_node_modules)? != expected_tree_digest {
            return Ok(false);
        }
        crate::local_apps_build::write_file(
            workspace,
            WORKSPACE_DEPENDENCY_ATTESTATION_FILE,
            expected_attestation.as_bytes(),
            true,
        )
        .map_err(|error| error.to_string())?;
        Ok(true)
    }

    fn workspace_dependencies_ready_path(workspace: &Path) -> Result<bool, String> {
        let vite = workspace.join("node_modules/vite/bin/vite.js");
        match std::fs::symlink_metadata(&vite) {
            Ok(metadata) if metadata.is_file() && !metadata.file_type().is_symlink() => Ok(true),
            Ok(metadata) if metadata.file_type().is_symlink() => Err(format!(
                "workspace dependency marker is invalid: {} must be a regular file",
                vite.display()
            )),
            Ok(_) => Ok(false),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(error) => Err(format!(
                "inspect workspace dependency marker {}: {error}",
                vite.display()
            )),
        }
    }

    fn publish_dependency_snapshot(
        source_node_modules: &Path,
        snapshot_root: &Path,
        lock_digest: &str,
    ) -> Result<(), String> {
        if Self::dependency_snapshot_is_ready(snapshot_root, lock_digest)? {
            return Ok(());
        }
        match std::fs::symlink_metadata(snapshot_root) {
            Ok(_) => Self::remove_owned_path(snapshot_root)?,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(format!(
                    "inspect existing dependency snapshot {}: {error}",
                    snapshot_root.display()
                ))
            }
        }
        let parent = snapshot_root.parent().ok_or_else(|| {
            format!(
                "dependency snapshot has no parent: {}",
                snapshot_root.display()
            )
        })?;
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("create dependency snapshot parent: {error}"))?;
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or_default();
        let staging_root = parent.join(format!(".{lock_digest}.staging-{stamp}"));
        Self::remove_owned_path(&staging_root)?;
        std::fs::create_dir_all(&staging_root)
            .map_err(|error| format!("create dependency snapshot staging: {error}"))?;
        if let Err(error) =
            clone_or_copy_tree(source_node_modules, &staging_root.join("node_modules"))
                .and_then(|_| {
                    let tree_digest = dependency_tree_digest(&staging_root.join("node_modules"))
                        .map_err(|error| io::Error::new(io::ErrorKind::Other, error))?;
                    let marker = staging_root.join(DEPENDENCY_SNAPSHOT_READY_FILE);
                    let expected = dependency_attestation(lock_digest, &tree_digest);
                    std::fs::write(marker, expected)
                        .map_err(|error| io::Error::new(io::ErrorKind::Other, error))
                })
                .and_then(|_| {
                    validate_dependency_tree(&staging_root.join("node_modules"))
                        .map_err(|error| io::Error::new(io::ErrorKind::Other, error))
                })
                .and_then(|_| make_dependency_files_read_only(&staging_root.join("node_modules")))
        {
            let _ = Self::remove_owned_path(&staging_root);
            return Err(format!("prepare dependency snapshot: {error}"));
        }
        if let Err(error) = std::fs::rename(&staging_root, snapshot_root) {
            let _ = Self::remove_owned_path(&staging_root);
            if snapshot_root.exists()
                && Self::dependency_snapshot_is_ready(snapshot_root, lock_digest)?
            {
                return Ok(());
            }
            return Err(format!("publish dependency snapshot: {error}"));
        }
        Ok(())
    }

    fn materialize_dependency_snapshot(
        snapshot_root: &Path,
        staging_root: &Path,
    ) -> Result<(), String> {
        let destination = staging_root.join("node_modules");
        Self::remove_owned_path(&destination)?;
        clone_or_copy_tree(&snapshot_root.join("node_modules"), &destination)
            .map_err(|error| format!("materialize dependency snapshot: {error}"))
    }

    fn promote_dependency_tree(workspace: &Path, staging: &Path) -> Result<(), String> {
        let staged_node_modules = staging.join("node_modules");
        let marker = staged_node_modules.join("vite/bin/vite.js");
        let marker_metadata = std::fs::symlink_metadata(&marker)
            .map_err(|error| format!("inspect staged Vite executable: {error}"))?;
        if !marker_metadata.is_file() || marker_metadata.file_type().is_symlink() {
            return Err(format!(
                "staged Vite executable is not a regular file: {}",
                marker.display()
            ));
        }
        let current = workspace.join("node_modules");
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or_default();
        let backup = workspace
            .join(".lingxi-build-state")
            .join(format!("node_modules.previous-{stamp}"));
        let had_current = std::fs::symlink_metadata(&current).is_ok();
        if had_current {
            let metadata = std::fs::symlink_metadata(&current)
                .map_err(|error| format!("inspect current dependencies: {error}"))?;
            if metadata.file_type().is_symlink() {
                return Err("workspace node_modules must not be a symlink".into());
            }
            std::fs::rename(&current, &backup)
                .map_err(|error| format!("stage previous dependencies: {error}"))?;
        }
        if let Err(error) = std::fs::rename(&staged_node_modules, &current) {
            if had_current {
                let _ = std::fs::rename(&backup, &current);
            }
            return Err(format!("promote staged dependencies: {error}"));
        }
        if had_current {
            Self::remove_owned_path(&backup)?;
        }
        Self::remove_owned_path(staging)
    }

    async fn install_dependencies_value(&self, input: Value) -> Result<Value, String> {
        let app_id = required_string(&input, "app_id")?.to_string();
        let wait = input.get("wait").and_then(Value::as_bool).unwrap_or(false);
        let dependency = self.ensure_dependency_install(&app_id, wait).await?;
        Ok(json!({
            "ok": dependency.state == AppDependencyState::Ready,
            "app_id": app_id,
            "dependencies": dependency,
        }))
    }

    pub(crate) async fn ensure_dependency_install(
        &self,
        app_id: &str,
        wait: bool,
    ) -> Result<local_apps::AppDependencyRecord, String> {
        let service = self.service()?;
        service
            .record(app_id)
            .await
            .map_err(|error| error.to_string())?;
        let layout = self.layout(app_id)?;
        let workspace = layout.root().join(layout.workspace_rel());
        if !Self::dependency_inputs_match(&workspace)? {
            let target = crate::local_apps_build::detect_build_target(&layout)
                .map_err(|error| error.to_string())?;
            crate::local_apps_build::restore_host_managed_files(&workspace, target)
                .map_err(|error| error.to_string())?;
        }
        let dependency = service
            .dependency_record(app_id)
            .await
            .map_err(|error| error.to_string())?;
        let lock_digest = Self::dependency_lock_digest(&layout)?;
        if dependency.state == AppDependencyState::Ready
            && dependency.lockfile_sha256.as_deref() == Some(lock_digest.as_str())
            && dependency.toolchain_key.as_deref() == Some(PNPM_TOOLCHAIN_KEY)
            && Self::workspace_dependencies_match_snapshot(
                &workspace,
                &self.dependency_snapshot_root(&lock_digest),
                &lock_digest,
            )?
        {
            return Ok(dependency);
        }
        if dependency.state == AppDependencyState::Ready {
            service
                .queue_dependency_install(app_id)
                .await
                .map_err(|error| error.to_string())?;
        }
        let started = match service.start_dependency_install(app_id).await {
            Ok(_) => true,
            Err(local_apps::AppError::InvalidRequest(_)) => false,
            Err(error) => return Err(error.to_string()),
        };
        if started {
            let weak = self.weak_self();
            let app_id = app_id.to_string();
            tokio::spawn(async move {
                let Some(host) = weak.upgrade() else {
                    return;
                };
                host.run_dependency_install(app_id).await;
            });
        }
        if wait {
            return self.wait_for_dependency_install(app_id).await;
        }
        service
            .dependency_record(app_id)
            .await
            .map_err(|error| error.to_string())
    }

    pub(crate) async fn wait_for_dependency_install(
        &self,
        app_id: &str,
    ) -> Result<local_apps::AppDependencyRecord, String> {
        let service = self.service()?;
        timeout(DEPENDENCY_INSTALL_TIMEOUT, async {
            loop {
                let dependency = service
                    .dependency_record(app_id)
                    .await
                    .map_err(|error| error.to_string())?;
                match dependency.state {
                    AppDependencyState::Installing => sleep(DEPENDENCY_INSTALL_POLL_INTERVAL).await,
                    _ => return Ok(dependency),
                }
            }
        })
        .await
        .map_err(|_| {
            format!(
                "workspace dependency installation is still running after {} seconds",
                DEPENDENCY_INSTALL_TIMEOUT.as_secs()
            )
        })?
    }

    async fn finalize_dependency_install(
        &self,
        service: &Arc<AppService>,
        layout: &AppLayout,
        app_id: &str,
        dependency_staging: &Path,
        expected_lock_digest: &str,
    ) -> Result<(), String> {
        let workspace = layout.root().join(layout.workspace_rel());
        // The lockfile is host-managed, but re-check it immediately before
        // promotion so a concurrent restore/edit cannot publish a tree built
        // for an older digest into the live workspace.
        let before_promotion = Self::dependency_lock_digest(layout)?;
        if before_promotion != expected_lock_digest {
            return Err("dependency lock changed before promotion".into());
        }
        Self::promote_dependency_tree(&workspace, dependency_staging)?;
        if !Self::workspace_dependencies_ready(layout)? {
            return Err(format!(
                "dependency install finished but {} was not produced",
                Self::app_dependency_marker(layout).display()
            ));
        }
        let actual_lock_digest = Self::dependency_lock_digest(layout)?;
        if actual_lock_digest != expected_lock_digest {
            return Err("dependency lock changed while installation was running".into());
        }
        let snapshot_root = self.dependency_snapshot_root(expected_lock_digest);
        let snapshot_marker = snapshot_root.join(DEPENDENCY_SNAPSHOT_READY_FILE);
        let tree_digest = dependency_tree_digest_from_marker(&snapshot_marker)?
            .ok_or_else(|| "dependency snapshot marker is malformed".to_string())?;
        let attestation = dependency_attestation(expected_lock_digest, &tree_digest);
        crate::local_apps_build::write_file(
            &workspace,
            WORKSPACE_DEPENDENCY_ATTESTATION_FILE,
            attestation.as_bytes(),
            true,
        )
        .map_err(|error| error.to_string())?;
        service
            .complete_dependency_install_with_metadata(
                app_id,
                Some(actual_lock_digest),
                Some(PNPM_TOOLCHAIN_KEY.to_string()),
            )
            .await
            .map_err(|error| error.to_string())?;
        Ok(())
    }

    async fn run_dependency_install(&self, app_id: String) {
        let service = match self.service() {
            Ok(service) => service,
            Err(error) => {
                tracing::warn!(app_id = %app_id, error = %error, "dependency install lost service");
                return;
            }
        };
        let layout = match self.layout(&app_id) {
            Ok(layout) => layout,
            Err(error) => {
                let _ = service
                    .fail_dependency_install(&app_id, error.clone())
                    .await;
                tracing::warn!(app_id = %app_id, error = %error, "dependency install lost layout");
                return;
            }
        };
        // Serialize dependency mutation with build, checkpoint restore, and
        // physical deletion. The lock is intentionally held across the
        // isolated command so a delete cannot remove the workspace while pnpm
        // is still writing its app-local node_modules tree.
        let _build_lock = match local_apps::storage::lock_app_build(&self.root, &app_id) {
            Ok(lock) => lock,
            Err(error) => {
                let message = error.to_string();
                let _ = service
                    .fail_dependency_install(&app_id, message.clone())
                    .await;
                tracing::warn!(app_id = %app_id, error = %message, "dependency install could not lock app");
                return;
            }
        };
        let workspace = layout.root().join(layout.workspace_rel());
        let lock_digest = match Self::dependency_lock_digest(&layout) {
            Ok(digest) => digest,
            Err(error) => {
                let _ = service
                    .fail_dependency_install(&app_id, error.clone())
                    .await;
                tracing::warn!(app_id = %app_id, error = %error, "dependency lock digest failed");
                return;
            }
        };
        let snapshot_root = self.dependency_snapshot_root(&lock_digest);
        let snapshot_lock = self.dependency_snapshot_lock(&lock_digest).await;
        let _snapshot_guard = snapshot_lock.lock().await;
        let dependency_staging = match Self::prepare_dependency_staging(&layout) {
            Ok(path) => path,
            Err(error) => {
                let _ = service
                    .fail_dependency_install(&app_id, error.clone())
                    .await;
                tracing::warn!(app_id = %app_id, error = %error, "dependency install staging failed");
                return;
            }
        };
        let snapshot_ready = match Self::dependency_snapshot_is_ready(&snapshot_root, &lock_digest)
        {
            Ok(ready) => ready,
            Err(error) => {
                let _ = Self::remove_owned_path(&dependency_staging);
                let _ = service
                    .fail_dependency_install(&app_id, error.clone())
                    .await;
                tracing::warn!(app_id = %app_id, error = %error, "dependency snapshot validation failed");
                return;
            }
        };
        if snapshot_ready {
            if let Err(error) =
                Self::materialize_dependency_snapshot(&snapshot_root, &dependency_staging)
            {
                let _ = Self::remove_owned_path(&dependency_staging);
                let _ = service
                    .fail_dependency_install(&app_id, error.clone())
                    .await;
                tracing::warn!(app_id = %app_id, error = %error, "dependency snapshot promotion failed");
                return;
            }
            if let Err(error) = self
                .finalize_dependency_install(
                    &service,
                    &layout,
                    &app_id,
                    &dependency_staging,
                    &lock_digest,
                )
                .await
            {
                let _ = service
                    .fail_dependency_install(&app_id, error.clone())
                    .await;
                tracing::warn!(app_id = %app_id, error = %error, "dependency snapshot verification failed");
            }
            return;
        }
        let Some(runtime) = self.mobile_linux() else {
            let error = "the mobile Node runtime is unavailable for dependency installation";
            let _ = service
                .fail_dependency_install(&app_id, error.to_string())
                .await;
            tracing::warn!(app_id = %app_id, error, "dependency install has no mobile runtime");
            return;
        };
        let build_mount = MountSpec {
            host_path: workspace.clone(),
            guest_path: guest_paths::local_app_build_project(&app_id, "store"),
            read_only: false,
            purpose: MountPurpose::LocalAppBuild,
        };
        let dependency_store = self.dependency_store_root();
        if let Err(error) = std::fs::create_dir_all(&dependency_store) {
            let message = format!("create pnpm dependency store: {error}");
            let _ = Self::remove_owned_path(&dependency_staging);
            let _ = service
                .fail_dependency_install(&app_id, message.clone())
                .await;
            tracing::warn!(app_id = %app_id, error = %message, "dependency install could not create store");
            return;
        }
        let store_mount = MountSpec {
            host_path: dependency_store,
            guest_path: guest_paths::LOCAL_APP_DEPENDENCY_STORE.to_string(),
            read_only: false,
            purpose: MountPurpose::Shared,
        };
        let project_guest_path = build_mount.guest_path.clone();
        let dependency_staging_guest_path =
            format!("{project_guest_path}/.lingxi-build-state/dependency-staging");
        let build_state_root = format!("{project_guest_path}/.lingxi-build-state");
        let mut env = std::collections::BTreeMap::new();
        env.insert("CI".into(), "1".into());
        env.insert("HOME".into(), format!("{build_state_root}/home"));
        env.insert("TMPDIR".into(), format!("{build_state_root}/tmp"));
        env.insert("TMP".into(), format!("{build_state_root}/tmp"));
        env.insert("TEMP".into(), format!("{build_state_root}/tmp"));
        env.insert(
            "XDG_CACHE_HOME".into(),
            format!("{build_state_root}/xdg-cache"),
        );
        env.insert(
            "XDG_CONFIG_HOME".into(),
            format!("{build_state_root}/xdg-config"),
        );
        env.insert(
            "XDG_DATA_HOME".into(),
            format!("{build_state_root}/xdg-data"),
        );
        env.insert("PNPM_HOME".into(), format!("{build_state_root}/pnpm-home"));
        env.insert(
            "COREPACK_HOME".into(),
            format!("{build_state_root}/corepack"),
        );
        let pnpm_store_root = guest_paths::LOCAL_APP_DEPENDENCY_STORE.to_string();
        let memory_mb =
            crate::local_apps_build::build_memory_budget_mb(self.physical_memory_bytes());
        let resource_limits = ResourceLimits {
            max_memory_mb: Some(memory_mb),
            ..ResourceLimits::default()
        };
        let request = LinuxCommandRequest {
            command: "/usr/bin/pnpm".into(),
            args: vec![
                "install".into(),
                "--frozen-lockfile".into(),
                "--ignore-scripts".into(),
                "--no-runtime".into(),
                "--prefer-offline".into(),
                "--store-dir".into(),
                pnpm_store_root,
                "--reporter=append-only".into(),
            ],
            cwd: Some(dependency_staging_guest_path),
            env,
            stdin: None,
            timeout_ms: Some(DEPENDENCY_INSTALL_TIMEOUT.as_millis() as u64),
            network: NetworkPolicy::Allowed,
            resource_limits,
            mounts: vec![build_mount, store_mount],
        };
        let install = runtime.run_isolated(request).await;
        let outcome = match install {
            Ok(result) => {
                if let Err(error) = result
                    .enforcement
                    .ensure_for(NetworkPolicy::Allowed, resource_limits)
                {
                    Err(error.to_string())
                } else if result.timed_out || result.cancelled || result.exit_code != 0 {
                    let detail = if !result.stderr.trim().is_empty() {
                        result.stderr
                    } else {
                        result.stdout
                    };
                    Err(format!(
                        "pnpm install failed (exit_code={}, timed_out={}, cancelled={}): {}",
                        result.exit_code,
                        result.timed_out,
                        result.cancelled,
                        detail.chars().take(8_000).collect::<String>()
                    ))
                } else {
                    Ok(())
                }
            }
            Err(error) => Err(format!("dependency install worker failed: {error}")),
        };
        match outcome {
            Ok(()) => {
                if let Err(error) = Self::publish_dependency_snapshot(
                    &dependency_staging.join("node_modules"),
                    &snapshot_root,
                    &lock_digest,
                ) {
                    let _ = Self::remove_owned_path(&dependency_staging);
                    let _ = service
                        .fail_dependency_install(&app_id, error.clone())
                        .await;
                    tracing::warn!(app_id = %app_id, error = %error, "dependency snapshot publication failed");
                    return;
                }
                if let Err(error) = self
                    .finalize_dependency_install(
                        &service,
                        &layout,
                        &app_id,
                        &dependency_staging,
                        &lock_digest,
                    )
                    .await
                {
                    let _ = service
                        .fail_dependency_install(&app_id, error.clone())
                        .await;
                    tracing::warn!(app_id = %app_id, error = %error, "dependency install verification failed");
                }
            }
            Err(error) => {
                let _ = Self::remove_owned_path(&dependency_staging);
                let _ = service
                    .fail_dependency_install(&app_id, error.clone())
                    .await;
                tracing::warn!(app_id = %app_id, error = %error, "dependency install failed");
            }
        }
    }

    pub(crate) fn physical_memory_bytes(&self) -> u64 {
        self.runtime_configuration
            .read()
            .expect("local-app runtime configuration poisoned")
            .physical_memory_bytes
    }

    pub(crate) fn configured_runtime_root(&self) -> Result<PathBuf, String> {
        self.runtime_configuration
            .read()
            .expect("local-app runtime configuration poisoned")
            .runtime_root
            .clone()
            .ok_or_else(|| {
                "local-app runtime root is not configured; stage the Vite local-app-runtime first"
                    .to_string()
            })
    }

    fn runtime_seed_ready(root: &Path) -> Result<bool, String> {
        let vite = root.join("node_modules/vite/bin/vite.js");
        let vite_ready = match std::fs::symlink_metadata(&vite) {
            Ok(metadata) if metadata.is_file() && !metadata.file_type().is_symlink() => Ok(true),
            Ok(metadata) if metadata.file_type().is_symlink() => Err(format!(
                "verified local-app runtime seed is invalid: {} must be a regular file",
                vite.display()
            )),
            Ok(_) => Ok(false),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(error) => Err(format!(
                "inspect verified local-app runtime seed {}: {error}",
                vite.display()
            )),
        }?;
        if !vite_ready {
            return Ok(false);
        }
        if !Self::runtime_seed_root_is_digest_addressed(root) {
            return Ok(true);
        }
        Self::runtime_seed_ready_marker(root)
    }

    fn runtime_seed_root_is_digest_addressed(root: &Path) -> bool {
        root.file_name()
            .and_then(|leaf| leaf.to_str())
            .is_some_and(|leaf| {
                leaf.len() == 64
                    && leaf
                        .bytes()
                        .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
            })
    }

    fn runtime_seed_ready_marker(root: &Path) -> Result<bool, String> {
        let Some(digest) = root.file_name().and_then(|leaf| leaf.to_str()) else {
            return Err(format!(
                "verified local-app runtime root {} has no digest leaf",
                root.display()
            ));
        };
        let marker = Self::runtime_seed_marker_path(root, ".ready")?;
        match std::fs::symlink_metadata(&marker) {
            Ok(metadata) if metadata.is_file() && !metadata.file_type().is_symlink() => {}
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err(format!(
                    "verified local-app runtime ready marker is invalid: {} must not be a symlink",
                    marker.display()
                ))
            }
            Ok(_) => {
                return Err(format!(
                    "verified local-app runtime ready marker is invalid: {} must be a regular file",
                    marker.display()
                ))
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
            Err(error) => {
                return Err(format!(
                    "inspect verified local-app runtime ready marker {}: {error}",
                    marker.display()
                ))
            }
        }
        let mut marker_file = std::fs::File::open(&marker).map_err(|error| {
            format!(
                "open verified local-app runtime ready marker {}: {error}",
                marker.display()
            )
        })?;
        let mut content = Vec::with_capacity(65);
        marker_file
            .by_ref()
            .take(65)
            .read_to_end(&mut content)
            .map_err(|error| {
                format!(
                    "read verified local-app runtime ready marker {}: {error}",
                    marker.display()
                )
            })?;
        if content.len() != digest.len() || content.as_slice() != digest.as_bytes() {
            return Err(format!(
                "verified local-app runtime ready marker is invalid: {} must contain exactly its digest leaf",
                marker.display()
            ));
        }
        Ok(true)
    }

    fn runtime_seed_marker_path(root: &Path, suffix: &str) -> Result<PathBuf, String> {
        let digest = root.file_name().ok_or_else(|| {
            format!(
                "verified local-app runtime root {} has no digest leaf",
                root.display()
            )
        })?;
        let parent = root.parent().ok_or_else(|| {
            format!(
                "verified local-app runtime root {} has no parent directory",
                root.display()
            )
        })?;
        let mut marker_name = std::ffi::OsString::from(".");
        marker_name.push(digest);
        marker_name.push(suffix);
        Ok(parent.join(marker_name))
    }

    fn runtime_seed_failure_marker(root: &Path) -> Result<Option<PathBuf>, String> {
        let marker = Self::runtime_seed_marker_path(root, ".failed")?;
        match std::fs::symlink_metadata(&marker) {
            Ok(metadata) if metadata.is_file() && !metadata.file_type().is_symlink() => {
                Ok(Some(marker))
            }
            Ok(metadata) if metadata.file_type().is_symlink() => Err(format!(
                "verified local-app runtime failure marker is invalid: {} must not be a symlink",
                marker.display()
            )),
            Ok(_) => Err(format!(
                "verified local-app runtime failure marker is invalid: {} must be a regular file",
                marker.display()
            )),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(format!(
                "inspect verified local-app runtime failure marker {}: {error}",
                marker.display()
            )),
        }
    }

    fn read_runtime_seed_failure(root: &Path) -> Result<Option<String>, String> {
        let Some(marker) = Self::runtime_seed_failure_marker(root)? else {
            return Ok(None);
        };
        let reason = std::fs::read_to_string(&marker).map_err(|error| {
            format!(
                "read verified local-app runtime failure marker {}: {error}",
                marker.display()
            )
        })?;
        let detail = reason.trim();
        if detail.is_empty() {
            Ok(Some(format!(
                "verified local-app runtime seed staging failed; see {}",
                marker.display()
            )))
        } else {
            Ok(Some(format!(
                "verified local-app runtime seed staging failed: {detail}"
            )))
        }
    }

    pub(crate) async fn await_fixed_runtime_root(
        &self,
        timeout_duration: Duration,
    ) -> Result<PathBuf, String> {
        let root = self.configured_runtime_root()?;
        let start = tokio::time::Instant::now();
        loop {
            match Self::runtime_seed_ready(&root)? {
                true => return Ok(root.clone()),
                false => {
                    if let Some(failure) = Self::read_runtime_seed_failure(&root)? {
                        return Err(failure);
                    }
                }
            }
            if start.elapsed() >= timeout_duration {
                return Err(format!(
                    "local-app runtime root is configured at {}, but the verified runtime seed is not ready yet; waited {} ms for node_modules/vite/bin/vite.js",
                    root.display(),
                    timeout_duration.as_millis()
                ));
            }
            sleep(RUNTIME_SEED_POLL_INTERVAL).await;
        }
    }

    pub(crate) async fn resolve_capability(
        &self,
        request_id: &str,
        decision: AppAuthorizationDecisionDto,
    ) -> bool {
        self.pending_capabilities
            .lock()
            .await
            .remove(request_id)
            .is_some_and(|sender| sender.send(decision).is_ok())
    }

    pub(crate) async fn resolve_ui(
        &self,
        request_id: &str,
        decision: AppAuthorizationDecisionDto,
        result_json: Option<String>,
        error: Option<String>,
    ) -> bool {
        self.pending_ui
            .lock()
            .await
            .remove(request_id)
            .is_some_and(|sender| {
                sender
                    .send(UiResolution {
                        decision,
                        result_json,
                        error,
                    })
                    .is_ok()
            })
    }

    async fn request_capability(
        &self,
        app_id: &str,
        capability: AppCapabilityKindDto,
        domain: Option<String>,
        reason: &str,
    ) -> Result<AppAuthorizationDecisionDto, String> {
        let request_id = self.request_id("app-capability");
        let (sender, receiver) = oneshot::channel();
        self.pending_capabilities
            .lock()
            .await
            .insert(request_id.clone(), sender);
        self.event_sink
            .emit(ClientEvent::AppEvent {
                event: AppEventDto::AppCapabilityRequested {
                    request: AppCapabilityRequestDto {
                        request_id: request_id.clone(),
                        app_id: app_id.to_string(),
                        capability,
                        domain,
                        reason: reason.to_string(),
                    },
                },
            })
            .await;
        match timeout(APPROVAL_TIMEOUT, receiver).await {
            Ok(Ok(decision)) => Ok(decision),
            Ok(Err(_)) => Err("capability request was cancelled".into()),
            Err(_) => {
                self.pending_capabilities.lock().await.remove(&request_id);
                Err("capability request timed out".into())
            }
        }
    }

    async fn authorize_capability(
        &self,
        app_id: &str,
        capability: AppCapability,
        wire_capability: AppCapabilityKindDto,
        reason: &str,
    ) -> Result<(), String> {
        let layout = self.layout(app_id)?;
        let persisted = load_permissions(&layout).map_err(|error| error.to_string())?;
        if persisted.allows(capability)
            || self
                .session_permissions
                .lock()
                .await
                .allows(app_id, capability)
        {
            return Ok(());
        }
        let decision = self
            .request_capability(app_id, wire_capability, None, reason)
            .await?;
        match raise_decision(decision) {
            PermissionDecision::Deny => Err(Self::DENIED_CAPABILITY_MESSAGE.into()),
            PermissionDecision::AllowOnce => Ok(()),
            PermissionDecision::AllowSession => {
                self.session_permissions
                    .lock()
                    .await
                    .grant(app_id, capability);
                Ok(())
            }
            PermissionDecision::AlwaysAllow => {
                let mut permissions = persisted;
                permissions.grant(capability);
                save_permissions(&layout, &permissions).map_err(|error| error.to_string())
            }
        }
    }

    /// Message [`Self::authorize_capability`] returns for an explicit user
    /// denial. [`Self::authorize_declared_capability`] compares against it to
    /// attach the `permission_denied` code — same-file constant, never prose
    /// matching.
    const DENIED_CAPABILITY_MESSAGE: &'static str = "user denied the local app capability";

    /// Manifest-only gate for read-only capabilities that do not need a
    /// separate user prompt. The declaration is still required so a generated
    /// app cannot silently discover host state it did not request.
    pub(super) fn ensure_declared_capability(
        &self,
        app_id: &str,
        capability: AppCapability,
    ) -> Result<(), BridgeFailure> {
        let layout = self.layout(app_id)?;
        let manifest = load_manifest(&layout).map_err(|error| error.to_string())?;
        if !manifest.capabilities.contains(&capability) {
            return Err(BridgeFailure::coded(
                "capability_not_declared",
                format!("capability {capability:?} is not declared in the app manifest"),
            ));
        }
        Ok(())
    }

    /// Declared-then-prompt gate shared by every plan-declared capability
    /// (device, llm, agent_notify): an app may only ever be ASKED about a
    /// capability its confirmed plan declared. An undeclared capability fails
    /// typed (`capability_not_declared`) WITHOUT raising a prompt — the same
    /// manifest-first contract [`Self::authorize_domain`] applies to network
    /// hosts. Declared capabilities then ride the existing persisted →
    /// session → prompt ladder unchanged.
    async fn authorize_declared_capability(
        &self,
        app_id: &str,
        capability: AppCapability,
        wire_capability: AppCapabilityKindDto,
        reason: &str,
    ) -> Result<(), BridgeFailure> {
        let layout = self.layout(app_id)?;
        let manifest = load_manifest(&layout).map_err(|error| error.to_string())?;
        if !manifest.capabilities.contains(&capability) {
            return Err(BridgeFailure::coded(
                "capability_not_declared",
                format!("capability {capability:?} is not declared in the app manifest"),
            ));
        }
        self.authorize_capability(app_id, capability, wire_capability, reason)
            .await
            .map_err(|message| {
                if message == Self::DENIED_CAPABILITY_MESSAGE {
                    BridgeFailure::coded("permission_denied", message)
                } else {
                    BridgeFailure::from(message)
                }
            })
    }

    async fn authorize_domain(&self, app_id: &str, domain: &str) -> Result<(), String> {
        let layout = self.layout(app_id)?;
        let manifest = load_manifest(&layout).map_err(|error| error.to_string())?;
        if !manifest
            .allowed_domains
            .iter()
            .any(|allowed| allowed == domain)
        {
            return Err(format!(
                "HTTPS domain {domain:?} is not declared in the app manifest"
            ));
        }
        let persisted = load_permissions(&layout).map_err(|error| error.to_string())?;
        if persisted.allows_domain(domain)
            || self
                .session_permissions
                .lock()
                .await
                .allows_domain(app_id, domain)
        {
            return Ok(());
        }
        let decision = self
            .request_capability(
                app_id,
                AppCapabilityKindDto::NetworkDomain,
                Some(domain.to_string()),
                "The local app requested first-time access to this HTTPS domain.",
            )
            .await?;
        match raise_decision(decision) {
            PermissionDecision::Deny => Err("user denied access to the network domain".into()),
            PermissionDecision::AllowOnce => Ok(()),
            PermissionDecision::AllowSession => self
                .session_permissions
                .lock()
                .await
                .grant_domain(app_id, domain)
                .map_err(|error| error.to_string()),
            PermissionDecision::AlwaysAllow => {
                let mut permissions = persisted;
                permissions
                    .grant_domain(domain)
                    .map_err(|error| error.to_string())?;
                save_permissions(&layout, &permissions).map_err(|error| error.to_string())
            }
        }
    }

    pub(crate) async fn approve_destructive_manifest_migration(
        &self,
        app_id: &str,
        preview: &DataMigrationPreview,
    ) -> Result<(), String> {
        if !preview.destructive {
            return Ok(());
        }
        let decision = self
            .request_capability(
                app_id,
                AppCapabilityKindDto::DataMutation,
                None,
                &manifest_migration_reason(preview),
            )
            .await?;
        if matches!(raise_decision(decision), PermissionDecision::Deny) {
            return Err("user denied destructive manifest migration".into());
        }
        Ok(())
    }

    async fn query_data_value(&self, input: Value) -> Result<Value, String> {
        let app_id = required_string(&input, "app_id")?.to_string();
        self.service()?
            .record(&app_id)
            .await
            .map_err(|e| e.to_string())?;
        let layout = self.layout(&app_id)?;
        let manifest = load_manifest(&layout).map_err(|error| error.to_string())?;
        let query = normalize_query(&input)?;
        tokio::task::spawn_blocking(move || AppDataStore::open(layout)?.query(&manifest, &query))
            .await
            .map_err(|error| format!("data query worker failed: {error}"))?
            .map(|page| json!(page))
            .map_err(|error| error.to_string())
    }

    async fn mutate_data_value(
        &self,
        input: Value,
        require_approval: bool,
    ) -> Result<Value, String> {
        let app_id = required_string(&input, "app_id")?.to_string();
        self.service()?
            .record(&app_id)
            .await
            .map_err(|e| e.to_string())?;
        if require_approval {
            self.authorize_capability(
                &app_id,
                AppCapability::DataMutation,
                AppCapabilityKindDto::DataMutation,
                "The agent requested permission to modify this app's persisted data.",
            )
            .await?;
        }
        let layout = self.layout(&app_id)?;
        let manifest = load_manifest(&layout).map_err(|error| error.to_string())?;
        let mutations = normalize_mutations(&input)?;
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |elapsed| {
                u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX)
            });
        tokio::task::spawn_blocking(move || {
            let mut store = AppDataStore::open(layout)?;
            store.mutate(&manifest, &mutations, now_ms)
        })
        .await
        .map_err(|error| format!("data mutation worker failed: {error}"))?
        .map(|results| json!({ "results": results }))
        .map_err(|error| error.to_string())
    }

    async fn request_ui(&self, request: AppUiRequestDto) -> Result<Value, String> {
        let request_id = request.request_id.clone();
        let (sender, receiver) = oneshot::channel();
        self.pending_ui
            .lock()
            .await
            .insert(request_id.clone(), sender);
        self.event_sink
            .emit(ClientEvent::AppEvent {
                event: AppEventDto::AppUiRequest { request },
            })
            .await;
        let resolution = match timeout(UI_TIMEOUT, receiver).await {
            Ok(Ok(resolution)) => resolution,
            Ok(Err(_)) => return Err("WebView action was cancelled".into()),
            Err(_) => {
                self.pending_ui.lock().await.remove(&request_id);
                return Err("WebView action timed out".into());
            }
        };
        if matches!(resolution.decision, AppAuthorizationDecisionDto::Deny) {
            return Err("user denied the WebView action".into());
        }
        if let Some(error) = resolution.error {
            return Err(error);
        }
        let result = resolution.result_json.unwrap_or_else(|| "{}".into());
        serde_json::from_str(&result)
            .map_err(|error| format!("invalid WebView result JSON: {error}"))
    }

    pub(crate) async fn execute_bridge(&self, request: AppBridgeRequestDto) {
        let result = self.execute_bridge_inner(&request).await;
        let response = match result {
            Ok(value) => AppBridgeResponseDto {
                request_id: request.request_id,
                app_id: request.app_id,
                ok: true,
                result_json: Some(value.to_string()),
                error: None,
                error_code: None,
            },
            Err(failure) => AppBridgeResponseDto {
                request_id: request.request_id,
                app_id: request.app_id,
                ok: false,
                result_json: None,
                error: Some(failure.message),
                error_code: failure.code.map(str::to_string),
            },
        };
        self.event_sink
            .emit(ClientEvent::AppEvent {
                event: AppEventDto::AppBridgeResponse { response },
            })
            .await;
    }

    async fn execute_bridge_inner(
        &self,
        request: &AppBridgeRequestDto,
    ) -> Result<Value, BridgeFailure> {
        let payload_json = request.payload_json.as_deref().unwrap_or("{}");
        let payload_limit = if matches!(
            request.operation,
            AppBridgeOperationDto::LlmChat | AppBridgeOperationDto::LlmStream
        ) {
            LOCAL_APP_BRIDGE_LLM_BYTES
        } else if matches!(
            request.operation,
            AppBridgeOperationDto::FileRead | AppBridgeOperationDto::FileWrite
        ) {
            LOCAL_APP_BRIDGE_FILE_BYTES
        } else {
            LOCAL_APP_BRIDGE_CONTROL_BYTES
        };
        if payload_json.len() > payload_limit {
            return Err(BridgeFailure::coded(
                "payload_too_large",
                format!(
                    "bridge payload is {} bytes; the limit for this operation is {payload_limit}",
                    payload_json.len()
                ),
            ));
        }
        let payload: Value = serde_json::from_str(payload_json).map_err(|error| {
            BridgeFailure::coded(
                "payload_invalid",
                format!("invalid bridge payload JSON: {error}"),
            )
        })?;
        // The page-facing DTO is intentionally small and legacy-compatible;
        // the v2 attribution context is created here, inside the trusted host,
        // before any capability handler runs. A page cannot manufacture its
        // origin, app instance, or grant epoch.
        self.build_bridge_invocation_context(request)
            .map_err(BridgeFailure::from)?;
        let mut input = payload.as_object().cloned().ok_or_else(|| {
            BridgeFailure::coded("payload_invalid", "bridge payload must be a JSON object")
        })?;
        input.insert("app_id".into(), Value::String(request.app_id.clone()));
        match request.operation {
            AppBridgeOperationDto::QueryData => self
                .query_data_value(Value::Object(input))
                .await
                .map_err(BridgeFailure::from),
            // The page is acting for the foreground user, not an agent.  Its
            // app id is host-bound and the manifest still constrains writes.
            AppBridgeOperationDto::MutateData => self
                .mutate_data_value(Value::Object(input), false)
                .await
                .map_err(BridgeFailure::from),
            AppBridgeOperationDto::RuntimeStatus => {
                let runtime = self
                    .service()?
                    .runtime_record(&request.app_id)
                    .await
                    .map_err(|error| error.to_string())?;
                Ok(json!(runtime))
            }
            AppBridgeOperationDto::NetworkRequest => self
                .network_request(&request.app_id, Value::Object(input))
                .await
                .map_err(BridgeFailure::from),
            AppBridgeOperationDto::CapturePhoto => {
                self.capture_photo_value(&request.app_id, &payload).await
            }
            AppBridgeOperationDto::PickImage => {
                self.pick_image_value(&request.app_id, &payload).await
            }
            AppBridgeOperationDto::RecordAudioStart => {
                self.record_audio_start_value(&request.app_id, &payload)
                    .await
            }
            AppBridgeOperationDto::RecordAudioStop => {
                self.record_audio_stop_value(&request.app_id).await
            }
            AppBridgeOperationDto::GetLocation => self.get_location_value(&request.app_id).await,
            AppBridgeOperationDto::PostNotification => {
                self.post_notification_value(&request.app_id, &payload)
                    .await
            }
            AppBridgeOperationDto::TranscribeSpeech => {
                self.transcribe_speech_value(&request.app_id, &payload)
                    .await
            }
            AppBridgeOperationDto::ClipboardGetText => {
                self.clipboard_get_text_value(&request.app_id).await
            }
            AppBridgeOperationDto::ClipboardSetText => {
                self.clipboard_set_text_value(&request.app_id, &payload)
                    .await
            }
            AppBridgeOperationDto::Share => self.share_value(&request.app_id, &payload).await,
            AppBridgeOperationDto::SynthesizeSpeech => {
                self.synthesize_speech_value(&request.app_id, &payload)
                    .await
            }
            AppBridgeOperationDto::FileRead => {
                self.file_read_value(&request.app_id, &payload).await
            }
            AppBridgeOperationDto::FileWrite => {
                self.file_write_value(&request.app_id, &payload).await
            }
            AppBridgeOperationDto::DeviceStatus => self.device_status_value(&request.app_id).await,
            AppBridgeOperationDto::Haptics => self.haptics_value(&request.app_id, &payload).await,
            AppBridgeOperationDto::DeepLink => {
                self.deep_link_value(&request.app_id, &payload).await
            }
            AppBridgeOperationDto::CalendarListEvents => {
                self.calendar_list_events_value(&request.app_id, &payload)
                    .await
            }
            AppBridgeOperationDto::ContactsSearch => {
                self.contacts_search_value(&request.app_id, &payload).await
            }
            AppBridgeOperationDto::MediaGet => self.media_value(&request.app_id, &payload).await,
            AppBridgeOperationDto::LlmChat => self.llm_chat_value(&request.app_id, &payload).await,
            AppBridgeOperationDto::LlmStream => {
                self.llm_stream_value(&request.app_id, &request.request_id, &payload)
                    .await
            }
            AppBridgeOperationDto::AgentPost => {
                self.agent_post_value(&request.app_id, &payload).await
            }
            AppBridgeOperationDto::AgentSessionCreate => self
                .agent_session_create_value(Value::Object(input))
                .await
                .map_err(BridgeFailure::from),
            AppBridgeOperationDto::AgentSessionList => self
                .agent_session_list_value(Value::Object(input))
                .await
                .map_err(BridgeFailure::from),
            AppBridgeOperationDto::AgentSessionResume => {
                input.insert("action".into(), Value::String("resume".into()));
                self.agent_session_update_value(Value::Object(input))
                    .await
                    .map_err(BridgeFailure::from)
            }
            AppBridgeOperationDto::AgentSessionClose => {
                input.insert("action".into(), Value::String("close".into()));
                self.agent_session_update_value(Value::Object(input))
                    .await
                    .map_err(BridgeFailure::from)
            }
            AppBridgeOperationDto::AgentSend => {
                self.agent_send_value(&request.app_id, &request.request_id, &payload)
                    .await
            }
            AppBridgeOperationDto::AgentStream => {
                self.agent_stream_value(&request.app_id, &request.request_id, &payload)
                    .await
            }
            AppBridgeOperationDto::AgentCancel => {
                self.agent_cancel_value(&request.app_id, &payload).await
            }
            AppBridgeOperationDto::AgentProfileProposeUpdate => self
                .agent_profile_propose_value(Value::Object(input))
                .await
                .map_err(BridgeFailure::from),
            AppBridgeOperationDto::BackgroundSchedule => self
                .background_schedule_value(Value::Object(input))
                .await
                .map_err(BridgeFailure::from),
            AppBridgeOperationDto::BackgroundList => self
                .background_list_value(Value::Object(input))
                .await
                .map_err(BridgeFailure::from),
            AppBridgeOperationDto::BackgroundStatus => self
                .background_status_value(Value::Object(input))
                .await
                .map_err(BridgeFailure::from),
            AppBridgeOperationDto::BackgroundCancel => self
                .background_cancel_value(Value::Object(input))
                .await
                .map_err(BridgeFailure::from),
            AppBridgeOperationDto::BackgroundRetry => self
                .background_retry_value(Value::Object(input))
                .await
                .map_err(BridgeFailure::from),
            _ => Err("unsupported bridge operation for this engine version".into()),
        }
    }

    fn build_bridge_invocation_context(
        &self,
        request: &AppBridgeRequestDto,
    ) -> Result<local_apps::InvocationContext, String> {
        let capability = match request.operation {
            AppBridgeOperationDto::QueryData => local_apps::CapabilityId::DataQuery,
            AppBridgeOperationDto::MutateData => local_apps::CapabilityId::DataMutate,
            AppBridgeOperationDto::NetworkRequest => local_apps::CapabilityId::NetworkRequest,
            AppBridgeOperationDto::RuntimeStatus => local_apps::CapabilityId::RuntimeStatus,
            AppBridgeOperationDto::CapturePhoto => local_apps::CapabilityId::Camera,
            AppBridgeOperationDto::PickImage => local_apps::CapabilityId::PhotoLibrary,
            AppBridgeOperationDto::RecordAudioStart | AppBridgeOperationDto::RecordAudioStop => {
                local_apps::CapabilityId::Microphone
            }
            AppBridgeOperationDto::GetLocation => local_apps::CapabilityId::Location,
            AppBridgeOperationDto::TranscribeSpeech => local_apps::CapabilityId::SpeechToText,
            AppBridgeOperationDto::PostNotification => local_apps::CapabilityId::Notifications,
            AppBridgeOperationDto::ClipboardGetText | AppBridgeOperationDto::ClipboardSetText => {
                local_apps::CapabilityId::Clipboard
            }
            AppBridgeOperationDto::Share => local_apps::CapabilityId::Share,
            AppBridgeOperationDto::SynthesizeSpeech => local_apps::CapabilityId::TextToSpeech,
            AppBridgeOperationDto::FileRead => local_apps::CapabilityId::FilesRead,
            AppBridgeOperationDto::FileWrite => local_apps::CapabilityId::FilesWrite,
            AppBridgeOperationDto::DeviceStatus => local_apps::CapabilityId::DeviceStatus,
            AppBridgeOperationDto::Haptics => local_apps::CapabilityId::Haptics,
            AppBridgeOperationDto::DeepLink => local_apps::CapabilityId::DeepLink,
            AppBridgeOperationDto::CalendarListEvents => local_apps::CapabilityId::Calendar,
            AppBridgeOperationDto::ContactsSearch => local_apps::CapabilityId::Contacts,
            AppBridgeOperationDto::MediaGet => local_apps::CapabilityId::Media,
            AppBridgeOperationDto::LlmChat => local_apps::CapabilityId::LlmComplete,
            AppBridgeOperationDto::LlmStream => local_apps::CapabilityId::LlmStream,
            AppBridgeOperationDto::AgentPost => local_apps::CapabilityId::AgentEmit,
            AppBridgeOperationDto::AgentSessionCreate => {
                local_apps::CapabilityId::AgentSessionCreate
            }
            AppBridgeOperationDto::AgentSessionList => local_apps::CapabilityId::AgentSessionList,
            AppBridgeOperationDto::AgentSessionResume => {
                local_apps::CapabilityId::AgentSessionResume
            }
            AppBridgeOperationDto::AgentSessionClose => local_apps::CapabilityId::AgentSessionClose,
            AppBridgeOperationDto::AgentSend => local_apps::CapabilityId::AgentSend,
            AppBridgeOperationDto::AgentStream => local_apps::CapabilityId::AgentStream,
            AppBridgeOperationDto::AgentCancel => local_apps::CapabilityId::AgentCancel,
            AppBridgeOperationDto::AgentProfileProposeUpdate => {
                local_apps::CapabilityId::AgentProfilePropose
            }
            AppBridgeOperationDto::BackgroundSchedule => {
                local_apps::CapabilityId::BackgroundSchedule
            }
            AppBridgeOperationDto::BackgroundList
            | AppBridgeOperationDto::BackgroundStatus
            | AppBridgeOperationDto::BackgroundCancel
            | AppBridgeOperationDto::BackgroundRetry => {
                local_apps::CapabilityId::BackgroundSchedule
            }
            _ => return Err("unsupported bridge operation for runtime v2 context".into()),
        };
        let layout = self.layout(&request.app_id)?;
        let manifest = load_manifest(&layout).map_err(|error| error.to_string())?;
        if !manifest.runtime_api_compatible() {
            return Err(format!(
                "runtime_api_incompatible: app manifest targets runtime API v{}",
                manifest.runtime_api_version
            ));
        }
        let permissions = load_permissions(&layout).map_err(|error| error.to_string())?;
        let context = local_apps::InvocationContext {
            app_id: request.app_id.clone(),
            app_instance_id: format!("page-{}", request.app_id),
            request_id: request.request_id.clone(),
            turn_id: None,
            origin: local_apps::InvocationOrigin::PageForeground,
            grant_epoch: permissions.grant_epoch,
            capability_instance: Some(format!("{}:{}", request.app_id, capability.as_str())),
            call_chain: Vec::new(),
        };
        context.validate().map_err(|error| error.to_string())?;
        Ok(context)
    }

    async fn network_request(&self, app_id: &str, input: Value) -> Result<Value, String> {
        let url_text = required_string(&input, "url")?;
        let url = reqwest::Url::parse(url_text).map_err(|error| format!("invalid URL: {error}"))?;
        if url.scheme() != "https" || url.username() != "" || url.password().is_some() {
            return Err(
                "network bridge accepts plain HTTPS URLs without embedded credentials".into(),
            );
        }
        let domain = url
            .host_str()
            .ok_or_else(|| "network URL has no hostname".to_string())?;
        if domain == "localhost" || !domain.contains('.') || domain.parse::<IpAddr>().is_ok() {
            return Err("network bridge requires a public DNS hostname".into());
        }
        self.authorize_domain(app_id, domain).await?;
        let port = url.port_or_known_default().unwrap_or(443);
        let resolved: Vec<SocketAddr> = tokio::net::lookup_host((domain, port))
            .await
            .map_err(|error| format!("resolve network domain: {error}"))?
            .collect();
        if resolved.is_empty() || resolved.iter().any(|address| !public_ip(address.ip())) {
            return Err("network domain resolved to a private, local, or invalid address".into());
        }
        let method = input
            .get("method")
            .and_then(Value::as_str)
            .unwrap_or("GET")
            .to_ascii_uppercase();
        if !matches!(method.as_str(), "GET" | "POST" | "PUT" | "PATCH" | "DELETE") {
            return Err("network method must be GET, POST, PUT, PATCH, or DELETE".into());
        }
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(30))
            .resolve_to_addrs(domain, &resolved)
            .build()
            .map_err(|error| format!("create network client: {error}"))?;
        let mut builder = client.request(method.parse().map_err(|_| "invalid HTTP method")?, url);
        if let Some(headers) = input.get("headers").and_then(Value::as_object) {
            if headers.len() > 32 {
                return Err("network request has more than 32 headers".into());
            }
            for (name, value) in headers {
                let Some(value) = value.as_str() else {
                    return Err("network header values must be strings".into());
                };
                let lower = name.to_ascii_lowercase();
                if matches!(
                    lower.as_str(),
                    "host" | "cookie" | "authorization" | "proxy-authorization"
                ) {
                    return Err(format!("network header {name:?} is reserved"));
                }
                builder = builder.header(name, value);
            }
        }
        if let Some(body) = input.get("body") {
            let encoded = if let Some(text) = body.as_str() {
                text.as_bytes().to_vec()
            } else {
                serde_json::to_vec(body).map_err(|error| error.to_string())?
            };
            if encoded.len() > 1024 * 1024 {
                return Err("network request body exceeds 1 MiB".into());
            }
            builder = builder.body(encoded);
        }
        let response = builder
            .send()
            .await
            .map_err(|error| format!("network request failed: {error}"))?;
        let status = response.status().as_u16();
        let headers: Map<String, Value> = response
            .headers()
            .iter()
            .filter_map(|(name, value)| {
                value
                    .to_str()
                    .ok()
                    .map(|value| (name.to_string(), Value::String(value.to_string())))
            })
            .collect();
        let bytes = read_limited_stream(
            response.bytes_stream(),
            MAX_NETWORK_RESPONSE_BYTES,
            "read network response",
            "network response exceeds 2 MiB",
        )
        .await?;
        Ok(json!({
            "status": status,
            "headers": headers,
            "body": String::from_utf8_lossy(&bytes),
        }))
    }

    pub(crate) async fn manage_runtime_value(&self, input: Value) -> Result<Value, String> {
        let app_id = required_string(&input, "app_id")?.to_string();
        let action = required_string(&input, "action")?;
        match action {
            "start" | "open" | "resume" => self.start_runtime(&app_id).await,
            "stop" | "suspend" => self.stop_runtime(&app_id).await,
            "restart" => {
                self.stop_runtime(&app_id).await?;
                self.start_runtime(&app_id).await
            }
            _ => {
                Err("runtime action must be start, stop, restart, open, suspend, or resume".into())
            }
        }
    }

    async fn start_runtime(&self, app_id: &str) -> Result<Value, String> {
        let service = self.service()?;
        service
            .record(app_id)
            .await
            .map_err(|error| error.to_string())?;
        let access_tick = self.next_request_id.fetch_add(1, Ordering::Relaxed);
        let mut wait_for_start = None;
        let mut return_running = false;
        let mut reserved_generation = None;
        {
            let mut runtimes = self.runtimes.lock().await;
            if let Some(entry) = runtimes.get_mut(app_id) {
                entry.last_used = access_tick;
                match &entry.state {
                    RuntimeEntryState::Starting { gate } => {
                        wait_for_start = Some(gate.subscribe());
                    }
                    RuntimeEntryState::Running { .. } => {
                        return_running = true;
                    }
                }
            } else {
                let generation = self.next_request_id.fetch_add(1, Ordering::Relaxed);
                let (gate, _) = watch::channel(RuntimeStartStatus::Pending);
                runtimes.insert(
                    app_id.to_string(),
                    RuntimeEntry {
                        state: RuntimeEntryState::Starting { gate },
                        last_used: access_tick,
                        generation,
                    },
                );
                reserved_generation = Some(generation);
            }
        }
        if let Some(receiver) = wait_for_start {
            return self.wait_for_runtime_start(app_id, receiver).await;
        }
        if return_running {
            let runtime = service
                .runtime_record(app_id)
                .await
                .map_err(|e| e.to_string())?;
            return Ok(json!({
                "app_id": app_id,
                "state": runtime.state,
                "url": runtime.port.map(|port| format!("http://127.0.0.1:{port}"))
            }));
        }
        let Some(generation) = reserved_generation else {
            return Err("runtime start reservation disappeared before completion".into());
        };
        self.start_reserved_runtime(app_id, generation).await
    }

    async fn wait_for_runtime_start(
        &self,
        app_id: &str,
        mut receiver: watch::Receiver<RuntimeStartStatus>,
    ) -> Result<Value, String> {
        loop {
            let status = receiver.borrow_and_update().clone();
            match status {
                RuntimeStartStatus::Pending => {
                    receiver
                        .changed()
                        .await
                        .map_err(|_| "runtime start was cancelled".to_string())?;
                }
                RuntimeStartStatus::Running => {
                    let runtime = self
                        .service()?
                        .runtime_record(app_id)
                        .await
                        .map_err(|error| error.to_string())?;
                    return Ok(json!({
                        "app_id": app_id,
                        "state": runtime.state,
                        "url": runtime.port.map(|port| format!("http://127.0.0.1:{port}"))
                    }));
                }
                RuntimeStartStatus::Failed(detail) => return Err(detail),
            }
        }
    }

    async fn start_reserved_runtime(&self, app_id: &str, generation: u64) -> Result<Value, String> {
        let _reservation = RuntimeReservation {
            runtimes: Arc::clone(&self.runtimes),
            app_id: app_id.to_string(),
            generation,
        };
        let service = self.service()?;
        let layout = self.layout(app_id)?;
        let manifest = local_apps::load_manifest(&layout).map_err(|error| error.to_string())?;
        if !manifest.runtime_api_compatible() {
            return self
                .fail_reserved_runtime_start(
                    app_id,
                    generation,
                    None,
                    format!(
                        "runtime_api_incompatible: app manifest targets runtime API v{}; regenerate or rebuild this app for v{}",
                        manifest.runtime_api_version,
                        local_apps::RUNTIME_API_MAJOR
                    ),
                )
                .await;
        }
        let static_root = layout
            .root()
            .join(layout.build_rel(false))
            .join(crate::local_apps_build::VITE_OUTPUT_DIR);
        if !static_root.join("index.html").is_file() {
            return self
                .fail_reserved_runtime_start(
                    app_id,
                    generation,
                    None,
                    "static build output is missing index.html; generate the app first".into(),
                )
                .await;
        }
        let current = service
            .runtime_record(app_id)
            .await
            .map_err(|e| e.to_string())?;
        // The listener has to be registered with the SAME immortal I/O driver
        // that will serve it.  `start_reserved_runtime` runs on the AMBIENT
        // runtime — for an agent-driven `manage_runtime {action:"start"}` that
        // is the per-engine one — and tokio invalidates every resource
        // registered with a dropped runtime's driver, so a listener bound here
        // fails every later `accept()` forever behind an entry that still
        // reports `running`.
        let bound = {
            let app_id = app_id.to_string();
            let assigned = current.port;
            let leases = Arc::clone(&self.port_leases);
            let registry = Arc::clone(&service);
            // Everything from the pin read to the choice is one step against
            // this broker's other ALLOCATIONS — but only against those: a
            // sibling that is already past its own allocation still persists
            // its pin and releases its lease inside this window, which is why
            // the choice is re-checked against a fresh read below rather than
            // trusted because the gate is held.  See `port_allocation`.
            let _allocation = self.port_allocation.lock().await;
            // The FIRST read is taken on THIS runtime, before the hop: the
            // derivation has to know which ports stopped siblings own
            // permanently, which no bind probe on the worker runtime can
            // discover.  The re-read after the lease runs on the worker
            // runtime, where it is an ordinary `AppService` read — no runtime
            // affinity, nothing blocking.
            let sibling_pins = sibling_pinned_ports(&service, &app_id).await;
            crate::local_apps_profile::worker_runtime()
                .spawn(async move {
                    bind_stable_loopback(&app_id, assigned, &sibling_pins, &leases, &registry).await
                })
                .await
                .map_err(|error| format!("bind stable app port: {error}"))?
        };
        let (listener, port, port_lease) = match bound {
            Ok(bound) => bound,
            Err(error) => {
                return self
                    .fail_reserved_runtime_start(app_id, generation, current.port, error)
                    .await;
            }
        };
        if let Err(error) = service
            .set_runtime_mode(app_id, AppRuntimeMode::StaticExport)
            .await
        {
            return self
                .fail_reserved_runtime_start(
                    app_id,
                    generation,
                    Some(port),
                    format!("persist runtime mode: {error}"),
                )
                .await;
        }
        if let Err(error) = service
            .update_runtime_record(app_id, AppRuntimeState::Starting, Some(port), None, None)
            .await
        {
            return self
                .fail_reserved_runtime_start(
                    app_id,
                    generation,
                    Some(port),
                    format!("persist starting runtime state: {error}"),
                )
                .await;
        }
        // The pin is durable HERE and not one line earlier: `with_app` has
        // already written the new record back under the state lock by the time
        // it returns, so from this point `sibling_pinned_ports` reports the
        // port for every later allocator and the lease has nothing left to
        // cover.  Every path above this line drops the guard instead, which
        // returns the port to the pool.
        //
        // That ORDER — persist, THEN release — is load-bearing beyond tidiness:
        // it is the whole premise of `bind_stable_loopback`'s post-lease pin
        // re-read.  A release moved above the persist would leave a port that
        // is in neither the records nor the leases, which is exactly the hole
        // both mechanisms exist to close.
        if let Some(lease) = port_lease {
            lease.commit();
        }

        let (shutdown, receiver) = oneshot::channel();
        self.spawn_static_server(
            service.clone(),
            app_id.to_string(),
            generation,
            listener,
            static_root,
            receiver,
        );
        let handle = RuntimeHandle::Static { shutdown };
        if let Err(error) = service
            .update_runtime_record(app_id, AppRuntimeState::Running, Some(port), None, None)
            .await
            .map_err(|error| error.to_string())
        {
            self.cleanup_runtime_handle(handle).await;
            return self
                .fail_reserved_runtime_start(
                    app_id,
                    generation,
                    Some(port),
                    format!("persist running runtime state: {error}"),
                )
                .await;
        }
        let gate = {
            let mut runtimes = self.runtimes.lock().await;
            let Some(entry) = runtimes.get_mut(app_id) else {
                self.cleanup_runtime_handle(handle).await;
                return Err("runtime start reservation disappeared before completion".into());
            };
            if entry.generation != generation {
                self.cleanup_runtime_handle(handle).await;
                return Err("runtime start reservation changed before completion".into());
            }
            let previous =
                std::mem::replace(&mut entry.state, RuntimeEntryState::Running { handle });
            match previous {
                RuntimeEntryState::Starting { gate } => gate,
                RuntimeEntryState::Running { handle } => {
                    entry.state = RuntimeEntryState::Running { handle };
                    return Err("runtime start reservation was already resolved".into());
                }
            }
        };
        let _ = gate.send(RuntimeStartStatus::Running);
        Ok(json!({"app_id": app_id, "state": "running", "url": format!("http://127.0.0.1:{port}")}))
    }

    async fn stop_runtime(&self, app_id: &str) -> Result<Value, String> {
        let service = self.service()?;
        service
            .record(app_id)
            .await
            .map_err(|error| error.to_string())?;
        self.release_app_runtime_state(app_id).await;
        // Classify and remove under ONE acquisition: a start woken in the gap
        // between a `remove` and its rollback `insert` finds no entry, kills the
        // runtime it just spawned and returns without resolving the gate,
        // leaving a reservation nothing can ever complete.
        let handle = {
            let mut runtimes = self.runtimes.lock().await;
            match runtimes.get(app_id).map(|entry| &entry.state) {
                None => return Ok(json!({"app_id": app_id, "state": "stopped"})),
                Some(RuntimeEntryState::Starting { .. }) => {
                    return Err("runtime is still starting; retry stop shortly".into());
                }
                Some(RuntimeEntryState::Running { .. }) => {}
            }
            match runtimes.remove(app_id).map(|entry| entry.state) {
                Some(RuntimeEntryState::Running { handle }) => handle,
                _ => return Ok(json!({"app_id": app_id, "state": "stopped"})),
            }
        };
        let runtime = service
            .runtime_record(app_id)
            .await
            .map_err(|e| e.to_string())?;
        service
            .update_runtime_record(
                app_id,
                AppRuntimeState::Stopping,
                runtime.port,
                runtime.pid,
                None,
            )
            .await
            .map_err(|error| error.to_string())?;
        match handle {
            RuntimeHandle::Static { shutdown } => {
                let _ = shutdown.send(());
            }
        }
        service
            .update_runtime_record(app_id, AppRuntimeState::Stopped, runtime.port, None, None)
            .await
            .map_err(|error| error.to_string())?;
        Ok(json!({"app_id": app_id, "state": "stopped"}))
    }

    async fn fail_reserved_runtime_start(
        &self,
        app_id: &str,
        generation: u64,
        port: Option<u16>,
        detail: String,
    ) -> Result<Value, String> {
        let gate = {
            let mut runtimes = self.runtimes.lock().await;
            let matches_generation = runtimes.get(app_id).is_some_and(|entry| {
                entry.generation == generation
                    && matches!(entry.state, RuntimeEntryState::Starting { .. })
            });
            if !matches_generation {
                None
            } else {
                match runtimes.remove(app_id) {
                    Some(RuntimeEntry {
                        state: RuntimeEntryState::Starting { gate },
                        ..
                    }) => Some(gate),
                    Some(entry) => {
                        runtimes.insert(app_id.to_string(), entry);
                        None
                    }
                    None => None,
                }
            }
        };
        if let Some(gate) = gate {
            let _ = gate.send(RuntimeStartStatus::Failed(detail.clone()));
        }
        // Same rule as `stop_runtime`'s failed kill: this bookkeeping write must
        // never mask the real failure, so its result stays discarded.
        //
        // Three callers reach here BEFORE the record leaves `stopped` (no
        // runtime mount, no static build, and the squatted permanent port).
        // `stopped -> failed` is now a legal edge (`local_apps::state::
        // runtime_transition_allowed`), added precisely so this write lands:
        // while it was rejected, the state, the `lastError`, and the
        // `RuntimeChanged` event were all discarded, leaving an app that could
        // not start and carried no recorded reason. The detail also reaches
        // every concurrent waiter through the gate above, and `failed ->
        // starting` keeps the record recoverable.
        if let Ok(service) = self.service() {
            let _ = service
                .update_runtime_record(
                    app_id,
                    AppRuntimeState::Failed,
                    port,
                    None,
                    Some(detail.clone()),
                )
                .await;
        }
        Err(detail)
    }

    async fn cleanup_runtime_handle(&self, handle: RuntimeHandle) {
        match handle {
            RuntimeHandle::Static { shutdown } => {
                let _ = shutdown.send(());
            }
        }
    }

    /// Everything an app's runtime owned that must not outlive it.
    ///
    /// Called from EVERY way a runtime can end — the explicit stop, the Full
    /// handle's exit watch, and the static listener's reconciliation — not
    /// just the one the user drives. A crashed app used to keep the iOS
    /// audio-session lease open with nothing left able to release it, which
    /// takes FlowMode, hold-to-talk and transcribeSpeech down with it for the
    /// life of the process.
    ///
    /// Session grants go too: the user answered "allow for this session"
    /// while USING the app, and a grant that quietly survives the app's death
    /// behaves as "always allow" while staying invisible to permissions.json
    /// and unrevokable short of a full reset.
    pub(crate) async fn release_app_runtime_state(&self, app_id: &str) {
        self.force_stop_recording(app_id).await;
        self.clear_media(app_id);
        self.session_permissions.lock().await.revoke_app(app_id);
    }

    /// Serves the static export on the anchored runtime, and reconciles the
    /// record when the LISTENER dies the way the Full handle's exit watch does.
    /// A static handle has no process to poll, so without this a retired server
    /// leaves `Running { Static }` in the map and `start_runtime`'s
    /// `return_running` short-circuit keeps handing out a URL nothing answers.
    ///
    /// `service` is passed in rather than re-resolved: the only caller already
    /// holds it, and a bail-out here would drop the listener while the entry
    /// went on to report `running`.
    fn spawn_static_server(
        &self,
        service: Arc<AppService>,
        app_id: String,
        generation: u64,
        listener: TcpListener,
        root: PathBuf,
        shutdown: oneshot::Receiver<()>,
    ) {
        let runtimes = Arc::clone(&self.runtimes);
        let broker = self.weak_self();
        crate::local_apps_profile::worker_runtime().spawn(async move {
            let Some(detail) = run_static_server(listener, root, shutdown).await else {
                return;
            };
            reconcile_static_runtime_exit(runtimes, service, app_id, generation, detail, broker)
                .await;
        });
    }

    pub(crate) async fn restore_checkpoint_value(&self, input: Value) -> Result<Value, String> {
        let app_id = required_string(&input, "app_id")?.to_string();
        let checkpoint_id = required_string(&input, "checkpoint_id")?.to_string();
        let service = self.service()?;
        service.record(&app_id).await.map_err(|e| e.to_string())?;
        // Pre-flight the one precondition the restore cannot recover from,
        // BEFORE reading any digest, BEFORE prompting the user and BEFORE
        // stopping the runtime: an app created with `git_enabled: false` has
        // no checkpoints to restore, and the store rejects it deep inside
        // git2 with a raw "could not find repository" message. Discovering
        // that after the stop leaves the user with an approved restore that
        // did nothing except take their app offline.
        if !service
            .git_version_control_enabled(&app_id)
            .await
            .map_err(|e| e.to_string())?
        {
            return Err(
                "this app was created without Git version control, so it has no checkpoints to \
                 restore"
                    .into(),
            );
        }
        let layout = self.layout(&app_id)?;
        let restore_reason = "Restoring rewinds application source code. The host will rebuild the fixed local-app scaffold from its verified runtime snapshot before the app can serve again. App data is not changed.".to_string();
        let decision = self
            .request_capability(
                &app_id,
                AppCapabilityKindDto::RestoreCheckpoint,
                None,
                &restore_reason,
            )
            .await?;
        if matches!(raise_decision(decision), PermissionDecision::Deny) {
            return Err("user denied checkpoint restoration".into());
        }
        // Remember whether the app was serving BEFORE the restore so a
        // successful rebuild can put it back the way the user had it.
        let was_running = service
            .runtime_record(&app_id)
            .await
            .map(|runtime| {
                matches!(
                    runtime.state,
                    AppRuntimeState::Starting | AppRuntimeState::Running
                )
            })
            .unwrap_or(false);
        self.stop_runtime(&app_id).await?;
        // The service does the durable work: a PreRestore safety checkpoint
        // first, then the workspace-only Git restore (data/runtime/build
        // paths sit outside the repository and are never reset).
        let safety = service
            .restore_checkpoint(&app_id, &checkpoint_id)
            .await
            .map_err(|error| error.to_string())?;
        // Rebuild the restored source so the served output matches it.
        let builder = crate::local_apps_build::LocalAppBuilder {
            mobile_linux: self.mobile_linux(),
            host: self,
        };
        if let Err(error) = builder.build_workspace(&layout).await {
            let source_rollback_error = service
                .restore_checkpoint(&app_id, &safety.id)
                .await
                .err()
                .map(|error| error.to_string());
            let rollback_build_error = if source_rollback_error.is_none() {
                builder
                    .build_workspace(&layout)
                    .await
                    .err()
                    .map(|error| error.to_string())
            } else {
                None
            };
            let restarted = if was_running && source_rollback_error.is_none() {
                self.manage_runtime_value(json!({
                    "app_id": app_id.clone(),
                    "action": "start",
                }))
                .await
                .is_ok()
            } else {
                false
            };
            return Err(format!(
                "checkpoint {checkpoint_id} was restored, but rebuilding failed: {error}; source rollback: {}; rollback rebuild: {}; runtime restarted: {restarted}. Read the build log via read_logs (log=\"build\"), fix the source, then run the build tool again.",
                source_rollback_error.as_deref().unwrap_or("completed"),
                rollback_build_error.as_deref().unwrap_or("completed")
            ));
        }
        // Best-effort restart when the runtime was serving before the
        // restore; a failure here leaves the app restored+rebuilt but
        // stopped, which the caller can see and fix via manage_runtime.
        let restarted = if was_running {
            self.manage_runtime_value(json!({ "app_id": app_id.clone(), "action": "start" }))
                .await
                .is_ok()
        } else {
            false
        };
        Ok(json!({
            "ok": true,
            "app_id": app_id,
            "checkpoint_id": checkpoint_id,
            "rebuilt": true,
            "restarted": restarted,
        }))
    }

    /// Initialize the host-owned metadata and repository-verified Vite
    /// scaffold for a freshly created app.
    pub(crate) async fn scaffold_app_value(
        &self,
        record: &local_apps::AppRecord,
    ) -> Result<(), String> {
        let layout = self.layout(&record.id)?;
        // Write the per-app LINGXI.md context file at the workspace root:
        // every session rooted in this workspace auto-loads it into the
        // system context (`orchestrator::prompt::real_provider`), so the
        // agent starts with the brief + the workspace contract without any
        // prompt plumbing. It sits OUTSIDE the writable roots, so the agent
        // cannot edit its own contract.
        let workspace = layout.root().join(layout.workspace_rel());
        let setup_path = "- This workspace already contains the repository-verified Vite + Tailwind + shadcn/ui foundation. The host prepares app-local dependencies in `workspace/node_modules`. Do not run `npm create vite`, do not create a second scaffold, do not add a wrapper build layer, and do not run a package manager in this local-app workspace.\n\
             - Host-managed files are `.gitignore`, `package.json`, `pnpm-lock.yaml`, `pnpm-workspace.yaml`, `components.json`, `jsconfig.json`, `index.html`, `vite.config.mjs`, `.lingxi/source-policy.json`, `lib/lingxi-bridge.js`, `lib/device-context.js`, `lib/platform-adapter.js`, `lib/lingxi-provider.jsx`, and `styles/foundation.css`. Do not edit them.\n\
             - Default editable entry points are `app/screens/home-screen.jsx` and `app/globals.css`. The preset files under `components/ui/` are app-owned and may be edited. You may also edit files under `app/`, `src/`, `components/`, `styles/`, `public/`, and add non-host-managed helpers under `lib/`. The component lab at `#/_components` is lazy-loaded and must stay outside normal navigation unless the user asks for it.\n\
             - Use repo tools exposed in this workspace for source status, diff, and checkpoint versioning when available; checkpoints are workspace Git history. The host rebuilds directly from this workspace as the sole writable mount, keeps temporary output under `.lingxi-build-state/`, and promotes only the validated output.\n";
        // `format!`, not a bare `&str`: this string is interpolated into the
        // enclosing `format!` as a VALUE, so its own `{{` and `{id}` would be
        // copied through verbatim and the agent would read a malformed example
        // of the one call it is required to make.
        let build_preview = format!(
            "- `LocalAppBuild {{\"app_id\":\"{id}\"}}` — offline `vite build` \
             (30-minute budget). The host waits for the app-local dependency state, mounts \
             the workspace as the sole writable `LocalAppBuild` root, runs the workspace's own \
             `node_modules/vite`, writes into private build-state, and serves only the promoted \
             `build/store/dist/`.\n",
            id = record.id,
        );
        let context = format!(
            "# Local App: {name} ({id})\n\n\
             Brief: {brief}\n\n\
             ## Workspace contract\n\
             - This workspace is already bound to local app `{id}`. Treat `{id}` as authoritative; do not call `LocalAppList` or `LocalAppGet` to rediscover or confirm it, and do not call `LocalAppCreate` again.\n\
             - Edit ONLY app-owned files under `app/`, `src/`, `components/`, `lib/`, `styles/`, `public/`.\n\
             {setup_path}\
             - The page reaches host data/network/device ONLY through `window.lingxi.v2` \
             (see `lib/lingxi-bridge.js`).\n\
             - Declare data collections / network domains / capabilities through \
             `LocalAppManifest` BEFORE the page relies on them; runtime \
             authorization still prompts the user. Every collection is `{{id,name,fields}}`; every field is `{{id,label,kind,required?,enumOptions?}}`; IDs use lower snake_case. Never declare host-owned `recordId`, `revision`, `createdAtMs`, or `updatedAtMs` as fields. Repair and retry any rejected manifest before building.\n\
             - If a material requirement is unresolved, call `AskUserQuestion` so the native client presents its sheet. Never leave unresolved questions in ordinary assistant text; when the brief and device context are sufficient, infer and continue.\n\n\
             ## Build & preview\n\
             {build_preview}\
             - `LocalAppRuntime {{\"app_id\":\"{id}\",\"action\":\"start\"}}` \
             — serve the built output and return the preview url.\n\
             - `LocalAppLogs {{\"app_id\":\"{id}\",\"log\":\"build\"}}` — build log.\n\
             - `LocalAppInstallDeps {{\"app_id\":\"{id}\",\"wait\":true}}` \
             — dependency state; `lastError` names why an install failed.\n\n\
             ### When a build fails\n\
             `LocalAppBuild` is the ONLY build path in this workspace, so \
             do NOT try a different build command, package manager, or scaffold tool — \
             there is nothing else to fall back to and improvising cannot succeed. Instead:\n\
             1. Read the failure: `LocalAppLogs {{\"app_id\":\"{id}\",\"log\":\"build\"}}`.\n\
             2. A `not yet available` build means dependencies are not ready. Call \
             `LocalAppInstallDeps {{\"app_id\":\"{id}\",\"wait\":true}}` and read \
             its `lastError`.\n\
             3. If the cause is your source, fix it and build again.\n\
             4. If the cause is the HOST — a missing toolchain, a failed dependency install, \
             an unavailable runtime — report it to the user and stop. Those cannot be worked \
             around from inside this workspace, and retrying will not clear them.\n\n\
             ## Verify\n\
             - `LocalAppInspectUi` / `LocalAppActOnUi` — read and drive \
             the running preview.\n\
             - `LocalAppQueryData {{\"app_id\":\"{id}\",\"collection\":\"<collection_id>\"}}` \
             — after a UI write, confirm the value reached native storage: it must appear in \
             `records[].document`. A value that exists only in page state is NOT persistence.\n\
             - `LocalAppLogs {{\"app_id\":\"{id}\",\"log\":\"runtime\"}}` — runtime log.\n\
             - After the user confirms a working state, record it with \
             `LocalAppCheckpointCreate`.\n",
            name = record.name,
            id = record.id,
            brief = record.brief,
            setup_path = setup_path,
            build_preview = build_preview,
        );
        tokio::task::spawn_blocking(move || {
            crate::local_apps_build::scaffold_workspace_initialized(&layout)?;
            std::fs::write(workspace.join("LINGXI.md"), context).map_err(|error| {
                local_apps::AppError::Io(format!("write workspace LINGXI.md: {error}"))
            })
        })
        .await
        .map_err(|error| format!("join workspace scaffold worker: {error}"))?
        .map_err(|error| error.to_string())
    }
}

impl LocalAppsHostBroker {
    async fn flow_execute_value(&self, input: Value) -> Result<Value, String> {
        let app_id = required_string(&input, "app_id")?.to_string();
        self.service()?
            .record(&app_id)
            .await
            .map_err(|error| error.to_string())?;
        // Flow execution is an app-owned Agent MCP capability. The LLM grant
        // is the host's durable/user-approved entry gate; every individual
        // step still goes through its own capability router below.
        self.authorize_agent_session_capability(&app_id).await?;
        let flow_value = input
            .get("flow")
            .cloned()
            .ok_or_else(|| "flow is required".to_string())?;
        let flow: local_apps::FlowDefinition =
            serde_json::from_value(flow_value).map_err(|error| format!("invalid flow: {error}"))?;
        let registry = local_apps::CapabilityRegistry::default();
        flow.validate(&registry)
            .map_err(|error| format!("invalid flow: {error}"))?;
        let flow_id = flow.flow_id.clone();
        let version = flow.version;
        let outputs = timeout(FLOW_EXECUTION_TIMEOUT, async {
            let mut outputs = Map::new();
            for step in flow.steps {
                let capability = step.capability;
                if !local_apps::allowed_for_synchronous_flow(capability) {
                    return Err(format!(
                        "flow capability {} is not valid for a synchronous flow",
                        capability.as_str()
                    ));
                }
                let step_input: Value =
                    serde_json::from_str(&step.input_json).map_err(|error| {
                        format!("flow step {} has invalid input: {error}", step.step_id)
                    })?;
                let value = timeout(
                    FLOW_STEP_TIMEOUT,
                    self.execute_flow_step(
                        &app_id,
                        &flow_id,
                        &step.step_id,
                        capability,
                        step_input,
                    ),
                )
                .await
                .map_err(|_| format!("flow step {} timed out", step.step_id))??;
                outputs.insert(step.step_id, value);
            }
            Ok::<Map<String, Value>, String>(outputs)
        })
        .await
        .map_err(|_| "flow execution exceeded its wall-clock budget".to_string())??;
        Ok(json!({
            "flowId": flow_id,
            "version": version,
            "outputs": outputs,
        }))
    }

    async fn execute_flow_step(
        &self,
        app_id: &str,
        flow_id: &str,
        step_id: &str,
        capability: local_apps::CapabilityId,
        mut input: Value,
    ) -> Result<Value, String> {
        let object = input
            .as_object_mut()
            .ok_or_else(|| format!("flow step {step_id} input must be a JSON object"))?;
        object.insert("app_id".into(), Value::String(app_id.into()));
        let request_id = format!("flow:{flow_id}:{step_id}");
        match capability {
            local_apps::CapabilityId::DataQuery => self.query_data_value(input).await,
            local_apps::CapabilityId::DataMutate => self.mutate_data_value(input, true).await,
            local_apps::CapabilityId::NetworkRequest => self.network_request(app_id, input).await,
            local_apps::CapabilityId::RuntimeStatus => Ok(json!({
                "app_id": app_id,
                "runtime": self.service()?.runtime_record(app_id).await.map_err(|error| error.to_string())?,
            })),
            local_apps::CapabilityId::FilesRead => self
                .file_read_value(app_id, &input)
                .await
                .map_err(|error| error.message),
            local_apps::CapabilityId::FilesWrite => self
                .file_write_value(app_id, &input)
                .await
                .map_err(|error| error.message),
            local_apps::CapabilityId::Clipboard => {
                if input.get("text").is_some() {
                    self.clipboard_set_text_value(app_id, &input)
                        .await
                        .map_err(|error| error.message)
                } else {
                    self.clipboard_get_text_value(app_id)
                        .await
                        .map_err(|error| error.message)
                }
            }
            local_apps::CapabilityId::Calendar => self
                .calendar_list_events_value(app_id, &input)
                .await
                .map_err(|error| error.message),
            local_apps::CapabilityId::Contacts => self
                .contacts_search_value(app_id, &input)
                .await
                .map_err(|error| error.message),
            local_apps::CapabilityId::Media => self
                .media_value(app_id, &input)
                .await
                .map_err(|error| error.message),
            local_apps::CapabilityId::DeviceStatus => self
                .device_status_value(app_id)
                .await
                .map_err(|error| error.message),
            local_apps::CapabilityId::Haptics => self
                .haptics_value(app_id, &input)
                .await
                .map_err(|error| error.message),
            local_apps::CapabilityId::DeepLink => self
                .deep_link_value(app_id, &input)
                .await
                .map_err(|error| error.message),
            local_apps::CapabilityId::TextToSpeech => self
                .synthesize_speech_value(app_id, &input)
                .await
                .map_err(|error| error.message),
            local_apps::CapabilityId::Location => self
                .get_location_value(app_id)
                .await
                .map_err(|error| error.message),
            local_apps::CapabilityId::Notifications => self
                .post_notification_value(app_id, &input)
                .await
                .map_err(|error| error.message),
            local_apps::CapabilityId::LlmComplete => self
                .llm_chat_value(app_id, &input)
                .await
                .map_err(|error| error.message),
            local_apps::CapabilityId::AgentSessionCreate => {
                self.agent_session_create_value(input).await
            }
            local_apps::CapabilityId::AgentSessionList => {
                self.agent_session_list_value(input).await
            }
            local_apps::CapabilityId::AgentSessionResume
            | local_apps::CapabilityId::AgentSessionClose => {
                self.agent_session_update_value(input).await
            }
            local_apps::CapabilityId::AgentSend => self
                .agent_send_value(app_id, &request_id, &input)
                .await
                .map_err(|error| error.message),
            local_apps::CapabilityId::AgentEmit => self
                .agent_post_value(app_id, &input)
                .await
                .map_err(|error| error.message),
            local_apps::CapabilityId::AgentProfilePropose => {
                self.agent_profile_propose_value(input).await
            }
            local_apps::CapabilityId::AgentStream
            | local_apps::CapabilityId::AgentCancel
            | local_apps::CapabilityId::LlmStream
            | local_apps::CapabilityId::FlowExecute
            | local_apps::CapabilityId::BackgroundSchedule
            | local_apps::CapabilityId::Camera
            | local_apps::CapabilityId::PhotoLibrary
            | local_apps::CapabilityId::Microphone
            | local_apps::CapabilityId::SpeechToText
            | local_apps::CapabilityId::Share => {
                unreachable!("synchronous flow capabilities are filtered before step execution")
            }
            _ => Err(format!(
                "flow capability {} is not supported by this host",
                capability.as_str()
            )),
        }
    }

    fn background_management_layout(&self, app_id: &str) -> Result<AppLayout, String> {
        let layout = self.layout(app_id)?;
        let manifest = load_manifest(&layout).map_err(|error| error.to_string())?;
        if !manifest
            .capabilities
            .contains(&AppCapability::BackgroundSchedule)
        {
            return Err("background task management is not declared in the app manifest".into());
        }
        let permissions = load_permissions(&layout).map_err(|error| error.to_string())?;
        if !permissions.allows(AppCapability::BackgroundSchedule) {
            return Err("background task management requires durable approval".into());
        }
        Ok(layout)
    }
}

#[async_trait]
impl LocalAppsMcpHost for LocalAppsHostBroker {
    fn create_next_step(&self) -> String {
        create_next_step_guidance()
    }

    async fn manage_runtime(&self, input: Value) -> Result<Value, String> {
        self.manage_runtime_value(input).await
    }

    async fn build_app(&self, input: Value) -> Result<Value, String> {
        let app_id = required_string(&input, "app_id")?.to_string();
        // Existence gate (same shape as the UI ops above).
        let service = self.service()?;
        service.record(&app_id).await.map_err(|e| e.to_string())?;
        let layout =
            AppLayout::new(self.root.clone(), app_id.clone()).map_err(|e| e.to_string())?;
        let builder = crate::local_apps_build::LocalAppBuilder {
            mobile_linux: self.mobile_linux(),
            host: self,
        };
        builder
            .build_workspace(&layout)
            .await
            .map_err(|e| e.to_string())?;
        // "Ready" means SERVABLE, not "the build tool exited 0". The static
        // preview server refuses to start without `build/store/dist/index.html`
        // (see `start_reserved_runtime`), and a build whose output landed
        // elsewhere exits 0 while producing nothing this host can serve.
        // Stamping `ready` there would leave a permanently unstartable app
        // advertised as ready in the library.
        let served_index = layout
            .root()
            .join(layout.build_rel(false))
            .join(crate::local_apps_build::VITE_OUTPUT_DIR)
            .join("index.html");
        if !served_index.exists() {
            return Err(format!(
                "the build finished but produced no servable output at {}. The build must emit \
                 the canonical `dist/` directory; restore the standard Vite output contract, \
                 then run the build tool again.",
                served_index.display()
            ));
        }
        service
            .mark_ready(&app_id)
            .await
            .map_err(|e| e.to_string())?;
        let dependencies = service
            .dependency_record(&app_id)
            .await
            .map_err(|e| e.to_string())?;
        Ok(serde_json::json!({
            "ok": true,
            "app_id": app_id,
            "target": "vite-react-static-v1",
            "dependencies": dependencies,
            "hint": "start or restart the runtime with manage_runtime to serve the new build",
        }))
    }

    async fn install_dependencies(&self, input: Value) -> Result<Value, String> {
        self.install_dependencies_value(input).await
    }

    async fn update_manifest(&self, input: Value) -> Result<Value, String> {
        let app_id = required_string(&input, "app_id")?.to_string();
        self.service()?
            .record(&app_id)
            .await
            .map_err(|e| e.to_string())?;
        let layout =
            AppLayout::new(self.root.clone(), app_id.clone()).map_err(|e| e.to_string())?;
        let mut manifest = local_apps::load_manifest(&layout).map_err(|e| e.to_string())?;
        if let Some(collections) = input.get("collections") {
            manifest.collections = serde_json::from_value(collections.clone())
                .map_err(|e| format!("invalid collections: {e}"))?;
        }
        if let Some(domains) = input.get("allowed_domains") {
            manifest.allowed_domains = serde_json::from_value(domains.clone())
                .map_err(|e| format!("invalid allowed_domains: {e}"))?;
        }
        if let Some(capabilities) = input.get("capabilities") {
            manifest.capabilities = serde_json::from_value(capabilities.clone())
                .map_err(|e| format!("invalid capabilities: {e}"))?;
        }
        if let Some(device_context) = input.get("device_context") {
            manifest.device_context = serde_json::from_value(device_context.clone())
                .map_err(|e| format!("invalid device_context: {e}"))?;
        }
        manifest.validate().map_err(|e| e.to_string())?;
        // Schema changes against live data go through the SAME preview +
        // destructive-approval gate the pipeline used — an agent declaring a
        // narrower schema cannot silently drop user rows.
        crate::local_apps_build::migrate_manifest_with_approval(self, &layout, &manifest)
            .await
            .map_err(|e| e.to_string())?;
        local_apps::save_manifest(&layout, &manifest).map_err(|e| e.to_string())?;
        if !manifest
            .capabilities
            .contains(&AppCapability::BackgroundSchedule)
        {
            for outcome in self
                .cancel_background_tasks_for_revoked_schedule(
                    &app_id,
                    "background scheduling capability was removed from the app manifest",
                )
                .await?
            {
                self.emit_background_task_changed(&outcome).await;
            }
        }
        Ok(serde_json::json!({
            "ok": true,
            "app_id": app_id,
            "collections": manifest.collections.len(),
            "allowed_domains": manifest.allowed_domains,
            "capabilities": manifest.capabilities,
            "device_context": manifest.device_context,
        }))
    }

    async fn query_data(&self, input: Value) -> Result<Value, String> {
        self.query_data_value(input).await
    }

    async fn mutate_data(&self, input: Value) -> Result<Value, String> {
        self.mutate_data_value(input, true).await
    }

    async fn inspect_ui(&self, input: Value) -> Result<Value, String> {
        let app_id = required_string(&input, "app_id")?.to_string();
        self.service()?
            .record(&app_id)
            .await
            .map_err(|e| e.to_string())?;
        self.request_ui(AppUiRequestDto {
            request_id: self.request_id("app-ui"),
            app_id,
            action: AppUiActionKindDto::Inspect,
            target: input
                .get("selector")
                .and_then(Value::as_str)
                .map(|selector| AppUiTargetDto {
                    element_id: Some(selector.to_string()),
                    role: None,
                    name: None,
                }),
            value: None,
        })
        .await
    }

    async fn act_on_ui(&self, input: Value) -> Result<Value, String> {
        let app_id = required_string(&input, "app_id")?.to_string();
        self.service()?
            .record(&app_id)
            .await
            .map_err(|e| e.to_string())?;
        self.authorize_capability(
            &app_id,
            AppCapability::UiControl,
            AppCapabilityKindDto::UiControl,
            "The agent requested permission to control this app's visible interface.",
        )
        .await?;
        let action = match required_string(&input, "action")? {
            "click" => AppUiActionKindDto::Click,
            "fill" => AppUiActionKindDto::Fill,
            "select" => AppUiActionKindDto::Select,
            "toggle" => AppUiActionKindDto::Toggle,
            "scroll" => AppUiActionKindDto::Scroll,
            "navigate" => AppUiActionKindDto::Navigate,
            "back" => AppUiActionKindDto::Back,
            "reload" => AppUiActionKindDto::Reload,
            _ => return Err("unsupported structured UI action".into()),
        };
        let target = normalize_ui_target(input.get("target"))?;
        let value = input.get("value").map(|value| match value {
            Value::String(value) => value.clone(),
            value => value.to_string(),
        });
        self.request_ui(AppUiRequestDto {
            request_id: self.request_id("app-ui"),
            app_id,
            action,
            target,
            value,
        })
        .await
    }

    async fn restore_checkpoint(&self, input: Value) -> Result<Value, String> {
        self.restore_checkpoint_value(input).await
    }

    async fn read_app_events(&self, input: Value) -> Result<Value, String> {
        self.read_app_events_value(input).await
    }

    async fn read_agent_events(&self, input: Value) -> Result<Value, String> {
        self.read_agent_events_value(input).await
    }

    async fn background_schedule_value(&self, input: Value) -> Result<Value, String> {
        let app_id = required_string(&input, "app_id")?.to_string();
        let layout = self.layout(&app_id)?;
        let manifest = local_apps::load_manifest(&layout).map_err(|error| error.to_string())?;
        if !manifest
            .capabilities
            .contains(&AppCapability::BackgroundSchedule)
        {
            return Err("background scheduling is not declared in the app manifest".into());
        }
        self.authorize_capability(
            &app_id,
            AppCapability::BackgroundSchedule,
            AppCapabilityKindDto::BackgroundSchedule,
            "应用请求在系统后台按计划运行一个流程。",
        )
        .await?;
        let permissions =
            local_apps::load_permissions(&layout).map_err(|error| error.to_string())?;
        if !permissions.allows(AppCapability::BackgroundSchedule) {
            return Err("background scheduling requires durable approval".into());
        }
        let interval_ms = input
            .get("interval_ms")
            .or_else(|| input.get("intervalMs"))
            .and_then(Value::as_u64)
            .ok_or_else(|| "interval_ms must be an integer".to_string())?;
        if !(15 * 60 * 1_000..=30 * 24 * 60 * 60 * 1_000).contains(&interval_ms) {
            return Err("background interval must be between 15 minutes and 30 days".into());
        }
        let flow_value = input
            .get("flow")
            .cloned()
            .ok_or_else(|| "flow is required".to_string())?;
        let flow: local_apps::FlowDefinition = serde_json::from_value(flow_value)
            .map_err(|error| format!("invalid background flow: {error}"))?;
        let registry = local_apps::CapabilityRegistry::default();
        flow.validate(&registry)
            .map_err(|error| format!("invalid background flow: {error}"))?;
        for step in &flow.steps {
            if matches!(
                step.capability,
                local_apps::CapabilityId::BackgroundSchedule
            ) {
                return Err("background flows cannot schedule another background flow".into());
            }
            let descriptor = registry
                .get(step.capability)
                .ok_or_else(|| format!("unknown capability {}", step.capability.as_str()))?;
            if !local_apps::allowed_for_origin(
                local_apps::InvocationOrigin::SystemScheduler,
                descriptor,
            ) {
                return Err(format!(
                    "background flow cannot use interactive capability {}",
                    step.capability.as_str()
                ));
            }
            self.authorize_background_schedule_step(&app_id, step.capability, &step.input_json)
                .await?;
        }
        self.service()?
            .record(&app_id)
            .await
            .map_err(|e| e.to_string())?;
        let layout = self.layout(&app_id)?;
        let _process_lock = self.acquire_background_process_lock(&app_id).await?;
        let _guard = self.background_task_writes.lock().await;
        let mut tasks = local_apps::background::load_tasks(&layout).map_err(|e| e.to_string())?;
        let mut journal =
            local_apps::background::load_journal(&layout).map_err(|e| e.to_string())?;
        if tasks.len() >= background_ops::MAX_BACKGROUND_TASKS {
            let terminal_index = tasks
                .iter()
                .enumerate()
                .filter(|(_, task)| {
                    matches!(
                        task.status,
                        local_apps::BackgroundTaskStatus::Succeeded
                            | local_apps::BackgroundTaskStatus::Failed
                            | local_apps::BackgroundTaskStatus::Cancelled
                    )
                })
                .min_by_key(|(_, task)| task.updated_at_ms)
                .map(|(index, _)| index);
            let Some(terminal_index) = terminal_index else {
                return Err(format!(
                    "an app may have at most {} active background tasks",
                    background_ops::MAX_BACKGROUND_TASKS
                ));
            };
            let removed = tasks.remove(terminal_index);
            journal.retain(|entry| entry.task_id != removed.task_id);
        }
        let task_id = loop {
            let candidate = self.request_id("background");
            if tasks.iter().all(|task| task.task_id != candidate) {
                break candidate;
            }
        };
        let now = now_ms();
        let task = local_apps::BackgroundTaskRecord {
            schema_version: local_apps::RUNTIME_CONTRACT_SCHEMA_VERSION,
            task_id: task_id.clone(),
            app_id: app_id.clone(),
            flow_id: flow.flow_id.clone(),
            flow: flow.clone(),
            trigger: local_apps::BackgroundTrigger::Schedule { interval_ms },
            status: local_apps::BackgroundTaskStatus::Scheduled,
            updated_at_ms: now,
        };
        tasks.push(task.clone());
        journal.retain(|entry| entry.task_id != task_id);
        journal.push(local_apps::BackgroundJournalEntry {
            task_id: task_id.clone(),
            flow_id: flow.flow_id,
            next_step_id: flow.steps.first().map(|step| step.step_id.clone()),
            next_run_at_ms: Some(now.saturating_add(interval_ms)),
            last_result_json: None,
            attempt: 0,
            last_error: None,
            updated_at_ms: now,
        });
        local_apps::background::save_state(&layout, &tasks, &journal).map_err(|e| e.to_string())?;
        drop(_guard);
        drop(_process_lock);
        self.emit_background_task_changed(&LocalAppBackgroundRunDto {
            app_id: app_id.clone(),
            task_id: task_id.clone(),
            status: "scheduled".into(),
            result_json: None,
            error: None,
            retryable: false,
        })
        .await;
        Ok(json!({"task": task, "scheduled": true, "scheduler": "host-journal"}))
    }

    async fn background_list_value(&self, input: Value) -> Result<Value, String> {
        let app_id = required_string(&input, "app_id")?.to_string();
        self.service()?
            .record(&app_id)
            .await
            .map_err(|e| e.to_string())?;
        let layout = self.background_management_layout(&app_id)?;
        let task_id = input.get("task_id").and_then(Value::as_str);
        let status = input
            .get("status")
            .and_then(Value::as_str)
            .map(parse_background_status)
            .transpose()?;
        let limit = input
            .get("limit")
            .and_then(Value::as_u64)
            .unwrap_or(50)
            .clamp(1, 100) as usize;
        let tasks = Self::background_task_summaries(&layout, task_id, status, limit)?;
        let count = tasks.len();
        Ok(json!({"app_id": app_id, "tasks": tasks, "count": count}))
    }

    async fn background_status_value(&self, input: Value) -> Result<Value, String> {
        let app_id = required_string(&input, "app_id")?.to_string();
        let task_id = required_string(&input, "task_id")?.to_string();
        let value = self
            .background_list_value(json!({"app_id": app_id, "task_id": task_id, "limit": 1}))
            .await?;
        if value
            .get("tasks")
            .and_then(Value::as_array)
            .is_none_or(|tasks| tasks.is_empty())
        {
            return Err("background task was not found".into());
        }
        Ok(value)
    }

    async fn background_cancel_value(&self, input: Value) -> Result<Value, String> {
        let app_id = required_string(&input, "app_id")?.to_string();
        let task_id = required_string(&input, "task_id")?.to_string();
        self.service()?
            .record(&app_id)
            .await
            .map_err(|e| e.to_string())?;
        self.background_management_layout(&app_id)?;
        let cancelled = self.cancel_background_task(&app_id, &task_id).await;
        self.emit_background_task_changed(&LocalAppBackgroundRunDto {
            app_id: app_id.clone(),
            task_id: task_id.clone(),
            status: if cancelled { "cancelled" } else { "unchanged" }.into(),
            result_json: None,
            error: None,
            retryable: false,
        })
        .await;
        Ok(json!({"app_id": app_id, "task_id": task_id, "cancelled": cancelled}))
    }

    async fn background_retry_value(&self, input: Value) -> Result<Value, String> {
        let app_id = required_string(&input, "app_id")?.to_string();
        let task_id = required_string(&input, "task_id")?.to_string();
        self.service()?
            .record(&app_id)
            .await
            .map_err(|e| e.to_string())?;
        self.background_management_layout(&app_id)?;
        let retried = self.retry_background_task(&app_id, &task_id).await?;
        self.emit_background_task_changed(&LocalAppBackgroundRunDto {
            app_id: app_id.clone(),
            task_id: task_id.clone(),
            status: if retried { "scheduled" } else { "unchanged" }.into(),
            result_json: None,
            error: None,
            retryable: false,
        })
        .await;
        Ok(json!({"app_id": app_id, "task_id": task_id, "retried": retried}))
    }

    async fn agent_session_create(&self, input: Value) -> Result<Value, String> {
        self.agent_session_create_value(input).await
    }

    async fn agent_session_list(&self, input: Value) -> Result<Value, String> {
        self.agent_session_list_value(input).await
    }

    async fn agent_session_update(&self, input: Value) -> Result<Value, String> {
        self.agent_session_update_value(input).await
    }

    async fn agent_profile_propose(&self, input: Value) -> Result<Value, String> {
        self.agent_profile_propose_value(input).await
    }

    async fn flow_execute(&self, input: Value) -> Result<Value, String> {
        self.flow_execute_value(input).await
    }

    async fn background_schedule(&self, input: Value) -> Result<Value, String> {
        self.background_schedule_value(input).await
    }

    async fn background_list(&self, input: Value) -> Result<Value, String> {
        self.background_list_value(input).await
    }

    async fn background_status(&self, input: Value) -> Result<Value, String> {
        self.background_status_value(input).await
    }

    async fn background_cancel(&self, input: Value) -> Result<Value, String> {
        self.background_cancel_value(input).await
    }

    async fn background_retry(&self, input: Value) -> Result<Value, String> {
        self.background_retry_value(input).await
    }

    async fn scaffold_app(&self, record: local_apps::AppRecord) -> Result<(), String> {
        self.scaffold_app_value(&record).await
    }
}

fn required_string<'a>(input: &'a Value, key: &str) -> Result<&'a str, String> {
    input
        .get(key)
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| format!("missing non-empty {key:?}"))
}

fn parse_background_status(value: &str) -> Result<BackgroundTaskStatus, String> {
    match value {
        "scheduled" => Ok(BackgroundTaskStatus::Scheduled),
        "running" => Ok(BackgroundTaskStatus::Running),
        "waiting_for_system" => Ok(BackgroundTaskStatus::WaitingForSystem),
        "succeeded" => Ok(BackgroundTaskStatus::Succeeded),
        "failed" => Ok(BackgroundTaskStatus::Failed),
        "cancelled" => Ok(BackgroundTaskStatus::Cancelled),
        _ => Err(format!("unsupported background task status {value:?}")),
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis().min(u128::from(u64::MAX)) as u64)
        .unwrap_or(0)
}

fn raise_decision(decision: AppAuthorizationDecisionDto) -> PermissionDecision {
    match decision {
        AppAuthorizationDecisionDto::Deny => PermissionDecision::Deny,
        AppAuthorizationDecisionDto::AllowOnce => PermissionDecision::AllowOnce,
        AppAuthorizationDecisionDto::AllowSession => PermissionDecision::AllowSession,
        AppAuthorizationDecisionDto::AllowAlways => PermissionDecision::AlwaysAllow,
        _ => PermissionDecision::Deny,
    }
}

fn normalize_query(input: &Value) -> Result<DataQuery, String> {
    if input.get("cursor").is_some() {
        return Err("query cursor is unsupported; use numeric offset".into());
    }
    let collection = required_string(input, "collection")?.to_string();
    let filters = input
        .get("filters")
        .cloned()
        .or_else(|| input.get("filter").map(|value| json!([value])))
        .unwrap_or_else(|| json!([]));
    let sort_key = input
        .get("sort_key")
        .or_else(|| input.get("sortKey"))
        .cloned()
        .unwrap_or(Value::Null);
    let sort_direction = input
        .get("sort_direction")
        .or_else(|| input.get("sortDirection"))
        .cloned()
        .unwrap_or_else(|| Value::String("ascending".into()));
    let limit = match input.get("limit") {
        None => 50,
        Some(value) => value
            .as_u64()
            .ok_or_else(|| "query limit must be an integer".to_string())?,
    };
    if !(1..=100).contains(&limit) {
        return Err("query limit must be between 1 and 100".into());
    }
    let offset = match input.get("offset") {
        None => 0,
        Some(value) => value
            .as_u64()
            .ok_or_else(|| "query offset must be a non-negative integer".to_string())?,
    };
    let filters = serde_json::from_value(filters)
        .map_err(|error| format!("invalid structured data query: {error}"))?;
    let (sort_key, sort_direction) = normalize_sort(input, sort_key, sort_direction)?;
    Ok(DataQuery {
        collection,
        filters,
        sort_key,
        sort_direction,
        limit: limit as u32,
        offset,
    })
}

fn normalize_sort(
    input: &Value,
    legacy_key: Value,
    legacy_direction: Value,
) -> Result<(Option<DataSortKey>, DataSortDirection), String> {
    let mut sort_key = normalize_sort_key_value(&legacy_key)?;
    let mut sort_direction = normalize_sort_direction_value(&legacy_direction)?;
    if let Some(sort) = input.get("sort") {
        let (public_key, public_direction) = normalize_public_sort(sort)?;
        if let Some(public_key) = public_key {
            sort_key = Some(public_key);
        }
        if let Some(public_direction) = public_direction {
            sort_direction = public_direction;
        }
    }
    Ok((sort_key, sort_direction))
}

fn normalize_public_sort(
    sort: &Value,
) -> Result<(Option<DataSortKey>, Option<DataSortDirection>), String> {
    match sort {
        Value::Null => Ok((None, None)),
        Value::String(_) => Ok((normalize_sort_key_value(sort)?, None)),
        Value::Object(object) => {
            let direction = object
                .get("direction")
                .or_else(|| object.get("sort_direction"))
                .or_else(|| object.get("sortDirection"))
                .map(normalize_sort_direction_value)
                .transpose()?;
            let key = if let Some(key) = object
                .get("key")
                .or_else(|| object.get("sort_key"))
                .or_else(|| object.get("sortKey"))
            {
                normalize_sort_key_value(key)?
            } else if object.contains_key("kind")
                || object.contains_key("field_id")
                || object.contains_key("fieldId")
            {
                Some(normalize_sort_key_object(object)?)
            } else {
                None
            };
            Ok((key, direction))
        }
        _ => Err("query sort must be a string, object, or null".into()),
    }
}

fn normalize_sort_key_value(value: &Value) -> Result<Option<DataSortKey>, String> {
    match value {
        Value::Null => Ok(None),
        Value::String(value) => Ok(Some(normalize_sort_key_string(value)?)),
        Value::Object(object) => Ok(Some(normalize_sort_key_object(object)?)),
        _ => Err("query sort key must be a string, object, or null".into()),
    }
}

fn normalize_sort_key_object(object: &Map<String, Value>) -> Result<DataSortKey, String> {
    if let Some(field_id) = object
        .get("field_id")
        .or_else(|| object.get("fieldId"))
        .and_then(Value::as_str)
    {
        return Ok(DataSortKey::Field(field_id.to_string()));
    }
    let Some(kind) = object.get("kind").and_then(Value::as_str) else {
        return Err("query sort object must include key/kind or field_id".into());
    };
    match normalize_sort_alias(kind).as_str() {
        "field" => {
            let field_id = object
                .get("field_id")
                .or_else(|| object.get("fieldId"))
                .and_then(Value::as_str)
                .ok_or_else(|| "field sort requires field_id".to_string())?;
            Ok(DataSortKey::Field(field_id.to_string()))
        }
        "record_id" => Ok(DataSortKey::RecordId),
        "created_at" => Ok(DataSortKey::CreatedAt),
        "updated_at" => Ok(DataSortKey::UpdatedAt),
        "revision" => Ok(DataSortKey::Revision),
        other => Err(format!("unsupported sort kind {other:?}")),
    }
}

fn normalize_sort_key_string(value: &str) -> Result<DataSortKey, String> {
    let value = value.trim();
    if value.is_empty() {
        return Err("query sort key must not be empty".into());
    }
    Ok(match normalize_sort_alias(value).as_str() {
        "record_id" => DataSortKey::RecordId,
        "created_at" => DataSortKey::CreatedAt,
        "updated_at" => DataSortKey::UpdatedAt,
        "revision" => DataSortKey::Revision,
        _ => DataSortKey::Field(value.to_string()),
    })
}

fn normalize_sort_alias(value: &str) -> String {
    value
        .trim()
        .replace('-', "_")
        .chars()
        .fold(String::new(), |mut normalized, ch| {
            if ch.is_uppercase() && !normalized.is_empty() {
                normalized.push('_');
            }
            normalized.push(ch.to_ascii_lowercase());
            normalized
        })
}

fn normalize_sort_direction_value(value: &Value) -> Result<DataSortDirection, String> {
    let Some(value) = value.as_str() else {
        return Err("query sort direction must be a string".into());
    };
    match normalize_sort_alias(value).as_str() {
        "ascending" | "asc" => Ok(DataSortDirection::Ascending),
        "descending" | "desc" => Ok(DataSortDirection::Descending),
        other => Err(format!("unsupported sort direction {other:?}")),
    }
}

fn normalize_ui_target(target: Option<&Value>) -> Result<Option<AppUiTargetDto>, String> {
    let Some(target) = target else {
        return Ok(None);
    };
    match target {
        Value::Null => Ok(None),
        Value::String(target) => Ok(Some(AppUiTargetDto {
            element_id: Some(target.to_string()),
            role: None,
            name: None,
        })),
        Value::Object(object) => {
            let target = AppUiTargetDto {
                element_id: object
                    .get("element_id")
                    .or_else(|| object.get("elementId"))
                    .and_then(Value::as_str)
                    .map(ToOwned::to_owned),
                role: object
                    .get("role")
                    .and_then(Value::as_str)
                    .map(ToOwned::to_owned),
                name: object
                    .get("name")
                    .and_then(Value::as_str)
                    .map(ToOwned::to_owned),
            };
            if target.element_id.is_none() && target.role.is_none() && target.name.is_none() {
                return Err("UI target object must include element_id, role, or name".into());
            }
            Ok(Some(target))
        }
        _ => Err("UI target must be a string, object, or null".into()),
    }
}

fn manifest_migration_reason(preview: &DataMigrationPreview) -> String {
    let from = preview
        .from_manifest_hash
        .as_deref()
        .map(short_hash)
        .unwrap_or("none");
    let reasons = preview.reasons.join("; ");
    format!(
        "Approve destructive app data migration for this exact migration attempt only. Current manifest: {from}; proposed manifest: {}. Effects: {reasons}",
        short_hash(&preview.to_manifest_hash)
    )
}

fn short_hash(hash: &str) -> &str {
    &hash[..hash.len().min(12)]
}

async fn read_limited_stream<S, C, E>(
    mut stream: S,
    limit: usize,
    read_error_context: &str,
    limit_error: &str,
) -> Result<Vec<u8>, String>
where
    S: futures_util::stream::Stream<Item = Result<C, E>> + Unpin,
    C: AsRef<[u8]>,
    E: std::fmt::Display,
{
    let mut body = Vec::new();
    let mut total = 0usize;
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|error| format!("{read_error_context}: {error}"))?;
        let chunk = chunk.as_ref();
        total = total.saturating_add(chunk.len());
        if total > limit {
            return Err(limit_error.to_string());
        }
        body.extend_from_slice(chunk);
    }
    Ok(body)
}

fn normalize_mutations(input: &Value) -> Result<Vec<DataMutation>, String> {
    let collection = required_string(input, "collection")?;
    let operations = input
        .get("operations")
        .and_then(Value::as_array)
        .ok_or_else(|| "mutations require an operations array".to_string())?;
    let mut normalized = Vec::with_capacity(operations.len());
    for operation in operations {
        let mut operation = operation
            .as_object()
            .cloned()
            .ok_or_else(|| "each mutation must be an object".to_string())?;
        operation.insert("collection".into(), Value::String(collection.to_string()));
        normalized.push(
            serde_json::from_value(Value::Object(operation))
                .map_err(|error| format!("invalid structured mutation: {error}"))?,
        );
    }
    Ok(normalized)
}

/// Bind the app's loopback listener: the port it was already given if it has
/// one, otherwise a fresh port derived from its id.
///
/// WHY there is no fallback when `assigned` is taken.  An app's port is
/// PERMANENT by design one layer down: `AppState::set_runtime`
/// (`local-apps/src/state.rs`, "a port is NEVER reassigned — `IndexedDB` origin
/// stability") rejects any request that carries a different port than the one
/// already recorded, because the WebView loads the app from
/// `http://127.0.0.1:<port>` and every store the app owns client-side
/// (`IndexedDB`, `localStorage`, cookies, service-worker registration) is keyed
/// by that ORIGIN — a port that moved between starts would silently orphan the
/// app's own data.  Handing back a different port here would therefore not
/// rescue the start: `update_runtime_record` two calls later refuses the
/// reassignment and the start fails anyway, one message further along, with a
/// listener bound to a port nothing will ever use.
/// `the_pinned_app_port_can_never_be_reassigned` pins that invariant so this
/// reasoning goes red rather than stale if the pin is ever lifted.
///
/// WHY the window is 20000..32000.  A port that can never be reassigned must be
/// drawn from a range nobody else is ever GIVEN, so the window sits below every
/// shipped platform's ephemeral floor — Linux/Android
/// `net.ipv4.ip_local_port_range` starts at 32768, iOS/macOS
/// `net.inet.ip.portrange.first` at 49152.  Below those floors the kernel can
/// never hand an app's permanent port to some other process's socket while the
/// app is stopped.  The previous window (30000..50000) sat inside Android's
/// ephemeral range across 86% of its span and inside iOS's across its top 848
/// ports, which made the OS itself the likeliest squatter of a port that, by
/// design, the app can never give up.
///
/// WHY a sibling app's pin also excludes a candidate.  The window slot is a
/// hash of the app id, so two ids in ONE profile can derive the same slot —
/// `6b4cb242` and `c3baea9e` both derive 30809, and the birthday rate over
/// 12000 slots is ~1.3% at 20 apps.  A bind probe cannot see that collision:
/// a pin outlives the runtime that made it (`stop_runtime` re-passes
/// `runtime.port`), so a STOPPED sibling's permanent port probes free and
/// would be pinned a second time.  After that neither app can start while the
/// other runs — `set_runtime` refuses to move either — and on Android the two
/// share one `http://127.0.0.1:<port>` origin, hence one `localStorage` /
/// `IndexedDB` store, because the Android WebView is built on the default
/// profile with no per-app data store (iOS partitions by app id and is not
/// exposed to that half).  `sibling_pinned_ports` is therefore consulted
/// alongside the probe.  It PREVENTS new collisions only: a pair that already
/// collided is permanent on both sides, so the `assigned` branch can do
/// nothing but name the sibling instead of reporting a bare bind failure.
///
/// WHY a lease is taken as well.  A pin only reaches the records after the
/// choice — see [`PortLeases`] — so `sibling_pins` cannot describe a sibling
/// that is choosing right now.  Choice and reservation are therefore ONE step
/// here: the lease is taken before the probe, which is what makes a candidate
/// visible to every concurrent allocator from the instant it is picked.  The
/// returned guard belongs to the CALLER, which must hold it until the port is
/// persisted and then `commit` it.
///
/// WHY the pins are then read a SECOND time, from `service`, for the candidate
/// the lease was just taken on.  `sibling_pins` is a SNAPSHOT the caller read
/// before this call; a sibling can persist its pin and drop its lease in the
/// interval between that read and the take, and a candidate caught mid-hand-off
/// like that appears in neither half of "pins UNION leases" (see
/// [`PortLeases`]).  Re-reading after the take is what removes the interval:
/// `PortLease::commit` runs only after the persist, so once we hold the lease
/// on a port, a sibling that could have owned it either still holds its own
/// lease — and then our take returned `None` and we never got here — or has
/// already made its pin readable.  A sibling cannot newly take the port either,
/// because we hold it.  The re-read costs one `AppService` state lock per
/// candidate actually leased, which is one per start in the ordinary case.
///
/// The `assigned` branch takes no lease and needs no re-read: an assigned port
/// is by definition already persisted, so every sibling's pin snapshot has it.
async fn bind_stable_loopback(
    app_id: &str,
    assigned: Option<u16>,
    sibling_pins: &[(String, u16)],
    leases: &PortLeases,
    service: &AppService,
) -> Result<(TcpListener, u16, Option<PortLease>), String> {
    if let Some(port) = assigned {
        return match TcpListener::bind(("127.0.0.1", port)).await {
            Ok(listener) => Ok((listener, port, None)),
            Err(error) => Err(
                match sibling_pins.iter().find(|(_, pinned)| *pinned == port) {
                    Some((sibling, _)) => format!(
                        "stable app port {port} is unavailable: app {sibling} is pinned to the same port and is holding it. \
                         A pinned port can never be reassigned, so only one of the two apps can run; \
                         recreate one of them to mint a fresh port ({error})"
                    ),
                    None => format!("stable app port {port} is unavailable: {error}"),
                },
            ),
        };
    }
    let first = derived_window_slot(app_id);
    for offset in 0..256u16 {
        let port = APP_PORT_WINDOW_FIRST + (first + offset) % APP_PORT_WINDOW_LEN;
        // A sibling's pin outlives its runtime, so a candidate that binds
        // cleanly can still be owned forever by an app that is merely stopped.
        if sibling_pins.iter().any(|(_, pinned)| *pinned == port) {
            continue;
        }
        // Taken BEFORE the probe, so a sibling that reaches this candidate
        // finds it occupied even though nothing is persisted and — on the full
        // runtime, once the probe below is released — nothing is bound either.
        // The lock is dropped by `take` itself and never spans the await.
        let Some(lease) = PortLease::take(leases, app_id, port) else {
            continue;
        };
        // Now that the candidate cannot move again, ask the records once more.
        // `sibling_pins` was read before this call and a sibling's pin may have
        // landed since; because a lease outlives its own persist, holding this
        // one makes the answer stable rather than merely fresher.  Order is the
        // point — a re-read BEFORE the take would reproduce the same interval
        // it is here to remove.
        if sibling_pinned_ports(service, app_id)
            .await
            .iter()
            .any(|(_, pinned)| *pinned == port)
        {
            drop(lease);
            continue;
        }
        if let Ok(listener) = TcpListener::bind(("127.0.0.1", port)).await {
            return Ok((listener, port, Some(lease)));
        }
        // Refused by the kernel: hand the candidate straight back rather than
        // holding it for the rest of this scan.
        drop(lease);
    }
    Err("no stable loopback port is available for the app".into())
}

/// Offset into the derived window an app's FIRST port candidate sits at.
///
/// Extracted so the collision regression probe uses the production derivation
/// rather than a copy of it — a copy would keep asserting itself after the
/// real derivation moved.
fn derived_window_slot(app_id: &str) -> u16 {
    let hash = app_id.bytes().fold(0u32, |hash, byte| {
        hash.wrapping_mul(16_777_619) ^ u32::from(byte)
    });
    u16::try_from(hash % u32::from(APP_PORT_WINDOW_LEN)).unwrap_or(0)
}

/// Ports every OTHER app in this profile has already pinned, each paired with
/// its owner.
///
/// Read on demand rather than kept as a registry: the records ARE the
/// registry, and a cached copy would be one more thing to invalidate on
/// create/delete. The service returns the record/runtime pair from one
/// in-memory state-lock pass.
///
/// Twice per start in the ordinary case, not once — the snapshot the caller
/// reads before `bind_stable_loopback` cannot be trusted to still be true when
/// a candidate is leased, so the leased candidate is re-checked against a fresh
/// read.  Every result is a snapshot; only one taken while the port in question
/// is leased says anything durable about it.
async fn sibling_pinned_ports(service: &AppService, app_id: &str) -> Vec<(String, u16)> {
    service.pinned_runtime_ports_except(app_id).await
}

/// Returns `None` on the requested shutdown, and `Some(detail)` when the
/// listener has stopped being usable at all — the caller must then retire the
/// runtime entry, because a static handle has nothing else to notice it.
async fn run_static_server(
    listener: TcpListener,
    root: PathBuf,
    mut shutdown: oneshot::Receiver<()>,
) -> Option<String> {
    let mut consecutive_errors = 0u32;
    let request_slots = Arc::new(Semaphore::new(STATIC_REQUEST_CONCURRENCY));
    loop {
        let slot = tokio::select! {
            _ = &mut shutdown => return None,
            acquired = Arc::clone(&request_slots).acquire_owned() => match acquired {
                Ok(slot) => slot,
                Err(_) => return Some("static app server request limiter closed".into()),
            },
        };
        let accepted = tokio::select! {
            _ = &mut shutdown => return None,
            accepted = listener.accept() => accepted,
        };
        match accepted {
            Ok((stream, _)) => {
                consecutive_errors = 0;
                let root = root.clone();
                tokio::spawn(async move {
                    let _slot = slot;
                    let _ = serve_static_request(stream, &root).await;
                });
            }
            Err(error) => {
                drop(slot);
                // ECONNABORTED / EMFILE / EINTR describe ONE would-be
                // connection, not the listener, so a single error must not
                // retire the loop.  A listener whose runtime's I/O driver
                // was dropped fails EVERY poll though, and retrying that
                // forever is a permanent busy loop behind an entry that
                // still reports `running`.
                consecutive_errors += 1;
                if consecutive_errors >= STATIC_ACCEPT_ERROR_LIMIT {
                    return Some(format!(
                        "static app server stopped accepting connections after {consecutive_errors} consecutive failures: {error}"
                    ));
                }
                sleep(STATIC_ACCEPT_RETRY).await;
            }
        }
    }
}

/// Drop the entry this dead server owns and fail the record, so the next start
/// is legal instead of short-circuiting on a stale `Running`.
async fn reconcile_static_runtime_exit(
    runtimes: Arc<Mutex<HashMap<String, RuntimeEntry>>>,
    service: Arc<AppService>,
    app_id: String,
    generation: u64,
    detail: String,
    broker: std::sync::Weak<LocalAppsHostBroker>,
) {
    let removed = {
        let mut runtimes = runtimes.lock().await;
        let should_remove = runtimes.get(&app_id).is_some_and(|entry| {
            entry.generation == generation
                && matches!(
                    entry.state,
                    RuntimeEntryState::Running {
                        handle: RuntimeHandle::Static { .. }
                    }
                )
        });
        if should_remove {
            runtimes.remove(&app_id);
        }
        should_remove
    };
    if !removed {
        return;
    }
    // The listener retired on its own: reclaim what the runtime owned before
    // recording the stop, so a page that was mid-recording does not leave the
    // audio session held by nothing.
    if let Some(broker) = broker.upgrade() {
        broker.release_app_runtime_state(&app_id).await;
    }
    if let Ok(record) = service.runtime_record(&app_id).await {
        let _ = service
            .update_runtime_record(
                &app_id,
                AppRuntimeState::Failed,
                record.port,
                record.pid,
                Some(detail),
            )
            .await;
    }
}

async fn serve_static_request(mut stream: TcpStream, root: &Path) -> Result<(), std::io::Error> {
    let mut request = vec![0u8; MAX_HTTP_REQUEST_BYTES];
    let count = stream.read(&mut request).await?;
    request.truncate(count);
    let request_text = String::from_utf8_lossy(&request);
    let line = request_text.lines().next().unwrap_or_default().to_string();
    let if_none_match = request_text
        .lines()
        .find_map(|line| {
            line.split_once(':')
                .filter(|(name, _)| name.eq_ignore_ascii_case("if-none-match"))
        })
        .map(|(_, value)| value.trim().to_string());
    let mut parts = line.split_whitespace();
    let method = parts.next().unwrap_or_default();
    let raw_path = parts.next().unwrap_or_default();
    if !matches!(method, "GET" | "HEAD") {
        return write_http(
            &mut stream,
            405,
            "text/plain",
            b"method not allowed",
            method == "HEAD",
        )
        .await;
    }
    let Some(relative) = safe_static_path(raw_path) else {
        return write_http(
            &mut stream,
            400,
            "text/plain",
            b"bad path",
            method == "HEAD",
        )
        .await;
    };
    let mut path = root.join(relative);
    if path.is_dir() {
        path.push("index.html");
    }
    let file = match tokio::fs::File::open(&path).await {
        Ok(file) => file,
        Err(_) => {
            return write_http(
                &mut stream,
                404,
                "text/plain",
                b"not found",
                method == "HEAD",
            )
            .await;
        }
    };
    let metadata = match file.metadata().await {
        Ok(metadata) if metadata.is_file() && metadata.len() <= MAX_STATIC_ASSET_BYTES => metadata,
        _ => {
            return write_http(
                &mut stream,
                404,
                "text/plain",
                b"not found",
                method == "HEAD",
            )
            .await;
        }
    };
    let served_relative = path.strip_prefix(root).unwrap_or(&path);
    let etag = static_etag(&path, &metadata);
    if if_none_match
        .as_deref()
        .is_some_and(|header| etag_matches(header, &etag))
    {
        return write_not_modified(&mut stream, &etag, static_cache_control(served_relative)).await;
    }
    write_static_file(
        &mut stream,
        file,
        metadata.len(),
        content_type(&path),
        static_cache_control(served_relative),
        &etag,
        method == "HEAD",
    )
    .await
}

fn static_cache_control(path: &Path) -> &'static str {
    if is_hashed_asset(path) {
        "public, max-age=31536000, immutable"
    } else if path.file_name().and_then(|name| name.to_str()) == Some("index.html") {
        "no-cache"
    } else {
        "no-store"
    }
}

fn is_hashed_asset(path: &Path) -> bool {
    let mut components = path.components();
    if !matches!(
        components.next(),
        Some(std::path::Component::Normal(component)) if component == "assets"
    ) {
        return false;
    }
    let Some(file_name) = path.file_stem().and_then(|name| name.to_str()) else {
        return false;
    };
    let Some(hash) = file_name.rsplit('-').next() else {
        return false;
    };
    hash.len() >= 8 && hash.bytes().all(|byte| byte.is_ascii_hexdigit())
}

async fn write_static_file(
    stream: &mut TcpStream,
    mut file: tokio::fs::File,
    content_length: u64,
    content_type: &str,
    cache_control: &str,
    etag: &str,
    head: bool,
) -> Result<(), std::io::Error> {
    let header =
        static_response_header(200, content_type, content_length, cache_control, Some(etag));
    stream.write_all(header.as_bytes()).await?;
    if !head {
        let mut buffer = vec![0u8; STATIC_ASSET_CHUNK_BYTES];
        loop {
            let count = file.read(&mut buffer).await?;
            if count == 0 {
                break;
            }
            stream.write_all(&buffer[..count]).await?;
        }
    }
    stream.shutdown().await
}

fn etag_matches(header: &str, current: &str) -> bool {
    header.split(',').any(|candidate| {
        let candidate = candidate.trim();
        candidate == "*" || candidate.strip_prefix("W/").unwrap_or(candidate) == current
    })
}

async fn write_not_modified(
    stream: &mut TcpStream,
    etag: &str,
    cache_control: &str,
) -> Result<(), std::io::Error> {
    let header = static_response_header(304, "text/plain", 0, cache_control, Some(etag));
    stream.write_all(header.as_bytes()).await?;
    stream.shutdown().await
}

fn safe_static_path(raw: &str) -> Option<PathBuf> {
    let raw = raw.split(['?', '#']).next()?;
    if !raw.starts_with('/') || raw.contains('%') || raw.contains('\\') {
        return None;
    }
    let relative = raw.trim_start_matches('/');
    let relative = if relative.is_empty() {
        "index.html"
    } else {
        relative
    };
    let path = Path::new(relative);
    if path
        .components()
        .any(|component| !matches!(component, std::path::Component::Normal(_)))
    {
        return None;
    }
    Some(path.to_path_buf())
}

async fn write_http(
    stream: &mut TcpStream,
    status: u16,
    content_type: &str,
    body: &[u8],
    head: bool,
) -> Result<(), std::io::Error> {
    let header = static_response_header(status, content_type, body.len() as u64, "no-store", None);
    stream.write_all(header.as_bytes()).await?;
    if !head {
        stream.write_all(body).await?;
    }
    stream.shutdown().await
}

fn static_response_header(
    status: u16,
    content_type: &str,
    content_length: u64,
    cache_control: &str,
    etag: Option<&str>,
) -> String {
    let reason = match status {
        200 => "OK",
        304 => "Not Modified",
        400 => "Bad Request",
        404 => "Not Found",
        405 => "Method Not Allowed",
        _ => "Error",
    };
    let etag_header = etag
        .map(|value| format!("ETag: {value}\r\n"))
        .unwrap_or_default();
    format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Length: {content_length}\r\nContent-Type: {content_type}\r\nContent-Security-Policy: {LOCAL_APP_CONTENT_SECURITY_POLICY}\r\nReferrer-Policy: no-referrer\r\nX-Content-Type-Options: nosniff\r\nX-Frame-Options: DENY\r\nCache-Control: {cache_control}\r\n{etag_header}Connection: close\r\n\r\n"
    )
}

fn static_etag(path: &Path, metadata: &std::fs::Metadata) -> String {
    let modified_ns = metadata
        .modified()
        .ok()
        .and_then(|value| value.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|value| value.as_nanos())
        .unwrap_or_default();
    let path_digest = Sha256::digest(path.to_string_lossy().as_bytes());
    format!("\"{}-{}-{:x}\"", modified_ns, metadata.len(), path_digest)
}

fn content_type(path: &Path) -> &'static str {
    match path.extension().and_then(|extension| extension.to_str()) {
        Some("html") => "text/html; charset=utf-8",
        Some("js" | "mjs") => "text/javascript; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("json") => "application/json; charset=utf-8",
        Some("svg") => "image/svg+xml",
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("webp") => "image/webp",
        Some("woff2") => "font/woff2",
        _ => "application/octet-stream",
    }
}

fn validate_dependency_tree(root: &Path) -> Result<(), String> {
    let metadata = std::fs::symlink_metadata(root)
        .map_err(|error| format!("inspect dependency tree {}: {error}", root.display()))?;
    if metadata.file_type().is_symlink() {
        return Err(format!(
            "dependency tree contains a symlink: {}",
            root.display()
        ));
    }
    if metadata.is_file() {
        return Ok(());
    }
    if !metadata.is_dir() {
        return Err(format!(
            "dependency tree entry is not regular: {}",
            root.display()
        ));
    }
    for entry in std::fs::read_dir(root)
        .map_err(|error| format!("read dependency tree {}: {error}", root.display()))?
    {
        let entry = entry.map_err(|error| format!("read dependency tree entry: {error}"))?;
        validate_dependency_tree(&entry.path())?;
    }
    Ok(())
}

fn dependency_attestation(lock_digest: &str, tree_digest: &str) -> String {
    format!("{DEPENDENCY_SNAPSHOT_VERSION}\n{lock_digest}\n{PNPM_TOOLCHAIN_KEY}\n{tree_digest}\n")
}

fn dependency_tree_digest_from_marker(marker: &Path) -> Result<Option<String>, String> {
    let contents = match std::fs::read_to_string(marker) {
        Ok(contents) => contents,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(format!(
                "read dependency marker {}: {error}",
                marker.display()
            ))
        }
    };
    let lines: Vec<&str> = contents.lines().collect();
    if lines.len() != 4 || lines[3].is_empty() {
        return Ok(None);
    }
    Ok(Some(lines[3].to_string()))
}

fn dependency_tree_digest(root: &Path) -> Result<String, String> {
    let mut files = Vec::new();
    collect_dependency_files(root, Path::new(""), &mut files)?;
    files.sort_by(|left, right| left.0.cmp(&right.0));
    let mut digest = Sha256::new();
    digest.update((files.len() as u64).to_le_bytes());
    for (relative, path) in files {
        let relative = relative.as_bytes();
        let bytes = std::fs::read(&path)
            .map_err(|error| format!("read dependency tree file {}: {error}", path.display()))?;
        digest.update((relative.len() as u64).to_le_bytes());
        digest.update(relative);
        digest.update((bytes.len() as u64).to_le_bytes());
        digest.update(bytes);
    }
    Ok(format!("{:x}", digest.finalize()))
}

fn collect_dependency_files(
    root: &Path,
    relative: &Path,
    files: &mut Vec<(String, PathBuf)>,
) -> Result<(), String> {
    let metadata = std::fs::symlink_metadata(root)
        .map_err(|error| format!("inspect dependency tree {}: {error}", root.display()))?;
    if metadata.file_type().is_symlink() {
        return Err(format!(
            "dependency tree contains a symlink: {}",
            root.display()
        ));
    }
    if metadata.is_file() {
        files.push((relative.to_string_lossy().into_owned(), root.to_path_buf()));
        return Ok(());
    }
    if !metadata.is_dir() {
        return Err(format!(
            "dependency tree entry is not regular: {}",
            root.display()
        ));
    }
    for entry in std::fs::read_dir(root)
        .map_err(|error| format!("read dependency tree {}: {error}", root.display()))?
    {
        let entry = entry.map_err(|error| format!("read dependency tree entry: {error}"))?;
        let child_relative = if relative.as_os_str().is_empty() {
            PathBuf::from(entry.file_name())
        } else {
            relative.join(entry.file_name())
        };
        collect_dependency_files(&entry.path(), &child_relative, files)?;
    }
    Ok(())
}

fn make_dependency_files_read_only(root: &Path) -> io::Result<()> {
    let metadata = std::fs::symlink_metadata(root)?;
    if metadata.file_type().is_symlink() {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!("dependency tree contains a symlink: {}", root.display()),
        ));
    }
    if metadata.is_file() {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut permissions = metadata.permissions();
            permissions.set_mode(permissions.mode() & !0o222);
            std::fs::set_permissions(root, permissions)?;
        }
        return Ok(());
    }
    for entry in std::fs::read_dir(root)? {
        make_dependency_files_read_only(&entry?.path())?;
    }
    Ok(())
}

fn clone_or_copy_tree(source: &Path, destination: &Path) -> io::Result<()> {
    let metadata = std::fs::symlink_metadata(source)?;
    if metadata.file_type().is_symlink() {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!(
                "dependency source symlink is forbidden: {}",
                source.display()
            ),
        ));
    }
    if metadata.is_file() {
        if let Some(parent) = destination.parent() {
            std::fs::create_dir_all(parent)?;
        }
        return std::fs::copy(source, destination).map(|_| ());
    }
    if !metadata.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("dependency source is not regular: {}", source.display()),
        ));
    }
    if try_clone_tree(source, destination).is_ok() {
        return Ok(());
    }
    let _ = std::fs::remove_dir_all(destination);
    std::fs::create_dir_all(destination)?;
    copy_dependency_tree(source, destination)
}

fn copy_dependency_tree(source: &Path, destination: &Path) -> io::Result<()> {
    for entry in std::fs::read_dir(source)? {
        let entry = entry?;
        let source_path = entry.path();
        let destination_path = destination.join(entry.file_name());
        let metadata = std::fs::symlink_metadata(&source_path)?;
        if metadata.file_type().is_symlink() {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                format!(
                    "dependency source symlink is forbidden: {}",
                    source_path.display()
                ),
            ));
        }
        if metadata.is_dir() {
            std::fs::create_dir_all(&destination_path)?;
            copy_dependency_tree(&source_path, &destination_path)?;
        } else if metadata.is_file() {
            std::fs::copy(&source_path, &destination_path)?;
        } else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "dependency source is not regular: {}",
                    source_path.display()
                ),
            ));
        }
    }
    Ok(())
}

fn try_clone_tree(source: &Path, destination: &Path) -> io::Result<()> {
    use std::process::{Command, Stdio};

    let mut command = Command::new("cp");
    command.stdout(Stdio::null()).stderr(Stdio::null());
    #[cfg(target_os = "macos")]
    command.args(["-R", "-c"]);
    #[cfg(any(target_os = "linux", target_os = "android"))]
    command.args(["-R", "--reflink=always"]);
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "android")))]
    return Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "copy-on-write clone is unavailable on this platform",
    ));
    command.arg("--").arg(source).arg(destination);
    let status = command.status()?;
    if !status.success() {
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            format!("copy-on-write clone exited with {status}"),
        ));
    }
    validate_dependency_tree(destination).map_err(io::Error::other)
}

fn public_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => {
            let octets = ip.octets();
            !(ip.is_private()
                || ip.is_loopback()
                || ip.is_link_local()
                || ip.is_broadcast()
                || ip.is_documentation()
                || ip.is_unspecified()
                || octets[0] == 0
                // Shared address space (CGNAT), protocol assignments,
                // deprecated 6to4 relay anycast, and benchmark networks must
                // not become SSRF paths into carrier/device infrastructure.
                || (octets[0] == 100 && (octets[1] & 0xc0) == 0x40)
                || (octets[0] == 192 && octets[1] == 0 && octets[2] == 0)
                || (octets[0] == 192 && octets[1] == 88 && octets[2] == 99)
                || (octets[0] == 198 && (octets[1] == 18 || octets[1] == 19))
                || octets[0] >= 224)
        }
        IpAddr::V6(ip) => {
            if let Some(mapped) = ip.to_ipv4_mapped() {
                return public_ip(IpAddr::V4(mapped));
            }
            let segments = ip.segments();
            let first = segments[0];
            !(ip.is_loopback()
                || ip.is_unspecified()
                || (first & 0xfe00) == 0xfc00
                || (first & 0xffc0) == 0xfe80
                || (first & 0xff00) == 0xff00
                // Discard-only prefix, NAT64 well-known prefixes and
                // documentation addresses are never valid public targets.
                || (first == 0x0100 && segments[1..].iter().all(|part| *part == 0))
                || (segments[0] == 0x0064
                    && segments[1] == 0xff9b
                    && (segments[2] == 0 || segments[2] == 1))
                || (segments[0] == 0x2001 && segments[1] == 0x0db8))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use client_adapter::{ClientEventSink, MockSink};
    use futures_util::stream;
    use local_apps::test_support::FixedClock;
    use local_apps::{storage, AppState, NoopAppEventObserver};
    use serde_json::json;
    use std::fs;
    use std::future::Future;
    use std::sync::atomic::{AtomicBool, AtomicUsize};
    use tempfile::TempDir;
    use traits::{
        LinuxCommandRequest, LinuxEnforcementReceipt, LinuxProcessHandle, MobileLinuxCapability,
        MobileLinuxError, MobileLinuxRuntimeMode, MobileLinuxTaskSnapshot, MobileLinuxTaskStatus,
        NetworkPolicy, PtyOpenRequest, PtySessionHandle, PtySize, RootfsState, RootfsStatus,
        SandboxBackend,
    };

    #[derive(Default)]
    struct NoopClientEventSink;

    #[async_trait]
    impl ClientEventSink for NoopClientEventSink {
        async fn emit(&self, _event: ClientEvent) {}
    }

    struct MockTask {
        snapshot: Mutex<MobileLinuxTaskSnapshot>,
        shutdown: Mutex<Option<oneshot::Sender<()>>>,
    }

    struct MockMobileLinuxRuntime {
        spawn_delay: Duration,
        spawn_count: AtomicUsize,
        next_task_id: AtomicU64,
        tasks: Mutex<HashMap<String, Arc<MockTask>>>,
        last_request: Mutex<Option<LinuxCommandRequest>>,
        enforcement_receipt: AtomicBool,
        fail_kill: AtomicBool,
    }

    impl MockMobileLinuxRuntime {
        fn new(spawn_delay: Duration) -> Arc<Self> {
            Arc::new(Self {
                spawn_delay,
                spawn_count: AtomicUsize::new(0),
                next_task_id: AtomicU64::new(1),
                tasks: Mutex::new(HashMap::new()),
                last_request: Mutex::new(None),
                enforcement_receipt: AtomicBool::new(true),
                fail_kill: AtomicBool::new(false),
            })
        }

        fn set_fail_kill(&self, fail: bool) {
            self.fail_kill.store(fail, Ordering::SeqCst);
        }

        fn set_enforcement_receipt(&self, enforced: bool) {
            self.enforcement_receipt.store(enforced, Ordering::SeqCst);
        }

        async fn recorded_request(&self) -> LinuxCommandRequest {
            self.last_request
                .lock()
                .await
                .clone()
                .expect("spawn request recorded")
        }

        fn enforce_network_policy(request: &LinuxCommandRequest) -> Result<(), MobileLinuxError> {
            if matches!(request.network, NetworkPolicy::LoopbackOnly)
                && request.resource_limits.max_memory_mb == Some(800)
            {
                Ok(())
            } else {
                Err(MobileLinuxError::InvalidRequest(
                    "full local-app runtime requires loopback-only networking and 800 MiB".into(),
                ))
            }
        }

        fn spawn_count(&self) -> usize {
            self.spawn_count.load(Ordering::SeqCst)
        }

        async fn first_task_id(&self) -> String {
            timeout(Duration::from_secs(2), async {
                loop {
                    if let Some(task_id) = self.tasks.lock().await.keys().next().cloned() {
                        return task_id;
                    }
                    sleep(Duration::from_millis(10)).await;
                }
            })
            .await
            .expect("task created")
        }

        async fn complete_task(&self, task_id: &str, status: MobileLinuxTaskStatus, detail: &str) {
            let task = self
                .tasks
                .lock()
                .await
                .get(task_id)
                .cloned()
                .expect("task exists");
            {
                let mut snapshot = task.snapshot.lock().await;
                snapshot.status = status;
                snapshot.finished_at_ms = Some(2);
                snapshot.detail = Some(detail.to_string());
                snapshot.exit_code = Some(match status {
                    MobileLinuxTaskStatus::Completed => 0,
                    _ => 1,
                });
            }
            let shutdown = { task.shutdown.lock().await.take() };
            if let Some(shutdown) = shutdown {
                let _ = shutdown.send(());
            }
        }
    }

    #[async_trait]
    impl MobileLinuxRuntime for MockMobileLinuxRuntime {
        fn backend(&self) -> SandboxBackend {
            SandboxBackend::IosIsh
        }

        fn mode(&self) -> MobileLinuxRuntimeMode {
            MobileLinuxRuntimeMode::MobileLinux
        }

        async fn probe_capability(&self) -> MobileLinuxCapability {
            MobileLinuxCapability {
                available: true,
                backend: self.backend(),
                mode: self.mode(),
                reason: None,
                streaming_output: false,
                background_processes: true,
                pty: false,
                bind_mounts: true,
                rootfs_integrity: false,
            }
        }

        async fn boot(&self) -> Result<RootfsStatus, MobileLinuxError> {
            Ok(self.rootfs_status().await?)
        }

        async fn shutdown(&self) -> Result<(), MobileLinuxError> {
            Ok(())
        }

        async fn run(
            &self,
            request: LinuxCommandRequest,
        ) -> Result<traits::LinuxCommandResult, MobileLinuxError> {
            Self::enforce_network_policy(&request)?;
            Err(MobileLinuxError::Unsupported)
        }

        async fn spawn_background(
            &self,
            request: LinuxCommandRequest,
        ) -> Result<LinuxProcessHandle, MobileLinuxError> {
            Self::enforce_network_policy(&request)?;
            *self.last_request.lock().await = Some(request.clone());
            self.spawn_count.fetch_add(1, Ordering::SeqCst);
            if !self.spawn_delay.is_zero() {
                sleep(self.spawn_delay).await;
            }
            let port = request
                .args
                .windows(2)
                .find_map(|window| (window[0] == "--port").then(|| window[1].parse::<u16>().ok()))
                .flatten()
                .ok_or_else(|| MobileLinuxError::InvalidRequest("missing --port".into()))?;
            let listener = TcpListener::bind(("127.0.0.1", port))
                .await
                .map_err(|error| {
                    MobileLinuxError::Io(format!("bind test runtime loopback: {error}"))
                })?;
            let (shutdown, mut receiver) = oneshot::channel();
            tokio::spawn(async move {
                loop {
                    tokio::select! {
                        _ = &mut receiver => break,
                        accepted = listener.accept() => {
                            match accepted {
                                Ok((mut stream, _)) => {
                                    let _ = stream.shutdown().await;
                                }
                                Err(_) => break,
                            }
                        }
                    }
                }
            });
            let task_id = format!("task-{}", self.next_task_id.fetch_add(1, Ordering::SeqCst));
            self.tasks.lock().await.insert(
                task_id.clone(),
                Arc::new(MockTask {
                    snapshot: Mutex::new(MobileLinuxTaskSnapshot {
                        task_id: task_id.clone(),
                        status: MobileLinuxTaskStatus::Backgrounded,
                        command: request.command,
                        started_at_ms: Some(1),
                        finished_at_ms: None,
                        exit_code: None,
                        detail: None,
                    }),
                    shutdown: Mutex::new(Some(shutdown)),
                }),
            );
            Ok(LinuxProcessHandle {
                id: task_id,
                enforcement: LinuxEnforcementReceipt {
                    network_policy_enforced: self.enforcement_receipt.load(Ordering::SeqCst),
                    memory_limit_enforced: self.enforcement_receipt.load(Ordering::SeqCst),
                },
            })
        }

        async fn kill(&self, handle: &LinuxProcessHandle) -> Result<(), MobileLinuxError> {
            if let Some(task) = self.tasks.lock().await.get(&handle.id).cloned() {
                {
                    let mut snapshot = task.snapshot.lock().await;
                    snapshot.status = MobileLinuxTaskStatus::Cancelled;
                    snapshot.finished_at_ms = Some(2);
                    snapshot.exit_code = Some(1);
                    snapshot.detail = Some("killed".into());
                }
                if let Some(shutdown) = task.shutdown.lock().await.take() {
                    let _ = shutdown.send(());
                }
            }
            if self.fail_kill.load(Ordering::SeqCst) {
                // Models the iSH `BACKGROUND_REAP_BUDGET` miss: the kill was
                // issued, only the exit confirmation timed out.
                return Err(MobileLinuxError::Io(format!(
                    "background task {} did not reap within 3 seconds",
                    handle.id
                )));
            }
            Ok(())
        }

        async fn open_pty(
            &self,
            _request: PtyOpenRequest,
        ) -> Result<PtySessionHandle, MobileLinuxError> {
            Err(MobileLinuxError::Unsupported)
        }

        async fn write_pty(
            &self,
            _handle: &PtySessionHandle,
            _input: Vec<u8>,
        ) -> Result<(), MobileLinuxError> {
            Err(MobileLinuxError::Unsupported)
        }

        async fn resize_pty(
            &self,
            _handle: &PtySessionHandle,
            _size: PtySize,
        ) -> Result<(), MobileLinuxError> {
            Err(MobileLinuxError::Unsupported)
        }

        async fn close_pty(&self, _handle: &PtySessionHandle) -> Result<(), MobileLinuxError> {
            Err(MobileLinuxError::Unsupported)
        }

        async fn rootfs_status(&self) -> Result<RootfsStatus, MobileLinuxError> {
            Ok(RootfsStatus {
                state: RootfsState::Ready,
                backend: self.backend(),
                mode: self.mode(),
                platform: "test".into(),
                abi: "test".into(),
                version: None,
                managed_root: None,
                active_root: None,
                staged_root: None,
                archive_sha256: None,
                installed_size_bytes: None,
                writable_guest_paths: vec![],
                last_error: None,
            })
        }

        async fn verify_rootfs(&self) -> Result<RootfsStatus, MobileLinuxError> {
            self.rootfs_status().await
        }

        async fn repair_rootfs(&self) -> Result<RootfsStatus, MobileLinuxError> {
            self.rootfs_status().await
        }

        async fn reset_rootfs(&self) -> Result<RootfsStatus, MobileLinuxError> {
            self.rootfs_status().await
        }

        async fn configure_mounts(&self, _mounts: Vec<MountSpec>) -> Result<(), MobileLinuxError> {
            Ok(())
        }

        async fn list_tasks(&self) -> Result<Vec<MobileLinuxTaskSnapshot>, MobileLinuxError> {
            let tasks = self.tasks.lock().await;
            let mut snapshots = Vec::with_capacity(tasks.len());
            for task in tasks.values() {
                snapshots.push(task.snapshot.lock().await.clone());
            }
            Ok(snapshots)
        }

        async fn task_status(
            &self,
            task_id: &str,
        ) -> Result<Option<MobileLinuxTaskSnapshot>, MobileLinuxError> {
            let task = self.tasks.lock().await.get(task_id).cloned();
            Ok(match task {
                Some(task) => Some(task.snapshot.lock().await.clone()),
                None => None,
            })
        }
    }

    async fn test_service(root: &TempDir) -> Arc<AppService> {
        Arc::new(
            AppService::load(
                root.path(),
                Arc::new(FixedClock::new(1)),
                Arc::new(NoopAppEventObserver),
            )
            .await
            .expect("load app service"),
        )
    }

    fn create_configured_runtime_root(root: &TempDir) -> PathBuf {
        let runtime_root = root.path().join("runtime-root");
        fs::create_dir_all(&runtime_root).expect("create runtime root");
        runtime_root
    }

    fn create_configured_digest_runtime_root(root: &TempDir) -> PathBuf {
        let runtime_container = root.path().join("runtime-root");
        fs::create_dir_all(&runtime_container).expect("create runtime container");
        let runtime_root = runtime_container
            .join("0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef");
        fs::create_dir_all(&runtime_root).expect("create digest runtime root");
        runtime_root
    }

    fn write_runtime_seed_ready_marker(runtime_root: &Path) {
        let digest = runtime_root
            .file_name()
            .and_then(|leaf| leaf.to_str())
            .expect("digest runtime root leaf");
        let marker = runtime_root
            .parent()
            .expect("digest runtime root parent")
            .join(format!(".{digest}.ready"));
        fs::write(marker, digest).expect("write ready marker");
    }

    fn create_runtime_root(root: &TempDir) -> PathBuf {
        let runtime_root = create_configured_runtime_root(root);
        let vite_bin = runtime_root.join("node_modules/vite/bin/vite.js");
        fs::create_dir_all(vite_bin.parent().unwrap()).expect("create Vite runtime root");
        fs::write(&vite_bin, b"#!/usr/bin/env node\n").expect("write Vite bin");
        runtime_root
    }

    #[tokio::test]
    async fn await_fixed_runtime_root_waits_for_a_configured_seed_to_finish_staging() {
        let root = TempDir::new().expect("tempdir");
        let runtime_root = create_configured_digest_runtime_root(&root);
        let broker = LocalAppsHostBroker::new(
            root.path().to_path_buf(),
            Arc::new(NoopClientEventSink),
            None,
            false,
            Some(runtime_root.clone()),
        );
        let runtime_root_for_seed = runtime_root.clone();
        tokio::spawn(async move {
            sleep(Duration::from_millis(150)).await;
            let vite_bin = runtime_root_for_seed.join("node_modules/vite/bin/vite.js");
            fs::create_dir_all(vite_bin.parent().expect("vite bin parent"))
                .expect("create staged runtime root");
            fs::write(&vite_bin, b"#!/usr/bin/env node\n").expect("write staged Vite bin");
            write_runtime_seed_ready_marker(&runtime_root_for_seed);
        });

        let ready = broker
            .await_fixed_runtime_root(Duration::from_secs(1))
            .await
            .expect("wait for runtime seed");

        assert_eq!(ready, runtime_root);
    }

    #[tokio::test]
    async fn await_fixed_runtime_root_times_out_when_the_seed_never_becomes_ready() {
        let root = TempDir::new().expect("tempdir");
        let runtime_root = create_configured_digest_runtime_root(&root);
        let broker = LocalAppsHostBroker::new(
            root.path().to_path_buf(),
            Arc::new(NoopClientEventSink),
            None,
            false,
            Some(runtime_root.clone()),
        );

        let error = broker
            .await_fixed_runtime_root(Duration::from_millis(250))
            .await
            .expect_err("unready runtime seed must time out");

        assert!(error.contains("runtime root is configured at"), "{error}");
        assert!(error.contains("waited 250 ms"), "{error}");
    }

    #[tokio::test]
    async fn await_fixed_runtime_root_accepts_an_immutable_bundle_root_without_a_ready_marker() {
        let root = TempDir::new().expect("tempdir");
        let runtime_root = create_runtime_root(&root);
        let broker = LocalAppsHostBroker::new(
            root.path().to_path_buf(),
            Arc::new(NoopClientEventSink),
            None,
            false,
            Some(runtime_root.clone()),
        );

        let ready = broker
            .await_fixed_runtime_root(Duration::from_millis(50))
            .await
            .expect("bundle root should stay ready without a marker");

        assert_eq!(ready, runtime_root);
    }

    #[tokio::test]
    async fn await_fixed_runtime_root_fails_fast_when_staging_wrote_a_failure_marker() {
        let root = TempDir::new().expect("tempdir");
        let runtime_root = create_configured_digest_runtime_root(&root);
        let failure_marker = runtime_root
            .parent()
            .expect("runtime root parent")
            .join(".0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef.failed");
        fs::write(
            &failure_marker,
            "runtime seed inventory validation failed before publish",
        )
        .expect("write failure marker");
        let broker = LocalAppsHostBroker::new(
            root.path().to_path_buf(),
            Arc::new(NoopClientEventSink),
            None,
            false,
            Some(runtime_root),
        );

        let started = tokio::time::Instant::now();
        let error = broker
            .await_fixed_runtime_root(Duration::from_secs(1))
            .await
            .expect_err("failure marker must fail fast");

        assert!(
            started.elapsed() < Duration::from_millis(500),
            "failure marker should stop polling early",
        );
        assert!(
            error.contains("runtime seed inventory validation failed before publish"),
            "{error}"
        );
    }

    #[tokio::test]
    async fn await_fixed_runtime_root_rejects_a_corrupt_ready_marker() {
        let root = TempDir::new().expect("tempdir");
        let runtime_root = create_configured_digest_runtime_root(&root);
        let vite_bin = runtime_root.join("node_modules/vite/bin/vite.js");
        fs::create_dir_all(vite_bin.parent().expect("vite bin parent"))
            .expect("create staged runtime root");
        fs::write(&vite_bin, b"#!/usr/bin/env node\n").expect("write staged Vite bin");
        let marker = runtime_root
            .parent()
            .expect("runtime root parent")
            .join(".0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef.ready");
        fs::write(&marker, "wrong-digest").expect("write corrupt ready marker");
        let broker = LocalAppsHostBroker::new(
            root.path().to_path_buf(),
            Arc::new(NoopClientEventSink),
            None,
            false,
            Some(runtime_root),
        );

        let error = broker
            .await_fixed_runtime_root(Duration::from_millis(50))
            .await
            .expect_err("corrupt ready marker must fail");

        assert!(
            error.contains("must contain exactly its digest leaf"),
            "{error}"
        );
    }

    async fn create_broker(
        full_runtime: bool,
        mobile_linux: Option<Arc<dyn MobileLinuxRuntime>>,
    ) -> (TempDir, Arc<AppService>, Arc<LocalAppsHostBroker>) {
        create_broker_over(TempDir::new().expect("tempdir"), full_runtime, mobile_linux).await
    }

    /// [`create_broker`] over a root somebody else prepared — the seam
    /// [`seed_app_fixture`] needs, because a seeded app has to be on disk
    /// BEFORE the service loads it.
    async fn create_broker_over(
        root: TempDir,
        full_runtime: bool,
        mobile_linux: Option<Arc<dyn MobileLinuxRuntime>>,
    ) -> (TempDir, Arc<AppService>, Arc<LocalAppsHostBroker>) {
        let service = test_service(&root).await;
        let runtime_root = full_runtime.then(|| create_runtime_root(&root));
        let broker = LocalAppsHostBroker::new(
            root.path().to_path_buf(),
            Arc::new(NoopClientEventSink),
            mobile_linux,
            full_runtime,
            runtime_root,
        );
        assert!(broker.attach_service(service.clone()).is_ok());
        (root, service, broker)
    }

    async fn create_app_fixture(root: &TempDir, service: &Arc<AppService>, name: &str) -> String {
        let record = service
            .create_app(Some(name), "a test app", None)
            .await
            .expect("create app");
        let layout = AppLayout::new(root.path().to_path_buf(), record.id.clone()).expect("layout");
        let static_dist = root
            .path()
            .join(layout.build_rel(false))
            .join(crate::local_apps_build::VITE_OUTPUT_DIR);
        fs::create_dir_all(&static_dist).expect("create static dist");
        fs::write(static_dist.join("index.html"), "<html>ok</html>").expect("write index.html");
        let full_build = root.path().join(layout.build_rel(true));
        fs::create_dir_all(&full_build).expect("create full build");
        let workspace = root.path().join(layout.workspace_rel());
        fs::write(workspace.join("vite.config.mjs"), "export default {};")
            .expect("mark fixture as a Vite app");
        record.id
    }

    async fn scaffolded_lingxi(
        full_runtime: bool,
        mobile_linux: Option<Arc<dyn MobileLinuxRuntime>>,
    ) -> (String, String) {
        let (root, service, broker) = create_broker(full_runtime, mobile_linux).await;
        let record = service
            .create_app(Some("Tracker"), "a test app", None)
            .await
            .expect("create app");
        broker
            .scaffold_app_value(&record)
            .await
            .expect("scaffold app");
        let layout = AppLayout::new(root.path().to_path_buf(), record.id.clone()).expect("layout");
        let lingxi =
            std::fs::read_to_string(root.path().join(layout.workspace_rel()).join("LINGXI.md"))
                .expect("read LINGXI.md");
        (record.id, lingxi)
    }

    #[tokio::test]
    async fn scaffold_writes_capability_neutral_lingxi_when_toolchain_is_available() {
        let runtime = MockMobileLinuxRuntime::new(Duration::ZERO);
        let (_app_id, lingxi) = scaffolded_lingxi(true, Some(runtime)).await;
        assert!(lingxi.contains("Do not run `npm create vite`"), "{lingxi}");
        // The contract is READ BY A MODEL as a set of examples to copy. An
        // unsubstituted placeholder or a doubled brace is a malformed call the
        // agent will faithfully reproduce, get a schema error from, and then
        // start improvising around — which is exactly the build flailing this
        // file exists to prevent. `build_preview` is a plain `&str`, so its
        // braces were never processed by the enclosing `format!`.
        // Catch ANY `{ident}` placeholder, not just `{id}` — a future fragment
        // added as a plain `&str` would leak `{name}`/`{brief}` the same way.
        // JSON examples in the contract are `{"key":...}`, so requiring a bare
        // lower-snake identifier between the braces does not false-positive.
        let leaked: Vec<&str> = lingxi
            .match_indices('{')
            .filter_map(|(start, _)| {
                let rest = &lingxi[start + 1..];
                let end = rest.find('}')?;
                let inner = &rest[..end];
                (!inner.is_empty()
                    && inner
                        .bytes()
                        .all(|b| b.is_ascii_lowercase() || b == b'_'))
                .then_some(&lingxi[start..start + end + 2])
            })
            .collect();
        assert!(
            leaked.is_empty(),
            "unsubstituted placeholder(s) {leaked:?} in the agent contract: {lingxi}"
        );
        assert!(
            !lingxi.contains("{{") && !lingxi.contains("}}"),
            "doubled braces leaked into the agent contract: {lingxi}"
        );
        assert!(
            lingxi.contains(&format!("LocalAppBuild {{\"app_id\":\"{_app_id}\"}}")),
            "the build example must carry this app's real id: {lingxi}"
        );
        // A failed build is the exact moment the agent goes off-script. The
        // contract must name the recovery path AND forbid improvising an
        // alternate build command — there is no second build path here.
        assert!(
            lingxi.contains("do NOT try a different build command"),
            "{lingxi}"
        );
        assert!(
            lingxi.contains("LocalAppInstallDeps"),
            "dependency state is the most common build failure; the tool that \
             reports it must be named: {lingxi}"
        );
        // Verification tools were documented only in the skill, so an agent
        // working from the workspace had to rediscover them.
        assert!(lingxi.contains("LocalAppQueryData"), "{lingxi}");
        assert!(lingxi.contains("LocalAppInspectUi"), "{lingxi}");
        assert!(
            lingxi.contains("do not run a package manager in this local-app workspace"),
            "{lingxi}"
        );
        assert!(
            lingxi
                .contains("Host-managed files are `.gitignore`, `package.json`, `pnpm-lock.yaml`"),
            "{lingxi}"
        );
        assert!(
            lingxi.contains("sole writable `LocalAppBuild` root"),
            "{lingxi}"
        );
        assert!(
            lingxi.contains("directly from this workspace as the sole writable mount"),
            "{lingxi}"
        );
        assert!(!lingxi.contains("isolated workspace mount"), "{lingxi}");
        assert!(lingxi.contains("`build/store/dist/`"), "{lingxi}");
    }

    #[tokio::test]
    async fn scaffold_writes_capability_neutral_lingxi_when_shell_is_missing() {
        let (_app_id, lingxi) = scaffolded_lingxi(true, None).await;
        assert!(
            lingxi.contains("repository-verified Vite + Tailwind + shadcn/ui foundation"),
            "{lingxi}"
        );
        assert!(lingxi.contains("Do not run `npm create vite`"), "{lingxi}");
        assert!(lingxi.contains("do not run a package manager in this local-app workspace"));
        assert!(!lingxi.contains("vite-fallback"), "{lingxi}");
    }

    #[tokio::test]
    async fn persisted_lingxi_does_not_bake_in_toolchain_availability() {
        let runtime = MockMobileLinuxRuntime::new(Duration::ZERO);
        let (root, service, broker) = create_broker(true, Some(runtime)).await;
        let record = service
            .create_app(Some("Tracker"), "a test app", None)
            .await
            .expect("create app");
        broker
            .scaffold_app_value(&record)
            .await
            .expect("scaffold app");
        let layout = AppLayout::new(root.path().to_path_buf(), record.id).expect("layout");
        let lingxi =
            std::fs::read_to_string(root.path().join(layout.workspace_rel()).join("LINGXI.md"))
                .expect("read LINGXI.md");

        assert!(lingxi.contains("do not run a package manager in this local-app workspace"));
        assert!(broker
            .create_next_step()
            .contains("Do not recreate the app scaffold"));
        assert!(broker.create_next_step().contains("pnpm install"));
    }

    /// `create_app_fixture` with a CHOSEN id, for the one test whose exercised
    /// PORT is derived from the app id.
    ///
    /// `AppService::create_app` mints a random id by design and offers no way
    /// to supply one, so an app created through it makes
    /// `derived_window_slot` land somewhere different on every run: six
    /// consecutive runs of the test below probed 26208, 20613, 26371, 24337,
    /// 30061 and 27589.  Every one passed, which is the problem — the run that
    /// eventually does not cannot be repeated.
    ///
    /// The app documents come from `save_app_files`, the same `storage` writer
    /// `create_app` commits, and the service then LOADS them, so the RECORD
    /// the start path below sees is the one a process restart would hand it.
    /// That is the whole of the fidelity claim, and two things sit outside it.
    ///
    /// The index goes through `save_index`, which REPLACES `apps/index.json`
    /// wholesale, not `create_app`'s locked, merging `save_index_preserving` —
    /// hence two constraints: run this BEFORE the service loads the root, and
    /// only once per root.
    ///
    /// And `create_app` also writes `manifest.json` and `permissions.json`
    /// (`save_manifest` / `save_permissions`, neither of which lives in
    /// `storage`) while this writes neither.  `load_permissions` returns the
    /// deny-by-default state when its file is absent, but `load_manifest`
    /// returns `NotFound`, so a seeded app cannot stand in for a created one on
    /// a manifest-reading path — the bridge and capability handlers above.
    fn seed_app_fixture(root: &TempDir, app_id: &str, name: &str) {
        let app = AppState::create(
            app_id.to_string(),
            name.to_string(),
            "a test app".to_string(),
            None,
            1,
        );
        let layout = AppLayout::new(root.path().to_path_buf(), app_id.to_string()).expect("layout");
        layout.initialize().expect("initialize the seeded layout");
        storage::save_app_files(root.path(), &app).expect("persist the seeded app documents");
        local_apps::save_manifest(
            &layout,
            &local_apps::AppManifest::for_new_app(app.record.id.clone(), app.record.name.clone()),
        )
        .expect("persist the seeded manifest");
        local_apps::save_permissions(&layout, &local_apps::AppPermissions::default())
            .expect("persist the seeded permissions");
        local_apps::save_workspace_permission_settings(&layout)
            .expect("persist the seeded workspace permissions");
        storage::save_index(root.path(), std::slice::from_ref(&app.record))
            .expect("persist the seeded index");
        let static_dist = root
            .path()
            .join(layout.build_rel(false))
            .join(crate::local_apps_build::VITE_OUTPUT_DIR);
        fs::create_dir_all(&static_dist).expect("create static dist");
        fs::write(static_dist.join("index.html"), "<html>ok</html>").expect("write index.html");
    }

    /// A registry of its own for a probe that drives `bind_stable_loopback`
    /// directly.  Production hands it the BROKER's — see
    /// `a_first_start_skips_a_port_a_concurrent_start_has_leased`, which is
    /// what holds that wiring in place.
    fn test_leases() -> PortLeases {
        Arc::new(std::sync::Mutex::new(HashMap::new()))
    }

    /// An `AppService` holding no apps, for the probes that drive
    /// `bind_stable_loopback` directly and hand it their sibling pins by hand.
    ///
    /// The allocator re-reads the pins from this service after leasing a
    /// candidate, so an EMPTY one is what keeps those probes saying what their
    /// names say: the re-read contributes no exclusion, leaving the passed
    /// snapshot as the only one in play.  A probe that wants the re-read itself
    /// seeds a real pin instead — see
    /// `a_pin_that_lands_after_the_snapshot_is_caught_before_the_choice_sticks`.
    ///
    /// The returned `TempDir` has to outlive the service; binding it to `_`
    /// drops it immediately and pulls the app root out from under the load.
    async fn empty_registry() -> (TempDir, Arc<AppService>) {
        let root = TempDir::new().expect("tempdir");
        let service = test_service(&root).await;
        (root, service)
    }

    async fn wait_until<F, Fut>(label: &str, timeout_duration: Duration, mut condition: F)
    where
        F: FnMut() -> Fut,
        Fut: Future<Output = bool>,
    {
        timeout(timeout_duration, async {
            loop {
                if condition().await {
                    return;
                }
                sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .unwrap_or_else(|_| panic!("timed out waiting for {label}"));
    }

    #[test]
    fn static_paths_reject_traversal_and_encoding() {
        assert_eq!(safe_static_path("/"), Some(PathBuf::from("index.html")));
        assert_eq!(
            safe_static_path("/assets/app.js?v=1"),
            Some(PathBuf::from("assets/app.js"))
        );
        assert_eq!(safe_static_path("/../secret"), None);
        assert_eq!(safe_static_path("/%2e%2e/secret"), None);
        assert_eq!(safe_static_path("/assets\\secret"), None);
    }

    #[test]
    fn static_assets_use_safe_cache_policy() {
        assert_eq!(
            static_cache_control(Path::new("assets/index-0123abcd.js")),
            "public, max-age=31536000, immutable"
        );
        assert_eq!(static_cache_control(Path::new("index.html")), "no-cache");
        assert_eq!(
            static_cache_control(Path::new("assets/runtime.js")),
            "no-store"
        );
        assert!(is_hashed_asset(Path::new(
            "assets/nested/chunk-deadbeef.css"
        )));
        assert!(!is_hashed_asset(Path::new("assets/chunk-short.js")));
    }

    #[test]
    fn if_none_match_supports_weak_lists_and_wildcard() {
        assert!(etag_matches("W/\"abc\", \"def\"", "\"def\""));
        assert!(etag_matches("*", "\"anything\""));
        assert!(!etag_matches("\"old\"", "\"new\""));
    }

    #[test]
    fn dependency_snapshot_is_atomic_and_reusable() {
        let root = TempDir::new().expect("tempdir");
        let source = root.path().join("install/node_modules");
        fs::create_dir_all(source.join("vite/bin")).expect("source tree");
        fs::write(source.join("vite/bin/vite.js"), b"vite").expect("vite marker");
        fs::write(source.join("react.js"), b"react").expect("dependency");
        let snapshot = root.path().join("cache/snapshot");
        LocalAppsHostBroker::publish_dependency_snapshot(&source, &snapshot, "lock-digest")
            .expect("publish snapshot");
        assert!(
            LocalAppsHostBroker::dependency_snapshot_is_ready(&snapshot, "lock-digest")
                .expect("validate snapshot")
        );

        let staging = root.path().join("staging");
        fs::create_dir_all(&staging).expect("staging");
        LocalAppsHostBroker::materialize_dependency_snapshot(&snapshot, &staging)
            .expect("materialize snapshot");
        assert_eq!(
            fs::read(staging.join("node_modules/react.js")).expect("materialized dependency"),
            b"react"
        );

        let workspace = root.path().join("workspace");
        LocalAppsHostBroker::materialize_dependency_snapshot(&snapshot, &workspace)
            .expect("materialize workspace dependencies");
        let tree_digest = dependency_tree_digest(&workspace.join("node_modules"))
            .expect("workspace dependency digest");
        let attestation_path = workspace.join(WORKSPACE_DEPENDENCY_ATTESTATION_FILE);
        fs::create_dir_all(attestation_path.parent().expect("attestation parent"))
            .expect("attestation directory");
        fs::write(
            &attestation_path,
            dependency_attestation("lock-digest", &tree_digest),
        )
        .expect("workspace attestation");
        assert!(LocalAppsHostBroker::workspace_dependencies_match_snapshot(
            &workspace,
            &snapshot,
            "lock-digest",
        )
        .expect("match attested workspace dependencies"));
    }

    #[test]
    fn dependency_tree_digest_frames_file_boundaries() {
        let root = TempDir::new().expect("tempdir");
        let first = root.path().join("first");
        let second = root.path().join("second");
        fs::create_dir_all(&first).expect("first tree");
        fs::create_dir_all(&second).expect("second tree");
        fs::write(first.join("a"), b"bc").expect("first file");
        fs::write(first.join("d"), b"X").expect("first boundary file");
        fs::write(second.join("a"), b"b").expect("second file");
        fs::write(second.join("cd"), b"X").expect("second boundary file");

        assert_ne!(
            dependency_tree_digest(&first).expect("first digest"),
            dependency_tree_digest(&second).expect("second digest"),
            "path/content boundaries must be unambiguous",
        );
    }

    #[cfg(unix)]
    #[test]
    fn dependency_snapshot_rejects_symlink_entries() {
        let root = TempDir::new().expect("tempdir");
        let source = root.path().join("node_modules");
        fs::create_dir_all(&source).expect("source tree");
        fs::write(root.path().join("outside"), b"outside").expect("outside");
        std::os::unix::fs::symlink(root.path().join("outside"), source.join("escape"))
            .expect("symlink");
        let error = validate_dependency_tree(&source).expect_err("symlink must be rejected");
        assert!(error.contains("symlink"), "{error}");
    }

    #[test]
    fn static_csp_allows_native_media_payloads_but_disables_workers() {
        assert!(LOCAL_APP_CONTENT_SECURITY_POLICY.contains("media-src 'self' data: blob:"));
        assert!(LOCAL_APP_CONTENT_SECURITY_POLICY.contains("worker-src 'none'"));
    }

    #[test]
    fn network_bridge_rejects_local_addresses() {
        assert!(!public_ip("127.0.0.1".parse().unwrap()));
        assert!(!public_ip("10.0.0.1".parse().unwrap()));
        assert!(!public_ip("100.64.0.1".parse().unwrap()));
        assert!(!public_ip("198.18.0.1".parse().unwrap()));
        assert!(!public_ip("224.0.0.1".parse().unwrap()));
        assert!(!public_ip("::1".parse().unwrap()));
        assert!(!public_ip("::ffff:127.0.0.1".parse().unwrap()));
        assert!(!public_ip("64:ff9b::7f00:1".parse().unwrap()));
        assert!(!public_ip("ff02::1".parse().unwrap()));
        assert!(public_ip("1.1.1.1".parse().unwrap()));
        assert!(public_ip("2606:4700:4700::1111".parse().unwrap()));
    }

    #[test]
    fn normalize_query_accepts_numeric_offset_and_sort_aliases() {
        let query = normalize_query(&json!({
            "collection": "items",
            "sort": {
                "kind": "field",
                "field_id": "score",
                "direction": "desc"
            },
            "offset": 7
        }))
        .unwrap();
        assert_eq!(query.collection, "items");
        assert_eq!(query.sort_key, Some(DataSortKey::Field("score".into())));
        assert_eq!(query.sort_direction, DataSortDirection::Descending);
        assert_eq!(query.offset, 7);

        let legacy = normalize_query(&json!({
            "collection": "items",
            "sortKey": "updatedAt",
            "sortDirection": "ascending"
        }))
        .unwrap();
        assert_eq!(legacy.sort_key, Some(DataSortKey::UpdatedAt));
        assert_eq!(legacy.sort_direction, DataSortDirection::Ascending);
    }

    #[test]
    fn normalize_data_wire_accepts_canonical_shapes_and_rejects_guesses() {
        let query = normalize_query(&json!({
            "collection": "items",
            "filters": [{
                "fieldId": "score",
                "operator": "greater_than",
                "value": 10
            }]
        }))
        .expect("canonical query filter");
        assert_eq!(query.filters[0].field_id, "score");
        assert_eq!(
            query.filters[0].operator,
            local_apps::DataFilterOperator::GreaterThan
        );

        let mutations = normalize_mutations(&json!({
            "collection": "items",
            "operations": [{
                "kind": "upsert",
                "recordId": "best",
                "document": {"score": 42}
            }, {
                "kind": "delete",
                "recordId": "old",
                "expectedRevision": 2
            }]
        }))
        .expect("canonical tagged mutations");
        assert_eq!(mutations.len(), 2);
        assert!(matches!(mutations[0], DataMutation::Upsert { .. }));
        assert!(matches!(mutations[1], DataMutation::Delete { .. }));

        let guessed = normalize_mutations(&json!({
            "collection": "items",
            "operations": [{"action": "create", "record": {"score": 42}}]
        }))
        .expect_err("the broken generated shape must stay invalid");
        assert!(guessed.contains("missing field `kind`"), "{guessed}");
    }

    #[test]
    fn normalize_query_rejects_cursor_and_invalid_page_bounds() {
        assert_eq!(
            normalize_query(&json!({"collection": "items", "cursor": "7"})).unwrap_err(),
            "query cursor is unsupported; use numeric offset"
        );
        assert_eq!(
            normalize_query(&json!({"collection": "items", "offset": -1})).unwrap_err(),
            "query offset must be a non-negative integer"
        );
        assert_eq!(
            normalize_query(&json!({"collection": "items", "limit": 101})).unwrap_err(),
            "query limit must be between 1 and 100"
        );
    }

    #[test]
    fn normalize_ui_target_accepts_string_and_structured_object() {
        assert_eq!(
            normalize_ui_target(Some(&json!("submit-button"))).unwrap(),
            Some(AppUiTargetDto {
                element_id: Some("submit-button".into()),
                role: None,
                name: None,
            })
        );
        assert_eq!(
            normalize_ui_target(Some(&json!({
                "role": "button",
                "name": "Save"
            })))
            .unwrap(),
            Some(AppUiTargetDto {
                element_id: None,
                role: Some("button".into()),
                name: Some("Save".into()),
            })
        );
    }

    #[tokio::test]
    async fn response_limit_is_enforced_while_streaming() {
        let chunks = vec![
            Ok::<Vec<u8>, &'static str>(vec![0; 1024 * 1024]),
            Ok::<Vec<u8>, &'static str>(vec![0; 1024 * 1024]),
            Ok::<Vec<u8>, &'static str>(vec![1]),
        ];
        let error = read_limited_stream(
            stream::iter(chunks),
            MAX_NETWORK_RESPONSE_BYTES,
            "read network response",
            "network response exceeds 2 MiB",
        )
        .await
        .unwrap_err();
        assert_eq!(error, "network response exceeds 2 MiB");
    }

    #[tokio::test]
    async fn concurrent_starts_reuse_one_static_runtime() {
        let runtime = MockMobileLinuxRuntime::new(Duration::from_millis(40));
        let (root, service, broker) = create_broker(true, Some(runtime.clone())).await;
        let app_id = create_app_fixture(&root, &service, "Concurrent").await;

        let (first, second, third) = tokio::join!(
            broker.manage_runtime_value(json!({"app_id": app_id, "action": "start"})),
            broker.manage_runtime_value(json!({"app_id": app_id, "action": "open"})),
            broker.manage_runtime_value(json!({"app_id": app_id, "action": "resume"})),
        );

        let urls: Vec<String> = [first, second, third]
            .into_iter()
            .map(|result| {
                result
                    .expect("runtime start succeeds")
                    .get("url")
                    .and_then(Value::as_str)
                    .expect("url present")
                    .to_string()
            })
            .collect();
        assert!(urls.windows(2).all(|pair| pair[0] == pair[1]));
        assert_eq!(runtime.spawn_count(), 0);
        assert_eq!(broker.runtimes.lock().await.len(), 1);
        assert_eq!(
            service
                .runtime_record(&app_id)
                .await
                .expect("runtime record")
                .mode,
            Some(AppRuntimeMode::StaticExport)
        );
    }

    #[tokio::test]
    async fn static_runtimes_are_not_counted_against_the_node_quota() {
        let (root, service, broker) = create_broker(false, None).await;
        let app_a = create_app_fixture(&root, &service, "Static A").await;
        let app_b = create_app_fixture(&root, &service, "Static B").await;

        broker
            .manage_runtime_value(json!({"app_id": app_a, "action": "start"}))
            .await
            .expect("first static runtime starts");
        broker
            .manage_runtime_value(json!({"app_id": app_b, "action": "start"}))
            .await
            .expect("second static runtime starts");

        assert_eq!(broker.runtimes.lock().await.len(), 2);
    }

    #[tokio::test]
    async fn vite_apps_use_the_static_runtime_even_in_a_full_build() {
        let mobile_linux = MockMobileLinuxRuntime::new(Duration::ZERO);
        let (root, service, broker) = create_broker(true, Some(mobile_linux.clone())).await;
        let app_id = create_app_fixture(&root, &service, "Vite Static").await;

        broker
            .manage_runtime_value(json!({"app_id": app_id, "action": "start"}))
            .await
            .expect("Vite static runtime starts");

        assert_eq!(mobile_linux.spawn_count(), 0);
        assert_eq!(
            service
                .runtime_record(&app_id)
                .await
                .expect("runtime record")
                .mode,
            Some(AppRuntimeMode::StaticExport)
        );
    }

    #[tokio::test]
    async fn legacy_next_runtime_mode_is_migrated_on_start() {
        let mobile_linux = MockMobileLinuxRuntime::new(Duration::ZERO);
        let (root, service, broker) = create_broker(true, Some(mobile_linux.clone())).await;
        let app_id = create_app_fixture(&root, &service, "Legacy Mode").await;
        service
            .set_runtime_mode(&app_id, AppRuntimeMode::NextProduction)
            .await
            .expect("persist legacy runtime mode");

        broker
            .manage_runtime_value(json!({"app_id": app_id, "action": "start"}))
            .await
            .expect("legacy runtime mode starts through static export");

        assert_eq!(mobile_linux.spawn_count(), 0);
        assert_eq!(
            service
                .runtime_record(&app_id)
                .await
                .expect("runtime record")
                .mode,
            Some(AppRuntimeMode::StaticExport)
        );
    }

    /// Lowest ephemeral floor across the shipped platforms: Linux/Android
    /// `net.ipv4.ip_local_port_range` starts here, iOS/macOS
    /// `net.inet.ip.portrange.first` at 49152.  At or above it the kernel can
    /// hand the port to any other process's socket.
    const LOWEST_SHIPPED_EPHEMERAL_FLOOR: u16 = 32_768;

    /// An app's port is permanent (`bind_stable_loopback`), so it has to come
    /// out of a window the kernel never allocates on its own.
    ///
    /// This is also the fix for a `--workspace` flake: the full-runtime host
    /// reserves the port, RELEASES it and lets the child bind it, so while the
    /// window overlapped the ephemeral range the OS could hand that port to
    /// somebody else in between — the mock runtime then failed with
    /// "bind test runtime loopback: Address already in use (os error 48)" and
    /// took a test with it.  Reproduced 1 run in 20 with the old window by
    /// churning ephemeral ports alongside the suite; 0 in 60 with this one.
    #[tokio::test]
    async fn derived_app_ports_stay_below_every_shipped_platform_ephemeral_floor() {
        let (_registry_root, registry) = empty_registry().await;
        // Real minted ids (eight lowercase hex, `ids::generate_app_id`).  Eight
        // of these ten drew a port at or above the Android floor from the old
        // 30000..50000 window.
        for app_id in [
            "0f3a91cc", "a71b04de", "5c92f8b1", "deadbeef", "00000000", "ffffffff", "9a1c7e40",
            "3b6d20af", "7e0091cd", "c4f5a3b2",
        ] {
            let (listener, port, _lease) =
                bind_stable_loopback(app_id, None, &[], &test_leases(), &registry)
                    .await
                    .expect("derive a port");
            assert!(
                port < LOWEST_SHIPPED_EPHEMERAL_FLOOR,
                "app {app_id} was pinned to {port}, which the kernel can hand out ephemerally"
            );
            assert!(
                (APP_PORT_WINDOW_FIRST..APP_PORT_WINDOW_FIRST + APP_PORT_WINDOW_LEN)
                    .contains(&port),
                "app {app_id} port {port} is outside the derived window"
            );
            assert_eq!(listener.local_addr().expect("local addr").port(), port);
        }
    }

    /// A squatted port is the one start failure the app cannot work around: the
    /// port is permanent, so the error has to NAME it or the user is told
    /// nothing they can act on.  Three of `start_reserved_runtime`'s bail-outs
    /// (this one, the missing runtime mount, the missing static build) run
    /// BEFORE the record leaves `stopped`, where `stopped -> failed` is not a
    /// transition the table has — so letting that bookkeeping rejection
    /// propagate replaced the real detail with "invalid runtime transition
    /// stopped -> failed for app <id>".
    #[tokio::test]
    async fn a_squatted_app_port_reports_the_squat_not_a_bookkeeping_rejection() {
        let runtime = MockMobileLinuxRuntime::new(Duration::ZERO);
        let (root, service, broker) = create_broker(true, Some(runtime)).await;
        let app_id = create_app_fixture(&root, &service, "Squatted").await;
        let squatter = TcpListener::bind(("127.0.0.1", 0))
            .await
            .expect("bind a squatter");
        let pinned = squatter.local_addr().expect("local addr").port();
        // Assign the port the way a first start does, then park the app
        // stopped with somebody else still sitting on it.
        for (state, port) in [
            (AppRuntimeState::Starting, Some(pinned)),
            (AppRuntimeState::Running, None),
            (AppRuntimeState::Stopping, None),
            (AppRuntimeState::Stopped, None),
        ] {
            service
                .update_runtime_record(&app_id, state, port, None, None)
                .await
                .expect("seed the app's permanent port");
        }

        let error = broker
            .manage_runtime_value(json!({"app_id": app_id, "action": "start"}))
            .await
            .expect_err("a squatted permanent port fails the start");
        assert!(
            error.contains(&format!("stable app port {pinned} is unavailable")),
            "{error}"
        );
        drop(squatter);
    }

    /// Two well-formed ids can derive the SAME window slot — `6b4cb242` and
    /// `c3baea9e` both land on 30809 — and a pin outlives the runtime that made
    /// it.  With the first app merely STOPPED its permanent port probes free,
    /// so a bind-only check pins it to the second app as well; from then on
    /// neither can start while the other runs (neither port can be moved) and
    /// on Android both apps share one WebView origin's `localStorage` /
    /// `IndexedDB`.
    #[tokio::test]
    async fn a_derived_port_skips_a_slot_a_stopped_sibling_app_has_pinned() {
        let (first_id, second_id) = ("6b4cb242", "c3baea9e");
        assert_eq!(
            derived_window_slot(first_id),
            derived_window_slot(second_id),
            "the fixture pair no longer collides, so this probe would pass vacuously"
        );

        let (_registry_root, registry) = empty_registry().await;
        let leases = test_leases();
        let (listener, first_port, first_lease) =
            bind_stable_loopback(first_id, None, &[], &leases, &registry)
                .await
                .expect("the first app derives a port");
        // The first app is stopped: nothing holds the port, only the pin
        // survives — precisely the state a bind probe cannot distinguish.  The
        // LEASE goes too, and the shared registry is deliberate: only the pin
        // may be why the second app moves off the slot.
        drop(listener);
        drop(first_lease);

        let (second_listener, second_port, _second_lease) = bind_stable_loopback(
            second_id,
            None,
            &[(first_id.to_string(), first_port)],
            &leases,
            &registry,
        )
        .await
        .expect("the second app derives a port");
        assert_ne!(
            second_port, first_port,
            "app {second_id} was pinned to {second_port}, which app {first_id} owns forever"
        );
        assert_eq!(
            second_listener.local_addr().expect("local addr").port(),
            second_port
        );
    }

    /// A pair that has ALREADY collided cannot be repaired at bind time — both
    /// records are permanent — so the only thing left is to say WHICH app is
    /// holding the port.  The negative half carries equal weight: a foreign
    /// squatter must not be reported as a sibling app, and the un-actionable
    /// "recreate one of them" advice must not appear when nothing collided.
    #[tokio::test]
    async fn a_pinned_port_held_by_a_sibling_app_names_the_sibling() {
        let holder = TcpListener::bind(("127.0.0.1", 0)).await.expect("bind");
        let port = holder.local_addr().expect("local addr").port();

        let (_registry_root, registry) = empty_registry().await;
        let named = bind_stable_loopback(
            "starter",
            Some(port),
            &[("sibling-app".into(), port)],
            &test_leases(),
            &registry,
        )
        .await
        .expect_err("a held pinned port fails the start");
        assert!(
            named.contains(&format!("stable app port {port} is unavailable")),
            "{named}"
        );
        assert!(named.contains("sibling-app"), "{named}");
        assert!(named.contains("can never be reassigned"), "{named}");

        let foreign = bind_stable_loopback(
            "starter",
            Some(port),
            &[("other-app".into(), port.wrapping_add(1))],
            &test_leases(),
            &registry,
        )
        .await
        .expect_err("a squatted pinned port still fails");
        assert!(
            foreign.contains(&format!("stable app port {port} is unavailable")),
            "{foreign}"
        );
        assert!(!foreign.contains("other-app"), "{foreign}");
        assert!(!foreign.contains("can never be reassigned"), "{foreign}");
        drop(holder);
    }

    /// The production half of the same defect: `start_reserved_runtime` has to
    /// COLLECT the sibling pins, or the exclusion above is never reached by a
    /// real start.  The sibling is parked stopped, so its permanent port probes
    /// free at bind time.
    ///
    /// The starter is SEEDED with a fixed id rather than created with a minted
    /// one: the contested port is derived from the id, so a minted id made this
    /// test bind a different port on every run (see [`seed_app_fixture`]).  The
    /// id is a well-formed minted-shape id whose slot no other test in this
    /// file derives, so the two ports this test touches — 23356 and whatever
    /// the skip lands on next — collide with nothing else in the binary.  The
    /// sibling keeps its minted id: it never starts, and its pin is written
    /// explicitly, so nothing about it is derived.
    #[tokio::test]
    async fn a_first_start_skips_a_port_a_stopped_sibling_app_already_owns() {
        const STARTER_ID: &str = "43b026c4";
        const STARTER_FIRST_PORT: u16 = 23_356;

        let root = TempDir::new().expect("tempdir");
        seed_app_fixture(&root, STARTER_ID, "Starter");
        let (root, service, broker) = create_broker_over(root, false, None).await;
        let starter = STARTER_ID.to_string();
        let sibling = create_app_fixture(&root, &service, "Sibling").await;
        let contested = APP_PORT_WINDOW_FIRST + derived_window_slot(&starter);
        // Computed from the production derivation, then held against the
        // documented value: if the derivation moves, this says so instead of
        // quietly exercising some other port.
        assert_eq!(
            contested, STARTER_FIRST_PORT,
            "app {starter} no longer derives the documented port; \
             re-pick the fixture id and update the doc comment"
        );
        // Nothing may HOLD the contested port, or the assertion below would
        // pass for the wrong reason.
        drop(
            TcpListener::bind(("127.0.0.1", contested))
                .await
                .expect("the contested port is free in this environment"),
        );
        for (state, port) in [
            (AppRuntimeState::Starting, Some(contested)),
            (AppRuntimeState::Running, None),
            (AppRuntimeState::Stopping, None),
            (AppRuntimeState::Stopped, None),
        ] {
            service
                .update_runtime_record(&sibling, state, port, None, None)
                .await
                .expect("pin the contested port on the sibling, then park it stopped");
        }

        broker
            .manage_runtime_value(json!({"app_id": starter, "action": "start"}))
            .await
            .expect("the start succeeds on a port the sibling does not own");

        let pinned = service
            .runtime_record(&starter)
            .await
            .expect("runtime record")
            .port
            .expect("the start pinned a port");
        assert_ne!(
            pinned, contested,
            "app {starter} was pinned to {contested}, which app {sibling} owns forever"
        );
        assert!(
            (APP_PORT_WINDOW_FIRST..APP_PORT_WINDOW_FIRST + APP_PORT_WINDOW_LEN).contains(&pinned),
            "app {starter} port {pinned} is outside the derived window"
        );
    }

    /// The stretch a pin snapshot cannot describe.  A chosen port only reaches
    /// the records ~11 ms later (see [`PortLeases`]), and on the full runtime
    /// the probe listener is released BEFORE then on purpose — so for that
    /// stretch the port is in no snapshot, is held by nothing, and binds
    /// cleanly for the next app that scans to it.  Both apps then own it
    /// forever and neither can run while the other does.
    ///
    /// Driven through the reservation API instead of by racing two real
    /// starts: a race reproduces the collision only sometimes, so it would pass
    /// for the wrong reason on most runs and rot without anyone noticing.  The
    /// port here is leased and deliberately NOT bound — precisely the state the
    /// production window leaves it in — so an allocator that does not consult
    /// the leases takes it on every run, not on a lucky one.
    #[tokio::test]
    async fn an_unpersisted_lease_moves_the_next_allocation_off_that_port() {
        const APP_ID: &str = "1a2b3c4d";
        const FIRST_CHOICE: u16 = 29_728;
        let first_choice = APP_PORT_WINDOW_FIRST + derived_window_slot(APP_ID);
        // Computed from the production derivation, then held against the
        // documented value, so a moved derivation says so instead of quietly
        // exercising some other port.
        assert_eq!(
            first_choice, FIRST_CHOICE,
            "app {APP_ID} no longer derives the documented port; \
             re-pick the fixture id and update the doc comment"
        );
        let (_registry_root, registry) = empty_registry().await;
        let leases = test_leases();

        // A sibling start that has CHOSEN this port and not yet persisted it.
        let concurrent = PortLease::take(&leases, "sibling-app", first_choice)
            .expect("the port is unleased before the sibling takes it");

        let (listener, port, lease) = bind_stable_loopback(APP_ID, None, &[], &leases, &registry)
            .await
            .expect("the app still gets a port");
        assert_ne!(
            port, first_choice,
            "app {APP_ID} was pinned to {first_choice}, which a sibling had already chosen"
        );
        assert!(
            (APP_PORT_WINDOW_FIRST..APP_PORT_WINDOW_FIRST + APP_PORT_WINDOW_LEN).contains(&port),
            "app {APP_ID} port {port} is outside the derived window"
        );
        // Choice and reservation are ONE step: the port it did take is already
        // spoken for, before any record has heard of it.
        assert!(
            PortLease::take(&leases, "third-app", port).is_none(),
            "port {port} was chosen but left free for a concurrent allocator"
        );

        // Dropped without a commit — every bail-out between the choice and the
        // persist ends here — so the port returns to the pool instead of being
        // lost for the life of the process.
        drop(listener);
        drop(lease);
        let reclaimed = PortLease::take(&leases, "third-app", port)
            .expect("a lease dropped without a commit releases its port");
        drop(reclaimed);
        // Committing releases it too; what `commit` buys is the order, not a
        // different effect.  The pin has taken over by then.
        PortLease::take(&leases, "fourth-app", port)
            .expect("free again")
            .commit();
        let after_commit = PortLease::take(&leases, "fifth-app", port);
        assert!(
            after_commit.is_some(),
            "port {port} stayed leased after its start committed"
        );
        drop(after_commit);

        // With the sibling gone the derivation is deterministic again: an app
        // that moved port on restart would orphan its own WebView storage.
        drop(concurrent);
        let (again, port_again, _lease) =
            bind_stable_loopback(APP_ID, None, &[], &leases, &registry)
                .await
                .expect("the app derives its port");
        assert_eq!(
            port_again, first_choice,
            "the first choice must stay deterministic while the slot is free"
        );
        drop(again);
    }

    /// The hand-off a lease and a gate BOTH miss, and the re-read that catches
    /// it.
    ///
    /// An allocator samples "persisted pins UNION live leases" in two steps:
    /// the pins first (in the caller, before `bind_stable_loopback`), the lease
    /// second.  A sibling that persists its pin and then commits its lease in
    /// between lands in neither half — the pin read was too early, the lease
    /// sample too late — and the allocation gate does not help, because the
    /// sibling's persist and commit both run after it has left that gate.  Both
    /// apps then pin the same port and neither can run while the other does.
    ///
    /// SEEDED, not raced, and the seed is exactly the post-hand-off state: the
    /// sibling's pin is in the records (persisted) and NOT in the leases
    /// (committed), while the snapshot handed to the allocator is the one that
    /// was read before either happened — an empty slice.  Every ordering
    /// question is therefore already settled when the call starts, so the
    /// allocator either consults the records again after leasing its candidate
    /// or takes the sibling's port on every single run.
    ///
    /// The control arm is the other half of the point.  Nothing binds the
    /// contested port here — the sibling is merely pinned — so an allocator
    /// that moved off it because some unrelated process happened to hold it
    /// would look identical to one that read the records.  Proving the port is
    /// free on THIS machine first is what tells those two apart; if it is not
    /// free the control fails loudly instead of handing the real assertion a
    /// free pass.
    #[tokio::test]
    async fn a_pin_that_lands_after_the_snapshot_is_caught_before_the_choice_sticks() {
        const APP_ID: &str = "8c6d31fa";
        const FIRST_CHOICE: u16 = 27_262;
        let first_choice = APP_PORT_WINDOW_FIRST + derived_window_slot(APP_ID);
        let next_choice =
            APP_PORT_WINDOW_FIRST + (derived_window_slot(APP_ID) + 1) % APP_PORT_WINDOW_LEN;
        assert_eq!(
            first_choice, FIRST_CHOICE,
            "app {APP_ID} no longer derives the documented port; \
             re-pick the fixture id and update the doc comment"
        );

        let root = TempDir::new().expect("tempdir");
        let service = test_service(&root).await;
        let sibling = create_app_fixture(&root, &service, "Hand-off").await;
        let leases = test_leases();

        // CONTROL: with the records still empty of pins, the derivation lands
        // on the contested port and the port is genuinely available here.  A
        // failure at this line means the fixture port is occupied by something
        // outside this test, and the assertion below would have passed without
        // proving anything.
        let (control, control_port, control_lease) =
            bind_stable_loopback(APP_ID, None, &[], &leases, &service)
                .await
                .expect("the app derives its port with nothing pinned");
        assert_eq!(
            control_port, first_choice,
            "port {first_choice} is not free on this machine (or the derivation moved), \
             so the contested arm below cannot distinguish the re-read from a busy port"
        );
        drop(control);
        drop(control_lease);

        // The hand-off, in the order production performs it: the pin becomes
        // durable FIRST, and only then is the lease released.  From here the
        // port is in the records and in no lease.
        let sibling_lease = PortLease::take(&leases, &sibling, first_choice)
            .expect("the sibling leases the port it is about to persist");
        service
            .update_runtime_record(
                &sibling,
                AppRuntimeState::Starting,
                Some(first_choice),
                None,
                None,
            )
            .await
            .expect("the sibling persists its pin");
        sibling_lease.commit();
        assert!(
            lock_port_leases(&leases).is_empty(),
            "the seed must leave the port in the records ONLY; a lease still held \
             would let the take alone move the allocation off it"
        );

        // The snapshot is the one the caller read BEFORE that hand-off, so the
        // pre-check cannot exclude the port and the lease is taken on it.  Only
        // a read of the records after that take can reject it.
        let (listener, port, lease) = bind_stable_loopback(APP_ID, None, &[], &leases, &service)
            .await
            .expect("the app still gets a port");
        assert_ne!(
            port, first_choice,
            "app {APP_ID} was pinned to {first_choice}, which app {sibling} persisted \
             after the snapshot was read; both apps now own it forever"
        );
        assert_eq!(
            port, next_choice,
            "the scan skipped more than the contested candidate, so something other \
             than the sibling's pin moved it"
        );
        // The rejected candidate went back to the pool: a lease dropped only on
        // the success path would strand every port the re-read rejects.
        assert!(
            lock_port_leases(&leases).get(&first_choice).is_none(),
            "port {first_choice} stayed leased after the re-read rejected it"
        );
        drop(listener);
        drop(lease);
    }

    /// The production half of the same defect: `start_reserved_runtime` has to
    /// hand the BROKER's registry to the allocator, or the exclusion above is
    /// never reached by a real start.  Deterministic — the sibling's lease is
    /// planted before the start rather than raced against it.
    ///
    /// Seeded with a fixed id for the reason [`seed_app_fixture`] documents:
    /// the contested port is derived from the id, so a minted one would probe
    /// a different port on every run.  This id's slot is derived by no other
    /// test in the binary.
    ///
    /// The control arm exists because the contested port here is held by
    /// NOTHING — the sibling has only leased it — so a start that moved off it
    /// because an unrelated process on this machine happened to be sitting on
    /// that port produces exactly the same green as a start that consulted the
    /// broker's leases.  Deriving the port through the production allocator
    /// first, against a registry with no lease planted, is what separates them:
    /// if the port is not free the control fails and says so, instead of the
    /// real assertion passing for the environment's reason.
    #[tokio::test]
    async fn a_first_start_skips_a_port_a_concurrent_start_has_leased() {
        const STARTER_ID: &str = "b17cc0de";
        const STARTER_FIRST_PORT: u16 = 26_141;

        let root = TempDir::new().expect("tempdir");
        seed_app_fixture(&root, STARTER_ID, "Leased");
        let (_root, service, broker) = create_broker_over(root, false, None).await;
        let contested = APP_PORT_WINDOW_FIRST + derived_window_slot(STARTER_ID);
        let next_slot =
            APP_PORT_WINDOW_FIRST + (derived_window_slot(STARTER_ID) + 1) % APP_PORT_WINDOW_LEN;
        assert_eq!(
            contested, STARTER_FIRST_PORT,
            "app {STARTER_ID} no longer derives the documented port; \
             re-pick the fixture id and update the doc comment"
        );

        // CONTROL, before anything is planted: the production allocator lands
        // on the contested port, so it is free on this machine and the skip
        // below can only be the lease's doing.  Its own lease registry, so
        // nothing survives into the start; the listener is released for the
        // same reason.  Releasing it leaves the kernel's brief rebind refusal
        // on that port, which is harmless here — the start rejects the
        // candidate at `PortLease::take`, before any probe touches it.
        let (control, control_port, control_lease) =
            bind_stable_loopback(STARTER_ID, None, &[], &test_leases(), &service)
                .await
                .expect("the starter derives its port with nothing leased");
        assert_eq!(
            control_port, contested,
            "port {contested} is not free on this machine (or the derivation moved), \
             so the assertion below cannot distinguish the lease from a busy port"
        );
        drop(control);
        drop(control_lease);

        // Nothing binds it and no record mentions it — the only thing that can
        // move the start off this port is the lease.
        let concurrent = PortLease::take(&broker.port_leases, "sibling-app", contested)
            .expect("the port is unleased before the sibling takes it");

        broker
            .manage_runtime_value(json!({"app_id": STARTER_ID, "action": "start"}))
            .await
            .expect("the start succeeds on a port no concurrent start holds");

        let pinned = service
            .runtime_record(STARTER_ID)
            .await
            .expect("runtime record")
            .port
            .expect("the start pinned a port");
        assert_ne!(
            pinned, contested,
            "app {STARTER_ID} was pinned to {contested}, which a concurrent start had chosen"
        );
        assert_eq!(
            pinned, next_slot,
            "the start skipped more than the leased candidate, so something other \
             than the broker's leases moved it"
        );
        drop(concurrent);
        // The start committed its own lease once the pin was durable: a port
        // still held after that is one the profile never gets back.
        assert!(
            lock_port_leases(&broker.port_leases).is_empty(),
            "a finished start left a port leased"
        );
    }

    /// One hole a lease alone cannot cover: the pin snapshot is read BEFORE a
    /// lease is taken, so without a gate two allocators can sit between those
    /// same two steps at once, read the same pins, and choose the same port.
    ///
    /// WHAT THIS PINS.  With the gate held, a start reaches neither the choice
    /// nor the pin it implies: no lease appears in the broker's registry and no
    /// port reaches the record.  That rules out a gate taken after the LEASE or
    /// after the PERSIST, which the older "the start has not finished"
    /// assertion alone could not, since any gate anywhere on the start path
    /// satisfies it.
    ///
    /// It does NOT rule out a gate taken after the SNAPSHOT: move the lock to
    /// just past `sibling_pinned_ports` and all three assertions still pass,
    /// because the start blocks before leasing either way.  That lower edge is
    /// as unobservable from outside as the upper edge below, and for the same
    /// reason.  Saying it was pinned was the tenth false comment here.
    ///
    /// WHAT IT CANNOT PIN — the gate's UPPER edge, that it is RELEASED before
    /// `update_runtime_record`.  That release is invisible from outside: the
    /// only external handle on it is acquiring the mutex, and the instant to
    /// try is between a start's choice and its persist, which is exactly the
    /// interval no observer can name without the start path telling it.  Take
    /// the gate too early and the start is still blocked on it; too late and it
    /// is already released either way.  Pinning it needs the start path
    /// instrumented — a barrier the test releases after the choice — and that
    /// is production machinery existing only for a test, so it is not here.
    /// The upper edge is held by `port_allocation`'s doc comment and by review,
    /// not by this test, and nothing below should be read as covering it.
    ///
    /// The control start is what keeps the gated assertion honest: it measures
    /// what an UNGATED start costs on this machine and sizes the wait from
    /// that, so "not finished yet" cannot quietly degrade into "not finished
    /// yet because everything here is slow".
    #[tokio::test]
    async fn port_allocation_is_serialized_across_one_brokers_starts() {
        let (root, service, broker) = create_broker(false, None).await;
        let control_id = create_app_fixture(&root, &service, "Ungated").await;
        let app_id = create_app_fixture(&root, &service, "Gated").await;

        let control_began = tokio::time::Instant::now();
        broker
            .manage_runtime_value(json!({"app_id": control_id, "action": "start"}))
            .await
            .expect("an ungated start succeeds");
        let ungated = control_began.elapsed();

        let gate = broker.port_allocation.lock().await;
        let start = tokio::spawn({
            let broker = broker.clone();
            let app_id = app_id.clone();
            async move {
                broker
                    .manage_runtime_value(json!({"app_id": app_id, "action": "start"}))
                    .await
            }
        });
        // Twenty times what a start just cost here, floored at the historical
        // 300 ms and capped so a pathological control cannot hang the suite.
        let budget = (ungated * 20).clamp(Duration::from_millis(300), Duration::from_secs(5));
        sleep(budget).await;
        assert!(
            !start.is_finished(),
            "a start finished within {budget:?} while the allocation gate was held \
             (an ungated start took {ungated:?} here), so it never took the gate"
        );
        assert!(
            lock_port_leases(&broker.port_leases).is_empty(),
            "a start leased a port while the allocation gate was held, so the gate \
             is taken after the choice"
        );
        assert_eq!(
            service
                .runtime_record(&app_id)
                .await
                .expect("runtime record")
                .port,
            None,
            "a start pinned a port while the allocation gate was held"
        );

        drop(gate);
        timeout(Duration::from_secs(10), start)
            .await
            .expect("the start is released by the gate")
            .expect("join the start")
            .expect("the start succeeds once the gate is free");
    }

    /// The invariant `bind_stable_loopback`'s "no fallback" reasoning rests on,
    /// made executable: it lives in `AppState::set_runtime` (another crate), so
    /// nothing here would notice it being lifted.  If this ever goes red, a
    /// port fallback becomes possible AND that doc comment is wrong.
    #[tokio::test]
    async fn the_pinned_app_port_can_never_be_reassigned() {
        let root = TempDir::new().expect("tempdir");
        let service = test_service(&root).await;
        let app_id = create_app_fixture(&root, &service, "Pinned").await;

        service
            .update_runtime_record(&app_id, AppRuntimeState::Starting, Some(20_123), None, None)
            .await
            .expect("the first start assigns the port");
        let same = service
            .update_runtime_record(&app_id, AppRuntimeState::Running, Some(20_123), None, None)
            .await
            .expect("re-recording the same port is what every later start does");
        assert_eq!(same.port, Some(20_123));

        let error = service
            .update_runtime_record(&app_id, AppRuntimeState::Running, Some(20_124), None, None)
            .await
            .expect_err("a moved port is refused, so a fallback would fail here instead");
        assert!(
            error.to_string().contains("can never be reassigned"),
            "{error}"
        );
        let record = service
            .runtime_record(&app_id)
            .await
            .expect("runtime record");
        assert_eq!(record.port, Some(20_123));
    }

    #[tokio::test]
    async fn explicit_stop_remains_stopped() {
        let runtime = MockMobileLinuxRuntime::new(Duration::ZERO);
        let (root, service, broker) = create_broker(true, Some(runtime)).await;
        let app_id = create_app_fixture(&root, &service, "Stop").await;

        broker
            .manage_runtime_value(json!({"app_id": app_id, "action": "start"}))
            .await
            .expect("runtime starts");
        broker
            .manage_runtime_value(json!({"app_id": app_id, "action": "stop"}))
            .await
            .expect("runtime stops");
        sleep(STATIC_ACCEPT_RETRY * 2).await;

        let runtime_record = service
            .runtime_record(&app_id)
            .await
            .expect("runtime record");
        assert_eq!(runtime_record.state, AppRuntimeState::Stopped);
        assert!(!broker.runtimes.lock().await.contains_key(&app_id));
    }

    #[tokio::test]
    async fn abandoned_runtime_reservation_is_released_for_the_next_start() {
        let (root, service, broker) = create_broker(false, None).await;
        let app_id = create_app_fixture(&root, &service, "Abandoned").await;
        // Keep the static-only start deterministically inside its reservation.
        // The old version relied on the removed full-runtime spawn delay, so
        // the start could already be Running by the time the test aborted it.
        let allocation = broker.port_allocation.lock().await;

        let start = tokio::spawn({
            let broker = broker.clone();
            let app_id = app_id.clone();
            async move {
                broker
                    .manage_runtime_value(json!({"app_id": app_id, "action": "start"}))
                    .await
            }
        });
        wait_until("runtime reservation", Duration::from_secs(3), || {
            let broker = broker.clone();
            let app_id = app_id.clone();
            async move { broker.runtimes.lock().await.contains_key(&app_id) }
        })
        .await;
        start.abort();
        let _ = start.await;

        wait_until("reservation released", Duration::from_secs(3), || {
            let broker = broker.clone();
            let app_id = app_id.clone();
            async move { !broker.runtimes.lock().await.contains_key(&app_id) }
        })
        .await;
        drop(allocation);

        let restarted = timeout(
            Duration::from_secs(10),
            broker.manage_runtime_value(json!({"app_id": app_id, "action": "start"})),
        )
        .await
        .expect("a later start is not blocked by the abandoned reservation")
        .expect("restart succeeds");
        assert_eq!(restarted["state"], "running");
    }

    #[tokio::test]
    async fn stop_during_start_keeps_the_reservation_intact() {
        let runtime = MockMobileLinuxRuntime::new(Duration::from_millis(40));
        let (root, service, broker) = create_broker(true, Some(runtime)).await;
        let app_id = create_app_fixture(&root, &service, "Race").await;

        let (start, stop) = tokio::join!(
            broker.manage_runtime_value(json!({"app_id": app_id, "action": "start"})),
            broker.manage_runtime_value(json!({"app_id": app_id, "action": "stop"})),
        );

        assert_eq!(
            start.expect("the start still completes")["state"],
            "running"
        );
        let stop_error = stop.expect_err("a stop during a start is refused");
        assert!(stop_error.contains("still starting"), "{stop_error}");
        assert_eq!(broker.runtimes.lock().await.len(), 1);
    }

    /// Declare `capability` in the app's persisted manifest, the way an
    /// `update_manifest` call reaches it.
    fn declare_capability(root: &TempDir, app_id: &str, capability: AppCapability) {
        let layout = AppLayout::new(root.path().to_path_buf(), app_id).expect("layout");
        let mut manifest = load_manifest(&layout).expect("fixture manifest");
        manifest.capabilities.push(capability);
        local_apps::save_manifest(&layout, &manifest).expect("declare capability");
    }

    #[tokio::test]
    async fn an_undeclared_capability_is_refused_without_prompting() {
        let root = TempDir::new().expect("tempdir");
        let service = test_service(&root).await;
        let sink = MockSink::arc();
        let broker =
            LocalAppsHostBroker::new(root.path().to_path_buf(), sink.clone(), None, false, None);
        assert!(broker.attach_service(service.clone()).is_ok());
        let app_id = create_app_fixture(&root, &service, "Undeclared").await;

        let failure = timeout(
            Duration::from_secs(2),
            broker.authorize_declared_capability(
                &app_id,
                AppCapability::Camera,
                AppCapabilityKindDto::Camera,
                "test reason",
            ),
        )
        .await
        .expect("the refusal must not wait on any approval")
        .expect_err("an undeclared capability must be refused");
        assert_eq!(failure.code, Some("capability_not_declared"));
        assert!(
            sink.is_empty().await,
            "an undeclared capability must never raise a user prompt"
        );
    }

    #[tokio::test]
    async fn a_declared_capability_with_a_persisted_grant_passes_silently() {
        let root = TempDir::new().expect("tempdir");
        let service = test_service(&root).await;
        let sink = MockSink::arc();
        let broker =
            LocalAppsHostBroker::new(root.path().to_path_buf(), sink.clone(), None, false, None);
        assert!(broker.attach_service(service.clone()).is_ok());
        let app_id = create_app_fixture(&root, &service, "Granted").await;
        declare_capability(&root, &app_id, AppCapability::Microphone);
        let layout = AppLayout::new(root.path().to_path_buf(), app_id.clone()).expect("layout");
        let mut permissions = load_permissions(&layout).expect("permissions");
        permissions.grant(AppCapability::Microphone);
        save_permissions(&layout, &permissions).expect("persist grant");

        broker
            .authorize_declared_capability(
                &app_id,
                AppCapability::Microphone,
                AppCapabilityKindDto::Microphone,
                "test reason",
            )
            .await
            .expect("a persisted grant authorizes silently");
        assert!(
            sink.is_empty().await,
            "a persisted grant must not re-prompt the user"
        );
    }

    #[tokio::test]
    async fn a_declared_capability_denial_carries_the_permission_denied_code() {
        let root = TempDir::new().expect("tempdir");
        let service = test_service(&root).await;
        let sink = MockSink::arc();
        let broker =
            LocalAppsHostBroker::new(root.path().to_path_buf(), sink.clone(), None, false, None);
        assert!(broker.attach_service(service.clone()).is_ok());
        let app_id = create_app_fixture(&root, &service, "Denied").await;
        declare_capability(&root, &app_id, AppCapability::Camera);

        let resolver = {
            let sink = sink.clone();
            let broker = broker.clone();
            tokio::spawn(async move {
                loop {
                    for event in sink.events().await {
                        if let ClientEvent::AppEvent {
                            event: AppEventDto::AppCapabilityRequested { request },
                        } = event
                        {
                            assert_eq!(request.capability, AppCapabilityKindDto::Camera);
                            assert!(
                                broker
                                    .resolve_capability(
                                        &request.request_id,
                                        AppAuthorizationDecisionDto::Deny,
                                    )
                                    .await
                            );
                            return;
                        }
                    }
                    tokio::time::sleep(Duration::from_millis(5)).await;
                }
            })
        };

        let failure = timeout(
            Duration::from_secs(5),
            broker.authorize_declared_capability(
                &app_id,
                AppCapability::Camera,
                AppCapabilityKindDto::Camera,
                "test reason",
            ),
        )
        .await
        .expect("the denial resolves promptly")
        .expect_err("a denied capability must fail");
        assert_eq!(failure.code, Some("permission_denied"));
        resolver.await.expect("resolver completes");
    }

    #[test]
    fn static_server_outlives_the_engine_runtime_that_started_it() {
        let engine_runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .expect("engine runtime");
        let (_root, _service, broker, port) = engine_runtime.block_on(async {
            let (root, service, broker) = create_broker(false, None).await;
            let app_id = create_app_fixture(&root, &service, "Survivor").await;
            let started = broker
                .manage_runtime_value(json!({"app_id": app_id, "action": "start"}))
                .await
                .expect("runtime starts");
            let url = started["url"].as_str().expect("url present").to_string();
            let port: u16 = url.rsplit(':').next().expect("port").parse().expect("port");
            (root, service, broker, port)
        });
        drop(engine_runtime);

        let probe = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("probe runtime");
        probe.block_on(async {
            let mut stream = TcpStream::connect(("127.0.0.1", port))
                .await
                .expect("the static server outlives the engine runtime that started it");
            // Connecting only proves the socket is still OPEN: the kernel
            // completes the handshake into the listen backlog even when nobody
            // will ever accept it.  Only a served response proves the listener
            // is still registered with a live I/O driver.
            stream
                .write_all(b"GET / HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n")
                .await
                .expect("write request");
            // Read until the BODY arrives, not once: a single `read` returns
            // whatever one poll produced, and under load that is routinely the
            // header block alone (`Content-Length: 15` with the 15 bytes still
            // in flight), which asserted against the body as a served-nothing
            // failure.
            let mut buffer = vec![0u8; 1024];
            let mut response = String::new();
            while !response.contains("<html>ok</html>") {
                let count = timeout(Duration::from_secs(5), stream.read(&mut buffer))
                    .await
                    .expect(
                        "the surviving static server answers instead of stranding the connection",
                    )
                    .expect("read response");
                assert_ne!(count, 0, "the connection closed mid-response: {response}");
                response.push_str(&String::from_utf8_lossy(&buffer[..count]));
            }
            assert!(response.starts_with("HTTP/1.1 200"), "{response}");
        });
        drop(broker);
    }

    /// Binds on a runtime that is then dropped, which is exactly what happens to
    /// a listener created on a `MobileEngineHandle`'s runtime: tokio invalidates
    /// the registration and every later `accept()` fails.
    fn listener_whose_io_driver_is_gone() -> TcpListener {
        let doomed = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()
            .expect("doomed runtime");
        let listener = doomed.block_on(async {
            TcpListener::bind(("127.0.0.1", 0))
                .await
                .expect("bind loopback")
        });
        drop(doomed);
        listener
    }

    #[test]
    fn static_accept_errors_are_retried_but_not_forever() {
        let listener = listener_whose_io_driver_is_gone();
        let root = TempDir::new().expect("tempdir");
        // Held for the whole test: a dropped sender would end the loop through
        // the SHUTDOWN arm and prove nothing about the error bound.
        let (_shutdown, receiver) = oneshot::channel();
        let join = crate::local_apps_profile::worker_runtime().spawn(run_static_server(
            listener,
            root.path().to_path_buf(),
            receiver,
        ));

        let probe = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("probe runtime");
        let outcome = probe.block_on(async {
            timeout(Duration::from_secs(30), join)
                .await
                .expect("the accept loop gives up instead of spinning at 20 Hz forever")
                .expect("the static server task did not panic")
        });
        assert!(outcome
            .expect("a listener that can never accept is a fatal exit, not a clean shutdown")
            .contains("stopped accepting connections"),);
    }

    #[test]
    fn a_dead_static_server_fails_the_runtime_record_and_frees_the_entry() {
        let listener = listener_whose_io_driver_is_gone();
        let port = listener.local_addr().expect("local addr").port();

        let harness = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .expect("harness runtime");
        harness.block_on(async move {
            let (root, service, broker) = create_broker(false, None).await;
            let app_id = create_app_fixture(&root, &service, "Dead").await;
            let generation = 7;
            for state in [AppRuntimeState::Starting, AppRuntimeState::Running] {
                service
                    .update_runtime_record(&app_id, state, Some(port), None, None)
                    .await
                    .expect("record the started runtime");
            }
            let (shutdown, receiver) = oneshot::channel();
            broker.runtimes.lock().await.insert(
                app_id.clone(),
                RuntimeEntry {
                    state: RuntimeEntryState::Running {
                        handle: RuntimeHandle::Static { shutdown },
                    },
                    last_used: 1,
                    generation,
                },
            );
            let layout = AppLayout::new(root.path().to_path_buf(), app_id.clone()).expect("layout");
            broker.spawn_static_server(
                service.clone(),
                app_id.clone(),
                generation,
                listener,
                root.path()
                    .join(layout.build_rel(false))
                    .join(crate::local_apps_build::VITE_OUTPUT_DIR),
                receiver,
            );

            // Both halves of the static reconciliation path:
            // waits: `reconcile_static_runtime_exit` removes the entry BEFORE it
            // writes the record, so waking on the removal alone and reading the
            // record next reports the pre-write `running` under load.
            wait_until(
                "the dead static server to retire its own entry and record",
                Duration::from_secs(30),
                || {
                    let broker = broker.clone();
                    let service = service.clone();
                    let app_id = app_id.clone();
                    async move {
                        !broker.runtimes.lock().await.contains_key(&app_id)
                            && service
                                .runtime_record(&app_id)
                                .await
                                .is_ok_and(|record| record.state == AppRuntimeState::Failed)
                    }
                },
            )
            .await;

            let record = service
                .runtime_record(&app_id)
                .await
                .expect("runtime record");
            assert_eq!(record.state, AppRuntimeState::Failed);
            assert!(record
                .last_error
                .expect("the accept failure is preserved")
                .contains("stopped accepting connections"));
        });
    }
}
