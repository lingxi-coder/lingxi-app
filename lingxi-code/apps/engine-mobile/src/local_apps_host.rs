//! Host-owned runtime, data, approval and WebView broker for local apps.
//!
//! The MCP provider deliberately has no direct filesystem, SQLite, process or
//! WebView handles.  This broker is the single trust boundary for those
//! operations and is also used by the native client command surface.

use crate::local_apps_bridge::lower_error_code;
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
    AppGenerationCoordinator, AppLayout, AppPermissions, AppRuntimeMode, AppRuntimeState,
    AppService, AppWorkflowState, DataMigrationPreview, DataMutation, DataQuery, DataSortDirection,
    DataSortKey, PermissionDecision, SessionPermissions,
};
use serde_json::{json, Map, Value};
use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{oneshot, watch, Mutex};
use tokio::time::{sleep, timeout, Duration};
use traits::{
    LinuxCommandRequest, LinuxProcessHandle, MobileLinuxRuntime, MobileLinuxTaskSnapshot,
    MobileLinuxTaskStatus, MountPurpose, MountSpec, NetworkPolicy,
};

const APPROVAL_TIMEOUT: Duration = Duration::from_secs(5 * 60);
const UI_TIMEOUT: Duration = Duration::from_secs(2 * 60);
const MAX_HTTP_REQUEST_BYTES: usize = 16 * 1024;
const MAX_STATIC_ASSET_BYTES: u64 = 32 * 1024 * 1024;
const FULL_RUNTIME_WATCH_POLL: Duration = Duration::from_millis(250);
const MAX_NETWORK_RESPONSE_BYTES: usize = 2 * 1024 * 1024;
const STATIC_ACCEPT_RETRY: Duration = Duration::from_millis(50);
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

#[derive(Debug)]
struct UiResolution {
    decision: AppAuthorizationDecisionDto,
    result_json: Option<String>,
    error: Option<String>,
}

enum RuntimeHandle {
    Static { shutdown: oneshot::Sender<()> },
    Full { process: LinuxProcessHandle },
}

impl RuntimeHandle {
    fn process(&self) -> Option<&LinuxProcessHandle> {
        match self {
            Self::Static { .. } => None,
            Self::Full { process } => Some(process),
        }
    }
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
        if held.get(&self.port).is_some_and(|owner| owner == &self.app_id) {
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

/// Profile-scoped broker.  The service is attached after its durable load has
/// completed, while command/capability resolution can be wired immediately.
pub(crate) struct LocalAppsHostBroker {
    root: PathBuf,
    event_sink: Arc<dyn ClientEventSink>,
    mobile_linux: Option<Arc<dyn MobileLinuxRuntime>>,
    full_runtime: bool,
    runtime_root: Option<PathBuf>,
    service: OnceLock<Arc<AppService>>,
    generation: OnceLock<Arc<AppGenerationCoordinator>>,
    /// Set once at profile load (same call site as `attach_service`), so the
    /// MCP `create` tool can trigger background authoring the same way
    /// `host.rs`'s wire-client path does — see
    /// [`LocalAppsMcpHost::trigger_authoring`].
    llm: OnceLock<Arc<crate::local_apps_profile::SharedLlm>>,
    /// Set at the same profile-load site as `llm` — live per-connection
    /// device handles behind a swap cell (see `local_apps_device`).
    device: OnceLock<Arc<crate::local_apps_device::SharedDeviceCapabilities>>,
    /// The single active `device.recordAudio*` session (one per broker — the
    /// platform has ONE audio session). Arc'd like `runtimes` so the duration
    /// watchdog task can reach it. See `device_ops`.
    recording: Arc<Mutex<Option<device_ops::ActiveRecording>>>,
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
    next_request_id: AtomicU64,
}

impl LocalAppsHostBroker {
    pub(crate) fn new(
        root: PathBuf,
        event_sink: Arc<dyn ClientEventSink>,
        mobile_linux: Option<Arc<dyn MobileLinuxRuntime>>,
        full_runtime: bool,
        runtime_root: Option<PathBuf>,
    ) -> Arc<Self> {
        Arc::new(Self {
            root,
            event_sink,
            mobile_linux,
            full_runtime,
            runtime_root,
            service: OnceLock::new(),
            generation: OnceLock::new(),
            llm: OnceLock::new(),
            device: OnceLock::new(),
            recording: Arc::new(Mutex::new(None)),
            pending_capabilities: Mutex::new(HashMap::new()),
            pending_ui: Mutex::new(HashMap::new()),
            session_permissions: Mutex::new(SessionPermissions::default()),
            runtimes: Arc::new(Mutex::new(HashMap::new())),
            port_leases: Arc::new(std::sync::Mutex::new(HashMap::new())),
            port_allocation: Mutex::new(()),
            next_request_id: AtomicU64::new(1),
        })
    }

    pub(crate) fn attach_service(&self, service: Arc<AppService>) -> Result<(), Arc<AppService>> {
        self.service.set(service)
    }

    pub(crate) fn attach_generation(
        &self,
        generation: Arc<AppGenerationCoordinator>,
    ) -> Result<(), Arc<AppGenerationCoordinator>> {
        self.generation.set(generation)
    }

    pub(crate) fn attach_llm(
        &self,
        llm: Arc<crate::local_apps_profile::SharedLlm>,
    ) -> Result<(), Arc<crate::local_apps_profile::SharedLlm>> {
        self.llm.set(llm)
    }

    pub(crate) fn attach_device(
        &self,
        device: Arc<crate::local_apps_device::SharedDeviceCapabilities>,
    ) -> Result<(), Arc<crate::local_apps_device::SharedDeviceCapabilities>> {
        self.device.set(device)
    }

    pub(crate) fn full_runtime_enabled(&self) -> bool {
        self.full_runtime
    }

    pub(crate) async fn reset_permissions(&self, app_id: &str) -> Result<(), String> {
        self.service()?
            .record(app_id)
            .await
            .map_err(|error| error.to_string())?;
        let layout = self.layout(app_id)?;
        save_permissions(&layout, &AppPermissions::default()).map_err(|error| error.to_string())?;
        self.session_permissions.lock().await.revoke_app(app_id);
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

    pub(crate) fn fixed_runtime_mount(&self) -> Result<MountSpec, String> {
        let root = self
            .runtime_root
            .as_ref()
            .filter(|path| path.join("node_modules/next/dist/bin/next").is_file())
            .ok_or_else(|| {
                "verified local-app Node runtime is unavailable; stage local-app-runtime first"
                    .to_string()
            })?;
        Ok(MountSpec {
            host_path: root.join("node_modules"),
            guest_path: "/opt/lingxi/local-app-runtime/node_modules".into(),
            read_only: true,
            purpose: MountPurpose::Shared,
        })
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
        let payload: Value = request
            .payload_json
            .as_deref()
            .map(serde_json::from_str)
            .transpose()
            .map_err(|error| format!("invalid bridge payload JSON: {error}"))?
            .unwrap_or_else(|| json!({}));
        let mut input = payload.as_object().cloned().unwrap_or_default();
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
                self.record_audio_start_value(&request.app_id, &payload).await
            }
            AppBridgeOperationDto::RecordAudioStop => {
                self.record_audio_stop_value(&request.app_id).await
            }
            AppBridgeOperationDto::GetLocation => self.get_location_value(&request.app_id).await,
            AppBridgeOperationDto::PostNotification => {
                self.post_notification_value(&request.app_id, &payload).await
            }
            _ => Err("unsupported bridge operation for this engine version".into()),
        }
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
        loop {
            let access_tick = self.next_request_id.fetch_add(1, Ordering::Relaxed);
            let mut wait_for_start = None;
            let mut inspect_full_runtime = None;
            let mut return_running = false;
            let mut victim = None;
            let mut reserved_generation = None;
            {
                let mut runtimes = self.runtimes.lock().await;
                if let Some(entry) = runtimes.get_mut(app_id) {
                    entry.last_used = access_tick;
                    match &entry.state {
                        RuntimeEntryState::Starting { gate } => {
                            wait_for_start = Some(gate.subscribe());
                        }
                        RuntimeEntryState::Running { handle } => {
                            if let Some(process) = handle.process() {
                                inspect_full_runtime = Some((entry.generation, process.id.clone()));
                            } else {
                                return_running = true;
                            }
                        }
                    }
                } else if runtimes.len() < runtime_instance_quota() {
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
                } else {
                    victim = runtimes
                        .iter()
                        .filter(|(candidate, entry)| {
                            *candidate != app_id
                                && matches!(entry.state, RuntimeEntryState::Running { .. })
                        })
                        .min_by_key(|(_, entry)| entry.last_used)
                        .map(|(candidate, _)| candidate.clone());
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
            if let Some((generation, process_id)) = inspect_full_runtime {
                if let Some(detail) = self
                    .inspect_full_runtime_terminal_detail(&process_id)
                    .await?
                {
                    self.reconcile_full_runtime_exit(app_id, generation, &process_id, detail)
                        .await?;
                    continue;
                }
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
            if let Some(victim) = victim {
                self.stop_runtime(&victim).await?;
                continue;
            }
            let Some(generation) = reserved_generation else {
                return Err(format!(
                    "runtime quota ({}) is temporarily saturated by apps that are still starting; retry shortly",
                    runtime_instance_quota()
                ));
            };
            return self.start_reserved_runtime(app_id, generation).await;
        }
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
        let runtime_mount = if self.full_runtime {
            if self.mobile_linux.is_none() {
                return self
                    .fail_reserved_runtime_start(
                        app_id,
                        generation,
                        None,
                        "Full local-app runtime requires the mobile Linux runtime".into(),
                    )
                    .await;
            }
            match self.fixed_runtime_mount() {
                Ok(mount) => Some(mount),
                Err(error) => {
                    return self
                        .fail_reserved_runtime_start(app_id, generation, None, error)
                        .await;
                }
            }
        } else {
            None
        };
        let static_root = if self.full_runtime {
            None
        } else {
            let layout = self.layout(app_id)?;
            let root = layout.root().join(layout.build_rel(false)).join("out");
            if !root.join("index.html").is_file() {
                return self
                    .fail_reserved_runtime_start(
                        app_id,
                        generation,
                        None,
                        "static build output is missing index.html; generate the app first".into(),
                    )
                    .await;
            }
            Some(root)
        };
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
        // On the full runtime that listener is only a PROBE — the Next process
        // binds the port itself — so it is released HERE rather than just
        // before the spawn.  Closing a listening socket is not instantaneous:
        // for a millisecond or so afterwards the kernel still answers a rebind
        // of that same port with EADDRINUSE while NOTHING holds it (`lsof
        // -iTCP:<port>` and `netstat -an` both empty at the moment of the
        // refusal; the identical bind succeeds on its next attempt 1.2-2.8 ms
        // later).  Whoever takes the port next must not land inside that
        // window, and everything between here and the spawn — two record
        // persistences, ~11 ms measured — is the distance that buys.  Releasing
        // early costs nothing: on this path nothing ever reads the listener.
        let static_listener = if self.full_runtime {
            drop(listener);
            None
        } else {
            Some(listener)
        };
        if let Err(error) = service
            .set_runtime_mode(
                app_id,
                if self.full_runtime {
                    AppRuntimeMode::NextProduction
                } else {
                    AppRuntimeMode::StaticExport
                },
            )
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

        let handle = if self.full_runtime {
            let runtime = self.mobile_linux.as_ref().ok_or_else(|| {
                "Full local-app runtime requires the mobile Linux runtime".to_string()
            })?;
            let layout = self.layout(app_id)?;
            let workspace = layout.root().join(layout.build_rel(true));
            let workspace_guest =
                format!("/var/lingxi/local-app-build/{app_id}/full");
            let request = LinuxCommandRequest {
                command: "/usr/bin/node".into(),
                args: vec![
                    "/opt/lingxi/local-app-runtime/node_modules/next/dist/bin/next".into(),
                    "start".into(),
                    "--hostname".into(),
                    "127.0.0.1".into(),
                    "--port".into(),
                    port.to_string(),
                ],
                cwd: Some(workspace_guest.clone()),
                env: [
                    ("LINGXI_APP_OUTPUT".into(), "server".into()),
                    (
                        "NODE_PATH".into(),
                        "/opt/lingxi/local-app-runtime/node_modules".into(),
                    ),
                ]
                .into_iter()
                .collect(),
                stdin: None,
                timeout_ms: None,
                // Both shipped mobile runtimes fail CLOSED on anything but
                // `Allowed` (ios-ish `validate_request`, Android PRoot's
                // `spawn_child`) because neither native bridge can enforce a
                // denied policy — `Disabled` was a promise no layer could keep.
                // The server binds 127.0.0.1 only, and the app's real egress
                // boundary is `authorize_domain`, not this field.
                network: NetworkPolicy::Allowed,
                mounts: vec![
                    MountSpec {
                        host_path: workspace,
                        guest_path: workspace_guest,
                        read_only: false,
                        purpose: MountPurpose::LocalAppBuild,
                    },
                    runtime_mount.expect("full runtime mount was preflighted"),
                ],
            };
            let process = match runtime.spawn_background(request).await {
                Ok(process) => process,
                Err(error) => {
                    let detail = format!("start fixed Next production server: {error}");
                    return self
                        .fail_reserved_runtime_start(app_id, generation, Some(port), detail)
                        .await;
                }
            };
            if let Err(error) = wait_for_loopback(port).await {
                let _ = runtime.kill(&process).await;
                return self
                    .fail_reserved_runtime_start(app_id, generation, Some(port), error)
                    .await;
            }
            RuntimeHandle::Full { process }
        } else {
            let (shutdown, receiver) = oneshot::channel();
            // The entry outlives any single `MobileEngineHandle`'s runtime; so
            // must the task that serves it, or a start reports `running`
            // against a socket that died with the previous engine.
            self.spawn_static_server(
                service.clone(),
                app_id.to_string(),
                generation,
                static_listener.expect("the static path keeps its probe listener"),
                static_root.expect("static output was preflighted"),
                receiver,
            );
            RuntimeHandle::Static { shutdown }
        };
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
        let watch_process = match &handle {
            RuntimeHandle::Full { process } => Some(process.clone()),
            RuntimeHandle::Static { .. } => None,
        };
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
        if let Some(process) = watch_process {
            self.spawn_full_runtime_exit_watch(app_id.to_string(), generation, process);
        }
        Ok(json!({"app_id": app_id, "state": "running", "url": format!("http://127.0.0.1:{port}")}))
    }

    async fn stop_runtime(&self, app_id: &str) -> Result<Value, String> {
        let service = self.service()?;
        service
            .record(app_id)
            .await
            .map_err(|error| error.to_string())?;
        // A stopping page must not keep the microphone hot — release any
        // recording it left behind before the runtime goes away.
        self.force_stop_recording(app_id).await;
        // Classify and remove under ONE acquisition: a start woken in the gap
        // between a `remove` and its rollback `insert` finds no entry, kills the
        // runtime it just spawned and returns without resolving the gate,
        // leaving a reservation nothing can ever complete.
        let handle = {
            let mut runtimes = self.runtimes.lock().await;
            match runtimes.get(app_id).map(|entry| &entry.state) {
                None => return Ok(json!({"app_id": app_id, "state": "stopped"})),
                Some(RuntimeEntryState::Starting { .. }) => {
                    return Err("runtime is still starting; retry stop shortly".into())
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
        let stop_result = match handle {
            RuntimeHandle::Static { shutdown } => {
                let _ = shutdown.send(());
                Ok(())
            }
            RuntimeHandle::Full { process } => match self.mobile_linux.as_ref() {
                Some(runtime) => runtime
                    .kill(&process)
                    .await
                    .map_err(|error| format!("stop Next process tree: {error}")),
                None => Err("mobile Linux runtime disappeared".to_string()),
            },
        };
        if let Err(detail) = stop_result {
            // `stopping -> failed` is not an edge the table has, and the record
            // must be left in a state it can LEAVE: stranded in `stopping` the
            // app is unstartable for the rest of the process.  The kill error
            // survives in `last_error`, and this bookkeeping write must never
            // mask the real failure.
            let _ = service
                .update_runtime_record(
                    app_id,
                    AppRuntimeState::Stopped,
                    runtime.port,
                    None,
                    Some(detail.clone()),
                )
                .await;
            return Err(detail);
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
        // never mask the real failure.  Three callers reach here BEFORE the
        // record leaves `stopped` (no runtime mount, no static build, and the
        // squatted permanent port), and `stopped -> failed` is not an edge the
        // transition table has — propagating that rejection replaced the only
        // actionable detail the caller gets with "invalid runtime transition
        // stopped -> failed".  The detail still reaches every concurrent waiter
        // through the gate above, and the record keeps a state it can leave.
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
            RuntimeHandle::Full { process } => {
                if let Some(runtime) = self.mobile_linux.as_ref() {
                    let _ = runtime.kill(&process).await;
                }
            }
        }
    }

    async fn inspect_full_runtime_terminal_detail(
        &self,
        process_id: &str,
    ) -> Result<Option<String>, String> {
        let runtime = self.mobile_linux.as_ref().ok_or_else(|| {
            "Full local-app runtime requires the mobile Linux runtime".to_string()
        })?;
        match runtime.task_status(process_id).await {
            Ok(Some(snapshot)) if !runtime_snapshot_is_terminal(snapshot.status) => Ok(None),
            Ok(Some(snapshot)) => Ok(Some(full_runtime_exit_detail(process_id, Some(&snapshot)))),
            Ok(None) => Ok(Some(full_runtime_exit_detail(process_id, None))),
            Err(error) => Ok(Some(format!(
                "inspect fixed Next production server {process_id}: {error}"
            ))),
        }
    }

    async fn reconcile_full_runtime_exit(
        &self,
        app_id: &str,
        generation: u64,
        process_id: &str,
        detail: String,
    ) -> Result<bool, String> {
        let removed = {
            let mut runtimes = self.runtimes.lock().await;
            let should_remove = runtimes.get(app_id).is_some_and(|entry| {
                entry.generation == generation
                    && matches!(
                        &entry.state,
                        RuntimeEntryState::Running {
                            handle: RuntimeHandle::Full { process }
                        } if process.id == process_id
                    )
            });
            if should_remove {
                runtimes.remove(app_id);
                true
            } else {
                false
            }
        };
        if !removed {
            return Ok(false);
        }
        let runtime = self
            .service()?
            .runtime_record(app_id)
            .await
            .map_err(|error| error.to_string())?;
        self.service()?
            .update_runtime_record(
                app_id,
                AppRuntimeState::Failed,
                runtime.port,
                runtime.pid,
                Some(detail),
            )
            .await
            .map_err(|error| error.to_string())?;
        Ok(true)
    }

    fn spawn_full_runtime_exit_watch(
        &self,
        app_id: String,
        generation: u64,
        process: LinuxProcessHandle,
    ) {
        let Some(runtime) = self.mobile_linux.as_ref().cloned() else {
            return;
        };
        let Ok(service) = self.service() else {
            return;
        };
        let runtimes = Arc::clone(&self.runtimes);
        // Same lifetime rule as the static server: the watcher must outlive the
        // engine runtime that happened to issue this start.
        crate::local_apps_profile::worker_runtime().spawn(async move {
            watch_full_runtime_exit(runtimes, service, runtime, app_id, generation, process).await;
        });
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
        crate::local_apps_profile::worker_runtime().spawn(async move {
            let Some(detail) = run_static_server(listener, root, shutdown).await else {
                return;
            };
            reconcile_static_runtime_exit(runtimes, service, app_id, generation, detail).await;
        });
    }

    pub(crate) async fn restore_checkpoint_value(&self, input: Value) -> Result<Value, String> {
        let app_id = required_string(&input, "app_id")?.to_string();
        let checkpoint_id = required_string(&input, "checkpoint_id")?.to_string();
        let record = self
            .service()?
            .record(&app_id)
            .await
            .map_err(|e| e.to_string())?;
        // The workspace hard reset is irreversible from the client, so refuse
        // before the capability prompt rather than asking the user to approve
        // something `begin_restore_rebuild` will reject afterwards.
        if record.workflow_state != AppWorkflowState::Ready {
            return Err(format!(
                "restoring app {app_id} is only available once it is ready (current workflow state {})",
                record.workflow_state
            ));
        }
        let decision = self
            .request_capability(
                &app_id,
                AppCapabilityKindDto::RestoreCheckpoint,
                None,
                "Restoring rewinds application source code. App data is not changed.",
            )
            .await?;
        if matches!(raise_decision(decision), PermissionDecision::Deny) {
            return Err("user denied checkpoint restoration".into());
        }
        self.stop_runtime(&app_id).await?;
        // The coordinator — not `AppService` — is the seam that checks both
        // preconditions BEFORE the `git reset --hard`: the workflow state (which
        // can have moved while the prompt was open) and any in-flight job over
        // the same workspace.
        let (safety, job) = self
            .generation
            .get()
            .ok_or_else(|| "generation coordinator is unavailable".to_string())?
            .restore_checkpoint(&app_id, &checkpoint_id)
            .await
            .map_err(|error| error.to_string())?;
        Ok(json!({
            "app_id": app_id,
            "restored_checkpoint_id": checkpoint_id,
            "pre_restore_checkpoint": safety,
            "data_preserved": true,
            "rebuild_required": false,
            "rebuild_queued": true,
            "generation_job": crate::local_apps_generation::lower_job(job)
        }))
    }
}

#[async_trait]
impl LocalAppsMcpHost for LocalAppsHostBroker {
    async fn manage_runtime(&self, input: Value) -> Result<Value, String> {
        self.manage_runtime_value(input).await
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

    async fn trigger_authoring(&self, app_id: String, epoch: u64) {
        let (Ok(service), Some(llm)) = (self.service(), self.llm.get()) else {
            // `service`/`llm` are attached together with `generation` at
            // profile load, right after `create_app` itself becomes
            // reachable — this should not happen. If it ever does, the app
            // is not stuck forever: the load-time sweep and the
            // `retry_questionnaire`-from-`authoring_questionnaire` escape
            // hatch both still apply.
            tracing::error!(
                app_id,
                "local-apps host: service or llm not attached; MCP-triggered authoring \
                 was skipped — the app stays recoverable via retry_questionnaire"
            );
            return;
        };
        let notifier: Arc<dyn crate::local_apps_profile::AppFailureNotifier> =
            Arc::new(BrokerFailureNotifier(self.event_sink.clone()));
        crate::local_apps_profile::spawn_authoring(service, llm.current(), notifier, app_id, epoch);
    }
}

/// The MCP host's [`crate::local_apps_profile::AppFailureNotifier`]: lowers
/// a synthesized failure directly onto the broker's own profile-wide client
/// fanout (the same sink every OTHER broker-originated event already rides —
/// `event_sink`, not the connection-scoped `AppEmissionQueue` `host.rs` uses,
/// since a profile-scoped broker has no single connection to prefer).
struct BrokerFailureNotifier(Arc<dyn ClientEventSink>);

#[async_trait]
impl crate::local_apps_profile::AppFailureNotifier for BrokerFailureNotifier {
    async fn notify_failure(&self, service: Option<&AppService>, app_id: Option<String>, error: &local_apps::AppError) {
        if let Some(service) = service {
            service.flush_events().await;
        }
        self.0
            .emit(ClientEvent::AppOperationFailed {
                app_id,
                code: lower_error_code(error.code()),
                message: error.to_string(),
            })
            .await;
    }
}

fn required_string<'a>(input: &'a Value, key: &str) -> Result<&'a str, String> {
    input
        .get(key)
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| format!("missing non-empty {key:?}"))
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
    let limit = input.get("limit").and_then(Value::as_u64).unwrap_or(50);
    let offset = input
        .get("offset")
        .and_then(Value::as_u64)
        .or_else(|| {
            input
                .get("cursor")
                .and_then(Value::as_str)
                .and_then(|value| value.parse().ok())
        })
        .unwrap_or(0);
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
/// create/delete.  One in-memory lock per app in the profile.
///
/// Twice per start in the ordinary case, not once — the snapshot the caller
/// reads before `bind_stable_loopback` cannot be trusted to still be true when
/// a candidate is leased, so the leased candidate is re-checked against a fresh
/// read.  Every result is a snapshot; only one taken while the port in question
/// is leased says anything durable about it.
async fn sibling_pinned_ports(service: &AppService, app_id: &str) -> Vec<(String, u16)> {
    let mut pinned = Vec::new();
    for record in service.records().await {
        if record.id == app_id {
            continue;
        }
        if let Ok(runtime) = service.runtime_record(&record.id).await {
            if let Some(port) = runtime.port {
                pinned.push((record.id, port));
            }
        }
    }
    pinned
}

async fn wait_for_loopback(port: u16) -> Result<(), String> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(120);
    loop {
        if TcpStream::connect(("127.0.0.1", port)).await.is_ok() {
            return Ok(());
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(format!(
                "Next server did not become healthy on loopback port {port} within 120 seconds"
            ));
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
}

fn runtime_snapshot_is_terminal(status: MobileLinuxTaskStatus) -> bool {
    matches!(
        status,
        MobileLinuxTaskStatus::Completed
            | MobileLinuxTaskStatus::Failed
            | MobileLinuxTaskStatus::Cancelled
            | MobileLinuxTaskStatus::TimedOut
    )
}

fn full_runtime_exit_detail(
    process_id: &str,
    snapshot: Option<&MobileLinuxTaskSnapshot>,
) -> String {
    let Some(snapshot) = snapshot else {
        return format!(
            "fixed Next production server {process_id} disappeared from the mobile Linux runtime"
        );
    };
    let status = match snapshot.status {
        MobileLinuxTaskStatus::Queued => "queued",
        MobileLinuxTaskStatus::Running => "running",
        MobileLinuxTaskStatus::Backgrounded => "backgrounded",
        MobileLinuxTaskStatus::Completed => "completed",
        MobileLinuxTaskStatus::Failed => "failed",
        MobileLinuxTaskStatus::Cancelled => "cancelled",
        MobileLinuxTaskStatus::TimedOut => "timed_out",
    };
    match (snapshot.exit_code, snapshot.detail.as_deref()) {
        (Some(exit_code), Some(detail)) => {
            format!(
                "fixed Next production server {process_id} {status} (exit {exit_code}): {detail}"
            )
        }
        (Some(exit_code), None) => {
            format!("fixed Next production server {process_id} {status} (exit {exit_code})")
        }
        (None, Some(detail)) => {
            format!("fixed Next production server {process_id} {status}: {detail}")
        }
        (None, None) => format!("fixed Next production server {process_id} {status}"),
    }
}

async fn watch_full_runtime_exit(
    runtimes: Arc<Mutex<HashMap<String, RuntimeEntry>>>,
    service: Arc<AppService>,
    runtime: Arc<dyn MobileLinuxRuntime>,
    app_id: String,
    generation: u64,
    process: LinuxProcessHandle,
) {
    loop {
        sleep(FULL_RUNTIME_WATCH_POLL).await;
        let still_current = {
            let runtimes = runtimes.lock().await;
            runtimes.get(&app_id).is_some_and(|entry| {
                entry.generation == generation
                    && matches!(
                        &entry.state,
                        RuntimeEntryState::Running {
                            handle: RuntimeHandle::Full { process: current }
                        } if current.id == process.id
                    )
            })
        };
        if !still_current {
            return;
        }
        let detail = match runtime.task_status(&process.id).await {
            Ok(Some(snapshot)) if !runtime_snapshot_is_terminal(snapshot.status) => None,
            Ok(Some(snapshot)) => Some(full_runtime_exit_detail(&process.id, Some(&snapshot))),
            Ok(None) => Some(full_runtime_exit_detail(&process.id, None)),
            Err(error) => Some(format!(
                "inspect fixed Next production server {}: {error}",
                process.id
            )),
        };
        let Some(detail) = detail else {
            continue;
        };
        let removed = {
            let mut runtimes = runtimes.lock().await;
            let should_remove = runtimes.get(&app_id).is_some_and(|entry| {
                entry.generation == generation
                    && matches!(
                        &entry.state,
                        RuntimeEntryState::Running {
                            handle: RuntimeHandle::Full { process: current }
                        } if current.id == process.id
                    )
            });
            if should_remove {
                runtimes.remove(&app_id);
                true
            } else {
                false
            }
        };
        if !removed {
            return;
        }
        if let Ok(runtime_record) = service.runtime_record(&app_id).await {
            let _ = service
                .update_runtime_record(
                    &app_id,
                    AppRuntimeState::Failed,
                    runtime_record.port,
                    runtime_record.pid,
                    Some(detail),
                )
                .await;
        }
        return;
    }
}

fn runtime_instance_quota() -> usize {
    let bytes = std::fs::read_to_string("/proc/meminfo")
        .ok()
        .and_then(|contents| {
            contents.lines().find_map(|line| {
                let value = line.strip_prefix("MemTotal:")?;
                value
                    .split_whitespace()
                    .next()
                    .and_then(|kilobytes| kilobytes.parse::<u64>().ok())
                    .map(|kilobytes| kilobytes.saturating_mul(1024))
            })
        })
        .unwrap_or(0);
    let gib = 1024_u64.pow(3);
    if bytes >= 8 * gib {
        3
    } else if bytes >= 6 * gib {
        2
    } else {
        // iOS does not expose /proc.  Fail conservatively to one process; the
        // native memory-warning hook can still evict it immediately.
        1
    }
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
    loop {
        tokio::select! {
            _ = &mut shutdown => return None,
            accepted = listener.accept() => match accepted {
                Ok((stream, _)) => {
                    consecutive_errors = 0;
                    let root = root.clone();
                    tokio::spawn(async move {
                        let _ = serve_static_request(stream, &root).await;
                    });
                }
                Err(error) => {
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
}

/// The static twin of [`watch_full_runtime_exit`]'s tail: drop the entry this
/// dead server owns and fail the record, so the next start is legal instead of
/// short-circuiting on a stale `Running`.
async fn reconcile_static_runtime_exit(
    runtimes: Arc<Mutex<HashMap<String, RuntimeEntry>>>,
    service: Arc<AppService>,
    app_id: String,
    generation: u64,
    detail: String,
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
    let line = String::from_utf8_lossy(&request)
        .lines()
        .next()
        .unwrap_or_default()
        .to_string();
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
    let metadata = match tokio::fs::metadata(&path).await {
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
    let body = tokio::fs::read(&path).await?;
    debug_assert_eq!(metadata.len(), body.len() as u64);
    write_http(
        &mut stream,
        200,
        content_type(&path),
        &body,
        method == "HEAD",
    )
    .await
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
    let reason = match status {
        200 => "OK",
        400 => "Bad Request",
        404 => "Not Found",
        405 => "Method Not Allowed",
        _ => "Error",
    };
    let header = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Length: {}\r\nContent-Type: {content_type}\r\nContent-Security-Policy: default-src 'self'; script-src 'self' 'unsafe-inline'; style-src 'self' 'unsafe-inline'; img-src 'self' data: blob:; font-src 'self' data:; connect-src 'self'; object-src 'none'; base-uri 'none'; frame-ancestors 'none'; form-action 'self'\r\nReferrer-Policy: no-referrer\r\nX-Content-Type-Options: nosniff\r\nX-Frame-Options: DENY\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n",
        body.len()
    );
    stream.write_all(header.as_bytes()).await?;
    if !head {
        stream.write_all(body).await?;
    }
    stream.shutdown().await
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
    use local_apps::{storage, AppState, NoopAppEventObserver, NoopContinuationSink};
    use serde_json::json;
    use std::fs;
    use std::future::Future;
    use std::sync::atomic::{AtomicBool, AtomicUsize};
    use tempfile::TempDir;
    use traits::{
        MobileLinuxCapability, MobileLinuxError, MobileLinuxRuntimeMode, PtyOpenRequest,
        PtySessionHandle, PtySize, RootfsState, RootfsStatus, SandboxBackend,
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
        fail_kill: AtomicBool,
    }

    impl MockMobileLinuxRuntime {
        fn new(spawn_delay: Duration) -> Arc<Self> {
            Arc::new(Self {
                spawn_delay,
                spawn_count: AtomicUsize::new(0),
                next_task_id: AtomicU64::new(1),
                tasks: Mutex::new(HashMap::new()),
                fail_kill: AtomicBool::new(false),
            })
        }

        fn set_fail_kill(&self, fail: bool) {
            self.fail_kill.store(fail, Ordering::SeqCst);
        }

        /// Both shipped runtimes reject anything but `NetworkPolicy::Allowed`
        /// before they boot; a mock that is more permissive than the device
        /// cannot catch a policy the device refuses.
        fn enforce_network_policy(request: &LinuxCommandRequest) -> Result<(), MobileLinuxError> {
            if matches!(request.network, NetworkPolicy::Allowed) {
                Ok(())
            } else {
                Err(MobileLinuxError::InvalidRequest(
                    "mobile Linux runtimes accept only NetworkPolicy::Allowed".into(),
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
            Ok(LinuxProcessHandle { id: task_id })
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
                Arc::new(NoopContinuationSink),
                Arc::new(NoopAppEventObserver),
            )
            .await
            .expect("load app service"),
        )
    }

    fn create_runtime_root(root: &TempDir) -> PathBuf {
        let runtime_root = root.path().join("runtime-root");
        let next_bin = runtime_root.join("node_modules/next/dist/bin/next");
        fs::create_dir_all(next_bin.parent().unwrap()).expect("create runtime root");
        fs::write(&next_bin, b"#!/bin/sh\n").expect("write next bin");
        runtime_root
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
        let static_out = root.path().join(layout.build_rel(false)).join("out");
        fs::create_dir_all(&static_out).expect("create static out");
        fs::write(static_out.join("index.html"), "<html>ok</html>").expect("write index.html");
        let full_build = root.path().join(layout.build_rel(true));
        fs::create_dir_all(&full_build).expect("create full build");
        record.id
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
        storage::save_index(root.path(), std::slice::from_ref(&app.record))
            .expect("persist the seeded index");
        let static_out = root.path().join(layout.build_rel(false)).join("out");
        fs::create_dir_all(&static_out).expect("create static out");
        fs::write(static_out.join("index.html"), "<html>ok</html>").expect("write index.html");
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
    fn normalize_query_accepts_public_sort_object_and_legacy_aliases() {
        let query = normalize_query(&json!({
            "collection": "items",
            "sort": {
                "kind": "field",
                "field_id": "score",
                "direction": "desc"
            },
            "cursor": "7"
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
    async fn concurrent_full_runtime_start_reuses_one_spawn() {
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
        assert_eq!(runtime.spawn_count(), 1);
        assert_eq!(broker.runtimes.lock().await.len(), 1);
    }

    #[tokio::test]
    async fn concurrent_starts_keep_runtime_quota_reserved_atomically() {
        let (root, service, broker) = create_broker(false, None).await;
        let quota = runtime_instance_quota();
        for index in 0..quota.saturating_sub(1) {
            let app_id = create_app_fixture(&root, &service, &format!("Warm {index}")).await;
            broker
                .manage_runtime_value(json!({"app_id": app_id, "action": "start"}))
                .await
                .expect("warm runtime starts");
        }
        let app_a = create_app_fixture(&root, &service, "A").await;
        let app_b = create_app_fixture(&root, &service, "B").await;

        let (result_a, result_b) = tokio::join!(
            broker.manage_runtime_value(json!({"app_id": app_a, "action": "start"})),
            broker.manage_runtime_value(json!({"app_id": app_b, "action": "start"})),
        );

        if quota == 1 {
            assert_eq!(
                [result_a.as_ref(), result_b.as_ref()]
                    .into_iter()
                    .filter(|result| result.is_ok())
                    .count(),
                1
            );
        } else {
            result_a.expect("first concurrent start succeeds");
            result_b.expect("second concurrent start succeeds");
        }
        assert_eq!(broker.runtimes.lock().await.len(), quota);
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
    async fn full_runtime_exit_marks_failed_and_allows_restart() {
        let runtime = MockMobileLinuxRuntime::new(Duration::ZERO);
        let (root, service, broker) = create_broker(true, Some(runtime.clone())).await;
        let app_id = create_app_fixture(&root, &service, "Recover").await;

        let started = broker
            .manage_runtime_value(json!({"app_id": app_id, "action": "start"}))
            .await
            .expect("initial start succeeds");
        let first_url = started["url"].as_str().expect("url present").to_string();
        let task_id = runtime.first_task_id().await;
        runtime
            .complete_task(&task_id, MobileLinuxTaskStatus::Failed, "process exited")
            .await;

        wait_until(
            "runtime failure reconciliation",
            Duration::from_secs(3),
            || {
                let service = service.clone();
                let broker = broker.clone();
                let app_id = app_id.clone();
                async move {
                    let runtime_record = service
                        .runtime_record(&app_id)
                        .await
                        .expect("runtime record");
                    runtime_record.state == AppRuntimeState::Failed
                        && !broker.runtimes.lock().await.contains_key(&app_id)
                }
            },
        )
        .await;

        let restarted = broker
            .manage_runtime_value(json!({"app_id": app_id, "action": "start"}))
            .await
            .expect("restart succeeds");
        assert_eq!(runtime.spawn_count(), 2);
        assert_eq!(restarted["state"], "running");
        // The WebView reloads this URL, and every client-side store the app
        // owns is keyed by its origin: a restart that moved the port would
        // orphan the app's own data.  See `bind_stable_loopback`.
        assert_eq!(restarted["url"].as_str(), Some(first_url.as_str()));
        let runtime_record = service
            .runtime_record(&app_id)
            .await
            .expect("runtime record");
        assert_eq!(runtime_record.state, AppRuntimeState::Running);
    }

    #[tokio::test]
    async fn explicit_stop_does_not_get_overwritten_by_exit_watch() {
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
        sleep(FULL_RUNTIME_WATCH_POLL * 2).await;

        let runtime_record = service
            .runtime_record(&app_id)
            .await
            .expect("runtime record");
        assert_eq!(runtime_record.state, AppRuntimeState::Stopped);
        assert!(!broker.runtimes.lock().await.contains_key(&app_id));
    }

    #[tokio::test]
    async fn abandoned_runtime_reservation_is_released_for_the_next_start() {
        let runtime = MockMobileLinuxRuntime::new(Duration::from_secs(1));
        let (root, service, broker) = create_broker(true, Some(runtime)).await;
        let app_id = create_app_fixture(&root, &service, "Abandoned").await;

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

        wait_until("reservation released", Duration::from_secs(3), || {
            let broker = broker.clone();
            let app_id = app_id.clone();
            async move { !broker.runtimes.lock().await.contains_key(&app_id) }
        })
        .await;

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
    async fn failed_kill_leaves_a_runtime_state_the_table_can_leave() {
        let runtime = MockMobileLinuxRuntime::new(Duration::ZERO);
        let (root, service, broker) = create_broker(true, Some(runtime.clone())).await;
        let app_id = create_app_fixture(&root, &service, "Wedge").await;

        broker
            .manage_runtime_value(json!({"app_id": app_id, "action": "start"}))
            .await
            .expect("runtime starts");
        runtime.set_fail_kill(true);
        let error = broker
            .manage_runtime_value(json!({"app_id": app_id, "action": "stop"}))
            .await
            .expect_err("a failed kill is reported to the caller");
        assert!(error.contains("stop Next process tree"), "{error}");

        let runtime_record = service
            .runtime_record(&app_id)
            .await
            .expect("runtime record");
        assert_eq!(runtime_record.state, AppRuntimeState::Stopped);
        assert!(runtime_record
            .last_error
            .expect("kill failure is preserved")
            .contains("did not reap"));

        runtime.set_fail_kill(false);
        let restarted = broker
            .manage_runtime_value(json!({"app_id": app_id, "action": "start"}))
            .await
            .expect("the app is startable again");
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

    #[tokio::test]
    async fn restore_checkpoint_is_refused_before_the_workspace_is_touched() {
        let root = TempDir::new().expect("tempdir");
        let service = test_service(&root).await;
        let sink = MockSink::arc();
        let broker =
            LocalAppsHostBroker::new(root.path().to_path_buf(), sink.clone(), None, false, None);
        assert!(broker.attach_service(service.clone()).is_ok());
        let app_id = create_app_fixture(&root, &service, "Restore").await;

        let refusal = timeout(
            Duration::from_secs(2),
            broker.restore_checkpoint_value(
                json!({"app_id": app_id, "checkpoint_id": "scaffold_created"}),
            ),
        )
        .await
        .expect("the refusal returns without waiting on an approval")
        .expect_err("a non-ready app cannot be restored");
        assert!(
            refusal.contains("only available once it is ready"),
            "{refusal}"
        );
        assert!(
            sink.is_empty().await,
            "no capability prompt is raised for a restore that will be refused"
        );
    }

    /// Declare `capability` in the app's persisted manifest, the way a
    /// confirmed plan reaches it through `reconcile_manifest`.
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
                root.path().join(layout.build_rel(false)).join("out"),
                receiver,
            );

            // Both halves, the way `full_runtime_exit_marks_failed_and_allows_restart`
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
