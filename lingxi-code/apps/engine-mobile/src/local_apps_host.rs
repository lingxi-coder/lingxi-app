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
    AppCapabilityKindDto, AppCapabilityRequestDto, AppDependencyChangeConfirmationRequestDto,
    AppDependencyChangeDto, AppDependencyChangeKindDto, AppEventDto, AppRuntimeProfileDto,
    AppRuntimeProfileOptionDto, AppRuntimeProfilePackageDto, AppRuntimeProfileSelectionRequestDto,
    AppSurfaceDto, AppUiActionKindDto, AppUiRequestDto, AppUiTargetDto,
};
use futures_util::StreamExt;
use local_apps::{
    load_manifest, load_permissions, save_permissions, AppCapability, AppDataStore,
    AppDependencyState, AppLayout, AppPermissions, AppRuntimeMode, AppRuntimeProfile,
    AppRuntimeState, AppService, BackgroundTaskStatus, DataMigrationPreview, DataMutation,
    DataQuery, DataSortDirection, DataSortKey, PermissionDecision, SessionPermissions,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashMap};
use std::io::{self, Read};
use std::net::{IpAddr, SocketAddr};
use std::path::{Component, Path, PathBuf};
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
const RUNTIME_PROFILE_RECEIPT_TTL: Duration = Duration::from_secs(10 * 60);
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
const PNPM_TOOLCHAIN_KEY: &str = crate::local_app_runtime_profiles::RUNTIME_PROFILE_TOOLCHAIN_KEY;
const DEPENDENCY_SNAPSHOT_VERSION: u8 = 2;
const DEPENDENCY_SNAPSHOT_READY_FILE: &str = ".lingxi-dependency-ready";
const DEPENDENCY_UPDATE_RECOVERY_FILE_REL: &str =
    ".lingxi-build-state/dependency-update-recovery.json";
const DEPENDENCY_UPDATE_RECOVERY_SCHEMA_VERSION: u32 = 1;
const MAX_DEPENDENCY_UPDATE_RECOVERY_BYTES: usize = 16 * 1024 * 1024;
/// Emitted by `stage-local-app-runtime.py` beside the staged `node_modules`.
const BUNDLED_SEED_MANIFEST_FILE: &str = "runtime-manifest.json";
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

/// Availability of the exact profile dependency lock on this host. The
/// selector distinguishes a reusable shared snapshot from a device-bundled
/// seed; both avoid a network download but carry different provenance.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RuntimeProfileDependencyAvailability {
    Cached,
    Bundled,
    DownloadRequired,
}

impl RuntimeProfileDependencyAvailability {
    fn as_str(self) -> &'static str {
        match self {
            Self::Cached => "cached",
            Self::Bundled => "bundled",
            Self::DownloadRequired => "download_required",
        }
    }
}

static LOCAL_APP_BUILD_LOCK: OnceLock<Arc<Mutex<()>>> = OnceLock::new();
static DEPENDENCY_SNAPSHOT_DIGESTS: OnceLock<std::sync::Mutex<HashMap<PathBuf, String>>> =
    OnceLock::new();
/// The policy every local app is served.
///
/// `worker-src 'self' blob:` and `script-src … 'wasm-unsafe-eval'` are DEFAULTS,
/// not a grant, because gating them would have been incoherent: this policy
/// already carries `'unsafe-inline'`, so the page can run any JavaScript it
/// shipped. WebAssembly is strictly WEAKER than that — no DOM, no network, no
/// files, only arithmetic and the imports the page hands it — and a `blob:`
/// worker runs the same same-origin JavaScript the page could have run on the
/// main thread. Charging a permission prompt for a capability the page already
/// exceeds buys nothing, and the failure mode when the generator forgets to ask
/// for it is bad: the app builds, then fails at runtime with a CSP refusal that
/// `inspect_ui` cannot see because the surface is a canvas.
///
/// The real boundary is elsewhere and unchanged: `default-src 'self'` plus
/// `connect-src 'self'` keep the page from loading or exfiltrating anything,
/// and every device/host power is gated per capability at the bridge.
///
/// Measured on device 2026-08-21: under the previous `worker-src 'none'` WebKit
/// rejected a worker with "The operation is insecure.", and without
/// `'wasm-unsafe-eval'` it rejected WebAssembly with "Refused to create a
/// WebAssembly object…" — both blocks were real, not theoretical.
const LOCAL_APP_CONTENT_SECURITY_POLICY: &str = "default-src 'self'; script-src 'self' 'unsafe-inline' 'wasm-unsafe-eval'; style-src 'self' 'unsafe-inline'; img-src 'self' data: blob:; font-src 'self' data:; connect-src 'self'; media-src 'self' data: blob:; worker-src 'self' blob:; object-src 'none'; base-uri 'none'; frame-ancestors 'none'; form-action 'self'";

#[derive(Debug)]
struct UiResolution {
    decision: AppAuthorizationDecisionDto,
    result_json: Option<String>,
    error: Option<String>,
}

#[derive(Clone, Debug)]
struct PendingRuntimeProfileReceipt {
    receipt_id: String,
    app_id: String,
    binding: local_apps::AppRuntimeProfileBinding,
    issued_at_ms: u64,
    expires_at_ms: u64,
    claimed: bool,
}

#[derive(Clone, Debug)]
struct PendingDependencyChangeReceipt {
    receipt_id: String,
    app_id: String,
    baseline: DependencyBaselineIdentity,
    requested_json: Vec<u8>,
    effective_package_json: Vec<u8>,
    issued_at_ms: u64,
    expires_at_ms: u64,
    summary: Vec<DependencyChange>,
    claimed: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PersistedMcpCandidate {
    validated: local_apps::ValidatedAppMcpProposal,
    approval_contract_sha256: String,
    review_surface: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    verification_sha256: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    catalog_sha256: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct DependencyBaselineIdentity {
    dependency_snapshot_sha256: String,
    requested_sha256: String,
    package_sha256: String,
    lockfile_sha256: String,
    toolchain_key: String,
    contract_sha256: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum DependencyChangeKind {
    Add,
    Update,
    Remove,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DependencyChange {
    kind: DependencyChangeKind,
    package: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    version: Option<String>,
}

fn dependency_change_kind_dto(kind: &DependencyChangeKind) -> AppDependencyChangeKindDto {
    match kind {
        DependencyChangeKind::Add => AppDependencyChangeKindDto::Add,
        DependencyChangeKind::Update => AppDependencyChangeKindDto::Update,
        DependencyChangeKind::Remove => AppDependencyChangeKindDto::Remove,
    }
}

fn dependency_change_cache_status(kind: &DependencyChangeKind) -> String {
    match kind {
        DependencyChangeKind::Remove => "not_needed".into(),
        DependencyChangeKind::Add | DependencyChangeKind::Update => {
            // A ready app tree says nothing about whether this particular
            // package/version is in the pnpm store.  Do not inspect or mutate
            // that store before approval; expose an honest unknown status.
            "unknown_until_resolution".into()
        }
    }
}

struct DependencyInstallCompletion {
    lockfile_sha256: String,
    toolchain_key: String,
}

#[derive(Debug)]
struct DependencyUpdateFileBackup {
    relative: &'static str,
    bytes: Option<Vec<u8>>,
}

#[derive(Debug)]
struct DependencyUpdateRollback {
    previous_dependency: local_apps::AppDependencyRecord,
    files: Vec<DependencyUpdateFileBackup>,
    manifest_bytes: Vec<u8>,
    node_modules_backup: Option<PathBuf>,
    build_backup: Option<PathBuf>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum DependencyUpdateRecoveryStatus {
    InProgress,
    Committed,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DependencyUpdateRecoveryFile {
    relative: String,
    bytes: Option<Vec<u8>>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DependencyUpdateRecoveryJournal {
    schema_version: u32,
    app_id: String,
    status: DependencyUpdateRecoveryStatus,
    previous_dependency: local_apps::AppDependencyRecord,
    files: Vec<DependencyUpdateRecoveryFile>,
    manifest_bytes: Vec<u8>,
    node_modules_backup: Option<String>,
    build_backup: Option<String>,
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

/// One app's in-flight `LocalAppScaffold` slot — §C.1 step 1's reservation.
///
/// Taken before validation and held until the transaction leaves by ANY path,
/// including a panic, because `Drop` is what releases it. Straight-line
/// cleanup after the awaits is not enough: the transaction's future is dropped
/// whenever the connection is torn down mid-call, while the broker outlives it
/// in the process-wide profile cache, and a leaked slot would make every later
/// scaffold of that app answer `scaffold_in_flight` for the life of the
/// process — bricking the very draft the reservation exists to protect.
///
/// It excludes a second `LocalAppScaffold` for the same app and NOTHING else.
/// A concurrent `DeleteApp` is excluded by `storage::lock_app_build`, which
/// the transaction holds across the landing and the commit.
struct ScaffoldReservation {
    app_id: String,
    slots: Arc<std::sync::Mutex<std::collections::HashSet<String>>>,
}

impl ScaffoldReservation {
    /// Reserve `app_id`, or refuse because another scaffold already holds it.
    ///
    /// A poisoned mutex is RECOVERED rather than propagated: the only code
    /// that ever holds this lock is the insert here and the remove in `Drop`,
    /// so poisoning can only have come from a panic elsewhere in the process,
    /// and treating it as "no app can ever be scaffolded again" would be a
    /// worse failure than the one that poisoned it.
    fn take(
        slots: &Arc<std::sync::Mutex<std::collections::HashSet<String>>>,
        app_id: &str,
    ) -> Result<Self, String> {
        let mut held = slots.lock().unwrap_or_else(|error| error.into_inner());
        if !held.insert(app_id.to_string()) {
            return Err(format!(
                "scaffold_in_flight: app {app_id} already has a scaffold in progress"
            ));
        }
        drop(held);
        Ok(Self {
            app_id: app_id.to_string(),
            slots: Arc::clone(slots),
        })
    }
}

impl Drop for ScaffoldReservation {
    fn drop(&mut self) {
        self.slots
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .remove(&self.app_id);
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

/// Where a session this host minted for an app lives on disk. The one spelling
/// [`remove_app_session_file`] and [`reconcile_app_init_session_title`] both
/// derive their path from, so they can never disagree about which file is the
/// app's pinned init session.
fn app_session_file(
    lingxi_home: &std::path::Path,
    data_root: &std::path::Path,
    record: &local_apps::AppRecord,
    session_id: &str,
) -> std::path::PathBuf {
    let workspace_cwd = canonical_cwd_string(&data_root.join(&record.workspace_rel));
    lingxi_home
        .join("projects")
        .join(session::jsonl::path::project_dir_name(&workspace_cwd))
        .join(format!("{session_id}.jsonl"))
}

/// The session-catalog facts the `LocalAppScaffold` commit point needs in order
/// to rename an app's pinned init session: where transcripts live
/// (`<lingxi_home>/projects/…`) and the filesystem that reads and appends them.
///
/// Attached by the engine builder, which owns both. `self.root` is already the
/// apps data root, so `lingxi_home` is the only path the broker is missing —
/// and it is deliberately passed rather than re-derived from `root`, because
/// `mobile_apps_data_root` degrades to `cwd` when `lingxi_home` has no usable
/// parent, and inverting that guess would point the rename at the wrong
/// catalog on exactly the configuration that already went wrong.
#[derive(Clone)]
pub(crate) struct SessionCatalog {
    /// The engine's per-profile data dir — `projects/` hangs off it.
    pub(crate) lingxi_home: std::path::PathBuf,
    /// The filesystem transcripts are read and appended through.
    pub(crate) fs: Arc<dyn traits::FileSystem>,
}

/// The latest effective `custom-title` for `session_id` in a transcript: the
/// title it resolves to, and whether that title is still one MOBILE wrote —
/// i.e. whether the user has never renamed this session themselves.
///
/// "Latest effective" mirrors [`session::jsonl::reader`] exactly: it folds
/// every `custom-title` line whose `sessionId` matches into one map slot, so
/// the LAST one on disk wins, and a record whose `customTitle` is not a string
/// is skipped (the reader's `and_then(Value::as_str)` drops it too).
///
/// ⚠️ The second half deliberately does NOT read the marker off the last
/// record. It cannot: the transcript writer's own 32 KiB metadata backstop
/// re-emits the CURRENT title as a PLAIN, unmarked `custom-title`
/// (`session::jsonl::re_append::plan_re_append` rebuilds the record from
/// `{type, customTitle, sessionId}` and has no marker to carry), so in any
/// interview long enough to trip it the last record is unmarked even though
/// nobody renamed anything. Reading the marker off the last record alone made
/// [`reconcile_app_init_session_title`] unreachable in production — see that
/// function and [`latest_custom_title_is_mobile_placeholder`].
///
/// So the scan tracks the ANCHOR — the title on the most recent marked record
/// — and treats an unmarked record as a user rename only when its text
/// DIFFERS from the anchor. A backstop echo copies the anchor's text verbatim;
/// a `/rename` writes something else.
fn latest_custom_title(transcript: &str, session_id: &str) -> Option<(String, bool)> {
    let mut latest: Option<String> = None;
    // The title on the most recent record that carried the mobile marker.
    // `None` until one is seen — an unmarked record BEFORE any anchor
    // (a `session::branch` fork's title, say) is superseded by the anchor and
    // must not poison it.
    let mut anchor: Option<String> = None;
    let mut user_renamed = false;
    for line in transcript.lines() {
        let Ok(value) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if value.get("type").and_then(Value::as_str) != Some("custom-title") {
            continue;
        }
        if value.get("sessionId").and_then(Value::as_str) != Some(session_id) {
            continue;
        }
        let Some(title) = value.get("customTitle").and_then(Value::as_str) else {
            continue;
        };
        if value.get("mobileEmptySession").and_then(Value::as_u64) == Some(1) {
            // Mobile is the only writer that marks, and it only marks a title
            // it was entitled to write, so its own record re-establishes the
            // baseline.
            anchor = Some(title.to_string());
            user_renamed = false;
        } else if anchor.as_deref().is_some_and(|anchored| anchored != title) {
            user_renamed = true;
        }
        latest = Some(title.to_string());
    }
    latest.map(|title| (title, anchor.is_some() && !user_renamed))
}

/// Whether this session's title is still one MOBILE wrote, i.e. the user has
/// never renamed it.
///
/// ⚠️ The title TEXT cannot decide this and must never be used to.
/// `/rename` (`orchestrator`'s `append_custom_title`), a hook's `sessionTitle`
/// and mobile's own placeholder anchor all write the SAME `custom-title`
/// channel with the same shape; the only discriminator is the extra
/// `"mobileEmptySession":1` field that
/// [`session::jsonl::writer::JsonlWriter::append_mobile_empty_session`] adds.
/// An ordinary `custom-title` carrying text mobile never wrote, anywhere after
/// the anchor, turns this `false` and keeps it `false` — which is the point.
///
/// ⛔ It is NOT enough to look at the marker on the LAST record, and that
/// mistake made this whole path dead code in production. `JsonlWriter`'s
/// metadata backstop fires once
/// [`session::jsonl::re_append::METADATA_REAPPEND_BACKSTOP_BYTES`] (32 KiB)
/// have been appended, re-emitting the current title as a PLAIN `custom-title`
/// — [`session::jsonl::re_append::plan_re_append`] rebuilds the record from
/// `{type, customTitle, sessionId}` and has no marker to carry. Worse, mobile
/// keeps ONE writer across sessions and `JsonlWriter::retarget` does not reset
/// that counter, so a user who chatted before pressing "+" can trip the
/// backstop on the interview's very FIRST append. An interview therefore
/// strips the marker as a matter of course, and a last-record test would make
/// every app created through this flow keep `untitled` forever.
///
/// So [`latest_custom_title`] anchors on the most recent MARKED record and
/// only counts a LATER unmarked record as a user rename when its text differs
/// from that anchor. A backstop echo copies the anchor verbatim; a `/rename`
/// does not.
///
/// The one case this cannot separate is a user who runs `/rename` and types
/// the placeholder string EXACTLY: `append_custom_title` then emits a record
/// byte-identical (modulo timestamp) to a backstop echo, so no reader can tell
/// them apart. Clause 3 of [`reconcile_app_init_session_title`] still declines
/// whenever the title already equals `record.name`, so the residue is a user
/// who deliberately renamed their session to `untitled` and then confirmed a
/// different app name.
///
/// Known cases where this declines for a session the user never touched. The
/// bias is deliberate and one-directional: a false negative costs a stale
/// title, a false positive overwrites something a user typed.
/// - a transcript with no `custom-title` at all — nothing this host anchored,
///   so nothing for it to reconcile;
/// - a CHAT-ORIGIN app, whose init session is forked by
///   `session::branch::create_branch_to_cwd`. That fork writes its own
///   unmarked `custom-title` (from `record.name`, i.e. the placeholder), so a
///   chat-origin shell keeps its `untitled` session title. Closing that would
///   mean either marking a forked, non-empty session as a mobile empty session
///   — which is what `mobileEmptySession` means elsewhere — or reasoning from
///   the title text, which is exactly what this function exists to avoid. It
///   is left open rather than papered over.
pub(crate) fn latest_custom_title_is_mobile_placeholder(
    transcript: &str,
    session_id: &str,
) -> bool {
    latest_custom_title(transcript, session_id).is_some_and(|(_, marker)| marker)
}

/// The ONE reconciliation between an app's pinned init session title and
/// `record.name`, shared by the `LocalAppScaffold` commit point (which calls it
/// immediately) and the boot backfill sweep (which is the retry that makes a
/// failed immediate rename recoverable rather than permanent).
///
/// The full predicate, all three clauses required:
/// 1. `record.scaffolded` — an app still in its interview is SUPPOSED to read
///    `untitled`; renaming it early would put a real name in the library on a
///    record that still opens the interview.
/// 2. the session's effective title is still one MOBILE wrote — the user has
///    not renamed it. Anchored on the most recent `mobileEmptySession: 1`
///    record, NOT on the marker of the last record: the transcript writer's
///    32 KiB metadata backstop re-emits the title unmarked, which is exactly
///    what an interview does. See [`latest_custom_title`] and
///    [`latest_custom_title_is_mobile_placeholder`].
/// 3. that title differs from `record.name` — otherwise there is nothing to do,
///    and this is also what makes the boot sweep idempotent.
///
/// The rename is written with `append_mobile_empty_session` again, KEEPING the
/// marker: the user still has not renamed anything, so a later `/rename` must
/// still be able to take precedence over a subsequent reconcile.
///
/// Returns `Ok(true)` when a rename was written, `Ok(false)` when the predicate
/// declined. A missing transcript is `Ok(false)`, not an error — the sweep's
/// re-anchor step, which runs before this one, writes `record.name` directly.
pub(crate) async fn reconcile_app_init_session_title(
    lingxi_home: &std::path::Path,
    data_root: &std::path::Path,
    fs: Arc<dyn traits::FileSystem>,
    record: &local_apps::AppRecord,
) -> Result<bool, String> {
    // Clause 1. Today no production state can reach this with a name that
    // differs from the session title — a shell is minted with `record.name`,
    // and `record.name` cannot change before the scaffold commits — so the
    // guard is unobservable through the app paths. It is still load-bearing as
    // a specification, and
    // `reconciliation_waits_for_the_scaffold_commit_before_renaming` pins it
    // directly so it cannot be deleted as dead code: a record that is still in
    // its interview must keep showing the placeholder, whatever its name says.
    if !record.scaffolded {
        return Ok(false);
    }
    let Some(init_id) = record.init_session_id.as_deref() else {
        return Ok(false);
    };
    let path = app_session_file(lingxi_home, data_root, record, init_id);
    let Some(path_str) = path.to_str() else {
        return Err(format!(
            "init-session path is not UTF-8: {}",
            path.display()
        ));
    };
    let Ok(file) = fs.read_file(path_str, None, None).await else {
        return Ok(false);
    };
    let Some((title, still_mobile_placeholder)) = latest_custom_title(&file.content, init_id)
    else {
        return Ok(false);
    };
    if !still_mobile_placeholder || title == record.name {
        return Ok(false);
    }
    session::jsonl::writer::JsonlWriter::new(path, fs)
        .append_mobile_empty_session(init_id, &record.name)
        .await
        .map_err(|error| format!("rename pinned init session: {error}"))?;
    Ok(true)
}

/// What to tell the agent immediately after an app is created.
///
/// It must NOT say "build it now". A create happens in a conversation that is
/// rooted somewhere ELSE — the library's intake chat sits in the project scope,
/// and an agent-driven create can happen in any chat at all. The new app's
/// workspace is a different directory, and the build workflow's agents inherit
/// the CALLING session's cwd, not the app's.
///
/// Observed on device: this used to read "the workspace already contains the
/// repository-verified foundation … then call LocalAppBuild", the agent obeyed
/// literally, and the whole build ran against the project workspace. It found a
/// previous run's leftover `apps/<other-id>/workspace` directory there and
/// edited that instead — every build failed on a workspace that was never the
/// app's, and nothing in the error said which directory was wrong.
///
/// The app already has its own session (`init_session_id` in this same
/// response). Handing off to it is what puts the agent in the right cwd with the
/// right `LINGXI.md` auto-loaded.
pub(crate) fn create_next_step_guidance() -> String {
    "The app now exists as an EMPTY shell, and this conversation is not rooted in it. Stop here: do not write source, do not call LocalAppBuild, and do not start a build workflow from this conversation — its working directory is not the app's workspace, so anything written here lands outside the app. The app has its own workspace and its own session (init_session_id in this result); continue there, where the guided workspace contract explains the interview and scaffold steps. Do not recreate the app, do not run a package-manager scaffold command, and do not install dependencies yet: runtime-profile confirmation and LocalAppScaffold happen first.".into()
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
    /// Stable native host facts, attached by the mobile composition root from
    /// the same `MobileConfig` that renders the mobile runtime reminder.
    ///
    /// This is the ONLY source of an app's device context. The agent cannot
    /// supply one: the reminder is all it sees, and the reminder's device
    /// vocabulary (`phone`/`tablet`) does not name an iOS form factor.
    /// Unattached — desktop embedders and host tests — means no context.
    host_environment: OnceLock<traits::MobileHostEnvironment>,
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
    pending_runtime_profile_selections:
        Mutex<HashMap<String, oneshot::Sender<Option<AppRuntimeProfileDto>>>>,
    pending_dependency_change_confirmations: Mutex<HashMap<String, oneshot::Sender<bool>>>,
    pending_ui: Mutex<HashMap<String, oneshot::Sender<UiResolution>>>,
    pending_runtime_profile_receipts: Mutex<HashMap<String, PendingRuntimeProfileReceipt>>,
    pending_dependency_change_receipts: Mutex<HashMap<String, PendingDependencyChangeReceipt>>,
    pending_mcp_receipts: Mutex<local_apps::McpReceiptBook>,
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
    /// App ids with a `LocalAppScaffold` transaction in flight — §C.1 step 1.
    ///
    /// ⛔ IN-PROCESS ONLY, and deliberately so. The obvious alternative — a
    /// persistent set-once field in the style of `AppService::set_init_session`
    /// — is WRONG here: a reservation that reaches disk survives the process
    /// being killed mid-transaction, and nothing ever clears it, so the draft
    /// is bricked forever. That is the exact opposite of §C.1 step 4's
    /// retry-safety. The engine is one process on device, so an in-process set
    /// is sufficient; after a restart the set is empty and `scaffolded` is
    /// still `false`, so the retry simply works.
    ///
    /// `AppService::with_app` cannot hold this either: its guard lives only as
    /// long as its own completion task, while steps 2-4 run entirely outside
    /// that lock.
    ///
    /// A std mutex behind an `Arc` on purpose, like `llm_inflight`: the slot is
    /// released by [`ScaffoldReservation::drop`], which cannot await, and the
    /// set is only ever insert/remove — no lock is ever held across an await.
    scaffold_reservations: Arc<std::sync::Mutex<std::collections::HashSet<String>>>,
    /// Where an app's pinned init session lives, so the §C.1 step 4 commit can
    /// rename it out of its `untitled` placeholder. See [`SessionCatalog`].
    ///
    /// Optional on purpose: a broker built without it (every unit-test root
    /// that has no session catalog at all) simply skips the immediate rename,
    /// and the boot backfill sweep — which is handed `lingxi_home` and the
    /// filesystem directly — still reconciles the title on the next launch.
    session_catalog: OnceLock<SessionCatalog>,
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
        if let Err(error) = Self::recover_dependency_updates_on_boot(&root) {
            tracing::warn!(
                root = %root.display(),
                %error,
                "dependency update recovery deferred until the next profile load"
            );
        }
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
            host_environment: OnceLock::new(),
            agent_executor: OnceLock::new(),
            agent_turns: Arc::new(Mutex::new(HashMap::new())),
            pending_profile_proposals: Mutex::new(HashMap::new()),
            background_task_writes: Mutex::new(()),
            background_inflight: Mutex::new(std::collections::HashSet::new()),
            recording_start: Mutex::new(()),
            self_ref: OnceLock::new(),
            pending_capabilities: Mutex::new(HashMap::new()),
            pending_runtime_profile_selections: Mutex::new(HashMap::new()),
            pending_dependency_change_confirmations: Mutex::new(HashMap::new()),
            pending_ui: Mutex::new(HashMap::new()),
            pending_runtime_profile_receipts: Mutex::new(HashMap::new()),
            pending_dependency_change_receipts: Mutex::new(HashMap::new()),
            pending_mcp_receipts: Mutex::new(local_apps::McpReceiptBook::default()),
            session_permissions: Mutex::new(SessionPermissions::default()),
            runtimes: Arc::new(Mutex::new(HashMap::new())),
            port_leases: Arc::new(std::sync::Mutex::new(HashMap::new())),
            port_allocation: Mutex::new(()),
            dependency_snapshot_locks: Mutex::new(HashMap::new()),
            scaffold_reservations: Arc::new(
                std::sync::Mutex::new(std::collections::HashSet::new()),
            ),
            session_catalog: OnceLock::new(),
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

    /// Bind the native host facts. Set once, at the same composition-root
    /// call site as [`Self::attach_agent_executor`].
    pub(crate) fn attach_host_environment(
        &self,
        environment: traits::MobileHostEnvironment,
    ) -> Result<(), traits::MobileHostEnvironment> {
        self.host_environment.set(environment)
    }

    /// Bind the session catalog the scaffold commit renames the pinned init
    /// session in. Set once, at the same composition-root call site as
    /// [`Self::attach_host_environment`].
    pub(crate) fn attach_session_catalog(
        &self,
        catalog: SessionCatalog,
    ) -> Result<(), SessionCatalog> {
        self.session_catalog.set(catalog)
    }

    /// The confirmed native target for apps generated on this host.
    ///
    /// `None` when no host facts are attached or the client could not classify
    /// the device — an absent context already means unknown, so neither case
    /// invents a platform.
    fn host_device_context(&self) -> Option<local_apps::DeviceContext> {
        self.host_environment
            .get()
            .and_then(local_apps::DeviceContext::from_host_environment)
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

    fn validate_workflow_run_id(workflow_run_id: &str) -> Result<(), String> {
        if workflow_run_id.is_empty()
            || workflow_run_id.len() > 128
            || !workflow_run_id
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '-')
        {
            return Err("workflow_run_id is invalid".into());
        }
        Ok(())
    }

    fn mcp_candidate_rel(
        app_id: &str,
        workflow_run_id: &str,
    ) -> Result<PathBuf, local_apps::AppError> {
        local_apps::ids::validate_app_id(app_id)?;
        Self::validate_workflow_run_id(workflow_run_id)
            .map_err(local_apps::AppError::InvalidRequest)?;
        Ok(PathBuf::from("apps")
            .join(app_id)
            .join(local_apps::manifest::MCP_CATALOGS_DIR)
            .join("candidates")
            .join(format!("{workflow_run_id}.json")))
    }

    fn save_mcp_candidate(
        &self,
        app_id: &str,
        workflow_run_id: &str,
        candidate: &PersistedMcpCandidate,
    ) -> Result<(), String> {
        let path =
            Self::mcp_candidate_rel(app_id, workflow_run_id).map_err(|error| error.to_string())?;
        let mut body = serde_json::to_vec_pretty(candidate)
            .map_err(|error| format!("serialize MCP candidate: {error}"))?;
        body.push(b'\n');
        traits::rooted_fs::atomic_write(
            &self.root,
            &path,
            &body,
            traits::rooted_fs::AtomicWriteOptions::default(),
        )
        .map_err(|error| local_apps::AppError::from_fs("write MCP candidate", &error).to_string())
    }

    fn load_mcp_candidate(
        &self,
        app_id: &str,
        workflow_run_id: &str,
    ) -> Result<PersistedMcpCandidate, String> {
        let path =
            Self::mcp_candidate_rel(app_id, workflow_run_id).map_err(|error| error.to_string())?;
        let body = traits::rooted_fs::read_to_string_limited(&self.root, &path, 512 * 1024)
            .map_err(|error| {
                local_apps::AppError::from_fs("read MCP candidate", &error).to_string()
            })?;
        serde_json::from_str(&body).map_err(|error| format!("parse MCP candidate: {error}"))
    }

    fn load_active_mcp_flow_contexts(
        &self,
        layout: &AppLayout,
    ) -> Result<BTreeMap<String, local_apps::AppMcpFlowContext>, String> {
        let rel = layout
            .workspace_rel()
            .join(".lingxi/mcp-flow-contexts.json");
        let body = traits::rooted_fs::read_to_string_limited(&self.root, &rel, 512 * 1024)
            .map_err(|error| match error {
                traits::FsError::NotFound(_) => {
                    "mcp_flow_contexts_missing: Host could not resolve any trusted MCP flow contexts for this app".to_string()
                }
                other => local_apps::AppError::from_fs("read MCP flow contexts", &other).to_string(),
            })?;
        serde_json::from_str(&body).map_err(|error| format!("parse MCP flow contexts: {error}"))
    }

    fn build_mcp_review_surface(
        manifest: &local_apps::AppManifest,
        validated: &local_apps::ValidatedAppMcpProposal,
        active_catalog: Option<&local_apps::AppMcpCatalogRef>,
    ) -> Value {
        json!({
            "appId": validated.proposal.app_id,
            "manifestRevision": manifest.revision,
            "summary": validated.proposal.summary,
            "proposalSha256": validated.proposal_sha256,
            "toolSurfaceSha256": validated.tool_surface_sha256,
            "tools": validated.tools.iter().map(|tool| json!({
                "name": tool.definition.name,
                "title": tool.definition.title,
                "description": tool.definition.description,
                "inputSchema": tool.definition.input_schema,
                "outputSchema": tool.definition.output_schema,
                "flow": tool.flow,
                "ceiling": tool.ceiling,
            })).collect::<Vec<_>>(),
            "requiredFlowChanges": validated.proposal.required_flow_changes,
            "excludedCapabilities": validated.proposal.excluded_capabilities,
            "previousActiveCatalog": active_catalog,
        })
    }

    async fn issue_runtime_profile_receipt(
        &self,
        app_id: &str,
        binding: local_apps::AppRuntimeProfileBinding,
    ) -> Result<PendingRuntimeProfileReceipt, String> {
        let issued_at_ms = now_ms();
        let expires_at_ms = issued_at_ms + RUNTIME_PROFILE_RECEIPT_TTL.as_millis() as u64;
        let mut receipts = self.pending_runtime_profile_receipts.lock().await;
        if let Some(current) = receipts.get(app_id) {
            if current.claimed && current.expires_at_ms >= issued_at_ms {
                return Err(format!(
                    "runtime profile receipt {} is already in use for app {}",
                    current.receipt_id, app_id
                ));
            }
        }
        let receipt = PendingRuntimeProfileReceipt {
            receipt_id: uuid::Uuid::new_v4().to_string(),
            app_id: app_id.to_string(),
            binding,
            issued_at_ms,
            expires_at_ms,
            claimed: false,
        };
        receipts.insert(app_id.to_string(), receipt.clone());
        Ok(receipt)
    }

    async fn claim_runtime_profile_receipt(
        &self,
        app_id: &str,
        receipt_id: &str,
    ) -> Result<local_apps::AppRuntimeProfileBinding, String> {
        let mut receipts = self.pending_runtime_profile_receipts.lock().await;
        let Some(current) = receipts.get_mut(app_id) else {
            return Err(format!(
                "runtime profile receipt {receipt_id} is missing or was already consumed for app {app_id}"
            ));
        };
        if current.receipt_id != receipt_id {
            return Err(format!(
                "runtime profile receipt {receipt_id} is stale or superseded for app {app_id}"
            ));
        }
        if current.expires_at_ms < now_ms() {
            return Err(format!(
                "runtime profile receipt {receipt_id} expired for app {app_id}"
            ));
        }
        if current.claimed {
            return Err(format!(
                "runtime profile receipt {receipt_id} is already in use for app {app_id}"
            ));
        }
        current.claimed = true;
        Ok(current.binding.clone())
    }

    async fn release_runtime_profile_receipt_claim(&self, app_id: &str, receipt_id: &str) {
        let mut receipts = self.pending_runtime_profile_receipts.lock().await;
        if let Some(current) = receipts.get_mut(app_id) {
            if current.receipt_id == receipt_id {
                current.claimed = false;
            }
        }
    }

    async fn consume_runtime_profile_receipt(&self, app_id: &str, receipt_id: &str) {
        let mut receipts = self.pending_runtime_profile_receipts.lock().await;
        if receipts
            .get(app_id)
            .is_some_and(|current| current.receipt_id == receipt_id)
        {
            receipts.remove(app_id);
        }
    }

    async fn issue_dependency_change_receipt(
        &self,
        app_id: &str,
        baseline: DependencyBaselineIdentity,
        requested_json: Vec<u8>,
        effective_package_json: Vec<u8>,
        summary: Vec<DependencyChange>,
    ) -> Result<PendingDependencyChangeReceipt, String> {
        let issued_at_ms = now_ms();
        let expires_at_ms = issued_at_ms + RUNTIME_PROFILE_RECEIPT_TTL.as_millis() as u64;
        let mut receipts = self.pending_dependency_change_receipts.lock().await;
        if let Some(current) = receipts.get(app_id) {
            if current.claimed && current.expires_at_ms >= issued_at_ms {
                return Err(format!(
                    "dependency change receipt {} is already in use for app {}",
                    current.receipt_id, app_id
                ));
            }
        }
        let receipt = PendingDependencyChangeReceipt {
            receipt_id: uuid::Uuid::new_v4().to_string(),
            app_id: app_id.to_string(),
            baseline,
            requested_json,
            effective_package_json,
            issued_at_ms,
            expires_at_ms,
            summary,
            claimed: false,
        };
        receipts.insert(app_id.to_string(), receipt.clone());
        Ok(receipt)
    }

    async fn claim_dependency_change_receipt(
        &self,
        app_id: &str,
        receipt_id: &str,
    ) -> Result<PendingDependencyChangeReceipt, String> {
        let mut receipts = self.pending_dependency_change_receipts.lock().await;
        let Some(current) = receipts.get_mut(app_id) else {
            return Err(format!(
                "dependency change receipt {receipt_id} is missing or was already consumed for app {app_id}"
            ));
        };
        if current.receipt_id != receipt_id {
            return Err(format!(
                "dependency change receipt {receipt_id} is stale or superseded for app {app_id}"
            ));
        }
        if current.expires_at_ms < now_ms() {
            return Err(format!(
                "dependency change receipt {receipt_id} expired for app {app_id}"
            ));
        }
        if current.claimed {
            return Err(format!(
                "dependency change receipt {receipt_id} is already in use for app {app_id}"
            ));
        }
        current.claimed = true;
        Ok(current.clone())
    }

    async fn release_dependency_change_receipt_claim(&self, app_id: &str, receipt_id: &str) {
        let mut receipts = self.pending_dependency_change_receipts.lock().await;
        if let Some(current) = receipts.get_mut(app_id) {
            if current.receipt_id == receipt_id {
                current.claimed = false;
            }
        }
    }

    async fn consume_dependency_change_receipt(&self, app_id: &str, receipt_id: &str) {
        let mut receipts = self.pending_dependency_change_receipts.lock().await;
        if receipts
            .get(app_id)
            .is_some_and(|current| current.receipt_id == receipt_id)
        {
            receipts.remove(app_id);
        }
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
        let toolchain_key_dir = PNPM_TOOLCHAIN_KEY
            .chars()
            .map(|ch| match ch {
                'a'..='z' | 'A'..='Z' | '0'..='9' | '.' | '-' => ch,
                _ => '_',
            })
            .collect::<String>();
        self.root
            .join("dependency-cache")
            .join("pnpm")
            .join(toolchain_key_dir)
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

    fn dependency_inputs_match(layout: &AppLayout) -> Result<bool, String> {
        let workspace = layout.root().join(layout.workspace_rel());
        let manifest = load_manifest(layout).map_err(|error| error.to_string())?;
        let expected_files: Vec<(&'static str, Vec<u8>)> = match manifest.runtime_profile.as_ref() {
            Some(binding) => {
                let contract = crate::local_app_runtime_profiles::contract_for_binding(binding)
                    .map_err(|error| error.to_string())?;
                let has_snapshot = manifest.dependency_snapshot.is_some();
                contract
                    .managed_files
                    .iter()
                    .copied()
                    .filter(|(relative, _)| {
                        matches!(
                            *relative,
                            "package.json" | "pnpm-lock.yaml" | "pnpm-workspace.yaml"
                        )
                    })
                    .map(|(relative, bytes)| {
                        let expected = match (relative, has_snapshot) {
                            ("package.json", true) => std::fs::read(workspace.join(
                                crate::local_app_runtime_profiles::EFFECTIVE_PACKAGE_FILE_REL,
                            ))
                            .map_err(|error| {
                                format!(
                                    "read {}: {error}",
                                    crate::local_app_runtime_profiles::EFFECTIVE_PACKAGE_FILE_REL
                                )
                            })?,
                            ("pnpm-lock.yaml", true) => std::fs::read(
                                workspace
                                    .join(crate::local_app_runtime_profiles::LOCKFILE_FILE_REL),
                            )
                            .map_err(|error| {
                                format!(
                                    "read {}: {error}",
                                    crate::local_app_runtime_profiles::LOCKFILE_FILE_REL
                                )
                            })?,
                            _ => bytes.to_vec(),
                        };
                        Ok((relative, expected))
                    })
                    .collect::<Result<Vec<_>, String>>()?
            }
            None => return Ok(false),
        };
        for (relative, expected) in expected_files {
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
                != expected
            {
                return Ok(false);
            }
        }
        Ok(true)
    }

    fn validate_dependency_package_name(package: &str) -> Result<(), String> {
        if package.is_empty() || package.len() > 214 {
            return Err(format!("invalid package name {package:?}"));
        }
        let chars: Vec<char> = package.chars().collect();
        if package.starts_with('@') {
            let slash_count = chars.iter().filter(|&&ch| ch == '/').count();
            if slash_count != 1 {
                return Err(format!(
                    "scoped package name must contain one slash: {package:?}"
                ));
            }
        } else if chars.iter().filter(|&&ch| ch == '/').count() != 0 {
            return Err(format!(
                "unscoped package name must not contain slash: {package:?}"
            ));
        }
        if chars.iter().any(|&ch| {
            !(ch.is_ascii_lowercase()
                || ch.is_ascii_digit()
                || matches!(ch, '@' | '/' | '.' | '_' | '-'))
        }) {
            return Err(format!(
                "package name {package:?} must use lowercase npm characters only"
            ));
        }
        Ok(())
    }

    fn validate_dependency_version(version: &str) -> Result<(), String> {
        let trimmed = version.trim();
        if trimmed.is_empty() {
            return Err("dependency version must not be empty".into());
        }
        let lowered = trimmed.to_ascii_lowercase();
        for forbidden in [
            "file:",
            "link:",
            "portal:",
            "patch:",
            "workspace:",
            "catalog:",
            "catalogs:",
            "npm:",
            "git+",
            "github:",
            "http://",
            "https://",
            "../",
            "./",
            "/",
            "\\",
        ] {
            if lowered.contains(forbidden) {
                return Err(format!(
                    "dependency version {version:?} must resolve from the npm registry only"
                ));
            }
        }
        Ok(())
    }

    fn dependency_manifest_bytes(
        contract: &crate::local_app_runtime_profiles::RuntimeProfileContract,
    ) -> Result<&'static [u8], String> {
        contract
            .managed_files
            .iter()
            .find(|(relative, _)| *relative == "package.json")
            .map(|(_, bytes)| *bytes)
            .ok_or_else(|| {
                format!(
                    "runtime profile {} r{} is missing package.json",
                    contract.family, contract.revision
                )
            })
    }

    fn load_requested_dependency_map(workspace: &Path) -> Result<BTreeMap<String, String>, String> {
        let bytes = Self::read_regular_dependency_input_bytes(
            workspace,
            crate::local_app_runtime_profiles::REQUESTED_FILE_REL,
        )?;
        let path = workspace.join(crate::local_app_runtime_profiles::REQUESTED_FILE_REL);
        let value: Value = serde_json::from_slice(&bytes)
            .map_err(|error| format!("parse {}: {error}", path.display()))?;
        let dependencies = value
            .get("dependencies")
            .and_then(Value::as_object)
            .ok_or_else(|| format!("{} must contain an object `dependencies`", path.display()))?;
        let mut map = BTreeMap::new();
        for (package, version) in dependencies {
            let version = version.as_str().ok_or_else(|| {
                format!(
                    "{} dependency {package:?} must map to a string version",
                    path.display()
                )
            })?;
            map.insert(package.clone(), version.to_string());
        }
        Ok(map)
    }

    fn read_regular_dependency_input_bytes(
        workspace: &Path,
        relative: &str,
    ) -> Result<Vec<u8>, String> {
        let path = workspace.join(relative);
        let metadata = std::fs::symlink_metadata(&path)
            .map_err(|error| format!("inspect {}: {error}", path.display()))?;
        if !metadata.is_file() || metadata.file_type().is_symlink() {
            return Err(format!(
                "dependencies_dirty: dependency input must be a regular file: {}",
                path.display()
            ));
        }
        std::fs::read(&path).map_err(|error| format!("read {}: {error}", path.display()))
    }

    fn load_trusted_dependency_baseline(
        layout: &AppLayout,
        dependency_record: &local_apps::AppDependencyRecord,
    ) -> Result<
        (
            local_apps::AppRuntimeProfileBinding,
            &'static crate::local_app_runtime_profiles::RuntimeProfileContract,
            BTreeMap<String, String>,
            DependencyBaselineIdentity,
        ),
        String,
    > {
        let manifest = load_manifest(layout).map_err(|error| error.to_string())?;
        let binding = manifest.runtime_profile.clone().ok_or_else(|| {
            format!(
                "app {} has no runtime profile yet; scaffold it before editing dependencies",
                layout.app_id()
            )
        })?;
        let snapshot = manifest.dependency_snapshot.clone().ok_or_else(|| {
            format!(
                "app {} has no verified dependency snapshot; rerun scaffold or dependency install before editing dependencies",
                layout.app_id()
            )
        })?;
        if snapshot.verified_profile_contract_sha256 != binding.contract_sha256 {
            return Err(format!(
                "runtime_contract_corrupt: app {} dependency snapshot no longer matches runtime profile {}",
                layout.app_id(),
                binding.family
            ));
        }
        if dependency_record.lockfile_sha256.as_deref() != Some(snapshot.lockfile_sha256.as_str())
            || dependency_record.toolchain_key.as_deref() != Some(snapshot.toolchain_key.as_str())
        {
            return Err(format!(
                "dependencies_dirty: app {} dependency record no longer matches the verified snapshot; run LocalAppUpdateDependencies to refresh it",
                layout.app_id()
            ));
        }
        let contract = crate::local_app_runtime_profiles::contract_for_binding(&binding)
            .map_err(|error| error.to_string())?;
        let workspace = layout.root().join(layout.workspace_rel());
        let requested_bytes = Self::read_regular_dependency_input_bytes(
            &workspace,
            crate::local_app_runtime_profiles::REQUESTED_FILE_REL,
        )?;
        if crate::local_app_runtime_profiles::hash_bytes(&requested_bytes)
            != snapshot.requested_sha256
        {
            return Err(format!(
                "dependencies_dirty: app {} requested dependency baseline was modified outside the host-managed dependency flow",
                layout.app_id()
            ));
        }
        let effective_package_bytes = Self::read_regular_dependency_input_bytes(
            &workspace,
            crate::local_app_runtime_profiles::EFFECTIVE_PACKAGE_FILE_REL,
        )?;
        if crate::local_app_runtime_profiles::hash_bytes(&effective_package_bytes)
            != snapshot.package_sha256
        {
            return Err(format!(
                "dependencies_dirty: app {} effective dependency baseline drifted from the verified snapshot",
                layout.app_id()
            ));
        }
        let lockfile_bytes = Self::read_regular_dependency_input_bytes(
            &workspace,
            crate::local_app_runtime_profiles::LOCKFILE_FILE_REL,
        )?;
        if crate::local_app_runtime_profiles::hash_bytes(&lockfile_bytes)
            != snapshot.lockfile_sha256
        {
            return Err(format!(
                "dependencies_dirty: app {} lockfile baseline drifted from the verified snapshot",
                layout.app_id()
            ));
        }
        let requested_dependencies = Self::load_requested_dependency_map(&workspace)?;
        let expected_package_json =
            Self::build_effective_package_json(contract, &requested_dependencies)?;
        if effective_package_bytes != expected_package_json {
            return Err(format!(
                "dependencies_dirty: app {} package.json no longer matches the committed dependency baseline",
                layout.app_id()
            ));
        }
        let baseline = DependencyBaselineIdentity {
            dependency_snapshot_sha256: manifest
                .dependency_snapshot_hash()
                .map_err(|error| error.to_string())?,
            requested_sha256: snapshot.requested_sha256,
            package_sha256: snapshot.package_sha256,
            lockfile_sha256: snapshot.lockfile_sha256,
            toolchain_key: snapshot.toolchain_key,
            contract_sha256: binding.contract_sha256.clone(),
        };
        Ok((binding, contract, requested_dependencies, baseline))
    }

    fn serialize_requested_dependency_map(
        dependencies: &BTreeMap<String, String>,
    ) -> Result<Vec<u8>, String> {
        let dependencies = dependencies
            .iter()
            .map(|(package, version)| (package.clone(), Value::String(version.clone())))
            .collect::<Map<String, Value>>();
        let mut bytes = serde_json::to_vec_pretty(&Value::Object(Map::from_iter([(
            "dependencies".to_string(),
            Value::Object(dependencies),
        )])))
        .map_err(|error| format!("serialize requested dependency manifest: {error}"))?;
        bytes.push(b'\n');
        Ok(bytes)
    }

    fn build_effective_package_json(
        contract: &crate::local_app_runtime_profiles::RuntimeProfileContract,
        requested_dependencies: &BTreeMap<String, String>,
    ) -> Result<Vec<u8>, String> {
        let template = Self::dependency_manifest_bytes(contract)?;
        let mut package_json: Value = serde_json::from_slice(template)
            .map_err(|error| format!("parse runtime profile package.json: {error}"))?;
        let package_object = package_json
            .as_object_mut()
            .ok_or_else(|| "runtime profile package.json must be an object".to_string())?;
        let mut dependencies = contract
            .core_packages
            .iter()
            .map(|(package, version)| (package.to_string(), Value::String((*version).to_string())))
            .collect::<BTreeMap<_, _>>();
        for (package, version) in requested_dependencies {
            dependencies.insert(package.clone(), Value::String(version.clone()));
        }
        package_object.insert(
            "dependencies".to_string(),
            Value::Object(Map::from_iter(dependencies)),
        );
        let mut bytes = serde_json::to_vec_pretty(&package_json)
            .map_err(|error| format!("serialize effective package.json: {error}"))?;
        bytes.push(b'\n');
        Ok(bytes)
    }

    fn prepare_dependency_change(
        layout: &AppLayout,
        dependency_record: &local_apps::AppDependencyRecord,
        changes_value: &Value,
    ) -> Result<
        (
            local_apps::AppRuntimeProfileBinding,
            DependencyBaselineIdentity,
            Vec<DependencyChange>,
            Vec<u8>,
            Vec<u8>,
        ),
        String,
    > {
        let changes: Vec<DependencyChange> = serde_json::from_value(changes_value.clone())
            .map_err(|error| {
                format!("invalid_argument: changes must be an array of objects: {error}")
            })?;
        if changes.is_empty() {
            return Err("invalid_argument: changes must not be empty".into());
        }
        let (binding, contract, mut requested_dependencies, baseline) =
            Self::load_trusted_dependency_baseline(layout, dependency_record)?;
        let original = requested_dependencies.clone();
        let core_packages = contract
            .core_packages
            .iter()
            .map(|(package, _)| *package)
            .collect::<std::collections::HashSet<_>>();
        for change in &changes {
            Self::validate_dependency_package_name(&change.package)?;
            if core_packages.contains(change.package.as_str()) {
                return Err(format!(
                    "dependency {} is core to runtime profile {} and can only change through runtime profile migration",
                    change.package, binding.family
                ));
            }
            match change.kind {
                DependencyChangeKind::Add | DependencyChangeKind::Update => {
                    let version = change.version.as_deref().ok_or_else(|| {
                        format!(
                            "dependency {} requires a version for {:?}",
                            change.package, change.kind
                        )
                    })?;
                    Self::validate_dependency_version(version)?;
                    requested_dependencies.insert(change.package.clone(), version.to_string());
                }
                DependencyChangeKind::Remove => {
                    if change.version.is_some() {
                        return Err(format!(
                            "dependency {} remove must not include a version",
                            change.package
                        ));
                    }
                    if requested_dependencies.remove(&change.package).is_none() {
                        return Err(format!(
                            "dependency {} is not currently requested by this app",
                            change.package
                        ));
                    }
                }
            }
        }
        if requested_dependencies == original {
            return Err("dependency change makes no observable change".into());
        }
        let requested_json = Self::serialize_requested_dependency_map(&requested_dependencies)?;
        let effective_package_json =
            Self::build_effective_package_json(contract, &requested_dependencies)?;
        Ok((
            binding,
            baseline,
            changes,
            requested_json,
            effective_package_json,
        ))
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

    fn reset_dependency_staging_node_modules(staging: &Path) -> Result<(), String> {
        let node_modules = staging.join("node_modules");
        Self::remove_owned_path(&node_modules)?;
        std::fs::create_dir_all(&node_modules).map_err(|error| {
            format!(
                "recreate dependency staging node_modules {}: {error}",
                node_modules.display()
            )
        })
    }

    fn dependency_install_request(
        build_mount: &MountSpec,
        store_mount: &MountSpec,
        dependency_staging_guest_path: String,
        build_state_root: &str,
        memory_mb: u32,
        network: NetworkPolicy,
        frozen_lockfile: bool,
        lockfile_only: bool,
        no_runtime: bool,
    ) -> LinuxCommandRequest {
        let mut env = BTreeMap::new();
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

        let mut args = vec!["install".into()];
        if lockfile_only {
            args.push("--lockfile-only".into());
        }
        args.push(if frozen_lockfile {
            "--frozen-lockfile".into()
        } else {
            "--no-frozen-lockfile".into()
        });
        args.push("--ignore-scripts".into());
        if no_runtime {
            args.push("--no-runtime".into());
        }
        args.extend([
            "--prefer-offline".into(),
            "--store-dir".into(),
            guest_paths::LOCAL_APP_DEPENDENCY_STORE.to_string(),
            "--reporter=append-only".into(),
        ]);

        LinuxCommandRequest {
            command: "/usr/bin/pnpm".into(),
            args,
            cwd: Some(dependency_staging_guest_path),
            env,
            stdin: None,
            timeout_ms: Some(DEPENDENCY_INSTALL_TIMEOUT.as_millis() as u64),
            network,
            resource_limits: ResourceLimits {
                max_memory_mb: Some(memory_mb),
                ..ResourceLimits::default()
            },
            mounts: vec![build_mount.clone(), store_mount.clone()],
        }
    }

    async fn run_dependency_install_command(
        runtime: &dyn MobileLinuxRuntime,
        request: LinuxCommandRequest,
    ) -> Result<(), String> {
        let network = request.network;
        let resource_limits = request.resource_limits;
        match runtime.run_isolated(request).await {
            Ok(result) => {
                result
                    .enforcement
                    .ensure_for(network, resource_limits)
                    .map_err(|error| error.to_string())?;
                if result.timed_out || result.cancelled || result.exit_code != 0 {
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
        }
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

    /// Adopt the read-only dependency tree staged into the app bundle as this
    /// device's snapshot for `lock_digest`.
    ///
    /// `stage-local-app-runtime.py` records `pnpm_lock_sha256` in
    /// `runtime-manifest.json` after validating the tree against the pinned
    /// template lockfile, so the seed carries its own identity and the match is
    /// exact rather than assumed. An app whose lockfile has drifted from the
    /// bundled one gets `false` and falls through to a real install -- the seed
    /// is an accelerator, never an override.
    ///
    /// Publication goes through `publish_dependency_snapshot` rather than
    /// writing an attestation here, so the tree digest and the marker are
    /// produced by the same code that validates every other snapshot.
    fn adopt_bundled_dependency_seed(
        runtime_root: &Path,
        lock_digest: &str,
        snapshot_root: &Path,
    ) -> Result<bool, String> {
        let manifest_path = runtime_root.join(BUNDLED_SEED_MANIFEST_FILE);
        let manifest = match std::fs::read(&manifest_path) {
            Ok(bytes) => bytes,
            // A build that ships no seed is the ordinary Store configuration,
            // not a fault: fall through to a real install.
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
            Err(error) => {
                return Err(format!(
                    "read bundled dependency seed manifest {}: {error}",
                    manifest_path.display()
                ))
            }
        };
        let manifest: Value = serde_json::from_slice(&manifest).map_err(|error| {
            format!(
                "parse bundled dependency seed manifest {}: {error}",
                manifest_path.display()
            )
        })?;
        let seed_digest = manifest
            .get("pnpm_lock_sha256")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                format!(
                    "bundled dependency seed manifest is missing pnpm_lock_sha256: {}",
                    manifest_path.display()
                )
            })?;
        if seed_digest != lock_digest {
            return Ok(false);
        }
        let seed_node_modules = runtime_root.join("node_modules");
        match std::fs::symlink_metadata(&seed_node_modules) {
            Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {}
            Ok(_) => {
                return Err(format!(
                    "bundled dependency seed is not a directory: {}",
                    seed_node_modules.display()
                ))
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
            Err(error) => {
                return Err(format!(
                    "inspect bundled dependency seed {}: {error}",
                    seed_node_modules.display()
                ))
            }
        }
        Self::publish_dependency_snapshot(&seed_node_modules, snapshot_root, lock_digest)?;
        Ok(true)
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
        if !Self::dependency_inputs_match(&layout)? {
            if load_manifest(&layout)
                .map_err(|error| error.to_string())?
                .runtime_profile
                .is_some()
            {
                return Err(
                    "dependencies_dirty: workspace package.json or pnpm-lock.yaml differs from the host-owned dependency snapshot; use LocalAppConfirmDependencyChange and LocalAppUpdateDependencies"
                        .into(),
                );
            } else {
                let target = crate::local_apps_build::detect_build_target(&layout)
                    .map_err(|error| error.to_string())?;
                crate::local_apps_build::restore_host_managed_files(&workspace, target)
                    .map_err(|error| error.to_string())?;
            }
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
        layout: &AppLayout,
        dependency_staging: &Path,
        expected_lock_digest: &str,
    ) -> Result<DependencyInstallCompletion, String> {
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
        refresh_runtime_profile_snapshot(layout, &tree_digest)?;
        Ok(DependencyInstallCompletion {
            lockfile_sha256: actual_lock_digest,
            toolchain_key: PNPM_TOOLCHAIN_KEY.to_string(),
        })
    }

    fn capture_dependency_update_rollback(
        &self,
        layout: &AppLayout,
        previous_dependency: local_apps::AppDependencyRecord,
    ) -> Result<DependencyUpdateRollback, String> {
        let workspace = layout.root().join(layout.workspace_rel());
        let mut files = Vec::new();
        for relative in [
            crate::local_app_runtime_profiles::REQUESTED_FILE_REL,
            crate::local_app_runtime_profiles::EFFECTIVE_PACKAGE_FILE_REL,
            crate::local_app_runtime_profiles::LOCKFILE_FILE_REL,
            crate::local_app_runtime_profiles::TREE_PROOF_FILE_REL,
            crate::local_app_runtime_profiles::SBOM_FILE_REL,
            crate::local_app_runtime_profiles::SNAPSHOT_FILE_REL,
            "package.json",
            "pnpm-lock.yaml",
            WORKSPACE_DEPENDENCY_ATTESTATION_FILE,
        ] {
            let path = workspace.join(relative);
            let bytes = match std::fs::read(&path) {
                Ok(bytes) => Some(bytes),
                Err(error) if error.kind() == io::ErrorKind::NotFound => None,
                Err(error) => {
                    return Err(format!(
                        "read dependency rollback source {}: {error}",
                        path.display()
                    ))
                }
            };
            files.push(DependencyUpdateFileBackup { relative, bytes });
        }
        let manifest_path = layout.root().join(layout.manifest_rel());
        let manifest_bytes = std::fs::read(&manifest_path).map_err(|error| {
            format!(
                "read dependency rollback manifest {}: {error}",
                manifest_path.display()
            )
        })?;
        let node_modules = workspace.join("node_modules");
        let node_modules_backup = match std::fs::symlink_metadata(&node_modules) {
            Ok(metadata) => {
                if metadata.file_type().is_symlink() {
                    return Err("workspace node_modules must not be a symlink".into());
                }
                if metadata.is_dir() {
                    let stamp = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|duration| duration.as_nanos())
                        .unwrap_or_default();
                    let backup = workspace
                        .join(".lingxi-build-state")
                        .join(format!("dependency-update-rollback-node_modules-{stamp}"));
                    Self::remove_owned_path(&backup)?;
                    clone_or_copy_tree(&node_modules, &backup).map_err(|error| {
                        format!("backup dependency tree {}: {error}", node_modules.display())
                    })?;
                    Some(backup)
                } else {
                    None
                }
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => None,
            Err(error) => {
                return Err(format!(
                    "inspect dependency rollback tree {}: {error}",
                    node_modules.display()
                ))
            }
        };
        let build_root = layout.root().join(layout.build_rel(false));
        let build_backup_result = (|| -> Result<Option<PathBuf>, String> {
            match std::fs::symlink_metadata(&build_root) {
                Ok(metadata) => {
                    if metadata.file_type().is_symlink() {
                        return Err("promoted build root must not be a symlink".into());
                    }
                    if !metadata.is_dir() {
                        return Ok(None);
                    }
                    let stamp = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|duration| duration.as_nanos())
                        .unwrap_or_default();
                    let backup = workspace
                        .join(".lingxi-build-state")
                        .join(format!("dependency-update-rollback-build-{stamp}"));
                    Self::remove_owned_path(&backup)?;
                    clone_or_copy_tree(&build_root, &backup).map_err(|error| {
                        format!("backup promoted build {}: {error}", build_root.display())
                    })?;
                    Ok(Some(backup))
                }
                Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
                Err(error) => Err(format!(
                    "inspect dependency rollback build {}: {error}",
                    build_root.display()
                )),
            }
        })();
        let build_backup = match build_backup_result {
            Ok(backup) => backup,
            Err(error) => {
                if let Some(backup) = &node_modules_backup {
                    let _ = Self::remove_owned_path(backup);
                }
                return Err(error);
            }
        };
        Ok(DependencyUpdateRollback {
            previous_dependency,
            files,
            manifest_bytes,
            node_modules_backup,
            build_backup,
        })
    }

    fn dependency_update_recovery_path(layout: &AppLayout) -> PathBuf {
        layout
            .root()
            .join(layout.workspace_rel())
            .join(DEPENDENCY_UPDATE_RECOVERY_FILE_REL)
    }

    fn dependency_update_workspace(layout: &AppLayout) -> Result<PathBuf, String> {
        let workspace = layout.root().join(layout.workspace_rel());
        let metadata = std::fs::symlink_metadata(&workspace).map_err(|error| {
            format!(
                "inspect dependency update workspace {}: {error}",
                workspace.display()
            )
        })?;
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            return Err(format!(
                "dependency update workspace is not a real directory: {}",
                workspace.display()
            ));
        }
        Ok(workspace)
    }

    fn dependency_update_file_is_allowed(relative: &str) -> bool {
        matches!(
            relative,
            crate::local_app_runtime_profiles::REQUESTED_FILE_REL
                | crate::local_app_runtime_profiles::EFFECTIVE_PACKAGE_FILE_REL
                | crate::local_app_runtime_profiles::LOCKFILE_FILE_REL
                | crate::local_app_runtime_profiles::TREE_PROOF_FILE_REL
                | crate::local_app_runtime_profiles::SBOM_FILE_REL
                | crate::local_app_runtime_profiles::SNAPSHOT_FILE_REL
                | "package.json"
                | "pnpm-lock.yaml"
                | WORKSPACE_DEPENDENCY_ATTESTATION_FILE
        )
    }

    fn dependency_update_backup_name(
        layout: &AppLayout,
        backup: &Path,
        kind: &str,
    ) -> Result<String, String> {
        let state_root = layout
            .root()
            .join(layout.workspace_rel())
            .join(".lingxi-build-state");
        let relative = backup.strip_prefix(&state_root).map_err(|_| {
            format!(
                "dependency rollback backup is outside the build state: {}",
                backup.display()
            )
        })?;
        let mut components = relative.components();
        let Some(Component::Normal(name)) = components.next() else {
            return Err(format!(
                "dependency rollback backup is not a single safe path: {}",
                backup.display()
            ));
        };
        if components.next().is_some() {
            return Err(format!(
                "dependency rollback backup is not a single safe path: {}",
                backup.display()
            ));
        }
        let name = name.to_str().ok_or_else(|| {
            format!(
                "dependency rollback backup name is not UTF-8: {}",
                backup.display()
            )
        })?;
        if !name.starts_with(kind) || name.len() == kind.len() {
            return Err(format!(
                "dependency rollback backup has an invalid name: {}",
                backup.display()
            ));
        }
        Ok(name.to_string())
    }

    fn dependency_update_recovery_journal(
        layout: &AppLayout,
        rollback: &DependencyUpdateRollback,
        status: DependencyUpdateRecoveryStatus,
    ) -> Result<DependencyUpdateRecoveryJournal, String> {
        let files = rollback
            .files
            .iter()
            .map(|file| DependencyUpdateRecoveryFile {
                relative: file.relative.to_string(),
                bytes: file.bytes.clone(),
            })
            .collect();
        let journal = DependencyUpdateRecoveryJournal {
            schema_version: DEPENDENCY_UPDATE_RECOVERY_SCHEMA_VERSION,
            app_id: layout.app_id().to_string(),
            status,
            previous_dependency: rollback.previous_dependency.clone(),
            files,
            manifest_bytes: rollback.manifest_bytes.clone(),
            node_modules_backup: rollback
                .node_modules_backup
                .as_deref()
                .map(|backup| {
                    Self::dependency_update_backup_name(
                        layout,
                        backup,
                        "dependency-update-rollback-node_modules-",
                    )
                })
                .transpose()?,
            build_backup: rollback
                .build_backup
                .as_deref()
                .map(|backup| {
                    Self::dependency_update_backup_name(
                        layout,
                        backup,
                        "dependency-update-rollback-build-",
                    )
                })
                .transpose()?,
        };
        Self::validate_dependency_update_recovery_journal(layout, &journal)?;
        Ok(journal)
    }

    fn dependency_update_backup_path(
        layout: &AppLayout,
        name: &str,
        kind: &str,
    ) -> Result<PathBuf, String> {
        if name.is_empty() || !name.starts_with(kind) || name.len() == kind.len() {
            return Err(format!("invalid dependency rollback backup name: {name:?}"));
        }
        let path = Path::new(name);
        let mut components = path.components();
        if !matches!(components.next(), Some(Component::Normal(_))) || components.next().is_some() {
            return Err(format!(
                "dependency rollback backup must be a single path component: {name:?}"
            ));
        }
        let state_root = layout
            .root()
            .join(layout.workspace_rel())
            .join(".lingxi-build-state");
        Ok(state_root.join(name))
    }

    fn validate_dependency_update_recovery_journal(
        layout: &AppLayout,
        journal: &DependencyUpdateRecoveryJournal,
    ) -> Result<(), String> {
        if journal.schema_version != DEPENDENCY_UPDATE_RECOVERY_SCHEMA_VERSION {
            return Err(format!(
                "dependency update recovery journal schemaVersion {} is unsupported (expected {})",
                journal.schema_version, DEPENDENCY_UPDATE_RECOVERY_SCHEMA_VERSION
            ));
        }
        if journal.app_id != layout.app_id() {
            return Err(format!(
                "dependency update recovery journal belongs to app {}, expected {}",
                journal.app_id,
                layout.app_id()
            ));
        }
        if journal.previous_dependency.app_id != layout.app_id() {
            return Err(format!(
                "dependency update recovery record belongs to app {}, expected {}",
                journal.previous_dependency.app_id,
                layout.app_id()
            ));
        }
        if journal.previous_dependency.schema_version != local_apps::APPS_SCHEMA_VERSION {
            return Err(format!(
                "dependency update recovery record schemaVersion {} is unsupported (expected {})",
                journal.previous_dependency.schema_version,
                local_apps::APPS_SCHEMA_VERSION
            ));
        }
        if journal.files.len() > 16 {
            return Err("dependency update recovery journal has too many files".into());
        }
        let mut total_bytes = journal.manifest_bytes.len();
        let mut seen = std::collections::HashSet::new();
        for file in &journal.files {
            if !Self::dependency_update_file_is_allowed(&file.relative) {
                return Err(format!(
                    "dependency update recovery journal contains an unexpected file: {}",
                    file.relative
                ));
            }
            let path = Path::new(&file.relative);
            if file.relative.is_empty()
                || path
                    .components()
                    .any(|component| !matches!(component, Component::Normal(_)))
                || !seen.insert(file.relative.as_str())
            {
                return Err(format!(
                    "dependency update recovery journal contains an unsafe or duplicate file: {}",
                    file.relative
                ));
            }
            total_bytes = total_bytes.saturating_add(file.bytes.as_ref().map_or(0, Vec::len));
            if total_bytes > MAX_DEPENDENCY_UPDATE_RECOVERY_BYTES {
                return Err("dependency update recovery journal is too large".into());
            }
        }
        for expected in [
            crate::local_app_runtime_profiles::REQUESTED_FILE_REL,
            crate::local_app_runtime_profiles::EFFECTIVE_PACKAGE_FILE_REL,
            crate::local_app_runtime_profiles::LOCKFILE_FILE_REL,
            crate::local_app_runtime_profiles::TREE_PROOF_FILE_REL,
            crate::local_app_runtime_profiles::SBOM_FILE_REL,
            crate::local_app_runtime_profiles::SNAPSHOT_FILE_REL,
            "package.json",
            "pnpm-lock.yaml",
            WORKSPACE_DEPENDENCY_ATTESTATION_FILE,
        ] {
            if !seen.iter().any(|relative| *relative == expected) {
                return Err(format!(
                    "dependency update recovery journal is missing file: {expected}"
                ));
            }
        }
        let manifest: local_apps::AppManifest = serde_json::from_slice(&journal.manifest_bytes)
            .map_err(|error| format!("parse dependency update recovery manifest: {error}"))?;
        if manifest.app_id != layout.app_id() {
            return Err(format!(
                "dependency update recovery manifest belongs to app {}, expected {}",
                manifest.app_id,
                layout.app_id()
            ));
        }
        manifest
            .validate()
            .map_err(|error| format!("validate dependency update recovery manifest: {error}"))?;
        if let Some(name) = &journal.node_modules_backup {
            Self::dependency_update_backup_path(
                layout,
                name,
                "dependency-update-rollback-node_modules-",
            )?;
        }
        if let Some(name) = &journal.build_backup {
            Self::dependency_update_backup_path(layout, name, "dependency-update-rollback-build-")?;
        }
        Ok(())
    }

    fn write_dependency_update_recovery_journal(
        layout: &AppLayout,
        journal: &DependencyUpdateRecoveryJournal,
    ) -> Result<(), String> {
        Self::validate_dependency_update_recovery_journal(layout, journal)?;
        let mut bytes = serde_json::to_vec(journal)
            .map_err(|error| format!("serialize dependency update recovery journal: {error}"))?;
        bytes.push(b'\n');
        if bytes.len() > MAX_DEPENDENCY_UPDATE_RECOVERY_BYTES {
            return Err("dependency update recovery journal is too large".into());
        }
        let workspace = Self::dependency_update_workspace(layout)?;
        crate::local_apps_build::write_file(
            &workspace,
            DEPENDENCY_UPDATE_RECOVERY_FILE_REL,
            &bytes,
            true,
        )
        .map_err(|error| format!("write dependency update recovery journal: {error}"))
    }

    fn load_dependency_update_recovery_journal(
        layout: &AppLayout,
    ) -> Result<Option<DependencyUpdateRecoveryJournal>, String> {
        let _workspace = Self::dependency_update_workspace(layout)?;
        let path = Self::dependency_update_recovery_path(layout);
        let metadata = match std::fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => {
                return Err(format!(
                    "inspect dependency update recovery journal {}: {error}",
                    path.display()
                ))
            }
        };
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Err(format!(
                "dependency update recovery journal is not a regular file: {}",
                path.display()
            ));
        }
        if metadata.len() > MAX_DEPENDENCY_UPDATE_RECOVERY_BYTES as u64 {
            return Err(format!(
                "dependency update recovery journal is too large: {}",
                path.display()
            ));
        }
        let bytes = std::fs::read(&path).map_err(|error| {
            format!(
                "read dependency update recovery journal {}: {error}",
                path.display()
            )
        })?;
        if bytes.len() > MAX_DEPENDENCY_UPDATE_RECOVERY_BYTES {
            return Err(format!(
                "dependency update recovery journal is too large: {}",
                path.display()
            ));
        }
        let journal: DependencyUpdateRecoveryJournal = serde_json::from_slice(&bytes)
            .map_err(|error| format!("parse dependency update recovery journal: {error}"))?;
        Self::validate_dependency_update_recovery_journal(layout, &journal)?;
        Ok(Some(journal))
    }

    fn remove_dependency_update_recovery_journal(layout: &AppLayout) -> Result<(), String> {
        Self::remove_owned_path(&Self::dependency_update_recovery_path(layout))
    }

    fn restore_dependency_update_recovery_files(
        layout: &AppLayout,
        journal: &DependencyUpdateRecoveryJournal,
    ) -> Result<(), String> {
        Self::validate_dependency_update_recovery_journal(layout, journal)?;
        let workspace = Self::dependency_update_workspace(layout)?;
        for file in &journal.files {
            let path = workspace.join(&file.relative);
            match &file.bytes {
                Some(bytes) => {
                    crate::local_apps_build::write_file(&workspace, &file.relative, bytes, true)
                        .map_err(|error| error.to_string())?
                }
                None => {
                    if std::fs::symlink_metadata(&path).is_ok() {
                        Self::remove_owned_path(&path)?;
                    }
                }
            }
        }
        let manifest: local_apps::AppManifest = serde_json::from_slice(&journal.manifest_bytes)
            .map_err(|error| format!("parse dependency rollback manifest: {error}"))?;
        local_apps::save_manifest(layout, &manifest).map_err(|error| error.to_string())?;

        let node_modules = workspace.join("node_modules");
        if std::fs::symlink_metadata(&node_modules).is_ok() {
            Self::remove_owned_path(&node_modules)?;
        }
        if let Some(name) = &journal.node_modules_backup {
            let backup = Self::dependency_update_backup_path(
                layout,
                name,
                "dependency-update-rollback-node_modules-",
            )?;
            let metadata = std::fs::symlink_metadata(&backup).map_err(|error| {
                format!(
                    "inspect dependency rollback tree {}: {error}",
                    backup.display()
                )
            })?;
            if metadata.file_type().is_symlink() || !metadata.is_dir() {
                return Err(format!(
                    "dependency rollback tree is not a real directory: {}",
                    backup.display()
                ));
            }
            clone_or_copy_tree(&backup, &node_modules).map_err(|error| {
                format!(
                    "restore dependency rollback tree {}: {error}",
                    node_modules.display()
                )
            })?;
        }

        let build_root = layout.root().join(layout.build_rel(false));
        if std::fs::symlink_metadata(&build_root).is_ok() {
            Self::remove_owned_path(&build_root)?;
        }
        if let Some(name) = &journal.build_backup {
            let backup = Self::dependency_update_backup_path(
                layout,
                name,
                "dependency-update-rollback-build-",
            )?;
            let metadata = std::fs::symlink_metadata(&backup).map_err(|error| {
                format!(
                    "inspect dependency rollback build {}: {error}",
                    backup.display()
                )
            })?;
            if metadata.file_type().is_symlink() || !metadata.is_dir() {
                return Err(format!(
                    "dependency rollback build is not a real directory: {}",
                    backup.display()
                ));
            }
            clone_or_copy_tree(&backup, &build_root).map_err(|error| {
                format!(
                    "restore dependency rollback build {}: {error}",
                    build_root.display()
                )
            })?;
        }
        local_apps::storage::save_dependency_record(layout.root(), &journal.previous_dependency)
            .map_err(|error| format!("restore dependency record: {error}"))
    }

    fn cleanup_dependency_update_recovery(
        layout: &AppLayout,
        journal: &DependencyUpdateRecoveryJournal,
    ) -> Result<(), String> {
        Self::validate_dependency_update_recovery_journal(layout, journal)?;
        let workspace = Self::dependency_update_workspace(layout)?;
        let staging = workspace.join(".lingxi-build-state/dependency-staging");
        Self::remove_owned_path(&staging)?;
        if let Some(name) = &journal.node_modules_backup {
            let path = Self::dependency_update_backup_path(
                layout,
                name,
                "dependency-update-rollback-node_modules-",
            )?;
            Self::remove_owned_path(&path)?;
        }
        if let Some(name) = &journal.build_backup {
            let path = Self::dependency_update_backup_path(
                layout,
                name,
                "dependency-update-rollback-build-",
            )?;
            Self::remove_owned_path(&path)?;
        }
        Self::remove_dependency_update_recovery_journal(layout)
    }

    pub(crate) fn recover_dependency_updates_on_boot(root: &Path) -> Result<(), String> {
        let apps_root = root.join("apps");
        let metadata = match std::fs::symlink_metadata(&apps_root) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(format!("inspect local apps directory: {error}")),
        };
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            return Err(format!(
                "local apps directory is not a real directory: {}",
                apps_root.display()
            ));
        }
        let mut first_error = None;
        let entries = std::fs::read_dir(&apps_root)
            .map_err(|error| format!("read local apps directory: {error}"))?;
        for entry in entries {
            let entry = match entry {
                Ok(entry) => entry,
                Err(error) => {
                    if first_error.is_none() {
                        first_error = Some(format!("read local app entry: {error}"));
                    }
                    continue;
                }
            };
            let app_path = entry.path();
            let app_metadata = match std::fs::symlink_metadata(&app_path) {
                Ok(metadata) => metadata,
                Err(error) => {
                    if first_error.is_none() {
                        first_error = Some(format!(
                            "inspect local app entry {}: {error}",
                            app_path.display()
                        ));
                    }
                    continue;
                }
            };
            if app_metadata.file_type().is_symlink() || !app_metadata.is_dir() {
                continue;
            }
            let app_name = entry.file_name();
            let Some(app_id) = app_name.to_str() else {
                continue;
            };
            let Ok(layout) = AppLayout::new(root.to_path_buf(), app_id.to_string()) else {
                continue;
            };
            let has_journal =
                match std::fs::symlink_metadata(Self::dependency_update_recovery_path(&layout)) {
                    Ok(_) => true,
                    Err(error) if error.kind() == io::ErrorKind::NotFound => false,
                    Err(error) => {
                        if first_error.is_none() {
                            first_error = Some(format!(
                                "inspect dependency update recovery journal for {app_id}: {error}"
                            ));
                        }
                        false
                    }
                };
            if !has_journal {
                continue;
            }
            let recovery_result = (|| -> Result<(), String> {
                let _build_lock =
                    local_apps::storage::lock_app_build(root, app_id).map_err(|error| {
                        format!("lock app {app_id} for dependency recovery: {error}")
                    })?;
                let Some(journal) = Self::load_dependency_update_recovery_journal(&layout)? else {
                    return Ok(());
                };
                match journal.status {
                    DependencyUpdateRecoveryStatus::InProgress => {
                        Self::restore_dependency_update_recovery_files(&layout, &journal)?;
                        // Once all authoritative old state is restored, make
                        // cleanup idempotent across another crash. A committed
                        // journal means "keep what is on disk"; the on-disk
                        // state is now the old state.
                        let mut cleaned = journal.clone();
                        cleaned.status = DependencyUpdateRecoveryStatus::Committed;
                        Self::write_dependency_update_recovery_journal(&layout, &cleaned)?;
                        Self::cleanup_dependency_update_recovery(&layout, &cleaned)
                    }
                    DependencyUpdateRecoveryStatus::Committed => {
                        Self::cleanup_dependency_update_recovery(&layout, &journal)
                    }
                }
            })();
            if let Err(error) = recovery_result {
                if first_error.is_none() {
                    first_error = Some(error);
                }
            }
        }
        match first_error {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }

    async fn restore_dependency_update_rollback(
        &self,
        service: &Arc<AppService>,
        app_id: &str,
        layout: &AppLayout,
        rollback: &DependencyUpdateRollback,
    ) -> Result<(), String> {
        if rollback.previous_dependency.app_id != app_id {
            return Err(format!(
                "dependency rollback record belongs to app {}, expected {app_id}",
                rollback.previous_dependency.app_id
            ));
        }
        let journal = Self::dependency_update_recovery_journal(
            layout,
            rollback,
            DependencyUpdateRecoveryStatus::InProgress,
        )?;
        Self::restore_dependency_update_recovery_files(layout, &journal)?;
        service
            .restore_dependency_record(rollback.previous_dependency.clone())
            .await
            .map_err(|error| error.to_string())?;
        Ok(())
    }

    fn discard_dependency_update_rollback(rollback: DependencyUpdateRollback) {
        if let Some(backup) = rollback.node_modules_backup {
            if let Err(error) = Self::remove_owned_path(&backup) {
                tracing::warn!(path = %backup.display(), error = %error, "failed to remove dependency rollback tree after commit");
            }
        }
        if let Some(backup) = rollback.build_backup {
            if let Err(error) = Self::remove_owned_path(&backup) {
                tracing::warn!(path = %backup.display(), error = %error, "failed to remove build rollback tree after commit");
            }
        }
    }

    async fn dependency_install_once(
        &self,
        layout: &AppLayout,
        app_id: &str,
    ) -> Result<DependencyInstallCompletion, String> {
        let workspace = layout.root().join(layout.workspace_rel());
        let lock_digest = match Self::dependency_lock_digest(&layout) {
            Ok(digest) => digest,
            Err(error) => return Err(error),
        };
        let snapshot_root = self.dependency_snapshot_root(&lock_digest);
        let snapshot_lock = self.dependency_snapshot_lock(&lock_digest).await;
        let _snapshot_guard = snapshot_lock.lock().await;
        let dependency_staging = match Self::prepare_dependency_staging(&layout) {
            Ok(path) => path,
            Err(error) => return Err(error),
        };
        let mut snapshot_ready =
            match Self::dependency_snapshot_is_ready(&snapshot_root, &lock_digest) {
                Ok(ready) => ready,
                Err(error) => {
                    let _ = Self::remove_owned_path(&dependency_staging);
                    return Err(error);
                }
            };
        if !snapshot_ready {
            // First install on this device: the app bundle already carries a
            // tree resolved from the pinned template lockfile, so adopt it
            // instead of resolving the same 169 packages over the network
            // inside the Linux guest.
            //
            // A seed that cannot be adopted is never fatal. Store builds ship
            // none at all, an app whose lockfile has drifted legitimately needs
            // a real install, and a damaged bundle should degrade to the slow
            // path rather than make app creation impossible -- so failures are
            // recorded and fall through.
            if let Ok(runtime_root) = self.configured_runtime_root() {
                match Self::adopt_bundled_dependency_seed(
                    &runtime_root,
                    &lock_digest,
                    &snapshot_root,
                ) {
                    Ok(adopted) => snapshot_ready = adopted,
                    Err(error) => {
                        tracing::warn!(app_id = %app_id, error = %error, "bundled dependency seed could not be adopted");
                    }
                }
            }
        }
        if snapshot_ready {
            if let Err(error) =
                Self::materialize_dependency_snapshot(&snapshot_root, &dependency_staging)
            {
                let _ = Self::remove_owned_path(&dependency_staging);
                return Err(error);
            }
            return self
                .finalize_dependency_install(&layout, &dependency_staging, &lock_digest)
                .await;
        }
        let Some(runtime) = self.mobile_linux() else {
            let _ = Self::remove_owned_path(&dependency_staging);
            return Err(
                "the mobile Node runtime is unavailable for dependency installation".into(),
            );
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
            return Err(message);
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
        let memory_mb =
            crate::local_apps_build::build_memory_budget_mb(self.physical_memory_bytes());
        let request = Self::dependency_install_request(
            &build_mount,
            &store_mount,
            dependency_staging_guest_path,
            &build_state_root,
            memory_mb,
            NetworkPolicy::Allowed,
            true,
            false,
            true,
        );
        let outcome = Self::run_dependency_install_command(runtime.as_ref(), request).await;
        match outcome {
            Ok(()) => {
                if let Err(error) = Self::publish_dependency_snapshot(
                    &dependency_staging.join("node_modules"),
                    &snapshot_root,
                    &lock_digest,
                ) {
                    let _ = Self::remove_owned_path(&dependency_staging);
                    return Err(error);
                }
                self.finalize_dependency_install(&layout, &dependency_staging, &lock_digest)
                    .await
            }
            Err(error) => {
                let _ = Self::remove_owned_path(&dependency_staging);
                Err(error)
            }
        }
    }

    async fn install_scaffold_dependencies(
        &self,
        service: &Arc<AppService>,
        app_id: &str,
        layout: &AppLayout,
    ) -> Result<(), String> {
        service
            .start_dependency_install(app_id)
            .await
            .map_err(|error| error.to_string())?;
        match self.dependency_install_once(layout, app_id).await {
            Ok(completion) => service
                .complete_dependency_install_with_metadata(
                    app_id,
                    Some(completion.lockfile_sha256),
                    Some(completion.toolchain_key),
                )
                .await
                .map(|_| ())
                .map_err(|error| error.to_string()),
            Err(error) => {
                let _ = service.fail_dependency_install(app_id, error.clone()).await;
                Err(error)
            }
        }
    }

    async fn install_uncommitted_create_dependencies(
        &self,
        record: &local_apps::AppRecord,
        layout: &AppLayout,
    ) -> Result<(), String> {
        update_uncommitted_dependency_record(&self.root, record, |dependency| {
            dependency.state = local_apps::AppDependencyState::Installing;
            dependency.install_attempts = dependency.install_attempts.saturating_add(1);
            dependency.last_error = None;
            dependency.updated_at_ms = now_ms();
        })?;
        match self.dependency_install_once(layout, &record.id).await {
            Ok(completion) => {
                update_uncommitted_dependency_record(&self.root, record, |dependency| {
                    dependency.state = local_apps::AppDependencyState::Ready;
                    dependency.lockfile_sha256 = Some(completion.lockfile_sha256);
                    dependency.toolchain_key = Some(completion.toolchain_key);
                    dependency.last_error = None;
                    dependency.updated_at_ms = now_ms();
                })
            }
            Err(error) => {
                let _ = update_uncommitted_dependency_record(&self.root, record, |dependency| {
                    dependency.state = local_apps::AppDependencyState::Failed;
                    dependency.last_error = Some(error.clone());
                    dependency.updated_at_ms = now_ms();
                });
                Err(error)
            }
        }
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
        if let Err(error) = self
            .install_scaffold_dependencies(&service, &app_id, &layout)
            .await
        {
            tracing::warn!(app_id = %app_id, error = %error, "dependency install failed");
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

    pub(crate) async fn resolve_runtime_profile_selection(
        &self,
        request_id: &str,
        selected_family: Option<AppRuntimeProfileDto>,
    ) -> bool {
        self.pending_runtime_profile_selections
            .lock()
            .await
            .remove(request_id)
            .is_some_and(|sender| sender.send(selected_family).is_ok())
    }

    /// Resolve one native dependency-change confirmation request.  This is a
    /// separate one-shot channel from generic capability approvals so the
    /// package diff and supply-chain policy shown by the client cannot be
    /// replaced by a generic allow/deny response.
    pub(crate) async fn resolve_dependency_change_confirmation(
        &self,
        request_id: &str,
        approved: bool,
    ) -> bool {
        self.pending_dependency_change_confirmations
            .lock()
            .await
            .remove(request_id)
            .is_some_and(|sender| sender.send(approved).is_ok())
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

    async fn runtime_profiles_value(&self, _input: Value) -> Result<Value, String> {
        Ok(json!({
            "profiles": crate::local_app_runtime_profiles::list_runtime_profiles()
                .into_iter()
                .map(|entry| {
                    let dependency_status = if entry.available {
                        self.runtime_profile_dependency_availability(entry.family, entry.revision)
                    } else {
                        RuntimeProfileDependencyAvailability::DownloadRequired
                    };
                    json!({
                        "family": entry.family.as_str(),
                        "revision": entry.revision,
                        "surface": entry.surface.as_str(),
                        "toolchain_key": entry.toolchain_key,
                        "core_packages": entry.core_packages,
                        "contract_sha256": entry.contract_sha256,
                        "available": entry.available,
                        "availability_reason": entry.availability_reason,
                        // Cache/download both describe the exact dependency
                        // provenance. A compiled source bundle is not a
                        // dependency cache: only a verified shared snapshot
                        // is `cached`, a matching configured seed is
                        // `bundled`, and neither is `download_required`.
                        "cache_status": if !entry.available { "unavailable" } else {
                            dependency_status.as_str()
                        },
                        "download_status": if !entry.available { "gated" } else {
                            dependency_status.as_str()
                        },
                        "available_migrations": local_apps::RUNTIME_PROFILE_MIGRATION_EDGES
                            .iter()
                            .filter(|edge| edge.family == entry.family && edge.from_revision == entry.revision)
                            .map(|edge| json!({
                                "family": edge.family.as_str(),
                                "from_revision": edge.from_revision,
                                "to_revision": edge.to_revision,
                                "rebuild_compatible": edge.rebuild_compatible,
                            }))
                            .collect::<Vec<_>>(),
                    })
                })
                .collect::<Vec<_>>(),
        }))
    }

    fn read_json_object(path: &Path) -> Option<Value> {
        let metadata = std::fs::symlink_metadata(path).ok()?;
        if !metadata.is_file() || metadata.file_type().is_symlink() {
            return None;
        }
        let bytes = std::fs::read(path).ok()?;
        let value: Value = serde_json::from_slice(bytes.as_slice()).ok()?;
        let _ = value.as_object()?;
        Some(value)
    }

    /// Whether this host already has a verified dependency tree for the exact
    /// profile lock, either in the shared cache or in the configured bundled
    /// runtime seed. Read-only: unlike `adopt_bundled_dependency_seed`, this
    /// helper never publishes a cache entry.
    fn runtime_profile_dependency_availability(
        &self,
        family: AppRuntimeProfile,
        revision: u32,
    ) -> RuntimeProfileDependencyAvailability {
        let Ok(binding) = crate::local_app_runtime_profiles::current_binding_for_family(family)
        else {
            return RuntimeProfileDependencyAvailability::DownloadRequired;
        };
        if binding.revision != revision {
            return RuntimeProfileDependencyAvailability::DownloadRequired;
        }
        let Ok(contract) = crate::local_app_runtime_profiles::contract_for_binding(&binding) else {
            return RuntimeProfileDependencyAvailability::DownloadRequired;
        };
        let lock_digest = crate::local_app_runtime_profiles::lockfile_sha256(contract);
        let snapshot_root = self.dependency_snapshot_root(&lock_digest);
        if Self::dependency_snapshot_is_ready(&snapshot_root, &lock_digest).unwrap_or(false) {
            return RuntimeProfileDependencyAvailability::Cached;
        }
        let Ok(runtime_root) = self.configured_runtime_root() else {
            return RuntimeProfileDependencyAvailability::DownloadRequired;
        };
        let Some(manifest) = Self::read_json_object(&runtime_root.join(BUNDLED_SEED_MANIFEST_FILE))
        else {
            return RuntimeProfileDependencyAvailability::DownloadRequired;
        };
        if manifest.get("pnpm_lock_sha256").and_then(Value::as_str) != Some(lock_digest.as_str()) {
            return RuntimeProfileDependencyAvailability::DownloadRequired;
        }
        let Ok(metadata) = std::fs::symlink_metadata(runtime_root.join("node_modules")) else {
            return RuntimeProfileDependencyAvailability::DownloadRequired;
        };
        if metadata.is_dir() && !metadata.file_type().is_symlink() {
            RuntimeProfileDependencyAvailability::Bundled
        } else {
            RuntimeProfileDependencyAvailability::DownloadRequired
        }
    }

    async fn confirm_runtime_profile_value(&self, input: Value) -> Result<Value, String> {
        let app_id = required_string(&input, "app_id")?.to_string();
        let recommended = input
            .get("recommended_profile")
            .and_then(Value::as_str)
            .map(local_apps::AppRuntimeProfile::parse)
            .transpose()
            .map_err(|error| format!("invalid_argument: {error}"))?;
        let service = self.service()?;
        let record = service
            .record(&app_id)
            .await
            .map_err(|error| error.to_string())?;
        if record.scaffolded {
            return Err(format!(
                "app {app_id} is already scaffolded; runtime profile is immutable after scaffold"
            ));
        }
        let options = crate::local_app_runtime_profiles::list_runtime_profiles()
            .into_iter()
            .map(|entry| {
                let dependency_status = if entry.available {
                    self.runtime_profile_dependency_availability(entry.family, entry.revision)
                } else {
                    RuntimeProfileDependencyAvailability::DownloadRequired
                };
                AppRuntimeProfileOptionDto {
                    family: lower_runtime_profile_family(entry.family),
                    revision: entry.revision,
                    contract_sha256: entry.contract_sha256.clone(),
                    surface: lower_surface(entry.surface),
                    core_packages: entry
                        .core_packages
                        .into_iter()
                        .map(|(name, version)| AppRuntimeProfilePackageDto {
                            name: name.to_string(),
                            version: version.to_string(),
                        })
                        .collect(),
                    cache_status: if entry.available {
                        dependency_status.as_str()
                    } else {
                        "unavailable"
                    }
                    .into(),
                    download_status: if entry.available {
                        dependency_status.as_str()
                    } else {
                        "gated"
                    }
                    .into(),
                    available: entry.available,
                    reason: entry.availability_reason.map(str::to_string),
                }
            })
            .collect::<Vec<_>>();
        let request_id = self.request_id("app-runtime-profile-selection");
        let (sender, receiver) = oneshot::channel();
        self.pending_runtime_profile_selections
            .lock()
            .await
            .insert(request_id.clone(), sender);
        self.event_sink
            .emit(ClientEvent::AppEvent {
                event: AppEventDto::AppRuntimeProfileSelectionRequested {
                    request: AppRuntimeProfileSelectionRequestDto {
                        request_id: request_id.clone(),
                        app_id: app_id.clone(),
                        reason: "Choose the immutable runtime family for this Local App. The family cannot be changed after scaffold; later upgrades require an explicit same-family migration.".into(),
                        recommended_family: recommended.map(lower_runtime_profile_family),
                        options,
                    },
                },
            })
            .await;
        let selected = match timeout(APPROVAL_TIMEOUT, receiver).await {
            Ok(Ok(Some(selected))) => raise_runtime_profile_family(selected)?,
            Ok(Ok(None)) => return Err("user cancelled runtime profile selection".into()),
            Ok(Err(_)) => return Err("runtime profile selection was cancelled".into()),
            Err(_) => {
                self.pending_runtime_profile_selections
                    .lock()
                    .await
                    .remove(&request_id);
                return Err("runtime profile selection timed out".into());
            }
        };
        let binding = crate::local_app_runtime_profiles::current_binding_for_family(selected)
            .map_err(|error| format!("selected runtime profile is unavailable: {error}"))?;
        let receipt = self
            .issue_runtime_profile_receipt(&app_id, binding.clone())
            .await?;
        Ok(json!({
            "ok": true,
            "app_id": app_id,
            "runtime_profile": {
                "family": binding.family.as_str(),
                "revision": binding.revision,
                "contract_sha256": binding.contract_sha256,
                "surface": binding.family.surface().as_str(),
            },
            "receipt": {
                "id": receipt.receipt_id,
                "app_id": receipt.app_id,
                "issued_at_ms": receipt.issued_at_ms,
                "expires_at_ms": receipt.expires_at_ms,
            }
        }))
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
        tokio::task::spawn_blocking(move || {
            AppDataStore::with_cached(layout, |store| store.query(&manifest, &query))
        })
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
            AppDataStore::with_cached(layout, |store| store.mutate(&manifest, &mutations, now_ms))
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
        if let Err(error) = crate::local_apps_build::validate_build_for_launch(&layout) {
            return self
                .fail_reserved_runtime_start(app_id, generation, None, error.to_string())
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

    /// Tell the CLIENT that an agent-driven create failed.
    ///
    /// The tool result already tells the model, and that used to be the only
    /// notification: `emit_app_failure` is reachable exclusively from the
    /// command handlers, so a create started by the agent produced no client
    /// event on either outcome. A client that armed a "creating…" state when
    /// the user submitted a brief therefore had nothing to disarm it with — the
    /// spinner and the disabled create button stayed that way until the app was
    /// killed.
    ///
    /// `app_id` is `None` because there is no app: the failure is precisely
    /// that one never came into being.
    pub(crate) async fn emit_create_failure(&self, error: &local_apps::AppError) {
        self.event_sink
            .emit(ClientEvent::AppOperationFailed {
                app_id: None,
                code: crate::local_apps_bridge::lower_error_code(error.code()),
                message: error.to_string(),
                // Correctly `None`, not a stub: `request_id` is the
                // correlation key a client puts on its own `CreateApp`, and
                // this failure belongs to an AGENT-driven create that no
                // client command started. There is nothing to echo.
                request_id: None,
            })
            .await;
    }

    /// Write the GUIDED workspace contract for a `CreateMode::Shell` app —
    /// the pre-commit initializer of the "+" button's create.
    ///
    /// This is the SHELL twin of [`Self::scaffold_app_value`], and the
    /// difference is the whole point: it lays down no source, stamps no
    /// surface, and touches nothing but `workspace/LINGXI.md`. A shell has no
    /// shape yet, so there is nothing to scaffold; what it needs is a contract
    /// that sends the agent to interview the user.
    ///
    /// Runs inside the create transaction, after `layout.initialize()` (so the
    /// workspace directory exists) and BEFORE the index commit that makes the
    /// app visible — an initializer failure rolls the whole create back, so an
    /// app can never become visible with an empty workspace and no contract.
    ///
    /// ⚠️ `workspace/LINGXI.md` is the ONE channel that reaches the model on
    /// every turn (it is auto-loaded by the memory hierarchy for any session
    /// rooted in this workspace). If this file is missing, the interview never
    /// starts: the agent sees an empty directory, assumes a normal app, and
    /// starts writing source that `LocalAppScaffold` is going to delete.
    pub(crate) async fn write_guided_contract_value(
        &self,
        record: &local_apps::AppRecord,
    ) -> Result<(), String> {
        let layout = self.layout(&record.id)?;
        let workspace = layout.root().join(layout.workspace_rel());
        let contract = guided_workspace_contract(record);
        tokio::task::spawn_blocking(move || {
            std::fs::write(workspace.join("LINGXI.md"), contract)
                .map_err(|error| format!("write guided workspace LINGXI.md: {error}"))
        })
        .await
        .map_err(|error| format!("join guided contract worker: {error}"))?
    }

    /// Initialize the host-owned metadata and repository-verified Vite
    /// scaffold for a freshly created app.
    pub(crate) async fn scaffold_app_value(
        &self,
        record: &local_apps::AppRecord,
        surface: local_apps::AppSurface,
        runtime_profile: Option<local_apps::AppRuntimeProfile>,
    ) -> Result<(), String> {
        let layout = self.layout(&record.id)?;
        let requested_binding = runtime_profile
            .map(crate::local_app_runtime_profiles::current_binding_for_family)
            .transpose()
            .map_err(|error| error.to_string())?;
        let (build_lock, recovery_lock, recovery) = self
            .land_scaffold(record, surface, requested_binding)
            .await?;
        if let Err(error) = self
            .install_uncommitted_create_dependencies(record, &layout)
            .await
        {
            let recovery_error = recovery.rollback().err();
            drop(build_lock);
            drop(recovery_lock);
            return Err(match recovery_error {
                Some(recovery_error) => {
                    format!("{error}; scaffold rollback failed: {recovery_error}")
                }
                None => error,
            });
        }
        if let Err(error) = recovery.commit() {
            // This path is the initializer for the create-with-scaffold
            // transaction. AppService still owns the outer index commit, so
            // cleanup failure must not turn a successfully landed workspace
            // into a false failure. A committed mirror lets the next load
            // discard any leftover recovery material safely.
            tracing::warn!(
                app_id = %record.id,
                %error,
                "scaffold recovery cleanup deferred after initializer success"
            );
        }
        drop(build_lock);
        drop(recovery_lock);
        Ok(())
    }

    /// `LocalAppScaffold` — the transaction that turns the "+" button's empty
    /// shell into a formed app. §C.1.
    ///
    /// The STEP ORDER below is the specification, not an implementation
    /// detail. Each step's comment says what it is protecting.
    ///
    /// Nothing this call does is visible in the catalog until step 4 returns
    /// `Ok`: any earlier failure leaves `scaffolded == false` and none of
    /// `name` / `brief` / `workflow_model` persisted, the reservation released
    /// by its guard, the build lock released with it, and the app retryable.
    /// The retry is safe precisely because a first scaffold WIPES the editable
    /// surface, so every attempt starts from clean ground (§C.0.1).
    pub(crate) async fn scaffold_shell_app_value(&self, input: Value) -> Result<Value, String> {
        let app_id = required_string(&input, "app_id")?.to_string();
        // STEP 1 — reserve, in process, before ANYTHING else, so a second
        // concurrent call is refused rather than racing this one into the same
        // workspace. Held until every path out of this function, `Drop`
        // included. See [`ScaffoldReservation`] for why it must never persist.
        let _reservation = ScaffoldReservation::take(&self.scaffold_reservations, &app_id)?;

        // STEP 2 — validate. Every bound is re-checked here and again in
        // `AppService::commit_scaffold`: the MCP schema's `maxLength` is a
        // hint to the model, not an enforcement point, and this path also
        // refuses before touching the workspace rather than after seeding it.
        let name = confirmed_field(&input, "name")?.to_string();
        if name.len() > local_apps::service::MAX_NAME_BYTES {
            return Err(format!(
                "invalid_argument: name is {} bytes (limit {})",
                name.len(),
                local_apps::service::MAX_NAME_BYTES
            ));
        }
        let brief = confirmed_field(&input, "brief")?.to_string();
        if brief.len() > local_apps::service::MAX_BRIEF_BYTES {
            return Err(format!(
                "invalid_argument: brief is {} bytes (limit {})",
                brief.len(),
                local_apps::service::MAX_BRIEF_BYTES
            ));
        }
        let runtime_profile_receipt = input
            .get("runtime_profile_receipt")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
            .ok_or_else(|| {
                "invalid_argument: runtime_profile_receipt is required; native runtime-profile confirmation must happen before scaffold".to_string()
            })?;
        if input.get("runtime_profile").is_some() || input.get("surface").is_some() {
            return Err(
                "invalid_argument: runtime_profile_receipt is authoritative; do not also send runtime_profile or surface".into(),
            );
        }
        let receipt_binding = self
            .claim_runtime_profile_receipt(&app_id, &runtime_profile_receipt)
            .await?;
        let scaffolded = async {
            let surface = receipt_binding.family.surface();
            let workflow_model = match input.get("workflow_model") {
                None | Some(Value::Null) => None,
                Some(value) => {
                    let model = value
                        .as_str()
                        .ok_or_else(|| {
                            "invalid_argument: workflow_model must be a string".to_string()
                        })?
                        .trim();
                    if model.is_empty() {
                        None
                    } else if model.len() > local_apps::service::MAX_WORKFLOW_MODEL_BYTES {
                        return Err(format!(
                            "invalid_argument: workflow_model is {} bytes (limit {})",
                            model.len(),
                            local_apps::service::MAX_WORKFLOW_MODEL_BYTES
                        ));
                    } else {
                        Some(model.to_string())
                    }
                }
            };

            let service = self.service()?;
            let record = service
                .record(&app_id)
                .await
                .map_err(|error| error.to_string())?;
            let original_dependency = service
                .dependency_record(&app_id)
                .await
                .map_err(|error| error.to_string())?;
            if record.scaffolded {
                return Err(format!(
                    "app {app_id} is already scaffolded; its shape and name were fixed when it was \
                     formed and cannot be changed"
                ));
            }

            let mut proposed = record.clone();
            proposed.name = name.clone();
            proposed.brief = brief.clone();
            if let Some(model) = &workflow_model {
                proposed.workflow_model = Some(model.clone());
            }
            let (build_lock, recovery_lock, recovery) = self
                .land_scaffold(&proposed, surface, Some(receipt_binding.clone()))
                .await?;
            let layout = self.layout(&app_id)?;
            let result: Result<local_apps::AppRecord, String> = async {
                self.install_scaffold_dependencies(&service, &app_id, &layout)
                    .await?;
                service
                    .commit_scaffold(&app_id, &name, &brief, workflow_model.as_deref())
                    .await
                    .map_err(|error| error.to_string())
            }
            .await;
            match result {
                Ok(committed) => {
                    if let Err(error) = recovery.commit() {
                        tracing::warn!(
                            app_id = %app_id,
                            %error,
                            "scaffold recovery cleanup deferred after commit"
                        );
                    }
                    drop(build_lock);
                    drop(recovery_lock);
                    Ok(committed)
                }
                Err(error) => {
                    let recovery_error = recovery.rollback().err();
                    let dependency_error = service
                        .restore_dependency_record(original_dependency.clone())
                        .await
                        .err()
                        .map(|error| error.to_string());
                    drop(build_lock);
                    drop(recovery_lock);
                    match (recovery_error, dependency_error) {
                        (Some(recovery_error), Some(dependency_error)) => Err(format!(
                            "{error}; scaffold rollback failed: {recovery_error}; dependency rollback failed: {dependency_error}"
                        )),
                        (Some(recovery_error), None) => {
                            Err(format!("{error}; scaffold rollback failed: {recovery_error}"))
                        }
                        (None, Some(dependency_error)) => {
                            Err(format!("{error}; dependency rollback failed: {dependency_error}"))
                        }
                        (None, None) => Err(error),
                    }
                }
            }
        }
        .await;
        let committed = match scaffolded {
            Ok(committed) => {
                self.consume_runtime_profile_receipt(&app_id, &runtime_profile_receipt)
                    .await;
                committed
            }
            Err(error) => {
                self.release_runtime_profile_receipt_claim(&app_id, &runtime_profile_receipt)
                    .await;
                return Err(error);
            }
        };

        // STEP 5 — the pinned init session's title, AFTER the commit and
        // deliberately outside it. The interview ran in a session titled
        // `untitled` (the shell's placeholder name, minted into a PERSISTED
        // session directory), and that title is what the user's session list
        // shows forever otherwise.
        //
        // ⚠️ A failure here is logged and NOT rolled back. The scaffold has
        // already committed — the app is formed, its workspace is seeded and
        // its record says so — and unwinding that because a metadata line did
        // not append would destroy real work over a cosmetic field. What makes
        // that acceptable is that the boot backfill sweep runs the SAME
        // reconciliation on every launch, so a title left behind here is
        // repaired rather than stranded.
        if let Some(catalog) = self.session_catalog.get() {
            match reconcile_app_init_session_title(
                &catalog.lingxi_home,
                &self.root,
                catalog.fs.clone(),
                &committed,
            )
            .await
            {
                Ok(true) => tracing::info!(
                    app_id = %committed.id,
                    "renamed the pinned init session after scaffold"
                ),
                Ok(false) => {}
                Err(error) => tracing::warn!(
                    app_id = %committed.id,
                    %error,
                    "pinned init-session rename failed; boot reconciliation will retry"
                ),
            }
        }
        Ok(json!({
            "app": committed,
            "next_step": scaffold_next_step_guidance(),
        }))
    }

    /// §C.1 step 3: everything that reaches DISK, under `lock_app_build` from
    /// the first byte, with the lock handed back to the caller still held.
    ///
    /// ⚠️ The lock is not optional and the in-process reservation is not a
    /// substitute. `lock_app_build`'s own contract is that a caller holds it
    /// for the COMPLETE operation that mutates an app's workspace tree, and
    /// physical deletion takes the SAME lock (`storage::trash_app_dir` via
    /// `lock_app_build_if_present`). The reservation excludes another
    /// `LocalAppScaffold`; it does not exclude a concurrent `DeleteApp`, which
    /// renames the app directory into `.trash` while the seed is still being
    /// written — leaving files under a path nothing indexes and nothing
    /// reclaims. Returning the guard, rather than dropping it here, is what
    /// keeps it held across the commit point.
    ///
    /// ⚠️ A durable shell snapshot/journal is written BEFORE the manifest or
    /// workspace is changed. `manifest.surface` is stamped before the seed,
    /// and `record.scaffolded` is written LAST (step 4). These orders are
    /// deliberate: a crash before the record commit is rolled back from the
    /// journal before the next service load, while a committed record causes
    /// only recovery-material cleanup.
    async fn land_scaffold(
        &self,
        proposed: &local_apps::AppRecord,
        surface: local_apps::AppSurface,
        requested_binding: Option<local_apps::AppRuntimeProfileBinding>,
    ) -> Result<
        (
            traits::rooted_fs::RootedFileLock,
            traits::rooted_fs::RootedFileLock,
            local_apps::storage::ScaffoldRecoveryHandle,
        ),
        String,
    > {
        let layout = self.layout(&proposed.id)?;
        let artifacts = scaffold_runtime_profile(requested_binding, surface)?;
        let target =
            crate::local_apps_build::LocalAppBuildTarget::from_runtime_binding(&artifacts.binding)
                .map_err(|error| error.to_string())?;
        // Rendered from the PROPOSED record — the confirmed name and brief.
        // Rendering it from the creation record writes `# Local App: untitled`
        // with an empty brief, permanently: see `formal_workspace_contract`.
        let context = formal_workspace_contract(proposed, &artifacts.binding);
        let name = proposed.name.clone();
        let brief = proposed.brief.clone();
        let device_context = self.host_device_context();
        let root = self.root.clone();
        let app_id = proposed.id.clone();
        tokio::task::spawn_blocking(
            move ||
                -> Result<
                    (
                        traits::rooted_fs::RootedFileLock,
                        traits::rooted_fs::RootedFileLock,
                        local_apps::storage::ScaffoldRecoveryHandle,
                    ),
                    String,
                > {
                // 3a — take the global recovery lock before the per-app build
                // lock. Store loading takes this global lock before its index
                // lock, preventing an index/build inversion while recovering.
                let recovery_lock = local_apps::storage::lock_scaffold_recovery(&root)
                    .map_err(|error| error.to_string())?;
                let build_lock = local_apps::storage::lock_app_build(&root, &app_id)
                    .map_err(|error| error.to_string())?;
                // The complete shell snapshot and journal are durable before
                // any manifest/workspace mutation. A crash after this point is
                // therefore recoverable before the next service load.
                let recovery = local_apps::storage::begin_scaffold_recovery(
                    &root,
                    &app_id,
                    &name,
                    &brief,
                )
                .map_err(|error| error.to_string())?;
                let landed: Result<(), String> = (|| {
                    // 3c — the manifest's `surface` and `name`, under the
                    // §C.1.4 invariant.
                    stamp_scaffold_identity(&layout, &name, &artifacts)?;
                    // 3d — wipe the editable surface, then seed it. `true` is
                    // the first-scaffold flag: everything an agent wrote during
                    // the interview is removed before the seed lands, because
                    // a pre-written `app/app.js` would out-resolve the seeded
                    // `app/app.jsx` and the seed would become dead code.
                    crate::local_apps_build::scaffold_workspace_initialized(&layout, target, true)
                        .map_err(|error| error.to_string())?;
                    // 3e — the formal contract, overwriting the guided one.
                    let workspace = layout.root().join(layout.workspace_rel());
                    persist_runtime_profile_files(&workspace, &artifacts)?;
                    std::fs::write(workspace.join("LINGXI.md"), &context)
                        .map_err(|error| format!("write workspace LINGXI.md: {error}"))?;
                    // The native target, on the same manifest, so a formed app
                    // carries it whether or not the agent ever calls
                    // `LocalAppManifest`. Same first-write window as the name.
                    if let Some(device_context) = device_context {
                        let mut manifest = local_apps::load_manifest(&layout)
                            .map_err(|error| error.to_string())?;
                        manifest.device_context = Some(device_context);
                        local_apps::save_manifest(&layout, &manifest)
                            .map_err(|error| error.to_string())?;
                    }
                    Ok(())
                })();
                if let Err(error) = landed {
                    let recovery_error = recovery.rollback().err();
                    drop(build_lock);
                    drop(recovery_lock);
                    return match recovery_error {
                        Some(recovery_error) => Err(format!(
                            "{error}; scaffold rollback failed: {recovery_error}"
                        )),
                        None => Err(error),
                    };
                }
                Ok((build_lock, recovery_lock, recovery))
            },
        )
        .await
        .map_err(|error| format!("join scaffold landing worker: {error}"))?
    }
}

/// Write the app's identity onto its manifest — `surface` and `name` — under
/// the §C.1.4 hash invariant.
///
/// ⛔ FIRST WRITE ONLY. `AppManifest::hash()` serialises the WHOLE struct
/// INCLUDING `name`, and `AppDataStore::ensure_manifest` compares that hash
/// against the SQLite `_lingxi_schema.manifest_hash` row. Changing `name`
/// after a data store exists therefore breaks EVERY subsequent data read and
/// write with "database manifest mismatch" — silent, total, user-visible data
/// loss. A freshly created shell is safe because it has no collections, so
/// `AppDataStore::open` (which is what writes that row) has never run and the
/// database file does not exist. That is asserted here rather than assumed.
///
/// This is the real reason renaming an app is not offered, and this function
/// must NEVER be generalised into a rename path.
fn stamp_scaffold_identity(
    layout: &AppLayout,
    name: &str,
    artifacts: &crate::local_app_runtime_profiles::RuntimeProfileScaffoldArtifacts,
) -> Result<(), String> {
    let database = layout.database_path();
    if database.exists() {
        return Err(format!(
            "app {} already has a database at {}; writing manifest.name now would change \
             AppManifest::hash() and make every later data read and write fail with a database \
             manifest mismatch",
            layout.app_id(),
            database.display()
        ));
    }
    let mut manifest = local_apps::load_manifest(layout).map_err(|error| error.to_string())?;
    manifest.surface = Some(artifacts.binding.family.surface());
    manifest.runtime_profile = Some(artifacts.binding.clone());
    manifest.dependency_snapshot = None;
    manifest.template_origin = Some(local_apps::AppTemplateOrigin {
        plugin_id: local_apps::AppTemplateOrigin::BUILTIN_PLUGIN_ID.into(),
        plugin_version: "builtin".into(),
        template_id: format!(
            "{}-r{}",
            artifacts.binding.family.as_str().replace('_', "-"),
            artifacts.binding.revision
        ),
        template_sha256: artifacts.binding.contract_sha256.clone(),
    });
    manifest.name = name.to_string();
    local_apps::save_manifest(layout, &manifest).map_err(|error| error.to_string())
}

fn scaffold_runtime_profile(
    requested_binding: Option<local_apps::AppRuntimeProfileBinding>,
    surface: local_apps::AppSurface,
) -> Result<crate::local_app_runtime_profiles::RuntimeProfileScaffoldArtifacts, String> {
    let binding = requested_binding.ok_or_else(|| {
        "runtime profile binding is required; scaffold must consume a native confirmation receipt"
            .to_string()
    })?;
    if binding.family.surface() != surface {
        return Err(format!(
            "runtime profile {} requires the {} surface, but scaffold requested {}",
            binding.family,
            binding.family.surface().as_str(),
            surface.as_str()
        ));
    }
    crate::local_app_runtime_profiles::scaffold_artifacts_for_binding(&binding)
        .map_err(|error| error.to_string())
}

fn persist_runtime_profile_files(
    workspace: &Path,
    artifacts: &crate::local_app_runtime_profiles::RuntimeProfileScaffoldArtifacts,
) -> Result<(), String> {
    for (relative, bytes) in &artifacts.files {
        crate::local_apps_build::write_file(workspace, relative, bytes, true)
            .map_err(|error| error.to_string())?;
    }
    Ok(())
}

fn canonicalize_json(value: Value) -> Value {
    match value {
        Value::Object(object) => {
            let ordered = object
                .into_iter()
                .map(|(key, value)| (key, canonicalize_json(value)))
                .collect::<BTreeMap<_, _>>();
            Value::Object(Map::from_iter(ordered))
        }
        Value::Array(values) => Value::Array(values.into_iter().map(canonicalize_json).collect()),
        other => other,
    }
}

fn installed_package_manifest(path: &Path) -> bool {
    if path.file_name().and_then(|name| name.to_str()) != Some("package.json") {
        return false;
    }
    let Some(package_dir) = path.parent() else {
        return false;
    };
    let Some(parent) = package_dir.parent() else {
        return false;
    };
    if parent.file_name().and_then(|name| name.to_str()) == Some("node_modules") {
        return true;
    }
    let Some(grandparent) = parent.parent() else {
        return false;
    };
    grandparent.file_name().and_then(|name| name.to_str()) == Some("node_modules")
        && parent
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with('@'))
}

fn package_license_string(value: &Value) -> Option<String> {
    match value {
        Value::String(value) => Some(value.trim().to_string()).filter(|value| !value.is_empty()),
        Value::Object(object) => object
            .get("type")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string),
        _ => None,
    }
}

fn collect_installed_packages(
    root: &Path,
    packages: &mut BTreeMap<(String, String), Option<String>>,
) -> Result<(), String> {
    let entries = std::fs::read_dir(root)
        .map_err(|error| format!("read dependency tree {}: {error}", root.display()))?;
    for entry in entries {
        let entry = entry.map_err(|error| format!("read dependency tree entry: {error}"))?;
        let path = entry.path();
        let file_type = entry.file_type().map_err(|error| {
            format!("inspect dependency tree entry {}: {error}", path.display())
        })?;
        if file_type.is_symlink() {
            continue;
        }
        if file_type.is_dir() {
            collect_installed_packages(&path, packages)?;
            continue;
        }
        if !file_type.is_file() || !installed_package_manifest(&path) {
            continue;
        }
        let body = std::fs::read(&path).map_err(|error| {
            format!(
                "read installed package manifest {}: {error}",
                path.display()
            )
        })?;
        let manifest: Value = serde_json::from_slice(&body).map_err(|error| {
            format!(
                "parse installed package manifest {}: {error}",
                path.display()
            )
        })?;
        let name = manifest
            .get("name")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| {
                format!(
                    "installed package manifest {} is missing name",
                    path.display()
                )
            })?
            .to_string();
        let version = manifest
            .get("version")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| {
                format!(
                    "installed package manifest {} is missing version",
                    path.display()
                )
            })?
            .to_string();
        let license = manifest.get("license").and_then(package_license_string);
        packages.entry((name, version)).or_insert(license);
    }
    Ok(())
}

fn spdx_ref_for_package(name: &str, version: &str) -> String {
    let normalized = format!("{name}-{version}")
        .chars()
        .map(|ch| match ch {
            'A'..='Z' | 'a'..='z' | '0'..='9' => ch,
            _ => '-',
        })
        .collect::<String>();
    let identity = format!("{name}\0{version}");
    let digest = format!("{:x}", Sha256::digest(identity.as_bytes()));
    format!("SPDXRef-Package-{normalized}-{digest}")
}

fn installed_dependency_sbom(
    node_modules_root: &Path,
    binding: &local_apps::AppRuntimeProfileBinding,
    tree_sha256: &str,
) -> Result<Vec<u8>, String> {
    let mut packages = BTreeMap::<(String, String), Option<String>>::new();
    collect_installed_packages(node_modules_root, &mut packages)?;
    if packages.is_empty() {
        return Err(format!(
            "dependency snapshot cannot be verified because {} contains no installed package manifests",
            node_modules_root.display()
        ));
    }
    let root_id = format!(
        "SPDXRef-LingXiRuntime-{}-r{}",
        binding.family.as_str(),
        binding.revision
    );
    let mut package_values = vec![canonicalize_json(Value::Object(Map::from_iter([
        ("SPDXID".to_string(), Value::String(root_id.clone())),
        (
            "name".to_string(),
            Value::String(format!(
                "LingXi Local App Installed Dependencies {} r{}",
                binding.family.as_str(),
                binding.revision
            )),
        ),
        (
            "versionInfo".to_string(),
            Value::String(format!("{}+{}", binding.contract_sha256, tree_sha256)),
        ),
        (
            "downloadLocation".to_string(),
            Value::String("NOASSERTION".to_string()),
        ),
        (
            "licenseConcluded".to_string(),
            Value::String("NOASSERTION".to_string()),
        ),
        (
            "licenseDeclared".to_string(),
            Value::String("NOASSERTION".to_string()),
        ),
        (
            "copyrightText".to_string(),
            Value::String("NOASSERTION".to_string()),
        ),
    ])))];
    let mut relationships = Vec::new();
    for ((name, version), license) in packages {
        let package_id = spdx_ref_for_package(&name, &version);
        let license = license.unwrap_or_else(|| "NOASSERTION".to_string());
        package_values.push(canonicalize_json(Value::Object(Map::from_iter([
            ("SPDXID".to_string(), Value::String(package_id.clone())),
            ("name".to_string(), Value::String(name)),
            ("versionInfo".to_string(), Value::String(version)),
            (
                "downloadLocation".to_string(),
                Value::String("NOASSERTION".to_string()),
            ),
            (
                "licenseConcluded".to_string(),
                Value::String("NOASSERTION".to_string()),
            ),
            ("licenseDeclared".to_string(), Value::String(license)),
            (
                "copyrightText".to_string(),
                Value::String("NOASSERTION".to_string()),
            ),
        ]))));
        relationships.push(canonicalize_json(Value::Object(Map::from_iter([
            ("spdxElementId".to_string(), Value::String(root_id.clone())),
            (
                "relationshipType".to_string(),
                Value::String("DEPENDS_ON".to_string()),
            ),
            ("relatedSpdxElement".to_string(), Value::String(package_id)),
        ]))));
    }
    let document = canonicalize_json(Value::Object(Map::from_iter([
        (
            "spdxVersion".to_string(),
            Value::String("SPDX-2.3".to_string()),
        ),
        (
            "dataLicense".to_string(),
            Value::String("CC0-1.0".to_string()),
        ),
        (
            "SPDXID".to_string(),
            Value::String("SPDXRef-DOCUMENT".to_string()),
        ),
        (
            "name".to_string(),
            Value::String(format!(
                "LingXi Installed Dependency SBOM {} r{}",
                binding.family.as_str(),
                binding.revision
            )),
        ),
        (
            "documentNamespace".to_string(),
            Value::String(format!(
                "https://lingxi.local/app-dependencies/{}/r{}/{}/{}",
                binding.family.as_str(),
                binding.revision,
                binding.contract_sha256,
                tree_sha256,
            )),
        ),
        (
            "creationInfo".to_string(),
            Value::Object(Map::from_iter([
                (
                    "created".to_string(),
                    Value::String("2026-08-27T00:00:00Z".to_string()),
                ),
                (
                    "creators".to_string(),
                    Value::Array(vec![Value::String(
                        "Tool: lingxi-local-app-installed-dependencies".to_string(),
                    )]),
                ),
            ])),
        ),
        (
            "documentDescribes".to_string(),
            Value::Array(vec![Value::String(root_id.clone())]),
        ),
        ("packages".to_string(), Value::Array(package_values)),
        ("relationships".to_string(), Value::Array(relationships)),
        ("files".to_string(), Value::Array(vec![])),
    ])));
    let mut bytes = serde_json::to_vec_pretty(&document)
        .map_err(|error| format!("serialize dependency SBOM: {error}"))?;
    bytes.push(b'\n');
    Ok(bytes)
}

fn refresh_runtime_profile_snapshot(
    layout: &AppLayout,
    tree_sha256: &str,
) -> Result<local_apps::AppDependencySnapshot, String> {
    let mut manifest = local_apps::load_manifest(layout).map_err(|error| error.to_string())?;
    let binding = manifest.runtime_profile.clone().ok_or_else(|| {
        format!(
            "app {} is missing its runtime profile binding",
            layout.app_id()
        )
    })?;
    let workspace = layout.root().join(layout.workspace_rel());
    let requested_bytes =
        std::fs::read(workspace.join(crate::local_app_runtime_profiles::REQUESTED_FILE_REL))
            .map_err(|error| format!("read requested dependency snapshot input: {error}"))?;
    let package_bytes = std::fs::read(
        workspace.join(crate::local_app_runtime_profiles::EFFECTIVE_PACKAGE_FILE_REL),
    )
    .map_err(|error| format!("read effective dependency package: {error}"))?;
    let lockfile_bytes =
        std::fs::read(workspace.join(crate::local_app_runtime_profiles::LOCKFILE_FILE_REL))
            .map_err(|error| format!("read dependency lockfile: {error}"))?;
    let sbom = installed_dependency_sbom(&workspace.join("node_modules"), &binding, tree_sha256)?;
    let artifacts = crate::local_app_runtime_profiles::snapshot_artifacts_for_binding(
        &binding,
        crate::local_app_runtime_profiles::hash_bytes(&requested_bytes),
        crate::local_app_runtime_profiles::hash_bytes(&package_bytes),
        crate::local_app_runtime_profiles::hash_bytes(&lockfile_bytes),
        tree_sha256.to_string(),
        &sbom,
    )
    .map_err(|error| error.to_string())?;
    for (relative, bytes) in &artifacts.files {
        crate::local_apps_build::write_file(&workspace, relative, bytes, true)
            .map_err(|error| error.to_string())?;
    }
    manifest.dependency_snapshot = Some(artifacts.snapshot.clone());
    local_apps::save_manifest(layout, &manifest).map_err(|error| error.to_string())?;
    Ok(artifacts.snapshot)
}

fn update_uncommitted_dependency_record(
    root: &Path,
    record: &local_apps::AppRecord,
    op: impl FnOnce(&mut local_apps::AppDependencyRecord),
) -> Result<(), String> {
    let mut dependency = local_apps::storage::load_dependency_record(root, record)
        .map_err(|error| error.to_string())?;
    op(&mut dependency);
    local_apps::storage::save_dependency_record(root, &dependency)
        .map_err(|error| error.to_string())
}

/// What to tell the agent immediately after `LocalAppScaffold` commits.
///
/// Unlike [`create_next_step_guidance`], this one runs in a session that IS
/// rooted in the app's workspace — that is the whole point of the shell flow —
/// so the correct next move is to re-read the contract that has just been
/// rewritten under it and continue there, not to hand off to another session.
fn scaffold_next_step_guidance() -> String {
    "The app now has its shape and its source tree. Re-read this workspace's LINGXI.md before \
     doing anything else: it has been REPLACED by the formal contract for the surface you just \
     committed, and it names the editable entry points, the host-managed files you must not \
     touch, and the rules this surface must be written to. It names no build workflow, and you \
     do not need one: the host authorizes exactly one build workflow for this surface and \
     refuses any other, so ask for a build without naming one. Anything written into the \
     workspace before this call is gone, as the guided contract said it would be. Do not create \
     a second scaffold, do not run a package manager, and do not call LocalAppScaffold again — \
     the shape and the name are now fixed."
        .into()
}

/// Render the FORMAL workspace contract — the `workspace/LINGXI.md` a
/// formed app carries, and the twin of [`guided_workspace_contract`].
///
/// ⚠️ `record` is the identity the contract SPEAKS. On the `LocalAppScaffold`
/// path the caller must pass the PROPOSED record (the confirmed name and
/// brief), not the one creation wrote: the shell was created as `untitled`
/// with an empty brief, this file is written exactly ONCE (it is absent from
/// `restore_host_managed_files`, and a second scaffold is refused), and it is
/// the only channel that reaches the model on every turn. Render it from the
/// creation record and the whole interview is lost in the one artefact meant
/// to carry it.
fn formal_workspace_contract(
    record: &local_apps::AppRecord,
    binding: &local_apps::AppRuntimeProfileBinding,
) -> String {
    // Two scaffolds, two contracts. The shared clauses are repeated rather
    // than composed: this text is the agent's whole picture of the
    // workspace, and a reader that has to assemble it from fragments is how
    // "edit home-screen.jsx" survived into a workspace that has no such
    // file.
    let profile_identity = format!(
        "- This app is permanently bound to runtime profile `{}` revision `{}` with contract SHA-256 `{}`. This line is an informational mirror for the agent; the persisted manifest binding and host catalog are authoritative. Do not infer or replace the profile from imports or package files.\n",
        binding.family.as_str(),
        binding.revision,
        binding.contract_sha256,
    );
    let setup_path = match binding.family {
        local_apps::AppRuntimeProfile::ReactDom => format!(
            "{profile_identity}\
             - This app's surface is `dom`. The host authorizes exactly one build workflow for this surface and refuses any other; you never name or choose a workflow yourself, and a request that named a different one would be refused. The surface and runtime profile are fixed at creation; do not infer them from source.\n\
             - This workspace already contains the repository-verified Vite + Ionic foundation. The host prepares app-local dependencies in `workspace/node_modules`. Do not run `npm create vite`, do not create a second scaffold, do not add a wrapper build layer, and do not run a package manager in this local-app workspace.\n\
             - Host-managed files are `.gitignore`, `package.json`, `pnpm-lock.yaml`, `pnpm-workspace.yaml`, `jsconfig.json`, `index.html`, `vite.config.mjs`, `.lingxi/source-policy.json`, `lib/lingxi-bridge.js`, `lib/device-context.js`, `lib/platform-adapter.js`, `lib/lingxi-provider.jsx`, and `styles/foundation.css`. Do not edit them.\n\
             - Default editable entry points are `app/screens/home-screen.jsx`, `app/screens/detail-screen.jsx`, and `app/globals.css`. You may edit files under `app/`, `src/`, `styles/`, `public/`, and add non-host-managed helpers under `lib/`.\n\
             - The UI kit is Ionic. Import components from `@ionic/react`; never from `@ionic/core/components`, which cannot be bundled here. There is no Tailwind: use Ionic's CSS variables and its utility classes (`ion-padding`, `ion-margin`, `ion-text-center`, `ion-justify-content-*`, `ion-hide-*`), and put anything else in `app/globals.css`.\n\
             - Routing is `IonRouterOutlet` with react-router 6 `Routes`/`Route`. Every routed screen must render `IonPage` as its ROOT element, or the outlet has nothing to animate and the platform back gesture does not attach. Navigate with `routerLink`, not an onClick handler.\n\
             - The platform look is chosen for you: the checked-in provider calls `setupIonicReact` with the host's OS, so components already render iOS or Material chrome. Do not branch on the user agent and do not hard-code one platform's metrics.\n\
             - Use repo tools exposed in this workspace for source status, diff, and checkpoint versioning when available; checkpoints are workspace Git history. The host rebuilds directly from this workspace as the sole writable mount, keeps temporary output under `.lingxi-build-state/`, and promotes only the validated output.\n"
        ),
        local_apps::AppRuntimeProfile::Canvas2d
        | local_apps::AppRuntimeProfile::Three3d
        | local_apps::AppRuntimeProfile::Phaser2d
        | local_apps::AppRuntimeProfile::Babylon3d => {
            let helper = match binding.family {
                local_apps::AppRuntimeProfile::Canvas2d
                | local_apps::AppRuntimeProfile::Three3d => "lib/frame-loop.js",
                local_apps::AppRuntimeProfile::Phaser2d => "lib/phaser-runtime.js",
                local_apps::AppRuntimeProfile::Babylon3d => "lib/babylon-runtime.js",
                local_apps::AppRuntimeProfile::ReactDom => unreachable!(),
            };
            let engine_rule = match binding.family {
                local_apps::AppRuntimeProfile::Canvas2d =>
                    "- This is a Canvas 2D profile: use the checked-in `lib/frame-loop.js` helper and the Canvas 2D APIs; do not add a game engine or physics library.",
                local_apps::AppRuntimeProfile::Three3d =>
                    "- This is a Three.js profile: import the locked `three` package directly and use the checked-in `lib/frame-loop.js` helper; do not add React Three Fiber, drei, or an external physics library.",
                local_apps::AppRuntimeProfile::Phaser2d =>
                    "- This is a Phaser profile: use the locked `phaser` package through the checked-in `lib/phaser-runtime.js` adapter; do not replace it with `createFrameLoop`, another engine, or an external physics library.",
                local_apps::AppRuntimeProfile::Babylon3d =>
                    "- This is a Babylon.js profile: use the locked Babylon packages through the checked-in `lib/babylon-runtime.js` adapter; do not replace it with `createFrameLoop`, React Three Fiber, or an external physics library.",
                local_apps::AppRuntimeProfile::ReactDom => unreachable!(),
            };
            format!(
                "{profile_identity}\
                 - This app's surface is `canvas`. The host authorizes exactly one build workflow for this surface and refuses any other; you never name or choose a workflow yourself, and a request that named a different one would be refused. It is one drawn surface plus overlays; do not infer a screen hierarchy or the surface from source.\n\
                 - This workspace already contains the repository-verified Vite + Ionic foundation, scaffolded for a single DRAWN SURFACE. The host prepares app-local dependencies in `workspace/node_modules`. Do not run `npm create vite`, do not create a second scaffold, do not add a wrapper build layer, and do not run a package manager in this local-app workspace.\n\
                 - Host-managed files are `.gitignore`, `package.json`, `pnpm-lock.yaml`, `pnpm-workspace.yaml`, `jsconfig.json`, `index.html`, `vite.config.mjs`, `.lingxi/source-policy.json`, `lib/lingxi-bridge.js`, `lib/device-context.js`, `lib/platform-adapter.js`, `lib/lingxi-provider.jsx`, `{helper}`, and `styles/foundation.css`. Do not edit them; `{helper}` is the profile's checked-in runtime adapter.\n\
                 - Default editable entry points are `app/screens/game-screen.jsx`, `src/stores/game-store.js`, and `app/globals.css`. You may edit files under `app/`, `src/`, `styles/`, `public/`, and add non-host-managed helpers under `lib/`, but never edit the managed adapter `{helper}`.\n\
                 - There is NO router: menus, pause and game-over are Ionic components layered on top of the canvas, not separate pages.\n\
                 {engine_rule}\n\
                 - Keep per-frame simulation state in a ref, NOT in React or the store. The store is for the phase machine, score and settings; pushing positions through React re-renders turns the app into a slideshow.\n\
                 - Use repo tools exposed in this workspace for source status, diff, and checkpoint versioning when available; checkpoints are workspace Git history. The host rebuilds directly from this workspace as the sole writable mount, keeps temporary output under `.lingxi-build-state/`, and promotes only the validated output.\n"
            )
        }
    };
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
    format!(
        "# Local App: {name} ({id})\n\n\
         Brief: {brief}\n\n\
         ## Workspace contract\n\
         - This workspace is already bound to local app `{id}`. Treat `{id}` as authoritative; do not call `LocalAppList` or `LocalAppGet` to rediscover or confirm it, and do not call `LocalAppCreate` again.\n\
         - Edit ONLY app-owned files under `app/`, `src/`, `lib/`, `styles/`, `public/`.\n\
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
         - `LocalAppCaptureUi {{\"app_id\":\"{id}\"}}` — a still image of the preview. \
         Use it when the DOM cannot describe what the app is showing: a canvas or WebGL \
         surface has no inspectable elements, so `LocalAppInspectUi` returns an empty \
         list whether the app is drawing correctly, drawing nothing, or has crashed.\n\
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
    )
}

/// The GUIDED workspace contract: what a `CreateMode::Shell` app's
/// `workspace/LINGXI.md` says before the app has a shape.
///
/// Written by [`LocalAppsHostBroker::write_guided_contract_value`] and
/// OVERWRITTEN wholesale by the formal contract `scaffold_app_value` renders
/// once `LocalAppScaffold` lands — the two never coexist, so this text does not
/// have to compose with the formal one and deliberately does not try.
///
/// Every clause is load-bearing:
///
/// - the app id, because an agent in this workspace has no other authoritative
///   source for it and the tool gate's own message is keyed on it;
/// - "任何源文件都会被删除", because the first scaffold WIPES the editable
///   surface (§C.0.1). An agent that writes code here does not merely waste the
///   turn, it loses work it believes it has done;
/// - "只有 `LocalAppScaffold` 对你有意义", because every other local-app tool
///   is gated off for an unformed app and will refuse;
/// - the surface vocabulary, because the surface is IMMUTABLE once scaffolded,
///   so it is the one decision the user has to make before anything is written;
/// - "这一轮**不要用 `AskUserQuestion`**" on step 1, because `AskUserQuestion`
///   renders a native picker and the opening turn has nothing to put in it.
///   The model knows only that an app is wanted, so any option list it writes
///   is a set of guesses at the user's idea, and the picker then collects a
///   choice among those guesses INSTEAD of the free-text description every
///   later step reads. The generic rule elsewhere ("问需求用
///   `AskUserQuestion`") is right for a decision between namable options and
///   wrong for the one turn that has none — which is why step 1 states the
///   exception in the same breath as the tool, rather than leaving a reader to
///   reconcile the two.
fn guided_workspace_contract(record: &local_apps::AppRecord) -> String {
    format!(
        "# Local App（新建，尚未定形态）\n\n\
         这个应用刚刚创建，**还没有形态**，工作区是空的。\n\n\
         这个工作区已经绑定到本地应用 `{id}`。把 `{id}` 当作权威：不要调 `LocalAppList` 或 \
         `LocalAppGet` 去重新发现或确认它，也不要再调一次 `LocalAppCreate`。\n\n\
         你现在的任务是引导用户，不是写代码。**你现在写下的任何源文件都会在脚手架落地时被删除**，\
         写了也是白写。\n\n\
         此刻先不要构建、安装依赖或操作运行时；在应用定形态并落脚手架之前，这些步骤都没有意义。\n\n\
         步骤：\n\
         1. **用普通对话文本**问用户想做什么，一句开放式的话，然后等他回答。\
         这一轮**不要用 `AskUserQuestion`**：它弹的是选择器，而此刻你对这个应用一无所知，\
         能填进选项里的只有你对用户想法的猜测——把猜测做成菜单，恰好挤掉了你真正需要的那段描述。\n\
         2. 读他的描述，能自己定的就自己定，别把他已经说过的再问一遍。\
         只有当某一点仍然悬着、**会改变最终做出来的东西**、而且是可以列出选项的选择时，\
         才用 `AskUserQuestion` 问一轮，提出 1-3 个聚焦问题；没有未决事项就省略这一轮。\n\
         描述已经说清楚的，直接进第 3 步。\n\
         3. 用 `AskUserQuestion` 把提议的**名称**与**形态**交给用户确认或修改：\n\
         \u{20}  - `dom` —— 多屏界面（表单、列表、页面导航）\n\
         \u{20}  - `canvas` —— 单一绘制面（游戏、3D、可视化）\n\
         4. 如需运行时细分，先读 `LocalAppRuntimeProfiles`，再用运行时确认工具为 `{id}` 取得短时 receipt。\n\
         5. 用户确认后调 `LocalAppScaffold`（`app_id` 用 `{id}`，并带上 runtime profile receipt）。\n\
         6. 重读本文件，按新合约继续。\n\n\
         形态一旦落地不可更改，所以必须让用户确认，不要自作主张。\n",
        id = record.id,
    )
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

/// Build the opaque `value` payload for a capture request.
///
/// The rect rides `AppUiRequestDto.value` — an `Option<String>` the wire
/// already carries — so a region crop costs no DTO change. Shape and
/// finiteness are checked here; CLAMPING to the viewport happens on the
/// client, which is the only side that knows the real viewport.
///
/// That split is why a NEGATIVE origin is accepted and forwarded verbatim.
/// `getBoundingClientRect().top` is negative for anything scrolled above the
/// fold, so `inspect_ui`'s `elements[].rect` routinely reports one — and
/// "inspect, take an element's rect, capture it" is the most natural flow the
/// two tools have. Refusing it here would also make the clients' own clamping
/// (`intersection` on iOS, `coerceIn` in `cropSourceRect` on Android)
/// unreachable code.
fn capture_ui_value(input: &Value) -> Result<Option<String>, String> {
    // An explicit `"rect": null` is a model's way of saying "not applicable",
    // i.e. capture the whole view. `.get` answers `Some(Value::Null)` for it,
    // so without this filter the absent-rect guard never fires and every field
    // lookup below fails — a hard tool error for a routine, well-meant input.
    let Some(rect) = input.get("rect").filter(|value| !value.is_null()) else {
        return Ok(None);
    };
    // Keep the original `Value` alongside its `f64` reading: shape/range
    // checks need the number, but re-emitting the parsed f64 would turn an
    // integral input like `10` into `10.0` in the outgoing JSON text, which
    // is a needless textual change the client never asked for.
    let field = |name: &str| -> Result<(&Value, f64), String> {
        rect.get(name)
            .and_then(|value| value.as_f64().filter(|n| n.is_finite()).map(|n| (value, n)))
            .ok_or_else(|| format!("capture_ui rect.{name} must be a finite number"))
    };
    // The origin's numeric reading is deliberately discarded: `field` already
    // proved it finite, and its SIGN is not this layer's business (see above).
    let ((x, _), (y, _), (w, wn), (h, hn)) =
        (field("x")?, field("y")?, field("width")?, field("height")?);
    if wn <= 0.0 || hn <= 0.0 {
        return Err("capture_ui rect must have positive width and height".into());
    }
    Ok(Some(
        json!({ "rect": { "x": x, "y": y, "width": w, "height": h } }).to_string(),
    ))
}

fn validate_create_stage_quality(
    quality_level: &str,
    family: local_apps::AppRuntimeProfile,
) -> Result<(), String> {
    if !matches!(quality_level, "fast" | "balanced" | "thorough") {
        return Err(
            "create_staging_invalid: quality_level must be fast, balanced, or thorough".into(),
        );
    }
    if quality_level == "fast" && family != local_apps::AppRuntimeProfile::ReactDom {
        return Err(
            "create_staging_invalid: canvas profiles require balanced or thorough quality".into(),
        );
    }
    Ok(())
}

#[async_trait]
impl LocalAppsMcpHost for LocalAppsHostBroker {
    fn create_next_step(&self) -> String {
        create_next_step_guidance()
    }

    async fn runtime_profiles(&self, input: Value) -> Result<Value, String> {
        self.runtime_profiles_value(input).await
    }

    async fn template_catalog(&self, _input: Value) -> Result<Value, String> {
        let view = crate::local_app_template_catalog::catalog_view()?;
        serde_json::to_value(view).map_err(|error| format!("serialize template catalog: {error}"))
    }

    async fn validate_template_selection(&self, input: Value) -> Result<Value, String> {
        let app_id = required_string(&input, "app_id")?;
        let workflow_run_id = required_string(&input, "workflow_run_id")?;
        if input.get("caller_role").is_some() {
            return Err(
                "template_selector_only: caller_role is not an authority proof; use the Host-issued selector_capability"
                    .into(),
            );
        }
        let record = self
            .service()?
            .record(app_id)
            .await
            .map_err(|error| error.to_string())?;
        if record.scaffolded {
            return Err("template_selection_rejected: app is already scaffolded; update/verify must use its persisted profile".into());
        }
        crate::local_app_template_catalog::validate_and_journal(
            &self.root,
            app_id,
            workflow_run_id,
            &input,
        )
    }

    async fn resolve_template_selection(&self, input: Value) -> Result<Value, String> {
        let app_id = required_string(&input, "app_id")?;
        let workflow_run_id = required_string(&input, "workflow_run_id")?;
        let handle = required_string(&input, "validated_selection_handle")?;
        self.service()?
            .record(app_id)
            .await
            .map_err(|error| error.to_string())?;
        crate::local_app_template_catalog::resolve(&self.root, app_id, workflow_run_id, handle)
    }

    async fn stage_create(&self, input: Value) -> Result<Value, String> {
        let app_id = required_string(&input, "app_id")?;
        let workflow_run_id = required_string(&input, "workflow_run_id")?;
        let handle = required_string(&input, "validated_selection_handle")?;
        let quality_level = required_string(&input, "quality_level")?;
        let record = self
            .service()?
            .record(app_id)
            .await
            .map_err(|error| error.to_string())?;
        if record.scaffolded {
            return Err("create_staging_rejected: app is already scaffolded".into());
        }
        let selection = crate::local_app_template_catalog::resolve_typed(
            &self.root,
            app_id,
            workflow_run_id,
            handle,
        )?;
        validate_create_stage_quality(quality_level, selection.runtime_profile.family)?;
        let artifacts = crate::local_app_runtime_profiles::scaffold_artifacts_for_binding(
            &selection.runtime_profile,
        )
        .map_err(|error| format!("stage template dependencies: {error}"))?;
        let requested = artifacts
            .files
            .iter()
            .find(|(path, _)| *path == crate::local_app_runtime_profiles::REQUESTED_FILE_REL)
            .map(|(_, bytes)| bytes.as_slice())
            .ok_or_else(|| {
                "create_staging_invalid: requested dependency input missing".to_string()
            })?;
        let effective = artifacts
            .files
            .iter()
            .find(|(path, _)| {
                *path == crate::local_app_runtime_profiles::EFFECTIVE_PACKAGE_FILE_REL
            })
            .map(|(_, bytes)| bytes.as_slice())
            .ok_or_else(|| "create_staging_invalid: effective package input missing".to_string())?;
        let lock = artifacts
            .files
            .iter()
            .find(|(path, _)| *path == crate::local_app_runtime_profiles::LOCKFILE_FILE_REL)
            .map(|(_, bytes)| bytes.as_slice())
            .ok_or_else(|| "create_staging_invalid: base lock input missing".to_string())?;
        let dependency_input_sha256 = crate::local_app_template_catalog::dependency_input_sha256(
            requested,
            effective,
            lock,
            crate::local_app_runtime_profiles::RUNTIME_PROFILE_TOOLCHAIN_KEY,
        );
        let verified_dependency_input_sha256 =
            crate::local_app_template_catalog::dependency_input_sha256(
                requested,
                effective,
                lock,
                crate::local_app_runtime_profiles::RUNTIME_PROFILE_TOOLCHAIN_KEY,
            );
        if dependency_input_sha256 != verified_dependency_input_sha256 {
            return Err(
                "create_staging_invalid: dependency_input_sha256 verification mismatch".into(),
            );
        }
        let staging = self
            .root
            .join(".lingxi-build-state/template-candidates")
            .join(app_id)
            .join(workflow_run_id)
            .join("staging")
            .join(handle);
        std::fs::create_dir_all(&staging)
            .map_err(|error| format!("create isolated staging: {error}"))?;
        // Materialize only the install-before-build inputs in the run-scoped
        // candidate staging area.  The app workspace and Manifest remain
        // untouched until the later receipt/publish phase.  Each file is
        // written atomically and read back before evidence is emitted so the
        // dependency digest covers bytes that actually reached staging.
        let template_root = staging.join("template");
        let mut staged_files = Vec::with_capacity(artifacts.files.len());
        for (relative, bytes) in artifacts.files {
            let relative_path = std::path::Path::new(relative);
            if relative_path.is_absolute()
                || relative_path
                    .components()
                    .any(|component| matches!(component, std::path::Component::ParentDir))
            {
                return Err(format!(
                    "create_staging_invalid: unsafe template artifact path {relative:?}"
                ));
            }
            let target = template_root.join(relative_path);
            let parent = target.parent().ok_or_else(|| {
                "create_staging_invalid: template artifact has no parent".to_string()
            })?;
            std::fs::create_dir_all(parent)
                .map_err(|error| format!("create template staging directory: {error}"))?;
            let temporary = target.with_file_name(format!(
                ".{}.tmp",
                target
                    .file_name()
                    .and_then(|name| name.to_str())
                    .ok_or_else(|| {
                        "create_staging_invalid: template artifact has invalid filename".to_string()
                    })?
            ));
            std::fs::write(&temporary, &bytes).map_err(|error| {
                format!("write template staging artifact {relative:?}: {error}")
            })?;
            std::fs::rename(&temporary, &target).map_err(|error| {
                format!("commit template staging artifact {relative:?}: {error}")
            })?;
            let materialized = std::fs::read(&target)
                .map_err(|error| format!("read template staging artifact {relative:?}: {error}"))?;
            if materialized != bytes {
                return Err(format!(
                    "create_staging_invalid: template artifact changed while staging {relative:?}"
                ));
            }
            staged_files.push(serde_json::json!({
                "path": relative,
                "sha256": format!("{:x}", sha2::Sha256::digest(&materialized)),
            }));
        }
        let evidence = serde_json::json!({
            "schemaVersion": 1,
            "staging": "isolated",
            "appId": app_id,
            "workflowRunId": workflow_run_id,
            "validatedSelectionHandle": handle,
            "templateId": selection.template_id,
            "dependencyInputSha256": dependency_input_sha256,
            "stagedFiles": staged_files,
            "published": false,
            "manifestCommitted": false,
        });
        let evidence_path = staging.join("evidence.json");
        let bytes = serde_json::to_vec_pretty(&evidence)
            .map_err(|error| format!("serialize staging evidence: {error}"))?;
        let temp_path = staging.join("evidence.json.tmp");
        std::fs::write(&temp_path, bytes)
            .map_err(|error| format!("write staging evidence: {error}"))?;
        std::fs::rename(&temp_path, &evidence_path)
            .map_err(|error| format!("commit staging evidence: {error}"))?;
        Ok(evidence)
    }

    async fn validate_mcp_proposal(&self, input: Value) -> Result<Value, String> {
        let app_id = required_string(&input, "app_id")?.to_string();
        let workflow_run_id = required_string(&input, "workflow_run_id")?.to_string();
        Self::validate_workflow_run_id(&workflow_run_id)?;
        let proposal_value = input
            .get("proposal")
            .cloned()
            .ok_or_else(|| "proposal is required".to_string())?;
        let service = self.service()?;
        let record = service
            .record(&app_id)
            .await
            .map_err(|error| error.to_string())?;
        if !record.scaffolded {
            return Err("mcp_proposal_rejected: app is not scaffolded".into());
        }
        let layout = self.layout(&app_id)?;
        let manifest = load_manifest(&layout).map_err(|error| error.to_string())?;
        let proposal: local_apps::AppMcpProposal = serde_json::from_value(proposal_value)
            .map_err(|error| format!("proposal_invalid: {error}"))?;
        let contexts = self.load_active_mcp_flow_contexts(&layout)?;
        let validated = local_apps::validate_app_mcp_proposal(
            proposal,
            &app_id,
            manifest.revision,
            &contexts,
            &local_apps::CapabilityRegistry::default(),
        )
        .map_err(|issues| {
            format!(
                "proposal_invalid: {}",
                issues
                    .into_iter()
                    .map(|issue| format!("{}: {}", issue.code, issue.message))
                    .collect::<Vec<_>>()
                    .join("; ")
            )
        })?;
        let review_surface = Self::build_mcp_review_surface(
            &manifest,
            &validated,
            manifest.active_mcp_catalog.as_ref(),
        );
        let approval_contract_sha256 = local_apps::approval_contract_sha256(review_surface.clone())
            .map_err(|issue| format!("proposal_invalid: {}", issue.message))?;
        let active_build_id =
            crate::local_apps_build::active_build_id(&layout).map_err(|error| error.to_string())?;
        let mut journal = local_apps::McpCandidateJournal {
            schema_version: local_apps::APPS_SCHEMA_VERSION,
            app_id: app_id.clone(),
            workflow_run_id: workflow_run_id.clone(),
            stage: local_apps::McpAuthoringStage::Prepared,
            previous_build_id: active_build_id,
            previous_catalog_sha256: manifest
                .active_mcp_catalog
                .as_ref()
                .map(|catalog| catalog.catalog_sha256.clone()),
            proposal_sha256: validated.proposal_sha256.clone(),
            approval_contract_sha256: approval_contract_sha256.clone(),
            tool_surface_sha256: validated.tool_surface_sha256.clone(),
            catalog_sha256: None,
            consumed_receipt_sha256: None,
            integrity_sha256: String::new(),
        }
        .seal()
        .map_err(|issue| issue.message)?;
        let unchanged_approval = manifest.active_mcp_catalog.as_ref().is_some_and(|catalog| {
            catalog.approval_contract_sha256 == approval_contract_sha256
                && catalog.tool_surface_sha256 == validated.tool_surface_sha256
        });
        if unchanged_approval {
            journal = journal
                .advance(local_apps::McpAuthoringStage::Approved)
                .map_err(|issue| issue.message)?;
        }
        local_apps::save_candidate_journal(&layout, &journal).map_err(|error| error.to_string())?;
        self.save_mcp_candidate(
            &app_id,
            &workflow_run_id,
            &PersistedMcpCandidate {
                validated: validated.clone(),
                approval_contract_sha256: approval_contract_sha256.clone(),
                review_surface: review_surface.clone(),
                verification_sha256: None,
                catalog_sha256: None,
            },
        )?;
        Ok(json!({
            "ok": true,
            "status": if unchanged_approval { "approved_reusable" } else { "approval_required" },
            "proposal_sha256": validated.proposal_sha256,
            "approval_contract_sha256": approval_contract_sha256,
            "tool_surface_sha256": validated.tool_surface_sha256,
            "findings": [],
            "review_surface": review_surface,
        }))
    }

    async fn approve_mcp_proposal(&self, input: Value) -> Result<Value, String> {
        let app_id = required_string(&input, "app_id")?.to_string();
        let workflow_run_id = required_string(&input, "workflow_run_id")?.to_string();
        let approval_contract_sha256 =
            required_string(&input, "approval_contract_sha256")?.to_string();
        let layout = self.layout(&app_id)?;
        let mut journal =
            local_apps::load_candidate_journal(&layout).map_err(|error| error.to_string())?;
        if journal.workflow_run_id != workflow_run_id {
            return Err(
                "receipt_invalid: workflow run does not match the prepared candidate".into(),
            );
        }
        if journal.approval_contract_sha256 != approval_contract_sha256 {
            return Err(
                "receipt_invalid: approval contract digest does not match the prepared candidate"
                    .into(),
            );
        }
        if journal.stage == local_apps::McpAuthoringStage::Prepared {
            let receipt = local_apps::McpConfirmationReceipt::new(
                &app_id,
                &workflow_run_id,
                approval_contract_sha256.clone(),
                journal.proposal_sha256.clone(),
                now_ms(),
            );
            let receipt_id = receipt.receipt_id.clone();
            self.pending_mcp_receipts
                .lock()
                .await
                .issue(receipt)
                .map_err(|issue| issue.message)?;
            journal = journal
                .advance(local_apps::McpAuthoringStage::Approved)
                .map_err(|issue| issue.message)?;
            local_apps::save_candidate_journal(&layout, &journal)
                .map_err(|error| error.to_string())?;
            return Ok(json!({
                "approved": true,
                "receipt_id": receipt_id,
                "status": "approved",
            }));
        }
        Ok(json!({
            "approved": true,
            "receipt_id": Value::Null,
            "status": "approved_reusable",
        }))
    }

    async fn qa_mcp_candidate(&self, input: Value) -> Result<Value, String> {
        let app_id = required_string(&input, "app_id")?.to_string();
        let workflow_run_id = required_string(&input, "workflow_run_id")?.to_string();
        let layout = self.layout(&app_id)?;
        let mut journal =
            local_apps::load_candidate_journal(&layout).map_err(|error| error.to_string())?;
        if journal.workflow_run_id != workflow_run_id {
            return Err(
                "journal_invalid: workflow run does not match the candidate journal".into(),
            );
        }
        if journal.stage < local_apps::McpAuthoringStage::Approved {
            return Err("approval_required: MCP candidate is not approved".into());
        }
        let mut candidate = self.load_mcp_candidate(&app_id, &workflow_run_id)?;
        let definitions = candidate
            .validated
            .tools
            .iter()
            .map(|tool| tool.definition.clone())
            .collect::<Vec<_>>();
        local_apps::validate_generated_mcp_catalog(&definitions).map_err(|issues| {
            format!(
                "mcp_qa_failed: {}",
                issues
                    .into_iter()
                    .map(|issue| format!("{}: {}", issue.code, issue.message))
                    .collect::<Vec<_>>()
                    .join("; ")
            )
        })?;
        let build_id = crate::local_apps_build::active_build_id(&layout)
            .map_err(|error| error.to_string())?
            .ok_or_else(|| "mcp_qa_failed: app has no active build".to_string())?;
        let execution = serde_json::to_value(
            candidate
                .validated
                .tools
                .iter()
                .map(|tool| {
                    json!({
                        "definition": tool.definition,
                        "flow": tool.flow,
                        "ceiling": tool.ceiling,
                    })
                })
                .collect::<Vec<_>>(),
        )
        .map_err(|error| format!("serialize execution bindings: {error}"))?;
        let catalog_sha256 = local_apps::catalog_sha256(&candidate.validated, &build_id, execution)
            .map_err(|issue| issue.message)?;
        journal.catalog_sha256 = Some(catalog_sha256.clone());
        while journal.stage < local_apps::McpAuthoringStage::McpVerified {
            let next_stage = match journal.stage {
                local_apps::McpAuthoringStage::Approved => local_apps::McpAuthoringStage::Built,
                local_apps::McpAuthoringStage::Built => local_apps::McpAuthoringStage::SmokePassed,
                local_apps::McpAuthoringStage::SmokePassed => {
                    local_apps::McpAuthoringStage::McpVerified
                }
                _ => local_apps::McpAuthoringStage::McpVerified,
            };
            journal = journal.advance(next_stage).map_err(|issue| issue.message)?;
        }
        let verification_sha256 = local_apps::approval_contract_sha256(json!({
            "appId": app_id,
            "workflowRunId": workflow_run_id,
            "catalogSha256": catalog_sha256,
            "checks": ["mcp_schema", "flow_binding", "calls", "isolation"],
        }))
        .map_err(|issue| issue.message)?;
        candidate.verification_sha256 = Some(verification_sha256.clone());
        candidate.catalog_sha256 = Some(catalog_sha256.clone());
        local_apps::save_candidate_journal(&layout, &journal).map_err(|error| error.to_string())?;
        self.save_mcp_candidate(&app_id, &workflow_run_id, &candidate)?;
        Ok(json!({
            "ok": true,
            "findings": [],
            "mcp_schema": "passed",
            "flow_binding": "passed",
            "calls": "passed",
            "isolation": "passed",
            "verification_sha256": verification_sha256,
            "summary": "Host-side MCP schema, binding, build identity and isolation gates passed.",
        }))
    }

    async fn promote_mcp_candidate(&self, input: Value) -> Result<Value, String> {
        let app_id = required_string(&input, "app_id")?.to_string();
        let workflow_run_id = required_string(&input, "workflow_run_id")?.to_string();
        let receipt_id = input
            .get("receipt_id")
            .and_then(Value::as_str)
            .map(str::to_string);
        let service = self.service()?;
        let layout = self.layout(&app_id)?;
        let mut journal =
            local_apps::load_candidate_journal(&layout).map_err(|error| error.to_string())?;
        if journal.workflow_run_id != workflow_run_id {
            return Err(
                "journal_invalid: workflow run does not match the candidate journal".into(),
            );
        }
        if journal.stage < local_apps::McpAuthoringStage::McpVerified {
            return Err("mcp_qa_failed: MCP candidate has not completed QA".into());
        }
        let candidate = self.load_mcp_candidate(&app_id, &workflow_run_id)?;
        let catalog_sha256 = candidate
            .catalog_sha256
            .clone()
            .or_else(|| journal.catalog_sha256.clone())
            .ok_or_else(|| "catalog_invalid: candidate catalog digest is missing".to_string())?;
        let build_id = crate::local_apps_build::active_build_id(&layout)
            .map_err(|error| error.to_string())?
            .ok_or_else(|| "promotion_failed: app has no active build".to_string())?;
        if let Some(receipt_id) = receipt_id.as_deref() {
            self.pending_mcp_receipts
                .lock()
                .await
                .consume_candidate(
                    receipt_id,
                    &app_id,
                    &workflow_run_id,
                    &journal.approval_contract_sha256,
                    &journal.proposal_sha256,
                    now_ms(),
                )
                .map_err(|issue| issue.message)?;
            journal.consumed_receipt_sha256 =
                Some(format!("{:x}", Sha256::digest(receipt_id.as_bytes())));
        }
        let manifest = load_manifest(&layout).map_err(|error| error.to_string())?;
        let catalog_body = json!({
            "appId": app_id,
            "buildId": build_id,
            "tools": candidate.validated.tools.iter().map(|tool| json!({
                "definition": tool.definition,
                "flow": tool.flow,
                "ceiling": tool.ceiling,
            })).collect::<Vec<_>>(),
        });
        local_apps::save_mcp_catalog(&layout, &catalog_sha256, &catalog_body)
            .map_err(|error| error.to_string())?;
        let previous = manifest.active_mcp_catalog.clone();
        let mut promoted_manifest = manifest.clone();
        promoted_manifest.active_mcp_catalog = Some(local_apps::AppMcpCatalogRef {
            build_id,
            manifest_revision: promoted_manifest.revision,
            authoring_revision: previous
                .as_ref()
                .map(|catalog| {
                    if catalog.tool_surface_sha256 == candidate.validated.tool_surface_sha256 {
                        catalog.authoring_revision
                    } else {
                        catalog.authoring_revision + 1
                    }
                })
                .unwrap_or(1),
            user_goal_sha256: candidate.validated.proposal.user_goal_sha256.clone(),
            proposal_sha256: candidate.validated.proposal_sha256.clone(),
            approval_contract_sha256: candidate.approval_contract_sha256.clone(),
            tool_surface_sha256: candidate.validated.tool_surface_sha256.clone(),
            catalog_sha256: catalog_sha256.clone(),
            mcp_verification_sha256: candidate
                .verification_sha256
                .clone()
                .ok_or_else(|| "promotion_failed: verification digest is missing".to_string())?,
        });
        local_apps::save_manifest(&layout, &promoted_manifest)
            .map_err(|error| error.to_string())?;
        if journal.stage < local_apps::McpAuthoringStage::Promoted {
            journal = journal
                .advance(local_apps::McpAuthoringStage::Promoted)
                .map_err(|issue| issue.message)?;
            local_apps::save_candidate_journal(&layout, &journal)
                .map_err(|error| error.to_string())?;
        }
        let _ = service.announce_record(&app_id).await;
        Ok(json!({
            "promoted": true,
            "catalog_sha256": catalog_sha256,
            "status": "promoted",
            "publication_state": "published_unverified",
        }))
    }

    async fn confirm_runtime_profile(&self, input: Value) -> Result<Value, String> {
        self.confirm_runtime_profile_value(input).await
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
        // Publication state is derived from the active build/catalog pair in
        // schema v3. A successful build alone must not mutate a persistent
        // workflow state or advertise an active MCP surface.
        let dependencies = service
            .dependency_record(&app_id)
            .await
            .map_err(|e| e.to_string())?;
        let target =
            crate::local_apps_build::detect_build_target(&layout).map_err(|e| e.to_string())?;
        Ok(serde_json::json!({
            "ok": true,
            "app_id": app_id,
            "target": target.template_id(),
            "dependencies": dependencies,
            "hint": "start or restart the runtime with manage_runtime to serve the new build",
        }))
    }

    async fn install_dependencies(&self, input: Value) -> Result<Value, String> {
        self.install_dependencies_value(input).await
    }

    async fn confirm_dependency_change(&self, _input: Value) -> Result<Value, String> {
        let app_id = required_string(&_input, "app_id")?.to_string();
        let service = self.service()?;
        service
            .record(&app_id)
            .await
            .map_err(|error| error.to_string())?;
        let dependency_record = service
            .dependency_record(&app_id)
            .await
            .map_err(|error| error.to_string())?;
        let layout = self.layout(&app_id)?;
        let (_binding, baseline, changes, requested_json, effective_package_json) =
            Self::prepare_dependency_change(
                &layout,
                &dependency_record,
                _input
                    .get("changes")
                    .ok_or_else(|| "invalid_argument: changes is required".to_string())?,
            )?;
        if changes
            .iter()
            .any(|change| !matches!(change.kind, DependencyChangeKind::Remove))
        {
            // These statuses intentionally describe what can be proven before
            // resolution.  Looking in the pnpm store or touching the registry
            // here would make a supposedly review-only call perform network or
            // cache work before the user's approval.
            let confirmation_changes = changes
                .iter()
                .map(|change| AppDependencyChangeDto {
                    kind: dependency_change_kind_dto(&change.kind),
                    package: change.package.clone(),
                    version: change.version.clone(),
                    cache_status: dependency_change_cache_status(&change.kind),
                    download_status: match change.kind {
                        DependencyChangeKind::Remove => "not_required".to_string(),
                        DependencyChangeKind::Add | DependencyChangeKind::Update => {
                            "may_be_required".to_string()
                        }
                    },
                })
                .collect();
            let request_id = self.request_id("app-dependency-change");
            let (sender, receiver) = oneshot::channel();
            self.pending_dependency_change_confirmations
                .lock()
                .await
                .insert(request_id.clone(), sender);
            self.event_sink
                .emit(ClientEvent::AppEvent {
                    event: AppEventDto::AppDependencyChangeConfirmationRequested {
                        request: AppDependencyChangeConfirmationRequestDto {
                            request_id: request_id.clone(),
                            app_id: app_id.clone(),
                            // Stable codes keep native clients localized while
                            // still making the policy explicit on the wire.
                            reason: "pre_resolution_no_network".into(),
                            changes: confirmation_changes,
                            license_risk: "unknown_until_resolution".into(),
                            sbom_risk: "unknown_until_resolution".into(),
                            lifecycle_scripts_blocked: true,
                            native_addons_blocked: true,
                            rollback_policy: "rollback_on_validation_failure".into(),
                        },
                    },
                })
                .await;
            let approved = match timeout(APPROVAL_TIMEOUT, receiver).await {
                Ok(Ok(approved)) => approved,
                Ok(Err(_)) => {
                    self.pending_dependency_change_confirmations
                        .lock()
                        .await
                        .remove(&request_id);
                    return Err("dependency change confirmation was cancelled".into());
                }
                Err(_) => {
                    self.pending_dependency_change_confirmations
                        .lock()
                        .await
                        .remove(&request_id);
                    return Err("dependency change confirmation timed out".into());
                }
            };
            if !approved {
                return Err("user denied dependency changes".into());
            }
        }
        let build_lock = self.build_lock();
        let _build_guard = build_lock.lock().await;
        let _process_build_guard =
            local_apps::storage::lock_app_build(layout.root(), layout.app_id())
                .map_err(|error| error.to_string())?;
        let current_dependency = service
            .dependency_record(&app_id)
            .await
            .map_err(|error| error.to_string())?;
        let (_, current_baseline, _, _, _) = Self::prepare_dependency_change(
            &layout,
            &current_dependency,
            _input.get("changes").unwrap(),
        )?;
        if current_baseline != baseline {
            return Err(
                "dependencies_dirty: dependency baseline changed while waiting for confirmation; reconfirm before updating"
                    .into(),
            );
        }
        let receipt = self
            .issue_dependency_change_receipt(
                &app_id,
                baseline,
                requested_json,
                effective_package_json,
                changes.clone(),
            )
            .await?;
        Ok(json!({
            "ok": true,
            "app_id": app_id,
            "changes": changes,
            "receipt": {
                "id": receipt.receipt_id,
                "app_id": receipt.app_id,
                "issued_at_ms": receipt.issued_at_ms,
                "expires_at_ms": receipt.expires_at_ms,
            }
        }))
    }

    async fn update_dependencies(&self, input: Value) -> Result<Value, String> {
        let app_id = required_string(&input, "app_id")?.to_string();
        let receipt_id = required_string(&input, "receipt_id")?.to_string();
        let service = self.service()?;
        let layout = self.layout(&app_id)?;
        // Keep the same lock order as LocalAppBuilder: broker-wide async
        // mutex first, then the per-app cross-process lock. The dependency
        // snapshot, production build and any rollback therefore form one
        // transaction without deadlocking the builder.
        let build_lock = self.build_lock();
        let _build_guard = build_lock.lock().await;
        let _process_build_guard =
            local_apps::storage::lock_app_build(layout.root(), layout.app_id())
                .map_err(|error| error.to_string())?;
        let previous_dependency = service
            .dependency_record(&app_id)
            .await
            .map_err(|error| error.to_string())?;
        let receipt = self
            .claim_dependency_change_receipt(&app_id, &receipt_id)
            .await?;
        let current_baseline =
            match Self::load_trusted_dependency_baseline(&layout, &previous_dependency) {
                Ok((_, _, _, baseline)) => baseline,
                Err(error) => {
                    self.consume_dependency_change_receipt(&app_id, &receipt_id)
                        .await;
                    return Err(error);
                }
            };
        if current_baseline != receipt.baseline {
            self.consume_dependency_change_receipt(&app_id, &receipt_id)
                .await;
            return Err(
                "dependencies_dirty: dependency confirmation became stale before update; reconfirm before applying it"
                    .into(),
            );
        }
        let rollback = match self.capture_dependency_update_rollback(&layout, previous_dependency) {
            Ok(rollback) => rollback,
            Err(error) => {
                self.release_dependency_change_receipt_claim(&app_id, &receipt_id)
                    .await;
                return Err(error);
            }
        };
        let recovery_journal = match Self::dependency_update_recovery_journal(
            &layout,
            &rollback,
            DependencyUpdateRecoveryStatus::InProgress,
        ) {
            Ok(journal) => journal,
            Err(error) => {
                Self::discard_dependency_update_rollback(rollback);
                self.release_dependency_change_receipt_claim(&app_id, &receipt_id)
                    .await;
                return Err(error);
            }
        };
        if let Err(error) =
            Self::write_dependency_update_recovery_journal(&layout, &recovery_journal)
        {
            let _ = Self::remove_dependency_update_recovery_journal(&layout);
            Self::discard_dependency_update_rollback(rollback);
            self.release_dependency_change_receipt_claim(&app_id, &receipt_id)
                .await;
            return Err(error);
        }
        let result: Result<Value, String> = async {
            service
                .record(&app_id)
                .await
                .map_err(|error| error.to_string())?;
            let current = service
                .dependency_record(&app_id)
                .await
                .map_err(|error| error.to_string())?;
            if current.state == AppDependencyState::Installing {
                return Err(format!(
                    "app {app_id} already has a dependency install in progress"
                ));
            }
            let workspace = layout.root().join(layout.workspace_rel());
            let runtime = self.mobile_linux().ok_or_else(|| {
                "the mobile Node runtime is unavailable for dependency updates".to_string()
            })?;
            let dependency_staging = Self::prepare_dependency_staging(&layout)?;
            crate::local_apps_build::write_file(
                &dependency_staging,
                "package.json",
                &receipt.effective_package_json,
                true,
            )
            .map_err(|error| error.to_string())?;
            if current.state == AppDependencyState::Ready {
                service
                    .queue_dependency_install(&app_id)
                    .await
                    .map_err(|error| error.to_string())?;
            }
            service
                .start_dependency_install(&app_id)
                .await
                .map_err(|error| error.to_string())?;
            let dependency_store = self.dependency_store_root();
            std::fs::create_dir_all(&dependency_store)
                .map_err(|error| format!("create pnpm dependency store: {error}"))?;
            let build_mount = MountSpec {
                host_path: workspace.clone(),
                guest_path: guest_paths::local_app_build_project(&app_id, "store"),
                read_only: false,
                purpose: MountPurpose::LocalAppBuild,
            };
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
            let memory_mb =
                crate::local_apps_build::build_memory_budget_mb(self.physical_memory_bytes());
            let requires_network = receipt
                .summary
                .iter()
                .any(|change| !matches!(change.kind, DependencyChangeKind::Remove));
            // Add/update flows may use approved network access to resolve the
            // user-confirmed manifest and preheat the shared store. Remove-only
            // flows stay offline throughout so they cannot silently upgrade an
            // unrelated dependency. The second pass is always the commit gate:
            // after clearing the first tree, it must materialize that same
            // lock with network disabled before any snapshot or workspace file
            // is published.
            let resolution_request = Self::dependency_install_request(
                &build_mount,
                &store_mount,
                dependency_staging_guest_path.clone(),
                &build_state_root,
                memory_mb,
                if requires_network {
                    NetworkPolicy::Allowed
                } else {
                    NetworkPolicy::Disabled
                },
                false,
                false,
                true,
            );
            if let Err(error) =
                Self::run_dependency_install_command(runtime.as_ref(), resolution_request).await
            {
                let _ = Self::remove_owned_path(&dependency_staging);
                return Err(error);
            }
            if let Err(error) = Self::reset_dependency_staging_node_modules(&dependency_staging) {
                let _ = Self::remove_owned_path(&dependency_staging);
                return Err(error);
            }
            let frozen_request = Self::dependency_install_request(
                &build_mount,
                &store_mount,
                dependency_staging_guest_path,
                &build_state_root,
                memory_mb,
                NetworkPolicy::Disabled,
                true,
                false,
                true,
            );
            if let Err(error) =
                Self::run_dependency_install_command(runtime.as_ref(), frozen_request).await
            {
                let _ = Self::remove_owned_path(&dependency_staging);
                return Err(error);
            }
            if let Err(error) =
                validate_dependency_lifecycle_scripts(&dependency_staging.join("node_modules"))
            {
                let _ = Self::remove_owned_path(&dependency_staging);
                return Err(error);
            }
            let lock_bytes = std::fs::read(dependency_staging.join("pnpm-lock.yaml"))
                .map_err(|error| format!("read updated pnpm-lock.yaml: {error}"))?;
            let lock_digest = format!("{:x}", Sha256::digest(&lock_bytes));
            let snapshot_root = self.dependency_snapshot_root(&lock_digest);
            let snapshot_lock = self.dependency_snapshot_lock(&lock_digest).await;
            let _snapshot_guard = snapshot_lock.lock().await;
            if !Self::dependency_snapshot_is_ready(&snapshot_root, &lock_digest)? {
                Self::publish_dependency_snapshot(
                    &dependency_staging.join("node_modules"),
                    &snapshot_root,
                    &lock_digest,
                )?;
            }
            let commit_result: Result<(), String> = (|| {
                crate::local_apps_build::write_file(
                    &workspace,
                    crate::local_app_runtime_profiles::REQUESTED_FILE_REL,
                    &receipt.requested_json,
                    true,
                )
                .map_err(|error| error.to_string())?;
                crate::local_apps_build::write_file(
                    &workspace,
                    crate::local_app_runtime_profiles::EFFECTIVE_PACKAGE_FILE_REL,
                    &receipt.effective_package_json,
                    true,
                )
                .map_err(|error| error.to_string())?;
                crate::local_apps_build::write_file(
                    &workspace,
                    crate::local_app_runtime_profiles::LOCKFILE_FILE_REL,
                    &lock_bytes,
                    true,
                )
                .map_err(|error| error.to_string())?;
                crate::local_apps_build::write_file(
                    &workspace,
                    "package.json",
                    &receipt.effective_package_json,
                    true,
                )
                .map_err(|error| error.to_string())?;
                crate::local_apps_build::write_file(
                    &workspace,
                    "pnpm-lock.yaml",
                    &lock_bytes,
                    true,
                )
                .map_err(|error| error.to_string())?;
                Ok(())
            })();
            if let Err(error) = commit_result {
                let _ = Self::remove_owned_path(&dependency_staging);
                return Err(error);
            }
            if let Err(error) = self
                .finalize_dependency_install(&layout, &dependency_staging, &lock_digest)
                .await
            {
                return Err(error);
            }
            service
                .complete_dependency_install_with_metadata(
                    &app_id,
                    Some(lock_digest),
                    Some(PNPM_TOOLCHAIN_KEY.to_string()),
                )
                .await
                .map_err(|error| error.to_string())?;
            let dependencies = service
                .dependency_record(&app_id)
                .await
                .map_err(|error| error.to_string())?;
            let builder = crate::local_apps_build::LocalAppBuilder {
                mobile_linux: self.mobile_linux(),
                host: self,
            };
            builder
                .build_workspace_locked(&layout, &dependencies)
                .await
                .map_err(|error| format!("dependency update production build failed: {error}"))?;
            crate::local_apps_build::validate_build_for_launch(&layout)
                .map_err(|error| format!("dependency update profile smoke failed: {error}"))?;
            let mut committed_journal = recovery_journal.clone();
            committed_journal.status = DependencyUpdateRecoveryStatus::Committed;
            Self::write_dependency_update_recovery_journal(&layout, &committed_journal)?;
            Ok(json!({
                "ok": true,
                "app_id": app_id,
                "changes": receipt.summary,
                "dependencies": dependencies,
            }))
        }
        .await;
        match result {
            Ok(value) => {
                Self::discard_dependency_update_rollback(rollback);
                if let Err(error) = Self::remove_dependency_update_recovery_journal(&layout) {
                    tracing::warn!(
                        app_id = %app_id,
                        %error,
                        "dependency update committed but recovery journal cleanup was deferred"
                    );
                }
                self.consume_dependency_change_receipt(&app_id, &receipt_id)
                    .await;
                Ok(value)
            }
            Err(error) => {
                let rollback_error = match self
                    .restore_dependency_update_rollback(&service, &app_id, &layout, &rollback)
                    .await
                {
                    Ok(()) => {
                        let mut committed_journal = recovery_journal.clone();
                        committed_journal.status = DependencyUpdateRecoveryStatus::Committed;
                        Self::write_dependency_update_recovery_journal(&layout, &committed_journal)
                            .and_then(|_| {
                                Self::cleanup_dependency_update_recovery(
                                    &layout,
                                    &committed_journal,
                                )
                            })
                            .err()
                    }
                    Err(error) => Some(error),
                };
                self.release_dependency_change_receipt_claim(&app_id, &receipt_id)
                    .await;
                match rollback_error {
                    Some(rollback_error) => {
                        Err(format!("{error}; rollback failed: {rollback_error}"))
                    }
                    None => Err(error),
                }
            }
        }
    }

    async fn migrate_runtime_profile(&self, _input: Value) -> Result<Value, String> {
        Err("runtime profile migration is not available in this host build".into())
    }

    async fn prepare_shell_app(&self, record: local_apps::AppRecord) -> Result<(), String> {
        self.write_guided_contract_value(&record).await
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
        // The scaffold is fixed at creation. The workspace on disk IS the
        // scaffold, so accepting a change here would leave the generated source
        // and the re-pinned infrastructure describing two different
        // applications, and the next build would write the other scaffold's
        // files over working code. Rejected loudly rather than ignored: an
        // agent that believes it just converted the app has to find out now,
        // not after a build silently reverts half its work.
        //
        // `manifest.surface` is otherwise carried through untouched by the
        // load-modify-save above, which is what keeps it stable.
        if input.get("surface").is_some() {
            return Err(
                "an app's surface is fixed when the app is created and cannot be changed; \
                 create a new app to build the other shape"
                    .into(),
            );
        }
        // The device context is host-derived, never taken from `input`: the
        // agent only ever sees the mobile runtime reminder, and that
        // reminder's `Device class: phone` is not an iOS form factor. Every
        // save re-stamps it so the record tracks the host the app is
        // actually being generated on.
        manifest.device_context = self.host_device_context();
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

    async fn capture_ui(&self, input: Value) -> Result<Value, String> {
        let app_id = required_string(&input, "app_id")?.to_string();
        self.service()?
            .record(&app_id)
            .await
            .map_err(|e| e.to_string())?;
        // No `target`: an element crop is expressed as `rect` (see
        // `capture_ui_value`), which the native side DOES honour, so a
        // selector here would be a second, redundant way to say the same
        // thing — with no way to report which one won.
        self.request_ui(AppUiRequestDto {
            request_id: self.request_id("app-ui"),
            app_id,
            action: AppUiActionKindDto::CaptureView,
            target: None,
            value: capture_ui_value(&input)?,
        })
        .await
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
            "pointer" => AppUiActionKindDto::Pointer,
            "key" => AppUiActionKindDto::Key,
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

    async fn scaffold_app(
        &self,
        record: local_apps::AppRecord,
        surface: local_apps::AppSurface,
        runtime_profile: Option<local_apps::AppRuntimeProfile>,
    ) -> Result<(), String> {
        self.scaffold_app_value(&record, surface, runtime_profile)
            .await
    }

    async fn scaffold_shell_app(&self, input: Value) -> Result<Value, String> {
        self.scaffold_shell_app_value(input).await
    }

    async fn emit_create_failure(&self, error: &local_apps::AppError) {
        LocalAppsHostBroker::emit_create_failure(self, error).await;
    }
}

/// One of `LocalAppScaffold`'s confirmed identity fields, trimmed.
///
/// ⚠️ There is deliberately no `is_empty()` check on the RESULT.
/// [`required_string`] already refuses a missing value, a non-string and a
/// whitespace-only string, so §C.1 step 2's "non-empty" half is enforced
/// there; re-testing it after `.trim()` here would be a branch that can never
/// be taken. This wrapper exists only to say which FIELD was wrong, because
/// `required_string`'s own message does not name the tool's vocabulary.
fn confirmed_field<'a>(input: &'a Value, key: &str) -> Result<&'a str, String> {
    required_string(input, key)
        .map(str::trim)
        .map_err(|_| format!("invalid_argument: {key} must be a non-empty string"))
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

fn lower_runtime_profile_family(profile: AppRuntimeProfile) -> AppRuntimeProfileDto {
    match profile {
        AppRuntimeProfile::ReactDom => AppRuntimeProfileDto::ReactDom,
        AppRuntimeProfile::Canvas2d => AppRuntimeProfileDto::Canvas2d,
        AppRuntimeProfile::Three3d => AppRuntimeProfileDto::Three3d,
        AppRuntimeProfile::Phaser2d => AppRuntimeProfileDto::Phaser2d,
        AppRuntimeProfile::Babylon3d => AppRuntimeProfileDto::Babylon3d,
    }
}

fn raise_runtime_profile_family(
    profile: AppRuntimeProfileDto,
) -> Result<AppRuntimeProfile, String> {
    match profile {
        AppRuntimeProfileDto::ReactDom => Ok(AppRuntimeProfile::ReactDom),
        AppRuntimeProfileDto::Canvas2d => Ok(AppRuntimeProfile::Canvas2d),
        AppRuntimeProfileDto::Three3d => Ok(AppRuntimeProfile::Three3d),
        AppRuntimeProfileDto::Phaser2d => Ok(AppRuntimeProfile::Phaser2d),
        AppRuntimeProfileDto::Babylon3d => Ok(AppRuntimeProfile::Babylon3d),
        _ => Err("unknown runtime profile selection returned by the client".into()),
    }
}

fn lower_surface(surface: local_apps::AppSurface) -> AppSurfaceDto {
    match surface {
        local_apps::AppSurface::Dom => AppSurfaceDto::Dom,
        local_apps::AppSurface::Canvas => AppSurfaceDto::Canvas,
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
        // `instantiateStreaming` REQUIRES this exact type and this server sends
        // `X-Content-Type-Options: nosniff`, so serving a .wasm as
        // application/octet-stream fails the streaming path outright — with a
        // MIME complaint that reads nothing like the CSP refusal it is not.
        Some("wasm") => "application/wasm",
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
    let canonical_root = std::fs::canonicalize(root)
        .map_err(|error| format!("resolve dependency tree {}: {error}", root.display()))?;
    validate_dependency_entry(root, &canonical_root)?;
    validate_dependency_lifecycle_scripts(root)
}

const FORBIDDEN_DEPENDENCY_LIFECYCLE_SCRIPTS: [&str; 4] =
    ["preinstall", "install", "postinstall", "prepare"];

const TRUSTED_TOOLCHAIN_NATIVE_BINDINGS: &[(&str, &str, &str)] = &[
    (
        "@rolldown/binding-linux-arm64-musl",
        "1.2.6",
        "rolldown-binding.linux-arm64-musl.node",
    ),
    (
        "@rolldown/binding-linux-x64-musl",
        "1.2.6",
        "rolldown-binding.linux-x64-musl.node",
    ),
    (
        "@rollup/rollup-linux-arm64-musl",
        "4.44.0",
        "rollup.linux-arm64-musl.node",
    ),
    (
        "@rollup/rollup-linux-x64-musl",
        "4.44.0",
        "rollup.linux-x64-musl.node",
    ),
    (
        "lightningcss-linux-arm64-musl",
        "1.33.0",
        "lightningcss.linux-arm64-musl.node",
    ),
    (
        "lightningcss-linux-x64-musl",
        "1.33.0",
        "lightningcss.linux-x64-musl.node",
    ),
];

const TRUSTED_TOOLCHAIN_LIFECYCLE_SCRIPTS: &[(&str, &str, &[&str])] = &[
    ("balanced-match", "4.0.4", &["prepare"]),
    ("brace-expansion", "5.0.9", &["prepare"]),
    ("dom-serializer", "2.0.0", &["prepare"]),
    ("domelementtype", "2.3.0", &["prepare"]),
    ("domhandler", "5.0.3", &["prepare"]),
    ("domutils", "3.2.2", &["prepare"]),
    ("entities", "4.5.0", &["prepare"]),
    ("html-dom-parser", "5.1.8", &["prepare"]),
    ("html-react-parser", "5.2.17", &["prepare"]),
    ("htmlparser2", "10.1.0", &["prepare"]),
    ("inline-style-parser", "0.2.7", &["prepare"]),
    ("lightningcss", "1.33.0", &["prepare"]),
    ("minimatch", "10.2.6", &["prepare"]),
    ("style-to-js", "1.1.21", &["prepare"]),
    ("style-to-object", "1.0.14", &["prepare"]),
];

fn trusted_toolchain_lifecycle_scripts(
    package: &str,
    version: &str,
) -> Option<&'static [&'static str]> {
    TRUSTED_TOOLCHAIN_LIFECYCLE_SCRIPTS.iter().find_map(
        |(trusted_package, trusted_version, scripts)| {
            (*trusted_package == package && *trusted_version == version).then_some(*scripts)
        },
    )
}

fn dependency_package_path(package: &str) -> PathBuf {
    let mut path = PathBuf::new();
    for part in package.split('/') {
        path.push(part);
    }
    path
}

fn validate_trusted_dependency_manifest(
    dependency_root: &Path,
    package: &str,
    version: &str,
) -> Result<(), String> {
    let manifest_path = dependency_root
        .join(dependency_package_path(package))
        .join("package.json");
    let metadata = std::fs::symlink_metadata(&manifest_path).map_err(|error| {
        format!(
            "inspect dependency package manifest {}: {error}",
            manifest_path.display()
        )
    })?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err(format!(
            "dependency package manifest must be a regular file: {}",
            manifest_path.display()
        ));
    }
    let bytes = std::fs::read(&manifest_path).map_err(|error| {
        format!(
            "read dependency package manifest {}: {error}",
            manifest_path.display()
        )
    })?;
    let manifest: Value = serde_json::from_slice(&bytes).map_err(|error| {
        format!(
            "parse dependency package manifest {}: {error}",
            manifest_path.display()
        )
    })?;
    if manifest.get("name").and_then(Value::as_str) != Some(package)
        || manifest.get("version").and_then(Value::as_str) != Some(version)
    {
        return Err(format!(
            "dependency package manifest {} does not match trusted package {}@{}",
            manifest_path.display(),
            package,
            version
        ));
    }
    Ok(())
}

fn trusted_dependency_lifecycle_script_path(
    dependency_root: &Path,
    manifest_path: &Path,
    package: &str,
    version: &str,
    script: &str,
) -> Result<bool, String> {
    let canonical_manifest = std::fs::canonicalize(manifest_path).map_err(|error| {
        format!(
            "canonicalize dependency package manifest {}: {error}",
            manifest_path.display()
        )
    })?;
    let relative = canonical_manifest
        .strip_prefix(dependency_root)
        .map_err(|_| {
            format!(
                "dependency package manifest {} is outside {}",
                canonical_manifest.display(),
                dependency_root.display()
            )
        })?;
    let expected = dependency_package_path(package).join("package.json");
    if relative != expected {
        return Ok(false);
    }
    validate_trusted_dependency_manifest(dependency_root, package, version)?;
    Ok(trusted_toolchain_lifecycle_scripts(package, version)
        .is_some_and(|allowed| allowed.contains(&script)))
}

fn trusted_dependency_native_binding_path(
    path: &Path,
    dependency_root: &Path,
) -> Result<bool, String> {
    let canonical_path = std::fs::canonicalize(path).map_err(|error| {
        format!(
            "canonicalize dependency tree entry {}: {error}",
            path.display()
        )
    })?;
    let relative = canonical_path.strip_prefix(dependency_root).map_err(|_| {
        format!(
            "dependency tree entry {} is outside {}",
            canonical_path.display(),
            dependency_root.display()
        )
    })?;
    for (package, version, file_name) in TRUSTED_TOOLCHAIN_NATIVE_BINDINGS {
        let expected = dependency_package_path(package).join(file_name);
        if relative == expected {
            validate_trusted_dependency_manifest(dependency_root, package, version)?;
            return Ok(true);
        }
    }
    Ok(false)
}

/// Reject package lifecycle hooks from a resolved dependency tree. The
/// resolver runs with scripts disabled, but retaining a hook in the snapshot
/// would let a later package-manager invocation execute it. Only explicitly
/// reviewed fixed-toolchain metadata is exempted.
fn validate_dependency_lifecycle_scripts(root: &Path) -> Result<(), String> {
    let dependency_root = root
        .canonicalize()
        .map_err(|error| format!("canonicalize dependency tree {}: {error}", root.display()))?;
    validate_dependency_lifecycle_scripts_from_root(&dependency_root, &dependency_root)
}

fn validate_dependency_lifecycle_scripts_from_root(
    dependency_root: &Path,
    current: &Path,
) -> Result<(), String> {
    let metadata = std::fs::symlink_metadata(current)
        .map_err(|error| format!("inspect dependency tree {}: {error}", current.display()))?;
    if metadata.file_type().is_symlink() || metadata.is_file() {
        return Ok(());
    }
    if !metadata.is_dir() {
        return Err(format!(
            "dependency tree entry is not regular: {}",
            current.display()
        ));
    }
    for entry in std::fs::read_dir(current)
        .map_err(|error| format!("read dependency tree {}: {error}", current.display()))?
    {
        let entry = entry.map_err(|error| format!("read dependency tree entry: {error}"))?;
        let path = entry.path();
        let metadata = std::fs::symlink_metadata(&path)
            .map_err(|error| format!("inspect dependency tree {}: {error}", path.display()))?;
        if metadata.file_type().is_symlink() {
            continue;
        }
        if metadata.is_dir() {
            validate_dependency_lifecycle_scripts_from_root(dependency_root, &path)?;
            continue;
        }
        if !metadata.is_file() || !installed_package_manifest(&path) {
            continue;
        }
        let bytes = std::fs::read(&path).map_err(|error| {
            format!(
                "read dependency package manifest {}: {error}",
                path.display()
            )
        })?;
        let manifest: Value = serde_json::from_slice(&bytes).map_err(|error| {
            format!(
                "parse dependency package manifest {}: {error}",
                path.display()
            )
        })?;
        let package = manifest
            .get("name")
            .and_then(Value::as_str)
            .filter(|name| !name.trim().is_empty())
            .unwrap_or("<unnamed>");
        let Some(scripts) = manifest.get("scripts").and_then(Value::as_object) else {
            continue;
        };
        let version = manifest
            .get("version")
            .and_then(Value::as_str)
            .unwrap_or_default();
        for script in FORBIDDEN_DEPENDENCY_LIFECYCLE_SCRIPTS {
            if scripts.contains_key(script) {
                if trusted_dependency_lifecycle_script_path(
                    dependency_root,
                    &path,
                    package,
                    version,
                    script,
                )? {
                    continue;
                }
                return Err(format!(
                    "dependency package {package} declares forbidden lifecycle script {script} in {}",
                    path.display()
                ));
            }
        }
    }
    Ok(())
}

fn validate_dependency_entry(path: &Path, canonical_root: &Path) -> Result<(), String> {
    let metadata = std::fs::symlink_metadata(path)
        .map_err(|error| format!("inspect dependency tree {}: {error}", path.display()))?;
    if metadata.file_type().is_symlink() {
        // A contained shim is legal; anything reaching outside the tree is not.
        // Traversal never descends THROUGH the link, so a link to a directory
        // inside the tree cannot make this recursion unbounded.
        let target = dependency_symlink_target(path, canonical_root)?;
        let path_is_native =
            path.extension().and_then(|extension| extension.to_str()) == Some("node");
        let target_is_native =
            target.extension().and_then(|extension| extension.to_str()) == Some("node");
        if path_is_native || target_is_native {
            return Err(format!(
                "dependency tree contains a native Node addon symlink: {}",
                path.display()
            ));
        }
        return Ok(());
    }
    if metadata.is_file() {
        if path.extension().and_then(|extension| extension.to_str()) == Some("node") {
            if trusted_dependency_native_binding_path(path, canonical_root)? {
                return Ok(());
            }
            return Err(format!(
                "dependency tree contains a native Node addon: {}",
                path.display()
            ));
        }
        return Ok(());
    }
    if !metadata.is_dir() {
        return Err(format!(
            "dependency tree entry is not regular: {}",
            path.display()
        ));
    }
    for entry in std::fs::read_dir(path)
        .map_err(|error| format!("read dependency tree {}: {error}", path.display()))?
    {
        let entry = entry.map_err(|error| format!("read dependency tree entry: {error}"))?;
        validate_dependency_entry(&entry.path(), canonical_root)?;
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
        let metadata = std::fs::symlink_metadata(&path)
            .map_err(|error| format!("inspect dependency tree file {}: {error}", path.display()))?;
        // A shim is digested by its TARGET, tagged so it can never collide with
        // a regular file whose contents happen to be that same path text --
        // otherwise swapping `.bin/vite` between a link and a file would leave
        // the attestation unchanged.
        let (kind, bytes) = if metadata.file_type().is_symlink() {
            let target = std::fs::read_link(&path).map_err(|error| {
                format!("read dependency tree symlink {}: {error}", path.display())
            })?;
            (1u8, target.as_os_str().as_encoded_bytes().to_vec())
        } else {
            let bytes = std::fs::read(&path).map_err(|error| {
                format!("read dependency tree file {}: {error}", path.display())
            })?;
            (0u8, bytes)
        };
        digest.update((relative.len() as u64).to_le_bytes());
        digest.update(relative);
        digest.update([kind]);
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
        files.push((relative.to_string_lossy().into_owned(), root.to_path_buf()));
        return Ok(());
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
        // Leave the link alone: `set_permissions` FOLLOWS it, so chmod-ing here
        // would re-apply to the target that the walk already visits on its own,
        // and there is no portable `lchmod`. The link node carries no content
        // to protect -- its target is inside the tree and is made read-only in
        // its own right.
        return Ok(());
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

/// Resolve a symlink and require that it lands inside `canonical_root`.
///
/// The invariant a dependency tree actually needs is that no link reaches
/// outside the tree -- the same rule `stage-local-app-runtime.py`'s
/// `validate_symlinks` already enforces for the staged runtime. Forbidding
/// links outright is stricter than the threat and rejects `node_modules/.bin`,
/// which `pnpm install` writes as relative shims for every package carrying a
/// `bin` field.
///
/// Resolution is strict: a shim whose target does not exist is rejected rather
/// than copied forward as a dangling entry that fails later at `vite` spawn
/// time with an unrelated message.
fn dependency_symlink_target(path: &Path, canonical_root: &Path) -> Result<PathBuf, String> {
    let resolved = std::fs::canonicalize(path).map_err(|error| {
        format!(
            "dependency tree symlink does not resolve: {}: {error}",
            path.display()
        )
    })?;
    if !resolved.starts_with(canonical_root) {
        return Err(format!(
            "dependency tree symlink escapes the tree: {} -> {}",
            path.display(),
            resolved.display()
        ));
    }
    Ok(resolved)
}

fn clone_or_copy_tree(source: &Path, destination: &Path) -> io::Result<()> {
    let metadata = std::fs::symlink_metadata(source)?;
    if metadata.file_type().is_symlink() {
        // The ROOT being a link is still refused: it would make the whole tree
        // an alias for somewhere else, which is the escape this guards.
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
    let canonical_source = std::fs::canonicalize(source)?;
    if try_clone_tree(source, destination).is_ok() {
        return Ok(());
    }
    let _ = std::fs::remove_dir_all(destination);
    std::fs::create_dir_all(destination)?;
    copy_dependency_tree(source, destination, &canonical_source)
}

#[cfg(unix)]
fn recreate_dependency_symlink(target: &Path, destination: &Path) -> io::Result<()> {
    std::os::unix::fs::symlink(target, destination)
}

#[cfg(not(unix))]
fn recreate_dependency_symlink(_target: &Path, _destination: &Path) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "dependency tree symlinks are unsupported on this platform",
    ))
}

fn copy_dependency_tree(
    source: &Path,
    destination: &Path,
    canonical_source_root: &Path,
) -> io::Result<()> {
    for entry in std::fs::read_dir(source)? {
        let entry = entry?;
        let source_path = entry.path();
        let destination_path = destination.join(entry.file_name());
        let metadata = std::fs::symlink_metadata(&source_path)?;
        if metadata.file_type().is_symlink() {
            // Containment is checked against the ORIGINAL root, not the
            // directory being walked, so `.bin/vite -> ../vite/bin/vite.js`
            // stays legal while `../../../etc/passwd` does not.
            dependency_symlink_target(&source_path, canonical_source_root)
                .map_err(io::Error::other)?;
            let target = std::fs::read_link(&source_path)?;
            if target.is_absolute() {
                // An absolute target resolves inside the tree only for as long
                // as the tree stays at this path; copying it into the snapshot
                // would silently re-point at the source app's workspace.
                return Err(io::Error::other(format!(
                    "dependency tree symlink must be relative: {} -> {}",
                    source_path.display(),
                    target.display()
                )));
            }
            recreate_dependency_symlink(&target, &destination_path)?;
            continue;
        }
        if metadata.is_dir() {
            std::fs::create_dir_all(&destination_path)?;
            copy_dependency_tree(&source_path, &destination_path, canonical_source_root)?;
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
        isolated_requests: Mutex<Vec<LinuxCommandRequest>>,
        pnpm_node_modules_entries: Mutex<Vec<Vec<String>>>,
        enforcement_receipt: AtomicBool,
        fail_kill: AtomicBool,
        fail_build: AtomicBool,
        fail_frozen_install: AtomicBool,
        inject_lifecycle_script: AtomicBool,
        omit_staged_vite_marker: AtomicBool,
    }

    impl MockMobileLinuxRuntime {
        fn new(spawn_delay: Duration) -> Arc<Self> {
            Arc::new(Self {
                spawn_delay,
                spawn_count: AtomicUsize::new(0),
                next_task_id: AtomicU64::new(1),
                tasks: Mutex::new(HashMap::new()),
                last_request: Mutex::new(None),
                isolated_requests: Mutex::new(Vec::new()),
                pnpm_node_modules_entries: Mutex::new(Vec::new()),
                enforcement_receipt: AtomicBool::new(true),
                fail_kill: AtomicBool::new(false),
                fail_build: AtomicBool::new(false),
                fail_frozen_install: AtomicBool::new(false),
                inject_lifecycle_script: AtomicBool::new(false),
                omit_staged_vite_marker: AtomicBool::new(false),
            })
        }

        fn set_fail_kill(&self, fail: bool) {
            self.fail_kill.store(fail, Ordering::SeqCst);
        }

        fn set_enforcement_receipt(&self, enforced: bool) {
            self.enforcement_receipt.store(enforced, Ordering::SeqCst);
        }

        fn set_omit_staged_vite_marker(&self, omit: bool) {
            self.omit_staged_vite_marker.store(omit, Ordering::SeqCst);
        }

        fn set_fail_build(&self, fail: bool) {
            self.fail_build.store(fail, Ordering::SeqCst);
        }

        fn set_fail_frozen_install(&self, fail: bool) {
            self.fail_frozen_install.store(fail, Ordering::SeqCst);
        }

        fn set_inject_lifecycle_script(&self, inject: bool) {
            self.inject_lifecycle_script.store(inject, Ordering::SeqCst);
        }

        async fn isolated_requests(&self) -> Vec<LinuxCommandRequest> {
            self.isolated_requests.lock().await.clone()
        }

        async fn pnpm_node_modules_entries(&self) -> Vec<Vec<String>> {
            self.pnpm_node_modules_entries.lock().await.clone()
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

        async fn run_isolated(
            &self,
            request: LinuxCommandRequest,
        ) -> Result<traits::LinuxCommandResult, MobileLinuxError> {
            *self.last_request.lock().await = Some(request.clone());
            self.isolated_requests.lock().await.push(request.clone());
            let build_mount = request.mounts.first().ok_or_else(|| {
                MobileLinuxError::InvalidRequest("missing LocalAppBuild mount".into())
            })?;
            let guest_cwd = request.cwd.clone().ok_or_else(|| {
                MobileLinuxError::InvalidRequest("missing dependency staging cwd".into())
            })?;
            let relative = guest_cwd
                .strip_prefix(&build_mount.guest_path)
                .map(|suffix| suffix.trim_start_matches('/'))
                .ok_or_else(|| {
                    MobileLinuxError::InvalidRequest(
                        "dependency staging cwd is outside the mounted workspace".into(),
                    )
                })?;
            let host_cwd = if relative.is_empty() {
                build_mount.host_path.clone()
            } else {
                build_mount.host_path.join(relative)
            };
            if request.command == "/usr/bin/pnpm" {
                let mut entries = fs::read_dir(host_cwd.join("node_modules"))
                    .into_iter()
                    .flatten()
                    .filter_map(Result::ok)
                    .map(|entry| entry.file_name().to_string_lossy().into_owned())
                    .collect::<Vec<_>>();
                entries.sort();
                self.pnpm_node_modules_entries.lock().await.push(entries);
            }
            if request.command == "/usr/bin/pnpm"
                && request.args.iter().any(|arg| arg == "--frozen-lockfile")
                && self.fail_frozen_install.load(Ordering::SeqCst)
            {
                return Ok(traits::LinuxCommandResult {
                    stdout: String::new(),
                    stderr: "synthetic frozen install failure".into(),
                    exit_code: 1,
                    timed_out: false,
                    cancelled: false,
                    enforcement: traits::LinuxEnforcementReceipt {
                        network_policy_enforced: self.enforcement_receipt.load(Ordering::SeqCst),
                        memory_limit_enforced: self.enforcement_receipt.load(Ordering::SeqCst),
                    },
                });
            }
            if request.command == "/usr/bin/pnpm"
                && request.args.iter().any(|arg| arg == "--lockfile-only")
            {
                return Ok(traits::LinuxCommandResult {
                    stdout: "lockfile resolved".into(),
                    stderr: String::new(),
                    exit_code: 0,
                    timed_out: false,
                    cancelled: false,
                    enforcement: traits::LinuxEnforcementReceipt {
                        network_policy_enforced: self.enforcement_receipt.load(Ordering::SeqCst),
                        memory_limit_enforced: self.enforcement_receipt.load(Ordering::SeqCst),
                    },
                });
            }
            if request.command == "/usr/bin/node" {
                if self.fail_build.load(Ordering::SeqCst) {
                    return Ok(traits::LinuxCommandResult {
                        stdout: String::new(),
                        stderr: "synthetic build failure".into(),
                        exit_code: 1,
                        timed_out: false,
                        cancelled: false,
                        enforcement: traits::LinuxEnforcementReceipt {
                            network_policy_enforced: self
                                .enforcement_receipt
                                .load(Ordering::SeqCst),
                            memory_limit_enforced: self.enforcement_receipt.load(Ordering::SeqCst),
                        },
                    });
                }
                let output_rel = request
                    .args
                    .windows(2)
                    .find_map(|pair| (pair[0] == "--outDir").then_some(pair[1].as_str()))
                    .ok_or_else(|| {
                        MobileLinuxError::InvalidRequest("missing Vite --outDir".into())
                    })?;
                let output = host_cwd.join(output_rel);
                fs::create_dir_all(&output).map_err(|error| {
                    MobileLinuxError::Io(format!("create fake build output: {error}"))
                })?;
                fs::write(
                    output.join("index.html"),
                    b"<!doctype html><title>built</title>",
                )
                .map_err(|error| {
                    MobileLinuxError::Io(format!("write fake build output: {error}"))
                })?;
                return Ok(traits::LinuxCommandResult {
                    stdout: "built".into(),
                    stderr: String::new(),
                    exit_code: 0,
                    timed_out: false,
                    cancelled: false,
                    enforcement: traits::LinuxEnforcementReceipt {
                        network_policy_enforced: self.enforcement_receipt.load(Ordering::SeqCst),
                        memory_limit_enforced: self.enforcement_receipt.load(Ordering::SeqCst),
                    },
                });
            }
            fs::create_dir_all(host_cwd.join("node_modules")).map_err(|error| {
                MobileLinuxError::Io(format!("create fake node_modules root: {error}"))
            })?;
            if !self.omit_staged_vite_marker.load(Ordering::SeqCst) {
                let vite = host_cwd.join("node_modules/vite/bin/vite.js");
                fs::create_dir_all(vite.parent().expect("vite parent")).map_err(|error| {
                    MobileLinuxError::Io(format!("create fake install tree: {error}"))
                })?;
                fs::write(&vite, b"#!/usr/bin/env node\n").map_err(|error| {
                    MobileLinuxError::Io(format!("write fake vite binary: {error}"))
                })?;
                fs::write(
                    host_cwd.join("node_modules/vite/package.json"),
                    r#"{"name":"vite","version":"8.2.1","license":"MIT"}"#,
                )
                .map_err(|error| {
                    MobileLinuxError::Io(format!("write fake vite manifest: {error}"))
                })?;
            }
            fs::write(host_cwd.join("node_modules/react.js"), b"react")
                .map_err(|error| MobileLinuxError::Io(format!("write fake dependency: {error}")))?;
            fs::create_dir_all(host_cwd.join("node_modules/react")).map_err(|error| {
                MobileLinuxError::Io(format!("create fake react package dir: {error}"))
            })?;
            let react_manifest = if self.inject_lifecycle_script.load(Ordering::SeqCst) {
                r#"{"name":"react","version":"19.2.8","license":"MIT","scripts":{"install":"echo unsafe"}}"#
            } else {
                r#"{"name":"react","version":"19.2.8","license":"MIT"}"#
            };
            fs::write(
                host_cwd.join("node_modules/react/package.json"),
                react_manifest,
            )
            .map_err(|error| MobileLinuxError::Io(format!("write fake react manifest: {error}")))?;
            Ok(traits::LinuxCommandResult {
                stdout: "ok".into(),
                stderr: String::new(),
                exit_code: 0,
                timed_out: false,
                cancelled: false,
                enforcement: traits::LinuxEnforcementReceipt {
                    network_policy_enforced: self.enforcement_receipt.load(Ordering::SeqCst),
                    memory_limit_enforced: self.enforcement_receipt.load(Ordering::SeqCst),
                },
            })
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
            Some(runtime_root.clone()),
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
            Some(runtime_root.clone()),
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
        seed_launchable_runtime_fixture(root.path(), &record, name);
        service
            .commit_scaffold(&record.id, name, "a test app", None)
            .await
            .expect("commit fixture scaffold");
        record.id
    }

    fn collect_fixture_files(current: &Path, files: &mut Vec<PathBuf>) {
        let metadata = fs::symlink_metadata(current).expect("inspect fixture output");
        assert!(
            !metadata.file_type().is_symlink(),
            "fixture output must not contain symlinks: {}",
            current.display()
        );
        if metadata.is_dir() {
            for entry in fs::read_dir(current).expect("read fixture output") {
                let entry = entry.expect("fixture output entry");
                collect_fixture_files(&entry.path(), files);
            }
        } else if metadata.is_file() {
            files.push(current.to_path_buf());
        } else {
            panic!(
                "fixture output must be a regular file or directory: {}",
                current.display()
            );
        }
    }

    fn fixture_output_digest(root: &Path) -> String {
        let mut files = Vec::new();
        collect_fixture_files(root, &mut files);
        files.sort();
        let mut hasher = Sha256::new();
        for path in files {
            let relative = path
                .strip_prefix(root)
                .expect("fixture output stays under the root");
            hasher.update(relative.to_string_lossy().replace('\\', "/").as_bytes());
            hasher.update([0]);
            hasher.update(fs::read(&path).expect("read fixture output file"));
            hasher.update([0]);
        }
        format!("{:x}", hasher.finalize())
    }

    fn write_fixture_package_manifest(node_modules: &Path, package: &str, version: &str) {
        let package_dir = node_modules.join(package);
        fs::create_dir_all(&package_dir).expect("create fixture package directory");
        fs::write(
            package_dir.join("package.json"),
            format!("{{\"name\":\"{package}\",\"version\":\"{version}\"}}\n"),
        )
        .expect("write fixture package manifest");
    }

    fn seed_launchable_runtime_fixture(root: &Path, record: &local_apps::AppRecord, name: &str) {
        let layout = AppLayout::new(root.to_path_buf(), record.id.clone()).expect("layout");
        let workspace = root.join(layout.workspace_rel());
        let binding = crate::local_app_runtime_profiles::current_binding_for_family(
            local_apps::AppRuntimeProfile::ReactDom,
        )
        .expect("published react-dom runtime profile");
        let artifacts =
            scaffold_runtime_profile(Some(binding.clone()), local_apps::AppSurface::Dom)
                .expect("react-dom scaffold artifacts");
        stamp_scaffold_identity(&layout, name, &artifacts).expect("stamp fixture scaffold");
        persist_runtime_profile_files(&workspace, &artifacts)
            .expect("persist fixture runtime profile files");

        let contract = crate::local_app_runtime_profiles::contract_for_binding(&binding)
            .expect("react-dom runtime contract");
        let node_modules = workspace.join("node_modules");
        for &(package, version) in contract.core_packages {
            write_fixture_package_manifest(&node_modules, package, version);
        }
        let vite_bin = node_modules.join("vite/bin/vite.js");
        fs::create_dir_all(vite_bin.parent().expect("vite bin parent"))
            .expect("create fixture vite bin dir");
        fs::write(&vite_bin, b"#!/usr/bin/env node\n").expect("write fixture vite marker");
        fs::write(workspace.join("vite.config.mjs"), "export default {};\n")
            .expect("mark fixture as a Vite app");

        let tree_sha256 =
            dependency_tree_digest(&workspace.join("node_modules")).expect("dependency tree");
        refresh_runtime_profile_snapshot(&layout, &tree_sha256)
            .expect("refresh fixture dependency snapshot");
        let manifest = load_manifest(&layout).expect("fixture manifest");
        storage::save_dependency_record(
            root,
            &local_apps::AppDependencyRecord {
                schema_version: local_apps::APPS_SCHEMA_VERSION,
                app_id: record.id.clone(),
                state: local_apps::AppDependencyState::Ready,
                lockfile_sha256: manifest
                    .dependency_snapshot
                    .as_ref()
                    .map(|snapshot| snapshot.lockfile_sha256.clone()),
                toolchain_key: manifest
                    .dependency_snapshot
                    .as_ref()
                    .map(|snapshot| snapshot.toolchain_key.clone()),
                install_attempts: 1,
                last_error: None,
                updated_at_ms: record.updated_at_ms,
            },
        )
        .expect("save fixture dependency record");

        let static_dist = root
            .join(layout.build_rel(false))
            .join(crate::local_apps_build::VITE_OUTPUT_DIR);
        fs::create_dir_all(&static_dist).expect("create static dist");
        fs::write(static_dist.join("index.html"), "<html>ok</html>").expect("write index.html");
        let full_build = root.join(layout.build_rel(true));
        fs::create_dir_all(&full_build).expect("create full build");
        let build_receipt = json!({
            "version": 3,
            "buildKey": "fixture-static-build",
            "runtimeContractSha256": manifest.runtime_contract_hash().expect("runtime contract hash"),
            "dependencySnapshotSha256": manifest.dependency_snapshot_hash().expect("dependency snapshot hash"),
            "outputSha256": fixture_output_digest(&static_dist),
        });
        fs::write(
            root.join(layout.build_rel(false)).join("build.json"),
            serde_json::to_vec_pretty(&build_receipt).expect("serialize fixture build receipt"),
        )
        .expect("write fixture build receipt");
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
            .scaffold_app_value(
                &record,
                local_apps::AppSurface::Dom,
                Some(local_apps::AppRuntimeProfile::ReactDom),
            )
            .await
            .expect("scaffold app");
        let layout = AppLayout::new(root.path().to_path_buf(), record.id.clone()).expect("layout");
        let lingxi =
            std::fs::read_to_string(root.path().join(layout.workspace_rel()).join("LINGXI.md"))
                .expect("read LINGXI.md");
        (record.id, lingxi)
    }

    /// Creation, not just `LocalAppManifest`, records the target: an app the
    /// agent never declares a manifest for still knows what it was built on.
    #[tokio::test]
    async fn scaffold_records_the_host_device_context() {
        let runtime = MockMobileLinuxRuntime::new(Duration::ZERO);
        let (root, service, broker) = create_broker(false, Some(runtime.clone())).await;
        assert!(broker
            .attach_host_environment(host_environment(
                traits::MobileHostOs::Ios,
                traits::MobileDeviceClass::Tablet,
            ))
            .is_ok());
        let record = service
            .create_app(Some("Scaffolded"), "a test app", None)
            .await
            .expect("create app");
        broker
            .scaffold_app_value(
                &record,
                local_apps::AppSurface::Dom,
                Some(local_apps::AppRuntimeProfile::ReactDom),
            )
            .await
            .expect("scaffold app");

        let layout = AppLayout::new(root.path().to_path_buf(), record.id).expect("layout");
        let recorded = load_manifest(&layout)
            .expect("manifest")
            .device_context
            .expect("creation records the native target");
        assert_eq!(recorded.os, "ios");
        assert_eq!(recorded.form_factor, "ipad");
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
                (!inner.is_empty() && inner.bytes().all(|b| b.is_ascii_lowercase() || b == b'_'))
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
        let runtime = MockMobileLinuxRuntime::new(Duration::ZERO);
        let (_app_id, lingxi) = scaffolded_lingxi(true, Some(runtime)).await;
        assert!(
            lingxi.contains("repository-verified Vite + Ionic foundation"),
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
            .scaffold_app_value(
                &record,
                local_apps::AppSurface::Dom,
                Some(local_apps::AppRuntimeProfile::ReactDom),
            )
            .await
            .expect("scaffold app");
        let layout = AppLayout::new(root.path().to_path_buf(), record.id).expect("layout");
        let lingxi =
            std::fs::read_to_string(root.path().join(layout.workspace_rel()).join("LINGXI.md"))
                .expect("read LINGXI.md");

        assert!(lingxi.contains("do not run a package manager in this local-app workspace"));
        assert!(broker
            .create_next_step()
            .contains("Do not recreate the app"));
        assert!(broker
            .create_next_step()
            .contains("do not install dependencies yet"));
    }

    #[tokio::test]
    async fn create_initializer_persists_dependency_snapshot_before_the_app_becomes_visible() {
        let runtime = MockMobileLinuxRuntime::new(Duration::ZERO);
        let (root, service, broker) = create_broker(false, Some(runtime.clone())).await;
        let host = Arc::clone(&broker);

        let record = service
            .create_app_with_git_and_workflow_model_and_initializer(
                Some("Tracker"),
                "a test app",
                None,
                false,
                None,
                local_apps::CreateMode::Scaffolded,
                None,
                move |record| {
                    let host = Arc::clone(&host);
                    async move {
                        host.scaffold_app(
                            record,
                            local_apps::AppSurface::Dom,
                            Some(local_apps::AppRuntimeProfile::ReactDom),
                        )
                        .await
                        .map_err(local_apps::AppError::Io)
                    }
                },
            )
            .await
            .expect("create app");

        let layout = AppLayout::new(root.path().to_path_buf(), record.id.clone()).expect("layout");
        let manifest = load_manifest(&layout).expect("manifest");
        assert!(
            record.scaffolded,
            "the returned app is visible only after commit"
        );
        assert_eq!(
            service
                .dependency_record(&record.id)
                .await
                .expect("dependency record")
                .state,
            local_apps::AppDependencyState::Ready
        );
        assert!(
            manifest.dependency_snapshot.is_some(),
            "the visible app must already carry a verified dependency snapshot"
        );
    }

    // ---- §C.1 `LocalAppScaffold` — the create transaction --------------

    /// The "+" button's shell, exactly as `host.rs` creates one: no brief, no
    /// surface, `scaffolded == false`, and the GUIDED workspace contract on
    /// disk so a test can prove the formal one replaced it.
    async fn shell_app_fixture(
        broker: &Arc<LocalAppsHostBroker>,
        service: &Arc<AppService>,
    ) -> local_apps::AppRecord {
        let record = service
            .create_app_with_mode(None, "", None, local_apps::CreateMode::Shell, None)
            .await
            .expect("create shell app");
        assert!(
            !record.scaffolded,
            "the fixture must actually be the state these tests name"
        );
        assert_eq!(record.name, local_apps::service::PLACEHOLDER_APP_NAME);
        assert_eq!(record.brief, "");
        broker
            .write_guided_contract_value(&record)
            .await
            .expect("write the guided contract");
        record
    }

    fn scaffold_input(app_id: &str, name: &str, brief: &str, surface: &str) -> Value {
        json!({
            "app_id": app_id,
            "name": name,
            "brief": brief,
            "surface": surface,
        })
    }

    async fn confirmed_scaffold_input(
        broker: &Arc<LocalAppsHostBroker>,
        app_id: &str,
        name: &str,
        brief: &str,
        surface: &str,
    ) -> Value {
        let profile = match surface {
            "dom" => local_apps::AppRuntimeProfile::ReactDom,
            "canvas" => local_apps::AppRuntimeProfile::Canvas2d,
            other => panic!("unsupported test scaffold surface {other}"),
        };
        let binding = crate::local_app_runtime_profiles::current_binding_for_family(profile)
            .expect("published runtime profile");
        let receipt = broker
            .issue_runtime_profile_receipt(app_id, binding)
            .await
            .expect("issue test runtime-profile receipt");
        json!({
            "app_id": app_id,
            "name": name,
            "brief": brief,
            "runtime_profile_receipt": receipt.receipt_id,
        })
    }

    fn workspace_of(root: &TempDir, app_id: &str) -> PathBuf {
        let layout = AppLayout::new(root.path().to_path_buf(), app_id.to_string()).expect("layout");
        root.path().join(layout.workspace_rel())
    }

    fn dependency_baseline_for(
        layout: &AppLayout,
        dependency_record: &local_apps::AppDependencyRecord,
    ) -> DependencyBaselineIdentity {
        LocalAppsHostBroker::load_trusted_dependency_baseline(layout, dependency_record)
            .expect("trusted dependency baseline")
            .3
    }

    /// Break the LAST step of the landing (§C.1 step 3e, the formal
    /// `LINGXI.md`) by putting a DIRECTORY where that file must be written.
    ///
    /// Chosen deliberately over corrupting an earlier step: it lets every
    /// preceding step SUCCEED, so the atomicity tests below prove the commit
    /// point held even when the landing got all the way to its final write —
    /// the interleaving a half-commit would actually survive. `LINGXI.md` is
    /// in `FIRST_SCAFFOLD_PRESERVED`, so the wipe leaves the directory alone.
    fn break_the_final_landing_step(root: &TempDir, app_id: &str) {
        let contract = workspace_of(root, app_id).join("LINGXI.md");
        let _ = fs::remove_file(&contract);
        fs::create_dir_all(contract.join("occupied")).expect("occupy the contract path");
    }

    fn repair_the_final_landing_step(root: &TempDir, app_id: &str) {
        let contract = workspace_of(root, app_id).join("LINGXI.md");
        fs::remove_dir_all(&contract).expect("free the contract path");
    }

    fn break_index_commit(root: &TempDir) -> Vec<u8> {
        let index = root.path().join(local_apps::storage::index_rel());
        let original = fs::read(&index).expect("read index before injected failure");
        fs::remove_file(&index).expect("remove index before injected failure");
        fs::create_dir_all(&index).expect("occupy index path");
        original
    }

    fn repair_index_commit(root: &TempDir, original: &[u8]) {
        let index = root.path().join(local_apps::storage::index_rel());
        fs::remove_dir_all(&index).expect("free index path");
        fs::write(index, original).expect("restore index after injected failure");
    }

    /// The opening turn of the interview must ask for a DESCRIPTION in ordinary
    /// text, not present a picker.
    ///
    /// `AskUserQuestion` renders a native option sheet. On the opening turn the
    /// model knows only that the user wants an app, so every option it could
    /// offer is a guess at the user's own idea — and the sheet then collects a
    /// choice among those guesses INSTEAD of the free-text description that
    /// steps 2 and 3 both read. The user reported exactly this: the flow opened
    /// by making them choose.
    ///
    /// Nothing else in this file would catch a regression. Step 1 reverting to
    /// "用 `AskUserQuestion` 问用户想做什么" compiles, keeps `尚未定形态`, and
    /// leaves every other contract test green, because they assert on the
    /// header and on the formal contract that REPLACES this text. So this test
    /// pins the two halves that carry the behaviour: that step 1 names ordinary
    /// text, and that it names the tool only to forbid it there.
    #[tokio::test]
    async fn the_interview_opens_with_a_description_prompt_not_a_picker() {
        let (root, service, broker) = create_broker(false, None).await;
        let shell = shell_app_fixture(&broker, &service).await;
        let guided = fs::read_to_string(workspace_of(&root, &shell.id).join("LINGXI.md"))
            .expect("read the guided contract");

        assert!(
            guided.contains("**用普通对话文本**问用户想做什么"),
            "step 1 must ask for a description in ordinary text: {guided}"
        );
        assert!(
            guided.contains("这一轮**不要用 `AskUserQuestion`**"),
            "step 1 must forbid the picker on the opening turn: {guided}"
        );
        // The generic "问需求用 `AskUserQuestion`" line used to sit above the
        // step list and contradicted step 1 outright. A model reading both
        // resolves the contradiction back to the picker, so the unqualified
        // form must not reappear.
        assert!(
            !guided.contains("问需求用 `AskUserQuestion`"),
            "the unqualified rule contradicts step 1 and must stay removed: {guided}"
        );
        // The tool is still the right instrument once there are options to pick
        // between — the name/surface confirmation. Forbidding it everywhere
        // would be the opposite mistake.
        assert!(
            guided.contains("用 `AskUserQuestion` 把提议的**名称**与**形态**交给用户确认或修改"),
            "the name/surface confirmation still belongs in a picker: {guided}"
        );
        assert!(
            guided.contains("1-3 个聚焦问题"),
            "a clarification round must contain one to three questions: {guided}"
        );
        assert!(
            guided.contains("没有未决事项就省略这一轮"),
            "no unresolved decisions must omit the clarification round: {guided}"
        );
    }

    #[tokio::test]
    async fn scaffold_requires_a_runtime_profile_receipt() {
        let (root, service, broker) = create_broker(false, None).await;
        let shell = shell_app_fixture(&broker, &service).await;

        let error = broker
            .scaffold_shell_app_value(scaffold_input(&shell.id, "A", "b", "dom"))
            .await
            .expect_err("scaffold must fail closed without a native-confirmed receipt");
        assert!(error.contains("runtime_profile_receipt"), "{error}");
        assert!(!service.record(&shell.id).await.expect("record").scaffolded);
        assert!(
            fs::read_to_string(workspace_of(&root, &shell.id).join("LINGXI.md"))
                .expect("guided contract")
                .contains("尚未定形态")
        );
    }

    #[tokio::test]
    async fn invalid_workflow_model_releases_the_runtime_profile_receipt_claim() {
        let (_root, service, broker) = create_broker(false, None).await;
        let shell = shell_app_fixture(&broker, &service).await;
        let binding = crate::local_app_runtime_profiles::current_binding_for_family(
            local_apps::AppRuntimeProfile::ReactDom,
        )
        .expect("published dom profile");
        let receipt = broker
            .issue_runtime_profile_receipt(&shell.id, binding)
            .await
            .expect("issue receipt");

        let error = broker
            .scaffold_shell_app_value(json!({
                "app_id": shell.id,
                "name": "bad workflow model",
                "brief": "b",
                "runtime_profile_receipt": receipt.receipt_id,
                "workflow_model": 123,
            }))
            .await
            .expect_err("invalid workflow_model must fail before scaffold");
        assert!(error.contains("workflow_model must be a string"), "{error}");

        let replacement = broker
            .issue_runtime_profile_receipt(
                &shell.id,
                crate::local_app_runtime_profiles::current_binding_for_family(
                    local_apps::AppRuntimeProfile::ReactDom,
                )
                .expect("published dom profile"),
            )
            .await
            .expect("claim must have been released");
        assert_ne!(replacement.receipt_id, receipt.receipt_id);
    }

    #[tokio::test]
    async fn runtime_profile_receipts_enforce_claim_supersede_cross_app_and_ttl() {
        let (_root, service, broker) = create_broker(false, None).await;
        let first = shell_app_fixture(&broker, &service).await;
        let second = shell_app_fixture(&broker, &service).await;
        let binding = crate::local_app_runtime_profiles::current_binding_for_family(
            local_apps::AppRuntimeProfile::ReactDom,
        )
        .expect("published dom profile");

        let original = broker
            .issue_runtime_profile_receipt(&first.id, binding.clone())
            .await
            .expect("issue receipt");
        assert_eq!(
            broker
                .claim_runtime_profile_receipt(&first.id, &original.receipt_id)
                .await
                .expect("claim")
                .family,
            binding.family
        );
        let in_use = broker
            .issue_runtime_profile_receipt(&first.id, binding.clone())
            .await
            .expect_err("claimed receipt must block supersede");
        assert!(in_use.contains("already in use"), "{in_use}");
        let cross_app = broker
            .claim_runtime_profile_receipt(&second.id, &original.receipt_id)
            .await
            .expect_err("receipt must be app-scoped");
        assert!(cross_app.contains(&second.id), "{cross_app}");

        broker
            .release_runtime_profile_receipt_claim(&first.id, &original.receipt_id)
            .await;
        let replacement = broker
            .issue_runtime_profile_receipt(&first.id, binding)
            .await
            .expect("issue replacement");
        let stale = broker
            .claim_runtime_profile_receipt(&first.id, &original.receipt_id)
            .await
            .expect_err("superseded receipt must not claim");
        assert!(stale.contains("stale or superseded"), "{stale}");
        broker
            .consume_runtime_profile_receipt(&first.id, &replacement.receipt_id)
            .await;
        let consumed = broker
            .claim_runtime_profile_receipt(&first.id, &replacement.receipt_id)
            .await
            .expect_err("consumed receipt must not replay");
        assert!(
            consumed.contains("missing or was already consumed"),
            "{consumed}"
        );

        let expired = broker
            .issue_runtime_profile_receipt(
                &first.id,
                crate::local_app_runtime_profiles::current_binding_for_family(
                    local_apps::AppRuntimeProfile::ReactDom,
                )
                .expect("published dom profile"),
            )
            .await
            .expect("issue expiring receipt");
        broker
            .pending_runtime_profile_receipts
            .lock()
            .await
            .get_mut(&first.id)
            .expect("stored receipt")
            .expires_at_ms = now_ms().saturating_sub(1);
        let expired_error = broker
            .claim_runtime_profile_receipt(&first.id, &expired.receipt_id)
            .await
            .expect_err("expired receipt must fail");
        assert!(expired_error.contains("expired"), "{expired_error}");
    }

    #[tokio::test]
    async fn runtime_profiles_report_availability_cache_download_and_migrations() {
        let (_root, _service, broker) = create_broker(false, None).await;
        let profiles = broker
            .runtime_profiles_value(json!({}))
            .await
            .expect("runtime profiles")["profiles"]
            .as_array()
            .expect("profiles array")
            .clone();
        let react = profiles
            .iter()
            .find(|entry| entry["family"] == "react_dom")
            .expect("react_dom profile");
        assert_eq!(react["cache_status"], "download_required");
        assert_eq!(react["download_status"], "download_required");
        assert_eq!(react["available_migrations"], json!([]));

        for family in ["three_3d", "phaser_2d"] {
            let entry = profiles
                .iter()
                .find(|entry| entry["family"] == family)
                .unwrap_or_else(|| panic!("{family} profile"));
            assert_eq!(entry["cache_status"], "download_required");
            assert_eq!(
                entry["download_status"], "download_required",
                "source bundle availability must not claim an installed dependency tree"
            );
        }

        let babylon = profiles
            .iter()
            .find(|entry| entry["family"] == "babylon_3d")
            .expect("babylon profile");
        assert_eq!(babylon["available"], false);
        assert_eq!(babylon["cache_status"], "unavailable");
        assert_eq!(babylon["download_status"], "gated");
        assert_eq!(babylon["available_migrations"], json!([]));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn runtime_profile_dependency_status_distinguishes_seed_and_shared_cache() {
        let root = TempDir::new().expect("tempdir");
        let binding = crate::local_app_runtime_profiles::current_binding_for_family(
            AppRuntimeProfile::ReactDom,
        )
        .expect("react profile binding");
        let contract = crate::local_app_runtime_profiles::contract_for_binding(&binding)
            .expect("react profile contract");
        let lock_digest = crate::local_app_runtime_profiles::lockfile_sha256(contract);

        // A configured seed with the exact lock is bundled, even though its
        // dependency tree has not been copied into the shared cache.
        let runtime_root = create_bundled_seed(root.path(), &lock_digest);
        let broker = LocalAppsHostBroker::new(
            root.path().to_path_buf(),
            Arc::new(NoopClientEventSink),
            None,
            false,
            Some(runtime_root.clone()),
        );
        assert_eq!(
            broker.runtime_profile_dependency_availability(
                AppRuntimeProfile::ReactDom,
                binding.revision,
            ),
            RuntimeProfileDependencyAvailability::Bundled,
        );

        // Once the exact lock is represented by a verified shared snapshot,
        // the provenance changes to cached. The selector must not keep
        // claiming that it will use the device bundle.
        let snapshot = broker.dependency_snapshot_root(&lock_digest);
        assert!(LocalAppsHostBroker::adopt_bundled_dependency_seed(
            &runtime_root,
            &lock_digest,
            &snapshot,
        )
        .expect("adopt matching seed"));
        assert_eq!(
            broker.runtime_profile_dependency_availability(
                AppRuntimeProfile::ReactDom,
                binding.revision,
            ),
            RuntimeProfileDependencyAvailability::Cached,
        );
    }

    #[tokio::test]
    async fn native_runtime_profile_selection_is_authoritative_over_the_recommendation() {
        let root = TempDir::new().expect("tempdir");
        let service = test_service(&root).await;
        let sink = MockSink::arc();
        let broker =
            LocalAppsHostBroker::new(root.path().to_path_buf(), sink.clone(), None, false, None);
        assert!(broker.attach_service(service.clone()).is_ok());
        let shell = shell_app_fixture(&broker, &service).await;

        let resolver = {
            let sink = sink.clone();
            let broker = broker.clone();
            tokio::spawn(async move {
                loop {
                    for event in sink.events().await {
                        if let ClientEvent::AppEvent {
                            event: AppEventDto::AppRuntimeProfileSelectionRequested { request },
                        } = event
                        {
                            assert_eq!(
                                request.recommended_family,
                                Some(AppRuntimeProfileDto::Babylon3d)
                            );
                            assert_eq!(request.options.len(), 5);
                            let babylon = request
                                .options
                                .iter()
                                .find(|option| option.family == AppRuntimeProfileDto::Babylon3d)
                                .expect("Babylon catalog option");
                            assert!(!babylon.available);
                            assert_eq!(babylon.download_status, "gated");
                            assert!(
                                broker
                                    .resolve_runtime_profile_selection(
                                        &request.request_id,
                                        Some(AppRuntimeProfileDto::Canvas2d),
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

        let selected = broker
            .confirm_runtime_profile_value(json!({
                "app_id": shell.id,
                "recommended_profile": "babylon_3d",
            }))
            .await
            .expect("native selection returns a receipt");
        resolver.await.expect("selection resolver");
        assert_eq!(selected["runtime_profile"]["family"], "canvas_2d");
        assert_ne!(selected["runtime_profile"]["family"], "babylon_3d");
    }

    #[tokio::test]
    async fn a_failed_scaffold_releases_the_receipt_claim_for_retry() {
        let runtime = MockMobileLinuxRuntime::new(Duration::ZERO);
        let (root, service, broker) = create_broker(false, Some(runtime.clone())).await;
        let shell = shell_app_fixture(&broker, &service).await;
        let original_manifest = load_manifest(
            &AppLayout::new(root.path().to_path_buf(), shell.id.clone()).expect("layout"),
        )
        .expect("shell manifest");
        let original_dependency = service
            .dependency_record(&shell.id)
            .await
            .expect("shell dependency record");
        let binding = crate::local_app_runtime_profiles::current_binding_for_family(
            local_apps::AppRuntimeProfile::Canvas2d,
        )
        .expect("published canvas profile");
        let receipt = broker
            .issue_runtime_profile_receipt(&shell.id, binding)
            .await
            .expect("issue receipt");
        break_the_final_landing_step(&root, &shell.id);

        let first = broker
            .scaffold_shell_app_value(json!({
                "app_id": shell.id,
                "name": "打飞机",
                "brief": "b",
                "runtime_profile_receipt": receipt.receipt_id,
            }))
            .await
            .expect_err("broken landing must fail");
        assert!(first.contains("LINGXI.md"), "{first}");
        let layout = AppLayout::new(root.path().to_path_buf(), shell.id.clone()).expect("layout");
        assert_eq!(
            load_manifest(&layout).expect("restored manifest"),
            original_manifest,
            "landing failure must restore the shell manifest"
        );
        assert_eq!(
            service
                .dependency_record(&shell.id)
                .await
                .expect("restored dependency record"),
            original_dependency,
            "landing failure must restore the service dependency cache"
        );
        assert!(
            !root
                .path()
                .join(local_apps::storage::scaffold_recovery_journal_rel(
                    &shell.id
                ))
                .exists(),
            "a synchronous rollback must remove its recovery journal"
        );
        assert!(
            workspace_of(&root, &shell.id).join("LINGXI.md").is_dir(),
            "the exact shell workspace must be restored, including the injected failure fixture"
        );
        repair_the_final_landing_step(&root, &shell.id);

        broker
            .scaffold_shell_app_value(json!({
                "app_id": shell.id,
                "name": "打飞机",
                "brief": "b",
                "runtime_profile_receipt": receipt.receipt_id,
            }))
            .await
            .expect("same receipt can retry after the claim is released");
        assert!(service.record(&shell.id).await.expect("record").scaffolded);
    }

    #[tokio::test]
    async fn scaffold_failure_after_dependency_snapshot_restores_shell_before_record_commit() {
        let runtime = MockMobileLinuxRuntime::new(Duration::ZERO);
        let (root, service, broker) = create_broker(false, Some(runtime.clone())).await;
        let shell = shell_app_fixture(&broker, &service).await;
        let layout = AppLayout::new(root.path().to_path_buf(), shell.id.clone()).expect("layout");
        let original_manifest = load_manifest(&layout).expect("shell manifest");
        let original_dependency = service
            .dependency_record(&shell.id)
            .await
            .expect("shell dependency record");
        let original_guided = fs::read(workspace_of(&root, &shell.id).join("LINGXI.md"))
            .expect("guided workspace contract");
        let original_index = break_index_commit(&root);
        let receipt = broker
            .issue_runtime_profile_receipt(
                &shell.id,
                crate::local_app_runtime_profiles::current_binding_for_family(
                    local_apps::AppRuntimeProfile::ReactDom,
                )
                .expect("published DOM profile"),
            )
            .await
            .expect("issue receipt");

        let error = broker
            .scaffold_shell_app_value(json!({
                "app_id": shell.id,
                "name": "回滚测试",
                "brief": "dependency snapshot then commit failure",
                "runtime_profile_receipt": receipt.receipt_id,
            }))
            .await
            .expect_err("the occupied index must fail after dependency snapshot");
        assert!(error.contains("index.json"), "{error}");
        repair_index_commit(&root, &original_index);

        let after = service.record(&shell.id).await.expect("shell record");
        assert!(
            !after.scaffolded,
            "record.scaffolded is the final commit point"
        );
        assert_eq!(
            load_manifest(&layout).expect("restored manifest"),
            original_manifest
        );
        assert_eq!(
            service
                .dependency_record(&shell.id)
                .await
                .expect("restored dependency record"),
            original_dependency
        );
        assert_eq!(
            fs::read(workspace_of(&root, &shell.id).join("LINGXI.md"))
                .expect("restored guided contract"),
            original_guided
        );
        assert!(
            !root
                .path()
                .join(local_apps::storage::scaffold_recovery_journal_rel(
                    &shell.id
                ))
                .exists(),
            "rollback must remove the durable journal after restoring the shell"
        );

        // The receipt claim is released and the repaired shell can retry from
        // the exact pre-landing state.
        broker
            .scaffold_shell_app_value(json!({
                "app_id": shell.id,
                "name": "回滚测试",
                "brief": "dependency snapshot then commit failure",
                "runtime_profile_receipt": receipt.receipt_id,
            }))
            .await
            .expect("same receipt retries after rollback");
        assert!(service.record(&shell.id).await.expect("record").scaffolded);
    }

    #[tokio::test]
    async fn cold_start_recovers_a_partial_scaffold_before_loading_the_app() {
        let root = TempDir::new().expect("tempdir");
        let service = test_service(&root).await;
        let broker = LocalAppsHostBroker::new(
            root.path().to_path_buf(),
            Arc::new(NoopClientEventSink),
            None,
            false,
            None,
        );
        assert!(broker.attach_service(service.clone()).is_ok());
        let shell = shell_app_fixture(&broker, &service).await;
        let layout = AppLayout::new(root.path().to_path_buf(), shell.id.clone()).expect("layout");
        let original_manifest = load_manifest(&layout).expect("shell manifest");
        let original_guided =
            fs::read(workspace_of(&root, &shell.id).join("LINGXI.md")).expect("guided contract");
        let binding = crate::local_app_runtime_profiles::current_binding_for_family(
            local_apps::AppRuntimeProfile::Canvas2d,
        )
        .expect("published canvas profile");
        let artifacts = scaffold_runtime_profile(Some(binding), local_apps::AppSurface::Canvas)
            .expect("scaffold artifacts");
        let target =
            crate::local_apps_build::LocalAppBuildTarget::from_runtime_binding(&artifacts.binding)
                .expect("build target");
        let build_lock =
            local_apps::storage::lock_app_build(root.path(), &shell.id).expect("build lock");
        let recovery = local_apps::storage::begin_scaffold_recovery(
            root.path(),
            &shell.id,
            "冷启动回滚",
            "crash recovery",
        )
        .expect("durable recovery journal");
        stamp_scaffold_identity(&layout, "冷启动回滚", &artifacts).expect("stamp partial identity");
        crate::local_apps_build::scaffold_workspace_initialized(&layout, target, true)
            .expect("land partial workspace");
        persist_runtime_profile_files(&workspace_of(&root, &shell.id), &artifacts)
            .expect("persist partial runtime files");
        // Simulate process death: neither commit nor rollback runs.
        std::mem::forget(recovery);
        drop(build_lock);
        drop(broker);
        drop(service);

        let loaded = local_apps::storage::load_all(root.path()).expect("cold-start recovery");
        assert_eq!(loaded.len(), 1);
        assert!(!loaded[0].record.scaffolded);
        assert_eq!(
            load_manifest(&layout).expect("restored manifest"),
            original_manifest
        );
        assert_eq!(
            fs::read(workspace_of(&root, &shell.id).join("LINGXI.md"))
                .expect("restored guided contract"),
            original_guided
        );
        assert!(
            !root
                .path()
                .join(local_apps::storage::scaffold_recovery_journal_rel(
                    &shell.id
                ))
                .exists(),
            "cold-start recovery must consume the journal"
        );
    }

    #[tokio::test]
    async fn runtime_profile_apps_fail_closed_on_dependency_input_drift() {
        let runtime = MockMobileLinuxRuntime::new(Duration::ZERO);
        let (root, service, broker) = create_broker(false, Some(runtime)).await;
        let shell = shell_app_fixture(&broker, &service).await;
        broker
            .scaffold_shell_app_value(
                confirmed_scaffold_input(&broker, &shell.id, "漂移测试", "b", "dom").await,
            )
            .await
            .expect("scaffold");
        fs::write(
            workspace_of(&root, &shell.id).join("package.json"),
            "{\n  \"name\": \"tampered\"\n}\n",
        )
        .expect("tamper package.json");

        let error = broker
            .ensure_dependency_install(&shell.id, false)
            .await
            .expect_err("runtime-profile apps must not auto-repair dependency drift");
        assert!(error.contains("dependencies_dirty"), "{error}");
        assert!(error.contains("LocalAppConfirmDependencyChange"), "{error}");
    }

    #[tokio::test]
    async fn remove_only_dependency_change_skips_native_confirmation() {
        let runtime = MockMobileLinuxRuntime::new(Duration::ZERO);
        let (root, service, broker) = create_broker(false, Some(runtime.clone())).await;
        let shell = shell_app_fixture(&broker, &service).await;
        broker
            .scaffold_shell_app_value(
                confirmed_scaffold_input(&broker, &shell.id, "移除依赖", "b", "dom").await,
            )
            .await
            .expect("scaffold");
        let layout = AppLayout::new(root.path().to_path_buf(), shell.id.clone()).expect("layout");
        let workspace = root.path().join(layout.workspace_rel());
        let binding = load_manifest(&layout)
            .expect("manifest")
            .runtime_profile
            .expect("runtime profile");
        let contract = crate::local_app_runtime_profiles::contract_for_binding(&binding)
            .expect("runtime contract");
        let mut requested = LocalAppsHostBroker::load_requested_dependency_map(&workspace)
            .expect("requested dependencies");
        requested.insert("dayjs".into(), "1.11.13".into());
        let dependency_record = service
            .dependency_record(&shell.id)
            .await
            .expect("dependency record");
        let receipt = broker
            .issue_dependency_change_receipt(
                &shell.id,
                dependency_baseline_for(&layout, &dependency_record),
                LocalAppsHostBroker::serialize_requested_dependency_map(&requested)
                    .expect("requested json"),
                LocalAppsHostBroker::build_effective_package_json(contract, &requested)
                    .expect("effective package"),
                vec![DependencyChange {
                    kind: DependencyChangeKind::Add,
                    package: "dayjs".into(),
                    version: Some("1.11.13".into()),
                }],
            )
            .await
            .expect("issue dependency receipt");
        broker
            .update_dependencies(json!({
                "app_id": shell.id,
                "receipt_id": receipt.receipt_id,
            }))
            .await
            .expect("seed committed dependency baseline");
        let request_start = runtime.isolated_requests().await.len();

        let result = broker
            .confirm_dependency_change(json!({
                "app_id": shell.id,
                "changes": [{"kind": "remove", "package": "dayjs"}],
            }))
            .await
            .expect("remove-only confirmation should not require native approval");
        assert_eq!(result["ok"], true);
        assert!(result["receipt"]["id"].as_str().is_some(), "{result}");
        broker
            .update_dependencies(json!({
                "app_id": shell.id,
                "receipt_id": result["receipt"]["id"].as_str().expect("receipt id"),
            }))
            .await
            .expect("remove-only dependency update");
        let requests = runtime.isolated_requests().await;
        let dependency_requests: Vec<_> = requests[request_start..]
            .iter()
            .filter(|request| request.command == "/usr/bin/pnpm")
            .collect();
        assert_eq!(dependency_requests.len(), 2, "{dependency_requests:?}");
        assert!(
            dependency_requests
                .iter()
                .all(|request| request.network == NetworkPolicy::Disabled),
            "remove-only updates must not perform a networked dependency resolution: {dependency_requests:?}"
        );
    }

    #[tokio::test]
    async fn dependency_change_confirmation_fails_closed_on_tampered_requested_baseline() {
        let runtime = MockMobileLinuxRuntime::new(Duration::ZERO);
        let (root, service, broker) = create_broker(false, Some(runtime)).await;
        let shell = shell_app_fixture(&broker, &service).await;
        broker
            .scaffold_shell_app_value(
                confirmed_scaffold_input(&broker, &shell.id, "依赖篡改", "b", "dom").await,
            )
            .await
            .expect("scaffold");
        let workspace = workspace_of(&root, &shell.id);
        fs::write(
            workspace.join(crate::local_app_runtime_profiles::REQUESTED_FILE_REL),
            "{\n  \"dependencies\": {\n    \"dayjs\": \"1.11.13\"\n  }\n}\n",
        )
        .expect("tamper requested dependency baseline");

        let error = broker
            .confirm_dependency_change(json!({
                "app_id": shell.id,
                "changes": [{"kind": "add", "package": "nanoid", "version": "5.1.6"}],
            }))
            .await
            .expect_err("tampered requested baseline must fail closed");
        assert!(error.contains("dependencies_dirty"), "{error}");
        assert!(
            error.contains("outside the host-managed dependency flow"),
            "{error}"
        );
    }

    #[tokio::test]
    async fn dependency_add_uses_dedicated_native_confirmation_before_receipt() {
        let runtime = MockMobileLinuxRuntime::new(Duration::ZERO);
        let root = TempDir::new().expect("tempdir");
        let service = test_service(&root).await;
        let sink = MockSink::arc();
        let broker = LocalAppsHostBroker::new(
            root.path().to_path_buf(),
            sink.clone(),
            Some(runtime.clone()),
            false,
            None,
        );
        assert!(broker.attach_service(service.clone()).is_ok());
        let shell = shell_app_fixture(&broker, &service).await;
        broker
            .scaffold_shell_app_value(
                confirmed_scaffold_input(&broker, &shell.id, "确认依赖", "b", "dom").await,
            )
            .await
            .expect("scaffold");
        let requests_before_confirmation = runtime.isolated_requests().await.len();

        let request = tokio::spawn({
            let broker = broker.clone();
            let app_id = shell.id.clone();
            async move {
                broker
                    .confirm_dependency_change(json!({
                        "app_id": app_id,
                        "changes": [{
                            "kind": "add",
                            "package": "dayjs",
                            "version": "1.11.13"
                        }]
                    }))
                    .await
            }
        });

        let confirmation = timeout(Duration::from_secs(2), async {
            loop {
                if let Some(request) = sink.events().await.into_iter().find_map(|event| {
                    if let ClientEvent::AppEvent {
                        event: AppEventDto::AppDependencyChangeConfirmationRequested { request },
                    } = event
                    {
                        Some(request)
                    } else {
                        None
                    }
                }) {
                    break request;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("dedicated confirmation event");
        assert_eq!(confirmation.app_id, shell.id);
        assert_eq!(confirmation.changes.len(), 1);
        assert_eq!(confirmation.changes[0].package, "dayjs");
        assert_eq!(
            confirmation.changes[0].cache_status,
            "unknown_until_resolution"
        );
        assert_eq!(confirmation.changes[0].download_status, "may_be_required");
        assert_eq!(confirmation.license_risk, "unknown_until_resolution");
        assert_eq!(confirmation.sbom_risk, "unknown_until_resolution");
        assert!(confirmation.lifecycle_scripts_blocked);
        assert!(confirmation.native_addons_blocked);
        assert_eq!(
            confirmation.rollback_policy,
            "rollback_on_validation_failure"
        );
        assert_eq!(
            runtime.isolated_requests().await.len(),
            requests_before_confirmation,
            "confirmation must not install or resolve dependencies before approval"
        );
        assert!(
            broker
                .pending_dependency_change_receipts
                .lock()
                .await
                .get(&shell.id)
                .is_none(),
            "a receipt must not exist before approval"
        );

        assert!(
            broker
                .resolve_dependency_change_confirmation(&confirmation.request_id, true)
                .await
        );
        let result = request
            .await
            .expect("confirmation task")
            .expect("approved dependency change");
        assert!(result["receipt"]["id"].as_str().is_some(), "{result}");
    }

    #[tokio::test]
    async fn dependency_add_denial_does_not_issue_receipt() {
        let runtime = MockMobileLinuxRuntime::new(Duration::ZERO);
        let root = TempDir::new().expect("tempdir");
        let service = test_service(&root).await;
        let sink = MockSink::arc();
        let broker = LocalAppsHostBroker::new(
            root.path().to_path_buf(),
            sink.clone(),
            Some(runtime),
            false,
            None,
        );
        assert!(broker.attach_service(service.clone()).is_ok());
        let shell = shell_app_fixture(&broker, &service).await;
        broker
            .scaffold_shell_app_value(
                confirmed_scaffold_input(&broker, &shell.id, "拒绝依赖", "b", "dom").await,
            )
            .await
            .expect("scaffold");

        let request = tokio::spawn({
            let broker = broker.clone();
            let app_id = shell.id.clone();
            async move {
                broker
                    .confirm_dependency_change(json!({
                        "app_id": app_id,
                        "changes": [{
                            "kind": "update",
                            "package": "dayjs",
                            "version": "1.11.14"
                        }]
                    }))
                    .await
            }
        });
        let request_id = timeout(Duration::from_secs(2), async {
            loop {
                if let Some(request_id) = sink.events().await.into_iter().find_map(|event| {
                    if let ClientEvent::AppEvent {
                        event: AppEventDto::AppDependencyChangeConfirmationRequested { request },
                    } = event
                    {
                        Some(request.request_id)
                    } else {
                        None
                    }
                }) {
                    break request_id;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("dedicated confirmation event");
        assert!(
            broker
                .resolve_dependency_change_confirmation(&request_id, false)
                .await
        );
        let error = request
            .await
            .expect("confirmation task")
            .expect_err("denial must fail closed");
        assert!(error.contains("denied"), "{error}");
        assert!(
            broker
                .pending_dependency_change_receipts
                .lock()
                .await
                .get(&shell.id)
                .is_none(),
            "a denied change must not issue a receipt"
        );
    }

    #[tokio::test]
    async fn dependency_update_resolves_then_verifies_with_frozen_network_denied_install() {
        let runtime = MockMobileLinuxRuntime::new(Duration::ZERO);
        let (root, service, broker) = create_broker(false, Some(runtime.clone())).await;
        let shell = shell_app_fixture(&broker, &service).await;
        broker
            .scaffold_shell_app_value(
                confirmed_scaffold_input(&broker, &shell.id, "依赖两阶段", "b", "dom").await,
            )
            .await
            .expect("scaffold");
        let request_start = runtime.isolated_requests().await.len();

        let layout = AppLayout::new(root.path().to_path_buf(), shell.id.clone()).expect("layout");
        let workspace = root.path().join(layout.workspace_rel());
        let binding = load_manifest(&layout)
            .expect("manifest")
            .runtime_profile
            .expect("runtime profile");
        let contract = crate::local_app_runtime_profiles::contract_for_binding(&binding)
            .expect("runtime contract");
        let mut requested = LocalAppsHostBroker::load_requested_dependency_map(&workspace)
            .expect("requested dependency map");
        requested.insert("dayjs".into(), "1.11.13".into());
        let dependency_record = service
            .dependency_record(&shell.id)
            .await
            .expect("dependency record");
        let receipt = broker
            .issue_dependency_change_receipt(
                &shell.id,
                dependency_baseline_for(&layout, &dependency_record),
                LocalAppsHostBroker::serialize_requested_dependency_map(&requested)
                    .expect("requested json"),
                LocalAppsHostBroker::build_effective_package_json(contract, &requested)
                    .expect("effective package"),
                vec![DependencyChange {
                    kind: DependencyChangeKind::Add,
                    package: "dayjs".into(),
                    version: Some("1.11.13".into()),
                }],
            )
            .await
            .expect("issue dependency receipt");

        broker
            .update_dependencies(json!({
                "app_id": shell.id,
                "receipt_id": receipt.receipt_id,
            }))
            .await
            .expect("dependency update");
        assert!(
            !LocalAppsHostBroker::dependency_update_recovery_path(&layout).exists(),
            "successful dependency update must remove its committed recovery journal"
        );

        let requests = runtime.isolated_requests().await;
        let dependency_requests: Vec<_> = requests[request_start..]
            .iter()
            .filter(|request| request.command == "/usr/bin/pnpm")
            .collect();
        assert_eq!(dependency_requests.len(), 2, "{dependency_requests:?}");
        let resolution = dependency_requests[0];
        assert_eq!(resolution.network, NetworkPolicy::Allowed);
        assert!(
            resolution
                .args
                .iter()
                .any(|arg| arg == "--no-frozen-lockfile"),
            "resolution must be allowed to produce a new lockfile: {resolution:?}"
        );
        assert!(
            !resolution.args.iter().any(|arg| arg == "--lockfile-only"),
            "networked resolution must preheat the store with a full install: {resolution:?}"
        );
        assert!(!resolution.args.iter().any(|arg| arg == "--frozen-lockfile"));
        assert!(resolution.args.iter().any(|arg| arg == "--ignore-scripts"));
        assert!(resolution.args.iter().any(|arg| arg == "--no-runtime"));

        let frozen = dependency_requests[1];
        assert_eq!(frozen.network, NetworkPolicy::Disabled);
        for flag in ["--frozen-lockfile", "--ignore-scripts", "--no-runtime"] {
            assert!(
                frozen.args.iter().any(|arg| arg == flag),
                "frozen verification must include {flag}: {frozen:?}"
            );
        }
        assert!(!frozen.args.iter().any(|arg| arg == "--no-frozen-lockfile"));
        assert!(!frozen.args.iter().any(|arg| arg == "--lockfile-only"));

        assert!(
            !workspace
                .join(".lingxi-build-state/dependency-staging/node_modules")
                .exists(),
            "successful dependency update must clean its staging tree after publication"
        );
    }

    #[tokio::test]
    async fn stale_dependency_receipt_cannot_overwrite_a_newer_committed_baseline() {
        let runtime = MockMobileLinuxRuntime::new(Duration::ZERO);
        let (root, service, broker) = create_broker(false, Some(runtime)).await;
        let shell = shell_app_fixture(&broker, &service).await;
        broker
            .scaffold_shell_app_value(
                confirmed_scaffold_input(&broker, &shell.id, "依赖过期回归", "b", "dom").await,
            )
            .await
            .expect("scaffold");

        let layout = AppLayout::new(root.path().to_path_buf(), shell.id.clone()).expect("layout");
        let workspace = root.path().join(layout.workspace_rel());
        let binding = load_manifest(&layout)
            .expect("manifest")
            .runtime_profile
            .expect("runtime profile");
        let contract = crate::local_app_runtime_profiles::contract_for_binding(&binding)
            .expect("runtime contract");
        let baseline_a_record = service
            .dependency_record(&shell.id)
            .await
            .expect("baseline A dependency record");
        let baseline_a = dependency_baseline_for(&layout, &baseline_a_record);
        let mut requested = LocalAppsHostBroker::load_requested_dependency_map(&workspace)
            .expect("requested dependency map");
        requested.insert("dayjs".into(), "1.11.13".into());
        let requested_json = LocalAppsHostBroker::serialize_requested_dependency_map(&requested)
            .expect("requested json");
        let effective_package_json =
            LocalAppsHostBroker::build_effective_package_json(contract, &requested)
                .expect("effective package");
        let fresh_receipt = broker
            .issue_dependency_change_receipt(
                &shell.id,
                baseline_a.clone(),
                requested_json.clone(),
                effective_package_json.clone(),
                vec![DependencyChange {
                    kind: DependencyChangeKind::Add,
                    package: "dayjs".into(),
                    version: Some("1.11.13".into()),
                }],
            )
            .await
            .expect("fresh receipt");
        broker
            .update_dependencies(json!({
                "app_id": shell.id,
                "receipt_id": fresh_receipt.receipt_id,
            }))
            .await
            .expect("commit newer dependency baseline");

        let stale_receipt_id = "dependency-change-stale".to_string();
        broker
            .pending_dependency_change_receipts
            .lock()
            .await
            .insert(
                shell.id.clone(),
                PendingDependencyChangeReceipt {
                    receipt_id: stale_receipt_id.clone(),
                    app_id: shell.id.clone(),
                    baseline: baseline_a,
                    requested_json,
                    effective_package_json,
                    issued_at_ms: now_ms(),
                    expires_at_ms: now_ms() + RUNTIME_PROFILE_RECEIPT_TTL.as_millis() as u64,
                    summary: vec![DependencyChange {
                        kind: DependencyChangeKind::Add,
                        package: "dayjs".into(),
                        version: Some("1.11.13".into()),
                    }],
                    claimed: false,
                },
            );

        let error = broker
            .update_dependencies(json!({
                "app_id": shell.id,
                "receipt_id": stale_receipt_id,
            }))
            .await
            .expect_err("stale receipt must fail closed");
        assert!(error.contains("reconfirm before applying"), "{error}");
        assert!(
            broker
                .pending_dependency_change_receipts
                .lock()
                .await
                .get(&shell.id)
                .is_none(),
            "stale receipt must be consumed after rejection"
        );
        assert_eq!(
            LocalAppsHostBroker::load_requested_dependency_map(&workspace)
                .expect("current requested dependency map")
                .get("dayjs")
                .map(String::as_str),
            Some("1.11.13"),
            "the newer committed baseline must remain authoritative"
        );
    }

    #[tokio::test]
    async fn frozen_dependency_install_failure_rolls_back_and_releases_receipt_claim() {
        let runtime = MockMobileLinuxRuntime::new(Duration::ZERO);
        let (root, service, broker) = create_broker(false, Some(runtime.clone())).await;
        let shell = shell_app_fixture(&broker, &service).await;
        broker
            .scaffold_shell_app_value(
                confirmed_scaffold_input(&broker, &shell.id, "依赖冻结失败", "b", "dom").await,
            )
            .await
            .expect("scaffold");
        let layout = AppLayout::new(root.path().to_path_buf(), shell.id.clone()).expect("layout");
        let workspace = root.path().join(layout.workspace_rel());
        let previous_package = fs::read(workspace.join("package.json")).expect("package.json");
        let previous_dependency = service
            .dependency_record(&shell.id)
            .await
            .expect("dependency record");
        let binding = load_manifest(&layout)
            .expect("manifest")
            .runtime_profile
            .expect("runtime profile");
        let contract = crate::local_app_runtime_profiles::contract_for_binding(&binding)
            .expect("runtime contract");
        let mut requested = LocalAppsHostBroker::load_requested_dependency_map(&workspace)
            .expect("requested dependency map");
        requested.insert("dayjs".into(), "1.11.13".into());
        let receipt = broker
            .issue_dependency_change_receipt(
                &shell.id,
                dependency_baseline_for(&layout, &previous_dependency),
                LocalAppsHostBroker::serialize_requested_dependency_map(&requested)
                    .expect("requested json"),
                LocalAppsHostBroker::build_effective_package_json(contract, &requested)
                    .expect("effective package"),
                vec![DependencyChange {
                    kind: DependencyChangeKind::Add,
                    package: "dayjs".into(),
                    version: Some("1.11.13".into()),
                }],
            )
            .await
            .expect("issue dependency receipt");
        let receipt_id = receipt.receipt_id;

        runtime.set_fail_frozen_install(true);
        let error = broker
            .update_dependencies(json!({
                "app_id": shell.id,
                "receipt_id": receipt_id,
            }))
            .await
            .expect_err("frozen install failure must abort the update");
        assert!(
            error.contains("synthetic frozen install failure"),
            "{error}"
        );
        assert_eq!(
            fs::read(workspace.join("package.json")).expect("restored package"),
            previous_package
        );
        assert!(
            workspace.join("node_modules/vite/bin/vite.js").is_file(),
            "failed verification must leave the previous dependency tree"
        );
        let current_dependency = service
            .dependency_record(&shell.id)
            .await
            .expect("current dependency record");
        assert_eq!(current_dependency.state, AppDependencyState::Ready);
        assert_eq!(
            current_dependency.lockfile_sha256,
            previous_dependency.lockfile_sha256
        );

        runtime.set_fail_frozen_install(false);
        let retried = broker
            .update_dependencies(json!({
                "app_id": shell.id,
                "receipt_id": receipt_id,
            }))
            .await
            .expect("failed frozen install releases the receipt claim");
        assert_eq!(retried["ok"], true);
    }

    #[tokio::test]
    async fn dependency_update_rejects_lifecycle_scripts_before_snapshot_and_releases_receipt() {
        let runtime = MockMobileLinuxRuntime::new(Duration::ZERO);
        let (root, service, broker) = create_broker(false, Some(runtime.clone())).await;
        let shell = shell_app_fixture(&broker, &service).await;
        broker
            .scaffold_shell_app_value(
                confirmed_scaffold_input(&broker, &shell.id, "依赖脚本", "b", "dom").await,
            )
            .await
            .expect("scaffold");
        let layout = AppLayout::new(root.path().to_path_buf(), shell.id.clone()).expect("layout");
        let workspace = root.path().join(layout.workspace_rel());
        let previous_package = fs::read(workspace.join("package.json")).expect("package.json");
        let previous_dependency = service
            .dependency_record(&shell.id)
            .await
            .expect("dependency record");
        let binding = load_manifest(&layout)
            .expect("manifest")
            .runtime_profile
            .expect("runtime profile");
        let contract = crate::local_app_runtime_profiles::contract_for_binding(&binding)
            .expect("runtime contract");
        let mut requested = LocalAppsHostBroker::load_requested_dependency_map(&workspace)
            .expect("requested dependency map");
        requested.insert("dayjs".into(), "1.11.13".into());
        let receipt = broker
            .issue_dependency_change_receipt(
                &shell.id,
                dependency_baseline_for(&layout, &previous_dependency),
                LocalAppsHostBroker::serialize_requested_dependency_map(&requested)
                    .expect("requested json"),
                LocalAppsHostBroker::build_effective_package_json(contract, &requested)
                    .expect("effective package"),
                vec![DependencyChange {
                    kind: DependencyChangeKind::Add,
                    package: "dayjs".into(),
                    version: Some("1.11.13".into()),
                }],
            )
            .await
            .expect("issue dependency receipt");
        let receipt_id = receipt.receipt_id;

        runtime.set_inject_lifecycle_script(true);
        let error = broker
            .update_dependencies(json!({
                "app_id": shell.id,
                "receipt_id": receipt_id,
            }))
            .await
            .expect_err("lifecycle script must abort the update before snapshot publication");
        assert!(error.contains("react"), "{error}");
        assert!(error.contains("install"), "{error}");
        assert_eq!(
            fs::read(workspace.join("package.json")).expect("restored package"),
            previous_package
        );
        assert!(workspace.join("node_modules/vite/bin/vite.js").is_file());
        let current_dependency = service
            .dependency_record(&shell.id)
            .await
            .expect("current dependency record");
        assert_eq!(current_dependency.state, AppDependencyState::Ready);
        assert_eq!(
            current_dependency.lockfile_sha256,
            previous_dependency.lockfile_sha256
        );

        runtime.set_inject_lifecycle_script(false);
        let retried = broker
            .update_dependencies(json!({
                "app_id": shell.id,
                "receipt_id": receipt_id,
            }))
            .await
            .expect("lifecycle-script rejection releases the receipt claim");
        assert_eq!(retried["ok"], true);
    }

    #[tokio::test]
    async fn dependency_update_rolls_back_authoritative_files_when_finalize_fails() {
        let runtime = MockMobileLinuxRuntime::new(Duration::ZERO);
        let (root, service, broker) = create_broker(false, Some(runtime.clone())).await;
        let shell = shell_app_fixture(&broker, &service).await;
        broker
            .scaffold_shell_app_value(
                confirmed_scaffold_input(&broker, &shell.id, "依赖回滚", "b", "dom").await,
            )
            .await
            .expect("scaffold");
        runtime.set_omit_staged_vite_marker(true);

        let layout = AppLayout::new(root.path().to_path_buf(), shell.id.clone()).expect("layout");
        let workspace = root.path().join(layout.workspace_rel());
        let previous_package = fs::read(workspace.join("package.json")).expect("package.json");
        let previous_requested =
            fs::read(workspace.join(crate::local_app_runtime_profiles::REQUESTED_FILE_REL))
                .expect("requested.json");
        let previous_dependency = service
            .dependency_record(&shell.id)
            .await
            .expect("dependency record");

        let binding = load_manifest(&layout)
            .expect("manifest")
            .runtime_profile
            .expect("runtime profile");
        let contract = crate::local_app_runtime_profiles::contract_for_binding(&binding)
            .expect("runtime contract");
        let mut requested = LocalAppsHostBroker::load_requested_dependency_map(&workspace)
            .expect("requested dependency map");
        requested.insert("dayjs".into(), "1.11.13".into());
        let requested_json = LocalAppsHostBroker::serialize_requested_dependency_map(&requested)
            .expect("requested json");
        let effective_package_json =
            LocalAppsHostBroker::build_effective_package_json(contract, &requested)
                .expect("effective package");
        let receipt = broker
            .issue_dependency_change_receipt(
                &shell.id,
                dependency_baseline_for(&layout, &previous_dependency),
                requested_json,
                effective_package_json,
                vec![DependencyChange {
                    kind: DependencyChangeKind::Add,
                    package: "dayjs".into(),
                    version: Some("1.11.13".into()),
                }],
            )
            .await
            .expect("issue dependency receipt");

        let error = broker
            .update_dependencies(json!({
                "app_id": shell.id,
                "receipt_id": receipt.receipt_id,
            }))
            .await
            .expect_err("missing staged vite marker must fail finalize");
        assert!(error.contains("staged Vite executable"), "{error}");
        assert_eq!(
            fs::read(workspace.join("package.json")).expect("restored package"),
            previous_package
        );
        assert_eq!(
            fs::read(workspace.join(crate::local_app_runtime_profiles::REQUESTED_FILE_REL))
                .expect("restored requested"),
            previous_requested
        );
        assert!(
            workspace.join("node_modules/vite/bin/vite.js").is_file(),
            "previous dependency tree must be restored"
        );
        let current_dependency = service
            .dependency_record(&shell.id)
            .await
            .expect("current dependency record");
        assert_eq!(current_dependency.state, AppDependencyState::Ready);
        assert_eq!(
            current_dependency.lockfile_sha256,
            previous_dependency.lockfile_sha256
        );
        assert_eq!(
            current_dependency.toolchain_key,
            previous_dependency.toolchain_key
        );
    }

    #[tokio::test]
    async fn dependency_update_builds_before_consuming_and_restores_the_old_build_on_failure() {
        let runtime = MockMobileLinuxRuntime::new(Duration::ZERO);
        let (root, service, broker) = create_broker(false, Some(runtime.clone())).await;
        let shell = shell_app_fixture(&broker, &service).await;
        broker
            .scaffold_shell_app_value(
                confirmed_scaffold_input(&broker, &shell.id, "依赖构建", "b", "dom").await,
            )
            .await
            .expect("scaffold");
        let layout = AppLayout::new(root.path().to_path_buf(), shell.id.clone()).expect("layout");
        let builder = crate::local_apps_build::LocalAppBuilder {
            mobile_linux: broker.mobile_linux(),
            host: &broker,
        };
        builder
            .build_workspace(&layout)
            .await
            .expect("initial production build");
        let built_index = layout
            .root()
            .join(layout.build_rel(false))
            .join(crate::local_apps_build::VITE_OUTPUT_DIR)
            .join("index.html");
        let old_index = fs::read(&built_index).expect("old build output");

        let workspace = root.path().join(layout.workspace_rel());
        let binding = load_manifest(&layout)
            .expect("manifest")
            .runtime_profile
            .expect("runtime profile");
        let contract = crate::local_app_runtime_profiles::contract_for_binding(&binding)
            .expect("runtime contract");
        let mut requested = LocalAppsHostBroker::load_requested_dependency_map(&workspace)
            .expect("requested dependency map");
        requested.insert("dayjs".into(), "1.11.13".into());
        let previous_dependency = service
            .dependency_record(&shell.id)
            .await
            .expect("dependency record");
        let receipt = broker
            .issue_dependency_change_receipt(
                &shell.id,
                dependency_baseline_for(&layout, &previous_dependency),
                LocalAppsHostBroker::serialize_requested_dependency_map(&requested)
                    .expect("requested json"),
                LocalAppsHostBroker::build_effective_package_json(contract, &requested)
                    .expect("effective package"),
                vec![DependencyChange {
                    kind: DependencyChangeKind::Add,
                    package: "dayjs".into(),
                    version: Some("1.11.13".into()),
                }],
            )
            .await
            .expect("issue dependency receipt");

        runtime.set_fail_build(true);
        let error = broker
            .update_dependencies(json!({
                "app_id": shell.id,
                "receipt_id": receipt.receipt_id,
            }))
            .await
            .expect_err("production build failure must roll back the dependency update");
        assert!(error.contains("production build failed"), "{error}");
        assert_eq!(
            fs::read(&built_index).expect("restored old build"),
            old_index
        );
        crate::local_apps_build::validate_build_for_launch(&layout)
            .expect("restored build receipt remains launchable");

        runtime.set_fail_build(false);
        let retried = broker
            .update_dependencies(json!({
                "app_id": shell.id,
                "receipt_id": receipt.receipt_id,
            }))
            .await
            .expect("failed build releases the receipt claim for retry");
        assert_eq!(retried["ok"], true);
        crate::local_apps_build::validate_build_for_launch(&layout)
            .expect("successful dependency update writes a launchable build receipt");
        let replay = broker
            .update_dependencies(json!({
                "app_id": shell.id,
                "receipt_id": receipt.receipt_id,
            }))
            .await
            .expect_err("successful build consumes the receipt");
        assert!(
            replay.contains("missing or was already consumed"),
            "{replay}"
        );
    }

    #[tokio::test]
    async fn dependency_update_cold_start_recovers_an_in_progress_journal() {
        let runtime = MockMobileLinuxRuntime::new(Duration::ZERO);
        let (root, service, broker) = create_broker(false, Some(runtime.clone())).await;
        let shell = shell_app_fixture(&broker, &service).await;
        broker
            .scaffold_shell_app_value(
                confirmed_scaffold_input(&broker, &shell.id, "依赖冷启动", "b", "dom").await,
            )
            .await
            .expect("scaffold");
        let layout = AppLayout::new(root.path().to_path_buf(), shell.id.clone()).expect("layout");
        let builder = crate::local_apps_build::LocalAppBuilder {
            mobile_linux: broker.mobile_linux(),
            host: &broker,
        };
        builder
            .build_workspace(&layout)
            .await
            .expect("initial production build");
        let workspace = root.path().join(layout.workspace_rel());
        let old_manifest = load_manifest(&layout).expect("old manifest");
        let old_package = fs::read(workspace.join("package.json")).expect("old package");
        let old_tree_digest =
            dependency_tree_digest(&workspace.join("node_modules")).expect("old dependency tree");
        let build_index = root
            .path()
            .join(layout.build_rel(false))
            .join(crate::local_apps_build::VITE_OUTPUT_DIR)
            .join("index.html");
        let old_build_index = fs::read(&build_index).expect("old build output");
        let old_dependency = service
            .dependency_record(&shell.id)
            .await
            .expect("old dependency record");
        let rollback = broker
            .capture_dependency_update_rollback(&layout, old_dependency.clone())
            .expect("capture durable rollback");
        let journal = LocalAppsHostBroker::dependency_update_recovery_journal(
            &layout,
            &rollback,
            DependencyUpdateRecoveryStatus::InProgress,
        )
        .expect("build recovery journal");
        LocalAppsHostBroker::write_dependency_update_recovery_journal(&layout, &journal)
            .expect("write recovery journal");

        // Simulate process death after new workspace files, dependency tree,
        // manifest, build and dependency state were partially published.
        fs::write(workspace.join("package.json"), b"{\"name\":\"new\"}\n")
            .expect("write partial package");
        let mut new_manifest = old_manifest.clone();
        new_manifest.name = "partial-new".into();
        new_manifest.revision += 1;
        local_apps::save_manifest(&layout, &new_manifest).expect("write partial manifest");
        LocalAppsHostBroker::remove_owned_path(&workspace.join("node_modules"))
            .expect("remove old tree");
        fs::create_dir_all(workspace.join("node_modules/vite/bin")).expect("new tree");
        fs::write(
            workspace.join("node_modules/vite/bin/vite.js"),
            b"partial-new",
        )
        .expect("new tree marker");
        fs::write(&build_index, b"partial-new-build").expect("partial build");
        fs::create_dir_all(
            workspace
                .join(".lingxi-build-state")
                .join("dependency-staging"),
        )
        .expect("partial staging");
        service
            .start_dependency_install(&shell.id)
            .await
            .expect("mark dependency update in progress");
        drop(rollback);
        drop(broker);
        drop(service);

        // The broker constructor runs recovery before AppService::load, so the
        // service observes the same exact old dependency record as disk.
        let restarted = LocalAppsHostBroker::new(
            root.path().to_path_buf(),
            Arc::new(NoopClientEventSink),
            Some(runtime),
            false,
            None,
        );
        let restarted_service = test_service(&root).await;
        assert!(restarted.attach_service(restarted_service.clone()).is_ok());
        assert_eq!(
            load_manifest(&layout).expect("restored manifest"),
            old_manifest
        );
        assert_eq!(
            fs::read(workspace.join("package.json")).expect("restored package"),
            old_package
        );
        assert_eq!(
            dependency_tree_digest(&workspace.join("node_modules")).expect("restored tree"),
            old_tree_digest
        );
        assert_eq!(
            fs::read(build_index).expect("restored build"),
            old_build_index
        );
        assert_eq!(
            restarted_service
                .dependency_record(&shell.id)
                .await
                .expect("restored dependency record"),
            old_dependency
        );
        assert!(
            !LocalAppsHostBroker::dependency_update_recovery_path(&layout).exists(),
            "boot recovery must consume the dependency journal"
        );
        assert!(
            !workspace
                .join(".lingxi-build-state/dependency-staging")
                .exists(),
            "boot recovery must remove interrupted staging"
        );
    }

    #[tokio::test]
    async fn dependency_update_cold_start_cleans_a_committed_journal_without_rollback() {
        let runtime = MockMobileLinuxRuntime::new(Duration::ZERO);
        let (root, service, broker) = create_broker(false, Some(runtime.clone())).await;
        let shell = shell_app_fixture(&broker, &service).await;
        broker
            .scaffold_shell_app_value(
                confirmed_scaffold_input(&broker, &shell.id, "依赖提交恢复", "b", "dom").await,
            )
            .await
            .expect("scaffold");
        let layout = AppLayout::new(root.path().to_path_buf(), shell.id.clone()).expect("layout");
        let builder = crate::local_apps_build::LocalAppBuilder {
            mobile_linux: broker.mobile_linux(),
            host: &broker,
        };
        builder
            .build_workspace(&layout)
            .await
            .expect("initial production build");
        let workspace = root.path().join(layout.workspace_rel());
        let mut new_manifest = load_manifest(&layout).expect("manifest");
        new_manifest.name = "committed-new".into();
        new_manifest.revision += 1;
        let old_dependency = service
            .dependency_record(&shell.id)
            .await
            .expect("old dependency record");
        let rollback = broker
            .capture_dependency_update_rollback(&layout, old_dependency)
            .expect("capture durable rollback");
        let node_modules_backup = rollback
            .node_modules_backup
            .clone()
            .expect("fixture has dependency tree");
        let build_backup = rollback
            .build_backup
            .clone()
            .expect("fixture has production build");
        let journal = LocalAppsHostBroker::dependency_update_recovery_journal(
            &layout,
            &rollback,
            DependencyUpdateRecoveryStatus::Committed,
        )
        .expect("build committed recovery journal");
        LocalAppsHostBroker::write_dependency_update_recovery_journal(&layout, &journal)
            .expect("write committed recovery journal");
        local_apps::save_manifest(&layout, &new_manifest).expect("write committed manifest");
        fs::write(
            workspace.join("package.json"),
            b"{\"name\":\"committed-new\"}\n",
        )
        .expect("write committed package");
        LocalAppsHostBroker::remove_owned_path(&workspace.join("node_modules"))
            .expect("remove old tree");
        fs::create_dir_all(workspace.join("node_modules/vite/bin")).expect("new tree");
        fs::write(
            workspace.join("node_modules/vite/bin/vite.js"),
            b"committed-new",
        )
        .expect("new tree marker");
        let build_index = root
            .path()
            .join(layout.build_rel(false))
            .join(crate::local_apps_build::VITE_OUTPUT_DIR)
            .join("index.html");
        fs::write(&build_index, b"committed-new-build").expect("committed build");
        fs::create_dir_all(
            workspace
                .join(".lingxi-build-state")
                .join("dependency-staging"),
        )
        .expect("committed staging");
        service
            .start_dependency_install(&shell.id)
            .await
            .expect("mark committed dependency state");
        drop(rollback);
        drop(broker);
        drop(service);

        let restarted = LocalAppsHostBroker::new(
            root.path().to_path_buf(),
            Arc::new(NoopClientEventSink),
            Some(runtime),
            false,
            None,
        );
        let restarted_service = test_service(&root).await;
        assert!(restarted.attach_service(restarted_service.clone()).is_ok());
        assert_eq!(
            load_manifest(&layout).expect("committed manifest"),
            new_manifest
        );
        assert_eq!(
            fs::read(workspace.join("package.json")).expect("committed package"),
            b"{\"name\":\"committed-new\"}\n"
        );
        assert_eq!(
            fs::read(workspace.join("node_modules/vite/bin/vite.js")).expect("committed tree"),
            b"committed-new"
        );
        assert_eq!(
            fs::read(build_index).expect("committed build"),
            b"committed-new-build"
        );
        assert_eq!(
            restarted_service
                .dependency_record(&shell.id)
                .await
                .expect("committed dependency record")
                .state,
            AppDependencyState::Installing
        );
        assert!(!node_modules_backup.exists());
        assert!(!build_backup.exists());
        assert!(!LocalAppsHostBroker::dependency_update_recovery_path(&layout).exists());
        assert!(!workspace
            .join(".lingxi-build-state/dependency-staging")
            .exists());
    }

    /// The whole point of the flow: what the user confirmed in the interview
    /// reaches BOTH the record and `LINGXI.md`.
    ///
    /// The contract assertions are not decoration. `workspace/LINGXI.md` is
    /// written exactly once and is the only channel that reaches the model on
    /// every turn; rendering it from the CREATION record instead of the
    /// proposed one writes `# Local App: untitled` with an empty brief and
    /// loses the entire interview, permanently, while every record assertion
    /// above still passes.
    #[tokio::test]
    async fn scaffold_commits_all_four_fields_and_writes_the_formal_contract() {
        let runtime = MockMobileLinuxRuntime::new(Duration::ZERO);
        let (root, service, broker) = create_broker(false, Some(runtime)).await;
        let shell = shell_app_fixture(&broker, &service).await;
        let guided = fs::read_to_string(workspace_of(&root, &shell.id).join("LINGXI.md"))
            .expect("read the guided contract");
        assert!(
            guided.contains("尚未定形态"),
            "the fixture must start on the guided contract: {guided}"
        );

        let mut input =
            confirmed_scaffold_input(&broker, &shell.id, "打飞机", "一个竖版射击小游戏", "canvas")
                .await;
        input["workflow_model"] = json!("anthropic/claude-opus-4");
        let value = broker
            .scaffold_shell_app_value(input)
            .await
            .expect("scaffold");

        let record = service.record(&shell.id).await.expect("record");
        assert!(record.scaffolded, "the commit point must have run");
        assert_eq!(record.name, "打飞机");
        assert_eq!(record.brief, "一个竖版射击小游戏");
        assert_eq!(
            record.workflow_model.as_deref(),
            Some("anthropic/claude-opus-4"),
            "the confirmed workflow model must be persisted by the same commit"
        );
        assert_eq!(
            value.get("app").and_then(|app| app.get("scaffolded")),
            Some(&json!(true)),
            "the tool result must echo the COMMITTED record: {value}"
        );
        let next_step = value
            .get("next_step")
            .and_then(Value::as_str)
            .expect("the result must carry a next step");
        assert!(
            next_step.contains("LINGXI.md"),
            "the agent must be sent back to the contract that just replaced              the guided one: {next_step}"
        );

        let contract = fs::read_to_string(workspace_of(&root, &shell.id).join("LINGXI.md"))
            .expect("read the formal contract");
        assert!(
            contract.contains("# Local App: 打飞机"),
            "must render the CONFIRMED name, not `untitled`: {contract}"
        );
        assert!(
            contract.contains("Brief: 一个竖版射击小游戏"),
            "must render the CONFIRMED brief: {contract}"
        );
        assert!(
            !contract.contains("尚未定形态"),
            "the guided contract must be overwritten, not appended to"
        );
        assert!(
            contract.contains("This app's surface is `canvas`"),
            "the contract must be the one for the CONFIRMED surface: {contract}"
        );
        for workflow in tool_workflow::BUILTIN_WORKFLOWS.local_app_build_workflow_names() {
            assert!(
                !contract.contains(workflow),
                "the contract must not name a build workflow: the host authorizes one and \
                 refuses any other, the model does not choose it — found `{workflow}` in \
                 {contract}"
            );
        }
        assert!(
            contract.contains("runtime profile `canvas_2d` revision `1`"),
            "the formal contract must mirror the persisted profile identity: {contract}"
        );
        assert!(
            contract.contains("informational mirror")
                && contract
                    .contains("persisted manifest binding and host catalog are authoritative"),
            "LINGXI.md must not become the runtime profile authority: {contract}"
        );
        assert!(
            contract.contains("lib/frame-loop.js") && !contract.contains("src/game/frame-loop.js"),
            "the managed Canvas frame helper must not be presented as editable: {contract}"
        );

        // The surface is on the manifest, and the seed is the canvas one.
        let layout = AppLayout::new(root.path().to_path_buf(), shell.id.clone()).expect("layout");
        let manifest = load_manifest(&layout).expect("manifest");
        assert_eq!(manifest.surface, Some(local_apps::AppSurface::Canvas));
        assert_eq!(manifest.name, "打飞机");
        assert!(
            manifest.dependency_snapshot.is_some(),
            "the commit point must not expose a scaffolded app without a verified dependency snapshot"
        );
        assert!(workspace_of(&root, &shell.id)
            .join("app/screens/game-screen.jsx")
            .is_file());
        assert_eq!(
            service
                .dependency_record(&shell.id)
                .await
                .expect("dependency record")
                .state,
            local_apps::AppDependencyState::Ready
        );
    }

    /// Phase -1 (P-1.4), §19.3: the Host contract carries no workflow/skill/
    /// agent names. `formal_workspace_contract` used to tell the model which
    /// build workflow to launch and which one NOT to launch; that authority
    /// is now the Host's alone — `LocalAppPluginBinding::resolve` computes the
    /// one workflow authorized for a build target and `enforce` refuses a
    /// caller-supplied mismatch by naming both ids in the error, so the model
    /// never needs (and must never be told) a workflow name to act correctly.
    /// This pins the absence for BOTH surfaces, not just the one the test
    /// above happens to exercise, so a name reintroduced on only one branch
    /// of `formal_workspace_contract`'s `match` still goes red — and over the
    /// next-step guidance family as well, which is model-visible tool-result
    /// prose that no test other than the component scanner covered.
    #[tokio::test]
    async fn lingxi_md_contract_prose_names_no_workflow() {
        // The needle set is derived from the PRODUCTION registry, never a
        // pair of names typed in here:
        // `tool_workflow::BUILTIN_WORKFLOWS.local_app_build_workflow_names()`
        // reads the same typed `is_local_app_build` field the component
        // scanner's needle derivation reads, and the scanner's module doc
        // forbids a second hand-typed copy of these names for exactly the
        // reason that applies here — add a third build workflow, name it in
        // this prose, and a hardcoded pair would sail past while only the
        // scanner (one allowlist entry away from being talked out of it)
        // fires.
        let workflows = tool_workflow::BUILTIN_WORKFLOWS.local_app_build_workflow_names();
        // An empty needle set would make every assertion below vacuously
        // true, which is the failure mode this whole test exists to prevent.
        assert!(
            workflows.len() >= 2,
            "the build-workflow registry must be non-trivially populated, or the absence \
             assertions below prove nothing: {workflows:?}"
        );

        for surface in ["dom", "canvas"] {
            let runtime = MockMobileLinuxRuntime::new(Duration::ZERO);
            let (root, service, broker) = create_broker(false, Some(runtime)).await;
            let shell = shell_app_fixture(&broker, &service).await;
            let input =
                confirmed_scaffold_input(&broker, &shell.id, "测试", "一个测试应用", surface).await;
            broker
                .scaffold_shell_app_value(input)
                .await
                .expect("scaffold");
            let contract = fs::read_to_string(workspace_of(&root, &shell.id).join("LINGXI.md"))
                .expect("read the formal contract");
            for workflow in &workflows {
                assert!(
                    !contract.contains(workflow),
                    "surface {surface}: the contract must name no build workflow — the host \
                     authorizes one and refuses any other, the model does not choose it — \
                     found `{workflow}` in {contract}"
                );
            }
        }

        // The contract file is not the only Host-authored prose the model
        // reads. The next-step guidance family is returned INSIDE the
        // `LocalAppCreate` / `LocalAppScaffold` tool results, so a workflow
        // name there reaches the model on exactly the turn it is deciding
        // what to do next — and it is otherwise guarded only by the component
        // scanner, whose documented ritual (change the constant, change the
        // allowlist in the same diff) is a sanctioned route back in.
        // Demonstrated by mutation: a build-workflow name planted in
        // `scaffold_next_step_guidance` left this test GREEN before this arm
        // existed, while only the scanner fired.
        for (generator, prose) in [
            ("scaffold_next_step_guidance", scaffold_next_step_guidance()),
            ("create_next_step_guidance", create_next_step_guidance()),
        ] {
            for workflow in &workflows {
                assert!(
                    !prose.contains(workflow),
                    "{generator} must name no build workflow — it is model-visible tool-result \
                     prose, and a name here reintroduces the model↔workflow-name coupling the \
                     Host-side resolve/enforce exists to remove — found `{workflow}` in {prose}"
                );
            }
        }
    }

    /// The branch's central guarantee, pinned at its PRODUCTION call site:
    /// not one byte written before the user confirmed reaches the real app.
    ///
    /// `land_scaffold` passes `first_scaffold = true` to
    /// `scaffold_workspace_initialized`. Everything else that covers the wipe
    /// calls that function DIRECTLY with `true`, which proves the mechanism
    /// works and proves nothing about the caller: flipping the production
    /// argument to `false` left the whole suite green while pre-confirmation
    /// source survived into the formed app. This test goes through
    /// `scaffold_shell_app_value`, so the argument itself is what it pins —
    /// verified by mutation (flip it to `false` and this test names
    /// `app/app.js`).
    #[tokio::test]
    async fn the_production_landing_wipes_what_the_interview_wrote() {
        // What an agent that ignored the guided contract leaves behind while
        // the interview is still running. `app/app.js` is the one that MATTERS
        // and the reason a per-path overwrite is not enough: Vite resolves
        // `.js` ahead of `.jsx`, so it out-resolves the seeded `app/app.jsx`
        // and the seed ships as dead code.
        const PRE_CONFIRMATION: &[&str] = &[
            "app/app.js",
            "app/screens/guessed-screen.jsx",
            "src/stores/premature-store.js",
            "notes.md",
        ];
        let runtime = MockMobileLinuxRuntime::new(Duration::ZERO);
        let (root, service, broker) = create_broker(false, Some(runtime)).await;
        let shell = shell_app_fixture(&broker, &service).await;
        let workspace = workspace_of(&root, &shell.id);

        for relative in PRE_CONFIRMATION {
            let path = workspace.join(relative);
            fs::create_dir_all(path.parent().expect("a parent")).expect("create parent");
            fs::write(&path, b"written before the user confirmed anything").expect("write");
        }
        // Host-owned state on the SAME tree, so a wipe that took too much
        // would be caught here rather than by a build minutes later.
        let installed = workspace.join("node_modules/.installed-marker");
        fs::create_dir_all(installed.parent().expect("a parent")).expect("create node_modules");
        fs::write(&installed, b"installed").expect("write");

        broker
            .scaffold_shell_app_value(
                confirmed_scaffold_input(&broker, &shell.id, "打飞机", "一个竖版射击小游戏", "dom")
                    .await,
            )
            .await
            .expect("scaffold");

        for relative in PRE_CONFIRMATION {
            assert!(
                !workspace.join(relative).exists(),
                "{relative} was written before the user confirmed anything and must not \
                 survive the landing"
            );
        }
        assert!(
            workspace.join("app/app.jsx").is_file(),
            "the seed must be what is on disk after the wipe"
        );
        assert!(
            workspace.join("node_modules/vite/bin/vite.js").is_file(),
            "the wipe may rebuild node_modules, but the committed app must finish with \
             host-managed dependencies installed"
        );
        let layout = AppLayout::new(root.path().to_path_buf(), shell.id.clone()).expect("layout");
        assert_eq!(
            load_manifest(&layout)
                .expect("the manifest must survive the wipe")
                .name,
            "打飞机",
            "`.lingxi/` holds the manifest the landing had already stamped"
        );
    }

    /// `workflow_model` is OPTIONAL, and omitting it must PRESERVE whatever
    /// the create carried rather than clearing it — a shell create can already
    /// name a model, and a scaffold that simply did not mention one must not
    /// drop the user's choice.
    #[tokio::test]
    async fn omitting_the_workflow_model_preserves_the_one_the_create_chose() {
        let runtime = MockMobileLinuxRuntime::new(Duration::ZERO);
        let (_root, service, broker) = create_broker(false, Some(runtime)).await;
        let record = service
            .create_app_with_git_and_workflow_model_and_initializer(
                None,
                "",
                None,
                false,
                Some("openai/gpt-5"),
                local_apps::CreateMode::Shell,
                None,
                |_| async { Ok(()) },
            )
            .await
            .expect("create shell app with a model");
        assert_eq!(record.workflow_model.as_deref(), Some("openai/gpt-5"));

        broker
            .scaffold_shell_app_value(
                confirmed_scaffold_input(&broker, &record.id, "A", "b", "dom").await,
            )
            .await
            .expect("scaffold");

        let after = service.record(&record.id).await.expect("record");
        assert_eq!(
            after.workflow_model.as_deref(),
            Some("openai/gpt-5"),
            "an omitted workflow_model must not clear the create-time choice"
        );
    }

    /// §C.1 step 4. A landing failure must leave the record EXACTLY as the
    /// create wrote it. The half-commit this forbids — a real name with
    /// `scaffolded == false` — is the worst of both states: the user sees a
    /// finished-looking app in the library that still opens the interview.
    #[tokio::test]
    async fn a_failed_landing_persists_none_of_the_four_fields() {
        let runtime = MockMobileLinuxRuntime::new(Duration::ZERO);
        let (root, service, broker) = create_broker(false, Some(runtime)).await;
        let shell = shell_app_fixture(&broker, &service).await;
        break_the_final_landing_step(&root, &shell.id);

        let mut input =
            confirmed_scaffold_input(&broker, &shell.id, "打飞机", "一个竖版射击小游戏", "canvas")
                .await;
        input["workflow_model"] = json!("anthropic/claude-opus-4");
        let error = broker
            .scaffold_shell_app_value(input)
            .await
            .expect_err("the landing must fail");
        assert!(
            error.contains("LINGXI.md"),
            "the failure must name the step that broke, not something else: {error}"
        );

        let after = service.record(&shell.id).await.expect("record");
        assert!(!after.scaffolded, "the commit point never ran");
        assert_eq!(
            after.name,
            local_apps::service::PLACEHOLDER_APP_NAME,
            "the name must NOT be half-committed"
        );
        assert_eq!(after.brief, "", "the brief must NOT be half-committed");
        assert_eq!(
            after.workflow_model, None,
            "the workflow model must NOT be half-committed"
        );
    }

    /// The reservation is IN-PROCESS and RAII. Had it been modelled on
    /// `set_init_session` — a set-once write to a PERSISTENT field — this
    /// retry would be refused forever and the draft would be bricked.
    #[tokio::test]
    async fn the_reservation_is_released_on_the_failure_path_so_a_retry_can_land() {
        let runtime = MockMobileLinuxRuntime::new(Duration::ZERO);
        let (root, service, broker) = create_broker(false, Some(runtime)).await;
        let shell = shell_app_fixture(&broker, &service).await;
        break_the_final_landing_step(&root, &shell.id);
        let input = confirmed_scaffold_input(&broker, &shell.id, "打飞机", "b", "canvas").await;
        broker
            .scaffold_shell_app_value(input.clone())
            .await
            .expect_err("the first attempt must fail");
        repair_the_final_landing_step(&root, &shell.id);

        broker
            .scaffold_shell_app_value(input)
            .await
            .expect("the retry must land");

        let after = service.record(&shell.id).await.expect("record");
        assert!(after.scaffolded);
        assert_eq!(after.name, "打飞机");
    }

    /// §C.1 step 1. Two scaffolds of the same app: exactly one wins, and the
    /// loser is refused BY THE RESERVATION — asserted on the stable
    /// `scaffold_in_flight` prefix so the test cannot pass because the second
    /// call failed for some unrelated reason.
    #[tokio::test]
    async fn two_concurrent_scaffolds_reject_the_second_at_the_in_process_reservation() {
        let runtime = MockMobileLinuxRuntime::new(Duration::ZERO);
        let (_root, service, broker) = create_broker(false, Some(runtime)).await;
        let shell = shell_app_fixture(&broker, &service).await;
        let input = confirmed_scaffold_input(&broker, &shell.id, "A", "b", "dom").await;
        let (first, second) = tokio::join!(
            broker.scaffold_shell_app_value(input.clone()),
            broker.scaffold_shell_app_value(input),
        );
        assert!(
            first.is_ok() ^ second.is_ok(),
            "exactly one must win: {first:?} / {second:?}"
        );
        let refusal = first.err().or(second.err()).expect("one must be refused");
        assert!(
            refusal.contains("scaffold_in_flight"),
            "the loser must be stopped by the reservation, not by anything else: {refusal}"
        );
    }

    /// An OUTCOME test for scaffold-versus-delete: whichever wins, the delete
    /// completes, no record survives, and no directory is left behind.
    ///
    /// ⚠️ Honest about what it does NOT prove: it does not discriminate on
    /// `lock_app_build`. Removing the lock entirely leaves this test green,
    /// because on a wall-clock race the delete finishes the whole trash
    /// removal before the landing's first write and `wipe_editable_surface`
    /// then refuses a workspace that is not there. The orphan the lock
    /// prevents needs the delete to land INSIDE the seed loop, a window no
    /// timing-based test can be made to hit reliably. What actually pins the
    /// lock is
    /// [`the_landing_takes_the_build_lock_first_and_hands_it_back_held`].
    #[tokio::test]
    async fn a_concurrent_delete_cannot_orphan_a_scaffold_in_flight() {
        let runtime = MockMobileLinuxRuntime::new(Duration::ZERO);
        let (root, service, broker) = create_broker(false, Some(runtime)).await;
        let shell = shell_app_fixture(&broker, &service).await;
        let app_dir = root.path().join("apps").join(&shell.id);
        assert!(app_dir.is_dir(), "the fixture must exist to be raced");
        let input = confirmed_scaffold_input(&broker, &shell.id, "A", "b", "dom").await;

        let (scaffolded, deleted) = tokio::join!(
            broker.scaffold_shell_app_value(input),
            service.delete_app(&shell.id),
        );
        deleted.expect("the delete must complete");
        assert!(
            service.record(&shell.id).await.is_err(),
            "a completed delete must leave no record, however the scaffold ended: {scaffolded:?}"
        );
        assert!(
            !app_dir.exists(),
            "no orphan workspace may survive the delete (scaffold outcome: {scaffolded:?})"
        );
    }

    /// §C.1 step 3a, and the discriminating test for it: the landing takes
    /// `lock_app_build` BEFORE it touches anything, and the guard it hands
    /// back is still held — which is what keeps a concurrent delete out of
    /// both the seed loop and the window before the commit point.
    ///
    /// A landing that took no lock would sail past a contender that already
    /// holds it, and the first timeout below would not fire.
    #[tokio::test]
    async fn the_landing_takes_the_build_lock_first_and_hands_it_back_held() {
        let (root, service, broker) = create_broker(false, None).await;
        let shell = shell_app_fixture(&broker, &service).await;
        let mut proposed = shell.clone();
        proposed.name = "A".into();
        proposed.brief = "b".into();
        let binding = crate::local_app_runtime_profiles::current_binding_for_family(
            local_apps::AppRuntimeProfile::ReactDom,
        )
        .expect("published react-dom runtime profile");

        let contend = |root: PathBuf, app_id: String| {
            tokio::task::spawn_blocking(move || local_apps::storage::lock_app_build(&root, &app_id))
        };
        let contender = contend(root.path().to_path_buf(), shell.id.clone())
            .await
            .expect("join the contender")
            .expect("the contender must get the lock first");

        let landing = tokio::spawn({
            let broker = Arc::clone(&broker);
            let binding = binding.clone();
            async move {
                broker
                    .land_scaffold(&proposed, local_apps::AppSurface::Dom, Some(binding))
                    .await
            }
        });
        let mut landing = landing;
        assert!(
            timeout(Duration::from_millis(400), &mut landing)
                .await
                .is_err(),
            "the landing must WAIT for the build lock before it writes anything"
        );
        assert!(
            !workspace_of(&root, &shell.id).join("app/app.jsx").exists(),
            "and it must not have seeded while it was waiting"
        );

        drop(contender);
        let held = timeout(Duration::from_secs(30), landing)
            .await
            .expect("the landing must proceed once the lock is free")
            .expect("join the landing")
            .expect("the landing must succeed");
        assert!(workspace_of(&root, &shell.id).join("app/app.jsx").is_file());

        let (held_build, held_recovery, recovery) = held;

        assert!(
            timeout(
                Duration::from_millis(400),
                contend(root.path().to_path_buf(), shell.id.clone()),
            )
            .await
            .is_err(),
            "the landing must STILL hold the lock when it returns, so the \
             commit point runs under it"
        );
        recovery
            .rollback()
            .expect("discard the uncommitted landing");
        drop(held_build);
        drop(held_recovery);
        timeout(
            Duration::from_secs(30),
            contend(root.path().to_path_buf(), shell.id.clone()),
        )
        .await
        .expect("the lock must become free once the landing's guard drops")
        .expect("join the contender")
        .expect("acquire the freed build lock");
    }

    /// A formed app is refused. Its workspace holds the user's own source and
    /// a second landing WIPES the editable surface before seeding, so this
    /// refusal is what stands between a stray tool call and the user's work.
    #[tokio::test]
    async fn scaffolding_a_formed_app_is_rejected() {
        let runtime = MockMobileLinuxRuntime::new(Duration::ZERO);
        let (root, service, broker) = create_broker(false, Some(runtime)).await;
        let shell = shell_app_fixture(&broker, &service).await;
        broker
            .scaffold_shell_app_value(
                confirmed_scaffold_input(&broker, &shell.id, "A", "b", "dom").await,
            )
            .await
            .expect("the first scaffold must land");
        let workspace = workspace_of(&root, &shell.id);
        fs::write(
            workspace.join("app/screens/mine.jsx"),
            b"// the user's own work",
        )
        .expect("write user source");

        let error = broker
            .scaffold_shell_app_value(
                confirmed_scaffold_input(&broker, &shell.id, "B", "b", "dom").await,
            )
            .await
            .expect_err("the second scaffold must be rejected");
        assert!(error.contains("already"), "got {error}");

        assert!(
            workspace.join("app/screens/mine.jsx").is_file(),
            "the refusal must have happened BEFORE the wipe"
        );
        let after = service.record(&shell.id).await.expect("record");
        assert_eq!(after.name, "A", "the committed name must be untouched");
        let contract = fs::read_to_string(workspace.join("LINGXI.md")).expect("read the contract");
        assert!(
            contract.contains("# Local App: A"),
            "the one-and-only contract must be untouched: {contract}"
        );
    }

    /// §C.1 step 2, and it must refuse BEFORE touching the workspace: an
    /// invalid argument may not cost the user the interview's workspace.
    #[tokio::test]
    async fn scaffold_rejects_an_empty_brief_and_an_unknown_surface() {
        let (root, service, broker) = create_broker(false, None).await;
        let shell = shell_app_fixture(&broker, &service).await;

        for (name, brief, expected) in [
            ("A", "   ", "brief must be a non-empty string"),
            ("   ", "b", "name must be a non-empty string"),
        ] {
            let error = broker
                .scaffold_shell_app_value(
                    confirmed_scaffold_input(&broker, &shell.id, name, brief, "dom").await,
                )
                .await
                .expect_err("must be rejected");
            assert!(
                error.contains(expected),
                "expected {expected:?} in {error:?}"
            );
        }
        let mut surface_override =
            confirmed_scaffold_input(&broker, &shell.id, "A", "b", "dom").await;
        surface_override["surface"] = json!("webgl");
        let error = broker
            .scaffold_shell_app_value(surface_override)
            .await
            .expect_err("surface overrides must be rejected");
        assert!(
            error.contains("runtime_profile_receipt is authoritative"),
            "got {error}"
        );
        let over_long = "x".repeat(local_apps::service::MAX_NAME_BYTES + 1);
        let error = broker
            .scaffold_shell_app_value(
                confirmed_scaffold_input(&broker, &shell.id, &over_long, "b", "dom").await,
            )
            .await
            .expect_err("an over-long name must be rejected");
        assert!(error.contains("limit"), "got {error}");

        let after = service.record(&shell.id).await.expect("record");
        assert!(!after.scaffolded);
        let contract = fs::read_to_string(workspace_of(&root, &shell.id).join("LINGXI.md"))
            .expect("read the contract");
        assert!(
            contract.contains("尚未定形态"),
            "a rejected argument must not have touched the workspace: {contract}"
        );
    }

    /// §C.1.4. `AppManifest::hash()` serialises the WHOLE struct INCLUDING
    /// `name`, and `AppDataStore::ensure_manifest` compares it against the
    /// SQLite `_lingxi_schema.manifest_hash`. Writing `name` after a store
    /// exists breaks EVERY data read and write with "database manifest
    /// mismatch" — which is why the first landing is the only write window,
    /// and why renaming an app is not offered at all.
    #[tokio::test]
    async fn the_manifest_name_may_only_be_written_before_any_database_exists() {
        let runtime = MockMobileLinuxRuntime::new(Duration::ZERO);
        let (root, service, broker) = create_broker(false, Some(runtime)).await;
        let shell = shell_app_fixture(&broker, &service).await;
        let layout = AppLayout::new(root.path().to_path_buf(), shell.id.clone()).expect("layout");
        assert!(
            !layout.database_path().exists(),
            "the write window is exactly 'no data store yet'"
        );

        broker
            .scaffold_shell_app_value(
                confirmed_scaffold_input(&broker, &shell.id, "A", "b", "dom").await,
            )
            .await
            .expect("the first landing must be allowed to write the name");
        assert_eq!(load_manifest(&layout).expect("manifest").name, "A");

        let opened = layout.clone();
        tokio::task::spawn_blocking(move || AppDataStore::open(opened).map(|_| ()))
            .await
            .expect("join data store worker")
            .expect("open the app data store");
        assert!(layout.database_path().exists(), "the store must be on disk");

        let artifacts = scaffold_runtime_profile(
            Some(
                crate::local_app_runtime_profiles::current_binding_for_family(
                    local_apps::AppRuntimeProfile::ReactDom,
                )
                .expect("published react-dom runtime profile"),
            ),
            local_apps::AppSurface::Dom,
        )
        .expect("react-dom profile");
        let error = stamp_scaffold_identity(&layout, "B", &artifacts)
            .expect_err("a name rewrite after the store exists must be refused");
        assert!(error.contains("database"), "got {error}");
        assert_eq!(
            load_manifest(&layout).expect("manifest").name,
            "A",
            "the refusal must have happened before the write"
        );
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
        seed_launchable_runtime_fixture(root.path(), &app.record, name);
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
    fn dependency_snapshot_rejects_native_node_addons() {
        let root = TempDir::new().expect("tempdir");
        let source = root.path().join("node_modules/pkg");
        fs::create_dir_all(&source).expect("source tree");
        fs::write(source.join("binding.node"), b"native").expect("native addon");
        let error = validate_dependency_tree(root.path().join("node_modules").as_path())
            .expect_err("native addons must be rejected");
        assert!(error.contains("native Node addon"), "{error}");
    }

    #[cfg(unix)]
    #[test]
    fn dependency_snapshot_rejects_native_node_addon_symlink_paths() {
        let root = TempDir::new().expect("tempdir");
        let source = root.path().join("node_modules/pkg");
        fs::create_dir_all(&source).expect("source tree");
        fs::write(source.join("payload.bin"), b"payload").expect("payload");
        std::os::unix::fs::symlink("payload.bin", source.join("binding.node"))
            .expect("native addon symlink");

        let error = validate_dependency_tree(root.path().join("node_modules").as_path())
            .expect_err("native addon symlink path must be rejected");
        assert!(error.contains("native Node addon symlink"), "{error}");
    }

    #[cfg(unix)]
    #[test]
    fn dependency_snapshot_rejects_symlinks_that_resolve_to_native_addons() {
        let root = TempDir::new().expect("tempdir");
        let pkg = root.path().join("node_modules/pkg");
        let bin = root.path().join("node_modules/.bin");
        fs::create_dir_all(&pkg).expect("package tree");
        fs::create_dir_all(&bin).expect("bin tree");
        fs::write(pkg.join("binding.node"), b"native").expect("native addon");
        std::os::unix::fs::symlink("../pkg/binding.node", bin.join("native-shim"))
            .expect("native shim");

        let error = validate_dependency_tree(root.path().join("node_modules").as_path())
            .expect_err("symlink target native addon must be rejected");
        assert!(error.contains("native Node addon"), "{error}");
    }

    #[test]
    fn dependency_snapshot_accepts_trusted_toolchain_native_bindings_and_prepare_metadata() {
        let root = TempDir::new().expect("tempdir");
        let node_modules = root.path().join("node_modules");
        for (package, version, _) in TRUSTED_TOOLCHAIN_LIFECYCLE_SCRIPTS {
            let package_dir = node_modules.join(package);
            fs::create_dir_all(&package_dir).expect("lifecycle package dir");
            fs::write(
                package_dir.join("package.json"),
                format!(
                    "{{\"name\":\"{package}\",\"version\":\"{version}\",\"scripts\":{{\"prepare\":\"node prepare.js\"}}}}\n"
                ),
            )
            .expect("lifecycle package manifest");
        }
        for (package, version, binary) in TRUSTED_TOOLCHAIN_NATIVE_BINDINGS {
            let package_dir = node_modules.join(package);
            fs::create_dir_all(&package_dir).expect("binding dir");
            fs::write(
                package_dir.join("package.json"),
                format!("{{\"name\":\"{package}\",\"version\":\"{version}\"}}\n"),
            )
            .expect("binding manifest");
            fs::write(package_dir.join(binary), b"\x7fELFfixture").expect("binding binary");
        }

        validate_dependency_tree(&node_modules)
            .expect("trusted fixed-toolchain binding and prepare metadata are allowed");
    }

    #[cfg(unix)]
    #[test]
    fn dependency_snapshot_accepts_trusted_toolchain_entries_through_a_canonicalized_parent() {
        let root = TempDir::new().expect("tempdir");
        let real_root = root.path().join("real");
        let node_modules = real_root.join("node_modules");
        for (package, version, _) in TRUSTED_TOOLCHAIN_LIFECYCLE_SCRIPTS {
            let package_dir = node_modules.join(package);
            fs::create_dir_all(&package_dir).expect("lifecycle package dir");
            fs::write(
                package_dir.join("package.json"),
                format!(
                    "{{\"name\":\"{package}\",\"version\":\"{version}\",\"scripts\":{{\"prepare\":\"node prepare.js\"}}}}\n"
                ),
            )
            .expect("lifecycle package manifest");
        }
        for (package, version, binary) in TRUSTED_TOOLCHAIN_NATIVE_BINDINGS {
            let package_dir = node_modules.join(package);
            fs::create_dir_all(&package_dir).expect("binding dir");
            fs::write(
                package_dir.join("package.json"),
                format!("{{\"name\":\"{package}\",\"version\":\"{version}\"}}\n"),
            )
            .expect("binding manifest");
            fs::write(package_dir.join(binary), b"\x7fELFfixture").expect("binding binary");
        }
        let alias_root = root.path().join("alias");
        std::os::unix::fs::symlink(&real_root, &alias_root).expect("alias root");

        validate_dependency_tree(&alias_root.join("node_modules"))
            .expect("trusted entries must survive canonical root/path spelling differences");
    }

    #[test]
    fn dependency_snapshot_accepts_trusted_hoisted_lifecycle_manifests_recursively() {
        let root = TempDir::new().expect("tempdir");
        let node_modules = root.path().join("node_modules");
        let package_dir = node_modules.join("balanced-match");
        fs::create_dir_all(package_dir.join("dist")).expect("package dir");
        fs::write(
            package_dir.join("package.json"),
            r#"{"name":"balanced-match","version":"4.0.4","scripts":{"prepare":"node prepare.js"}}"#,
        )
        .expect("package manifest");
        fs::write(package_dir.join("dist/index.js"), "export {};\n").expect("nested file");

        validate_dependency_tree(&node_modules)
            .expect("recursive validation must keep the hoisted dependency root stable");
    }

    #[test]
    fn dependency_snapshot_rejects_untrusted_lifecycle_scripts() {
        let root = TempDir::new().expect("tempdir");
        let package_dir = root.path().join("node_modules/dayjs");
        fs::create_dir_all(&package_dir).expect("package dir");
        fs::write(
            package_dir.join("package.json"),
            r#"{"name":"dayjs","version":"1.11.13","scripts":{"prepare":"node build.js"}}"#,
        )
        .expect("package manifest");
        let error = validate_dependency_tree(root.path().join("node_modules").as_path())
            .expect_err("arbitrary dependency lifecycle metadata must fail closed");
        assert!(
            error.contains("forbidden lifecycle script prepare"),
            "{error}"
        );
    }

    #[test]
    fn dependency_versions_reject_non_registry_and_alias_protocols() {
        for version in [
            "file:../pkg",
            "workspace:*",
            "patch:left-pad@1.3.0#./left-pad.patch",
            "portal:../pkg",
            "catalog:default",
            "npm:react@19.2.8",
            "git+https://example.invalid/repo.git",
        ] {
            let error = LocalAppsHostBroker::validate_dependency_version(version)
                .expect_err("only ordinary npm registry versions are accepted");
            assert!(error.contains("npm registry only"), "{version}: {error}");
        }
        for version in ["1.2.3", "^1.2.3", "~1.2.3", ">=1 <2", "latest"] {
            LocalAppsHostBroker::validate_dependency_version(version)
                .unwrap_or_else(|error| panic!("ordinary registry version {version}: {error}"));
        }
    }

    /// The shape `pnpm install` ACTUALLY produces for this template: every
    /// package with a `bin` field gets a relative shim under `node_modules/.bin`
    /// that points back inside the tree. Measured against the pinned template
    /// lockfile with the engine's exact flags, that is `.bin/{jiti,nanoid,
    /// rolldown,vite}` -- four internal relative symlinks, with `nodeLinker:
    /// hoisted` keeping `.pnpm/` itself symlink-free.
    ///
    /// `dependency_snapshot_rejects_symlink_entries` above only ever builds a
    /// symlink that ESCAPES the tree, so it pins the real security invariant
    /// while never crossing the line this fixture crosses. Rejecting internal
    /// shims too means `publish_dependency_snapshot` fails on every install
    /// that pnpm completes successfully, so the snapshot cache can never be
    /// populated and every app re-runs a full install.
    #[cfg(unix)]
    #[test]
    fn dependency_snapshot_accepts_internal_bin_shims() {
        let root = TempDir::new().expect("tempdir");
        let source = root.path().join("install/node_modules");
        fs::create_dir_all(source.join("vite/bin")).expect("source tree");
        fs::write(source.join("vite/bin/vite.js"), b"vite").expect("vite marker");
        fs::create_dir_all(source.join(".bin")).expect("bin dir");
        std::os::unix::fs::symlink("../vite/bin/vite.js", source.join(".bin/vite"))
            .expect("bin shim");

        let snapshot = root.path().join("cache/snapshot");
        LocalAppsHostBroker::publish_dependency_snapshot(&source, &snapshot, "lock-digest")
            .expect("a pnpm tree with internal bin shims must publish");
        assert!(
            LocalAppsHostBroker::dependency_snapshot_is_ready(&snapshot, "lock-digest")
                .expect("validate snapshot")
        );

        let staging = root.path().join("staging");
        fs::create_dir_all(&staging).expect("staging");
        LocalAppsHostBroker::materialize_dependency_snapshot(&snapshot, &staging)
            .expect("materialize snapshot");
        // The shim must survive as a shim: Node resolves `.bin/vite` through the
        // link, so materializing it as a dangling entry would break the build
        // just as surely as dropping it.
        let shim = staging.join("node_modules/.bin/vite");
        let shim_metadata = fs::symlink_metadata(&shim).expect("materialized bin shim");
        assert!(
            shim_metadata.file_type().is_symlink(),
            "bin shim must stay a symlink"
        );
        assert_eq!(
            fs::read(&shim).expect("shim resolves to its target"),
            b"vite"
        );
    }

    /// Loosening "no symlinks" to "no escaping symlink" only holds if the
    /// containment check is enforced where the bytes actually move. Validation
    /// runs on the staged COPY, so a copy layer that faithfully reproduced an
    /// escaping link would already have read through it.
    #[cfg(unix)]
    #[test]
    fn dependency_snapshot_still_refuses_escaping_shims() {
        let root = TempDir::new().expect("tempdir");
        let source = root.path().join("install/node_modules");
        fs::create_dir_all(source.join("vite/bin")).expect("source tree");
        fs::write(source.join("vite/bin/vite.js"), b"vite").expect("vite marker");
        fs::write(root.path().join("secret"), b"secret").expect("outside file");
        fs::create_dir_all(source.join(".bin")).expect("bin dir");
        std::os::unix::fs::symlink("../../../secret", source.join(".bin/exfil"))
            .expect("escaping shim");

        let snapshot = root.path().join("cache/snapshot");
        let error =
            LocalAppsHostBroker::publish_dependency_snapshot(&source, &snapshot, "lock-digest")
                .expect_err("an escaping shim must not publish");
        assert!(error.contains("escapes the tree"), "{error}");
        assert!(
            !snapshot.join("node_modules/.bin/exfil").exists(),
            "a refused publish must leave no snapshot behind"
        );
    }

    /// An absolute target can point inside the tree at publish time and still be
    /// wrong: the snapshot is consumed from a different directory than it was
    /// built in, so the link would silently re-point at the app that created it.
    #[cfg(unix)]
    #[test]
    fn dependency_snapshot_refuses_absolute_shims_that_currently_resolve_inside() {
        let root = TempDir::new().expect("tempdir");
        let source = root.path().join("install/node_modules");
        fs::create_dir_all(source.join("vite/bin")).expect("source tree");
        fs::write(source.join("vite/bin/vite.js"), b"vite").expect("vite marker");
        fs::create_dir_all(source.join(".bin")).expect("bin dir");
        std::os::unix::fs::symlink(source.join("vite/bin/vite.js"), source.join(".bin/vite"))
            .expect("absolute shim");

        let snapshot = root.path().join("cache/snapshot");
        let error =
            LocalAppsHostBroker::publish_dependency_snapshot(&source, &snapshot, "lock-digest")
                .expect_err("an absolute shim must not publish");
        assert!(error.contains("must be relative"), "{error}");
    }

    /// The attestation has to see the difference between a shim and a regular
    /// file holding that same path as text, or swapping one for the other would
    /// leave `dependency_snapshot_is_ready` satisfied.
    #[cfg(unix)]
    #[test]
    fn dependency_tree_digest_separates_a_shim_from_its_target_text() {
        let root = TempDir::new().expect("tempdir");
        let linked = root.path().join("linked/node_modules");
        fs::create_dir_all(linked.join("vite/bin")).expect("linked tree");
        fs::write(linked.join("vite/bin/vite.js"), b"vite").expect("vite marker");
        std::os::unix::fs::symlink("../vite/bin/vite.js", linked.join("shim")).expect("shim");

        let plain = root.path().join("plain/node_modules");
        fs::create_dir_all(plain.join("vite/bin")).expect("plain tree");
        fs::write(plain.join("vite/bin/vite.js"), b"vite").expect("vite marker");
        fs::write(plain.join("shim"), b"../vite/bin/vite.js").expect("plain shim");

        assert_ne!(
            dependency_tree_digest(&linked).expect("linked digest"),
            dependency_tree_digest(&plain).expect("plain digest"),
        );
    }

    #[test]
    fn dependency_sbom_spdx_ids_are_collision_free_for_punctuation_variants() {
        let root = TempDir::new().expect("tempdir");
        let node_modules = root.path().join("node_modules");
        fs::create_dir_all(&node_modules).expect("node_modules");
        write_fixture_package_manifest(&node_modules, "a.b", "1_0");
        write_fixture_package_manifest(&node_modules, "a-b", "1.0");
        let binding = crate::local_app_runtime_profiles::current_binding_for_family(
            local_apps::AppRuntimeProfile::ReactDom,
        )
        .expect("binding");
        let sbom =
            installed_dependency_sbom(&node_modules, &binding, &"d".repeat(64)).expect("sbom");
        let document: Value = serde_json::from_slice(&sbom).expect("sbom json");
        let packages = document
            .get("packages")
            .and_then(Value::as_array)
            .expect("packages");
        let ids = packages
            .iter()
            .filter_map(|package| package.get("SPDXID").and_then(Value::as_str))
            .filter(|id| id.starts_with("SPDXRef-Package-"))
            .collect::<Vec<_>>();
        assert_eq!(ids.len(), 2);
        assert_ne!(ids[0], ids[1], "distinct packages must not collide");
        let relationships = document
            .get("relationships")
            .and_then(Value::as_array)
            .expect("relationships");
        for id in ids {
            assert!(
                relationships.iter().any(|relationship| {
                    relationship
                        .get("relatedSpdxElement")
                        .and_then(Value::as_str)
                        == Some(id)
                }),
                "relationship must target {id}"
            );
        }
    }

    /// Build a staged seed shaped the way `stage-local-app-runtime.py` emits
    /// one: a `node_modules` tree with a real `vite/bin/vite.js`, the `.bin`
    /// shims pnpm writes, and a `runtime-manifest.json` naming the lockfile the
    /// tree was resolved from.
    #[cfg(unix)]
    fn create_bundled_seed(root: &Path, manifest_lock_digest: &str) -> PathBuf {
        let seed = root.join("bundle/local-app-runtime");
        let node_modules = seed.join("node_modules");
        fs::create_dir_all(node_modules.join("vite/bin")).expect("seed tree");
        fs::write(
            node_modules.join("vite/bin/vite.js"),
            b"#!/usr/bin/env node\n",
        )
        .expect("seed vite");
        fs::create_dir_all(node_modules.join(".bin")).expect("seed bin dir");
        std::os::unix::fs::symlink("../vite/bin/vite.js", node_modules.join(".bin/vite"))
            .expect("seed shim");
        fs::write(
            seed.join("runtime-manifest.json"),
            format!(
                "{{\"schema_version\":1,\"pnpm_lock_sha256\":\"{manifest_lock_digest}\",\"read_only\":true}}\n"
            ),
        )
        .expect("seed manifest");
        seed
    }

    /// The whole point of shipping the seed: a device that has never installed
    /// anything gets a ready snapshot without a package manager, a Linux guest,
    /// or a network round trip.
    #[cfg(unix)]
    #[test]
    fn bundled_seed_becomes_the_snapshot_for_its_own_lockfile() {
        let root = TempDir::new().expect("tempdir");
        let seed = create_bundled_seed(root.path(), "lock-digest");
        let snapshot = root.path().join("cache/snapshot");

        let adopted =
            LocalAppsHostBroker::adopt_bundled_dependency_seed(&seed, "lock-digest", &snapshot)
                .expect("adopt bundled seed");

        assert!(adopted, "a matching seed must be adopted");
        assert!(
            LocalAppsHostBroker::dependency_snapshot_is_ready(&snapshot, "lock-digest")
                .expect("validate adopted snapshot"),
            "the adopted snapshot must satisfy the same readiness check a real install produces"
        );
        assert!(
            fs::symlink_metadata(snapshot.join("node_modules/.bin/vite"))
                .expect("adopted bin shim")
                .file_type()
                .is_symlink()
        );
    }

    /// Editing `package.json` re-resolves the lockfile, and the bundled tree no
    /// longer describes those dependencies. Adopting it anyway would install
    /// the wrong packages under an attestation claiming they were right.
    #[cfg(unix)]
    #[test]
    fn bundled_seed_is_declined_when_the_app_lockfile_has_drifted() {
        let root = TempDir::new().expect("tempdir");
        let seed = create_bundled_seed(root.path(), "bundled-digest");
        let snapshot = root.path().join("cache/snapshot");

        let adopted = LocalAppsHostBroker::adopt_bundled_dependency_seed(
            &seed,
            "the-apps-own-digest",
            &snapshot,
        )
        .expect("evaluate bundled seed");

        assert!(!adopted, "a seed for a different lockfile must be declined");
        assert!(
            !snapshot.exists(),
            "declining must not leave a partial snapshot behind"
        );
    }

    /// Store builds ship no seed at all, so absence is an ordinary outcome and
    /// must not fail the install that would otherwise proceed over the network.
    #[test]
    fn bundled_seed_absence_is_not_an_error() {
        let root = TempDir::new().expect("tempdir");
        let adopted = LocalAppsHostBroker::adopt_bundled_dependency_seed(
            &root.path().join("no-such-bundle"),
            "lock-digest",
            &root.path().join("cache/snapshot"),
        )
        .expect("absent seed must not be an error");
        assert!(!adopted);
    }

    /// Asserted as one whole string, not by `contains` on the directives that
    /// are interesting today: a CSP is only as strong as its most permissive
    /// directive, so the thing worth locking is the WHOLE policy — a widened
    /// `connect-src` or a dropped `object-src` is exactly what a substring
    /// check cannot see.
    #[test]
    fn the_served_policy_allows_wasm_and_workers_and_nothing_else_new() {
        assert_eq!(
            LOCAL_APP_CONTENT_SECURITY_POLICY,
            "default-src 'self'; script-src 'self' 'unsafe-inline' 'wasm-unsafe-eval'; style-src 'self' 'unsafe-inline'; img-src 'self' data: blob:; font-src 'self' data:; connect-src 'self'; media-src 'self' data: blob:; worker-src 'self' blob:; object-src 'none'; base-uri 'none'; frame-ancestors 'none'; form-action 'self'"
        );
    }

    /// `instantiateStreaming` rejects anything but `application/wasm`, and this
    /// server sends `nosniff`, so the default `application/octet-stream` would
    /// fail the streaming path with a MIME error that reads nothing like the
    /// CSP refusal it is not.
    #[test]
    fn wasm_is_served_with_the_type_streaming_instantiation_requires() {
        assert_eq!(
            content_type(Path::new("assets/physics-a1b2c3d4.wasm")),
            "application/wasm"
        );
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

    #[test]
    fn capture_without_a_rect_keeps_the_whole_frame_behaviour() {
        let value = capture_ui_value(&json!({ "app_id": "demo" })).unwrap();
        assert_eq!(
            value, None,
            "no rect means the whole view, exactly as before"
        );
    }

    #[test]
    fn capture_with_a_rect_serializes_it_into_the_opaque_value() {
        let value = capture_ui_value(&json!({
            "app_id": "demo",
            "rect": { "x": 10, "y": 20, "width": 120, "height": 80 }
        }))
        .unwrap()
        .expect("a rect must produce a value payload");
        let parsed: Value = serde_json::from_str(&value).unwrap();
        assert_eq!(parsed["rect"]["x"], 10);
        assert_eq!(parsed["rect"]["width"], 120);
    }

    #[test]
    fn capture_rejects_a_malformed_or_non_positive_size_rect() {
        for bad in [
            json!({ "x": 0, "y": 0, "width": 0, "height": 10 }),
            json!({ "x": 0, "y": 0, "width": 10, "height": -5 }),
            // Not a number at all: the finiteness filter is what rejects this,
            // and it is the check that must survive the negative-origin one
            // being dropped below.
            json!({ "x": "0", "y": 0, "width": 10, "height": 10 }),
            json!({ "x": 0, "y": 0, "width": 10 }),
        ] {
            let out = capture_ui_value(&json!({ "app_id": "demo", "rect": bad }));
            assert!(
                out.is_err(),
                "invalid rect must be refused host-side: {bad}"
            );
        }
    }

    /// A NEGATIVE origin is the common case, not an error.
    ///
    /// `getBoundingClientRect().top` is negative for anything scrolled above
    /// the fold, and that is exactly what `inspect_ui`'s `elements[].rect`
    /// hands the agent — so "inspect, take an element's rect, capture it"
    /// produced a hard tool error on the most natural flow there is. Clamping
    /// belongs to the client, which is the only side that knows the real
    /// viewport (`intersection` on iOS, `coerceIn` in `cropSourceRect` on
    /// Android); a host-side refusal made that client code unreachable.
    #[test]
    fn capture_accepts_a_negative_origin_and_leaves_clamping_to_the_client() {
        let value = capture_ui_value(&json!({
            "app_id": "demo",
            "rect": { "x": -50, "y": -40.5, "width": 200, "height": 150 }
        }))
        .expect("a scrolled-above-the-fold element rect must not be refused")
        .expect("a rect must produce a value payload");
        let parsed: Value = serde_json::from_str(&value).unwrap();
        assert_eq!(
            parsed["rect"]["x"], -50,
            "the origin must reach the client UNCHANGED so the client can clamp it"
        );
        assert_eq!(parsed["rect"]["y"], -40.5);
        assert_eq!(parsed["rect"]["width"], 200);
    }

    /// An explicit `"rect": null` means "not applicable", which is a WHOLE-VIEW
    /// capture — not a malformed request.
    ///
    /// `input.get("rect")` answers `Some(Value::Null)` for it, so the
    /// absent-rect guard never fired and every field lookup below then failed,
    /// turning a routine model habit into a hard tool error.
    #[test]
    fn capture_treats_an_explicit_null_rect_as_a_whole_view_capture() {
        let value = capture_ui_value(&json!({ "app_id": "demo", "rect": Value::Null }))
            .expect("an explicit null rect is not malformed");
        assert_eq!(
            value, None,
            "an explicit null must collapse to the same no-value payload as an absent rect"
        );
    }

    #[test]
    fn create_staging_quality_gate_rejects_fast_canvas_profiles() {
        assert!(
            validate_create_stage_quality("fast", local_apps::AppRuntimeProfile::ReactDom).is_ok()
        );
        let error = validate_create_stage_quality("fast", local_apps::AppRuntimeProfile::Canvas2d)
            .expect_err("canvas create staging must reject fast quality");
        assert!(error.contains("balanced or thorough"));
        let error = validate_create_stage_quality("turbo", local_apps::AppRuntimeProfile::ReactDom)
            .expect_err("unknown quality must fail closed");
        assert!(error.contains("quality_level"));
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

    /// Build the host facts a native client reports for one device.
    fn host_environment(
        host_os: traits::MobileHostOs,
        device_class: traits::MobileDeviceClass,
    ) -> traits::MobileHostEnvironment {
        traits::MobileHostEnvironment::new(
            host_os,
            Some("19.0".into()),
            device_class,
            traits::MobileExecutionTarget::PhysicalDevice,
            traits::MobileLaunchMode::Interactive,
        )
    }

    /// The reported iPhone failure: the agent was asked to declare the native
    /// device context, but the only device facts it can see are the runtime
    /// reminder's `Host OS: iOS` plus `Device class: phone` — and
    /// `(ios, phone)` is exactly the pair the manifest validator rejects. The
    /// host owns these facts, so it stamps them itself and the agent never
    /// supplies them.
    #[tokio::test]
    async fn the_host_stamps_the_iphone_device_context_the_agent_cannot_name() {
        let root = TempDir::new().expect("tempdir");
        let service = test_service(&root).await;
        let broker = LocalAppsHostBroker::new(
            root.path().to_path_buf(),
            MockSink::arc(),
            None,
            false,
            None,
        );
        assert!(broker.attach_service(service.clone()).is_ok());
        assert!(broker
            .attach_host_environment(host_environment(
                traits::MobileHostOs::Ios,
                traits::MobileDeviceClass::Phone,
            ))
            .is_ok());
        let app_id = create_app_fixture(&root, &service, "Device").await;

        let result = broker
            .update_manifest(json!({"app_id": app_id}))
            .await
            .expect("the host derives the device context without the agent");
        assert_eq!(result["device_context"]["os"], "ios");
        assert_eq!(result["device_context"]["formFactor"], "iphone");

        let layout = AppLayout::new(root.path().to_path_buf(), app_id).expect("layout");
        let recorded = load_manifest(&layout)
            .expect("manifest")
            .device_context
            .expect("the host records the confirmed native target");
        assert_eq!(recorded.os, "ios");
        assert_eq!(recorded.form_factor, "iphone");
    }

    /// The same derivation on the other platform, where the reminder's
    /// vocabulary happens to match the manifest's.
    #[tokio::test]
    async fn the_host_stamps_the_android_tablet_device_context() {
        let root = TempDir::new().expect("tempdir");
        let service = test_service(&root).await;
        let broker = LocalAppsHostBroker::new(
            root.path().to_path_buf(),
            MockSink::arc(),
            None,
            false,
            None,
        );
        assert!(broker.attach_service(service.clone()).is_ok());
        assert!(broker
            .attach_host_environment(host_environment(
                traits::MobileHostOs::Android,
                traits::MobileDeviceClass::Tablet,
            ))
            .is_ok());
        let app_id = create_app_fixture(&root, &service, "Tablet").await;

        let result = broker
            .update_manifest(json!({"app_id": app_id}))
            .await
            .expect("the host derives the device context without the agent");
        assert_eq!(result["device_context"]["os"], "android");
        assert_eq!(result["device_context"]["formFactor"], "tablet");
    }

    /// An unclassified host records NO context rather than a guessed one:
    /// `DeviceContext` is documented as absent-means-unknown, and every
    /// os/form-factor pair naming a real platform would be a fabrication.
    #[tokio::test]
    async fn an_unclassified_host_records_no_device_context() {
        let root = TempDir::new().expect("tempdir");
        let service = test_service(&root).await;
        let broker = LocalAppsHostBroker::new(
            root.path().to_path_buf(),
            MockSink::arc(),
            None,
            false,
            None,
        );
        assert!(broker.attach_service(service.clone()).is_ok());
        assert!(broker
            .attach_host_environment(host_environment(
                traits::MobileHostOs::Ios,
                traits::MobileDeviceClass::Unknown,
            ))
            .is_ok());
        let app_id = create_app_fixture(&root, &service, "Unclassified").await;

        let result = broker
            .update_manifest(json!({"app_id": app_id}))
            .await
            .expect("an unclassified host still updates the manifest");
        assert!(result["device_context"].is_null(), "{result}");
    }

    /// A model-authored `device_context` is no longer part of the contract:
    /// the tool schema rejects the key outright rather than letting a guessed
    /// pair reach the validator.
    #[tokio::test]
    async fn an_agent_supplied_device_context_never_overrides_the_host() {
        let root = TempDir::new().expect("tempdir");
        let service = test_service(&root).await;
        let broker = LocalAppsHostBroker::new(
            root.path().to_path_buf(),
            MockSink::arc(),
            None,
            false,
            None,
        );
        assert!(broker.attach_service(service.clone()).is_ok());
        assert!(broker
            .attach_host_environment(host_environment(
                traits::MobileHostOs::Ios,
                traits::MobileDeviceClass::Tablet,
            ))
            .is_ok());
        let app_id = create_app_fixture(&root, &service, "Ignored").await;

        let result = broker
            .update_manifest(json!({
                "app_id": app_id,
                "device_context": {"os": "ios", "formFactor": "phone"},
            }))
            .await
            .expect("a stray key never fails the call");
        assert_eq!(result["device_context"]["formFactor"], "ipad");
    }

    /// The surface is fixed at creation, and `update_manifest` is the one
    /// mutation path an agent can reach after that. Its refusal was the only
    /// member of the family without a test — `create` (`local_apps_mcp.rs`:
    /// `create_rejects_runtime_profile_and_surface_overrides`), `scaffold`
    /// (`scaffold_rejects_an_empty_brief_and_an_unknown_surface`) and the
    /// shell-mode create (`host.rs`:
    /// `create_app_in_shell_mode_rejects_a_surface`) are all covered — so
    /// deleting the four-line `if` was a silent green.
    ///
    /// It must REFUSE, not ignore: `manifest.surface` is carried through the
    /// load-modify-save untouched, so a dropped refusal returns `ok` to an
    /// agent that then believes it converted the app, while the workspace on
    /// disk still holds the other scaffold's source.
    #[tokio::test]
    async fn update_manifest_rejects_a_caller_supplied_surface() {
        let root = TempDir::new().expect("tempdir");
        let service = test_service(&root).await;
        let broker = LocalAppsHostBroker::new(
            root.path().to_path_buf(),
            MockSink::arc(),
            None,
            false,
            None,
        );
        assert!(broker.attach_service(service.clone()).is_ok());
        let app_id = create_app_fixture(&root, &service, "Fixed").await;
        let layout = AppLayout::new(root.path().to_path_buf(), app_id.clone()).expect("layout");
        let before = load_manifest(&layout).expect("manifest").surface;

        let error = broker
            .update_manifest(json!({
                "app_id": app_id,
                "surface": "canvas",
            }))
            .await
            .expect_err("a caller-supplied surface must be refused, not silently ignored");
        assert!(
            error.contains("an app's surface is fixed when the app is created"),
            "got {error}"
        );

        let after = load_manifest(&layout).expect("manifest").surface;
        assert_eq!(
            before, after,
            "the refusal must happen before the manifest is saved"
        );
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

    // ------------------------------------------------------------------
    // Task 9: the pinned init session's title.
    //
    // A shell app's init session is minted while `record.name` is still the
    // `untitled` placeholder, and that title lands in a PERSISTED session
    // directory. Scaffolding renames it — but only when the user has not
    // renamed it first, and the boot sweep must apply the SAME rule.
    // ------------------------------------------------------------------

    /// A shell app with a pinned init session, plus everything needed to read
    /// and rewrite that session's title.
    struct PinnedShell {
        root: TempDir,
        service: Arc<AppService>,
        broker: Arc<LocalAppsHostBroker>,
        lingxi_home: PathBuf,
        fs: Arc<dyn traits::FileSystem>,
        app_id: String,
        init_session_id: String,
        /// Captured at creation so the transcript path is derived exactly the
        /// way production derives it, from the record's own workspace.
        workspace_rel: String,
    }

    impl PinnedShell {
        fn transcript(&self) -> PathBuf {
            self.lingxi_home
                .join("projects")
                .join(session::jsonl::path::project_dir_name(
                    &canonical_cwd_string(&self.root.path().join(&self.workspace_rel)),
                ))
                .join(format!("{}.jsonl", self.init_session_id))
        }

        /// The title the session catalog would resolve for this session.
        fn title(&self) -> String {
            let transcript =
                fs::read_to_string(self.transcript()).expect("read the pinned transcript");
            latest_custom_title(&transcript, &self.init_session_id)
                .expect("the pinned session always carries a custom-title")
                .0
        }

        /// The user renaming the session themselves — `/rename`'s channel
        /// (`append_custom_title`), which carries NO `mobileEmptySession`.
        async fn user_rename(&self, title: &str) {
            session::jsonl::writer::JsonlWriter::new(self.transcript(), self.fs.clone())
                .append_custom_title(&self.init_session_id, title)
                .await
                .expect("user rename");
        }

        /// Run the transcript past `JsonlWriter`'s REAL 32 KiB metadata
        /// backstop, which is what an interview of any length does to this
        /// transcript.
        ///
        /// Deliberately NOT a hand-written unmarked `custom-title` line: the
        /// record has to come out of `plan_re_append` itself, so the test
        /// keeps pinning the production behaviour if that rebuild ever changes
        /// shape. `append_file_history_snapshot` accounts its bytes against
        /// the backstop counter without polling it; the next side-record
        /// append is what fires the poll. Both are ordinary public writer
        /// calls — no test-only hook.
        async fn trip_the_metadata_backstop(&self) {
            let writer =
                session::jsonl::writer::JsonlWriter::new(self.transcript(), self.fs.clone());
            writer
                .append_file_history_snapshot(&json!({
                    "type": "file-history-snapshot",
                    "sessionId": self.init_session_id,
                    "messageId": "interview",
                    "snapshot": "x".repeat(
                        session::jsonl::re_append::METADATA_REAPPEND_BACKSTOP_BYTES,
                    ),
                }))
                .await
                .expect("bulk interview transcript");
            writer
                .append_permission_mode("default")
                .await
                .expect("the append that polls the backstop");
            assert!(
                !self.latest_title_record_carries_the_marker(),
                "the backstop must really have re-emitted the title UNMARKED — without \
                 that this test proves nothing"
            );
        }

        /// Whether the LAST `custom-title` on disk still carries
        /// `mobileEmptySession`. Only a probe: nothing in production may
        /// decide anything from the last record alone.
        fn latest_title_record_carries_the_marker(&self) -> bool {
            let transcript =
                fs::read_to_string(self.transcript()).expect("read the pinned transcript");
            let mut marked = false;
            for line in transcript.lines() {
                let Ok(value) = serde_json::from_str::<Value>(line) else {
                    continue;
                };
                if value.get("type").and_then(Value::as_str) != Some("custom-title")
                    || value.get("sessionId").and_then(Value::as_str)
                        != Some(self.init_session_id.as_str())
                {
                    continue;
                }
                marked = value.get("mobileEmptySession").and_then(Value::as_u64) == Some(1);
            }
            marked
        }

        async fn scaffold(&self, name: &str) -> Result<Value, String> {
            let input = confirmed_scaffold_input(
                &self.broker,
                &self.app_id,
                name,
                "a confirmed brief",
                "canvas",
            )
            .await;
            self.broker.scaffold_shell_app_value(input).await
        }

        async fn run_boot_backfill_sweep(&self) {
            crate::host::run_app_boot_backfill_sweep(
                self.lingxi_home.clone(),
                self.root.path().to_string_lossy().to_string(),
                self.root.path().to_path_buf(),
                self.fs.clone(),
                self.service.clone(),
            )
            .await;
        }
    }

    /// The "+" button's state: an unscaffolded shell whose pinned init session
    /// is titled with the `untitled` placeholder.
    async fn pinned_shell() -> PinnedShell {
        let root = TempDir::new().expect("tempdir");
        let lingxi_home = root.path().join(".lingxi");
        fs::create_dir_all(&lingxi_home).expect("create lingxi home");
        let fs_impl: Arc<dyn traits::FileSystem> = Arc::new(
            platform_posix_minimal::PosixFileSystem::new(root.path().to_path_buf()),
        );
        let service = test_service(&root).await;
        let broker = LocalAppsHostBroker::new(
            root.path().to_path_buf(),
            Arc::new(NoopClientEventSink),
            Some(MockMobileLinuxRuntime::new(Duration::ZERO)),
            false,
            None,
        );
        assert!(broker.attach_service(service.clone()).is_ok());
        assert!(broker
            .attach_session_catalog(SessionCatalog {
                lingxi_home: lingxi_home.clone(),
                fs: fs_impl.clone(),
            })
            .is_ok());

        let record = service
            .create_app_with_mode(None, "", None, local_apps::CreateMode::Shell, None)
            .await
            .expect("create the shell app");
        assert!(!record.scaffolded);
        assert_eq!(record.name, local_apps::PLACEHOLDER_APP_NAME);

        let init_session_id = crate::host::mint_app_init_session(
            &lingxi_home,
            &root.path().to_string_lossy(),
            root.path(),
            fs_impl.clone(),
            &record,
        )
        .await
        .expect("mint the pinned init session");
        service
            .set_init_session(&record.id, &init_session_id)
            .await
            .expect("pin the init session");

        let shell = PinnedShell {
            root,
            service,
            broker,
            lingxi_home,
            fs: fs_impl,
            app_id: record.id,
            init_session_id,
            workspace_rel: record.workspace_rel.clone(),
        };
        // The defect this task exists for: the placeholder is already on disk.
        assert_eq!(shell.title(), local_apps::PLACEHOLDER_APP_NAME);
        shell
    }

    #[tokio::test]
    async fn scaffold_renames_the_pinned_session_when_the_user_never_renamed_it() {
        let shell = pinned_shell().await;

        shell.scaffold("打飞机").await.expect("scaffold");

        assert_eq!(shell.title(), "打飞机");
    }

    /// A rename that fails is only "retryable" if something actually retries
    /// it. The scaffold has already committed by then and is NOT rolled back,
    /// so the boot sweep is the whole of that guarantee.
    #[cfg(unix)]
    #[tokio::test]
    async fn the_boot_sweep_reconciles_a_title_a_failed_rename_left_behind() {
        use std::os::unix::fs::PermissionsExt;

        let shell = pinned_shell().await;
        // Make the append genuinely fail: a read-only transcript cannot be
        // opened for append. This is the real failure path, not a skipped one.
        let transcript = shell.transcript();
        fs::set_permissions(&transcript, fs::Permissions::from_mode(0o444))
            .expect("make the transcript read-only");

        shell
            .scaffold("打飞机")
            .await
            .expect("a failed rename must not roll the scaffold back");

        fs::set_permissions(&transcript, fs::Permissions::from_mode(0o644))
            .expect("restore the transcript");
        assert_eq!(
            shell.title(),
            local_apps::PLACEHOLDER_APP_NAME,
            "the rename really did fail, so the retry has something to repair"
        );
        assert!(
            shell
                .service
                .record(&shell.app_id)
                .await
                .expect("record")
                .scaffolded,
            "the scaffold itself committed"
        );

        shell.run_boot_backfill_sweep().await;

        assert_eq!(
            shell.title(),
            "打飞机",
            "a failed rename must have a real trigger that fixes it later"
        );
    }

    /// The real flow, not the shortest one: an interview long enough to trip
    /// the transcript writer's 32 KiB metadata backstop still gets its title.
    ///
    /// This is the test whose absence made the whole reconciliation invisible.
    /// The backstop re-emits the title as a PLAIN `custom-title`, so a
    /// predicate that read the marker off the LAST record declined for every
    /// app created through this flow and they all kept `untitled` forever —
    /// with `scaffold_renames_the_pinned_session_when_the_user_never_renamed_it`
    /// (a transcript of two lines) staying green throughout.
    #[tokio::test]
    async fn the_rename_survives_the_metadata_backstop_a_real_interview_trips() {
        let shell = pinned_shell().await;
        shell.trip_the_metadata_backstop().await;

        shell.scaffold("打飞机").await.expect("scaffold");

        assert_eq!(
            shell.title(),
            "打飞机",
            "an interview longer than 32 KiB must not cost the app its name"
        );
    }

    /// The other half, and the one that must never regress: tolerating the
    /// backstop's unmarked echo must not make a real `/rename` overwritable.
    ///
    /// After `/rename`, the backstop echoes the USER'S title unmarked — text
    /// the anchor never carried — so the predicate declines, immediately and
    /// on every later boot sweep.
    #[tokio::test]
    async fn a_user_rename_still_wins_after_the_backstop_echoes_it() {
        let shell = pinned_shell().await;
        shell.user_rename("我的宝贝项目").await;
        shell.trip_the_metadata_backstop().await;
        assert_eq!(
            shell.title(),
            "我的宝贝项目",
            "the backstop echoes the user's title, so that is what the scaffold sees"
        );

        shell.scaffold("打飞机").await.expect("scaffold");
        shell.run_boot_backfill_sweep().await;

        assert_eq!(shell.title(), "我的宝贝项目");
    }

    #[tokio::test]
    async fn an_immediate_rename_never_clobbers_a_user_rename() {
        let shell = pinned_shell().await;
        shell.user_rename("我的宝贝项目").await;

        shell.scaffold("打飞机").await.expect("scaffold");

        assert_eq!(shell.title(), "我的宝贝项目");
    }

    #[tokio::test]
    async fn the_boot_sweep_never_clobbers_a_user_rename_either() {
        let shell = pinned_shell().await;
        shell.user_rename("我的宝贝项目").await;
        shell.scaffold("打飞机").await.expect("scaffold");

        shell.run_boot_backfill_sweep().await;

        assert_eq!(shell.title(), "我的宝贝项目");
    }

    /// Clause 1 of the predicate, pinned directly: an app still in its
    /// interview keeps the placeholder title even when its record already
    /// carries a real name. Driven through `reconcile_app_init_session_title`
    /// rather than a whole scaffold, because the app paths cannot currently
    /// produce this state — the point is that the rule survives a refactor
    /// that lets them.
    #[tokio::test]
    async fn reconciliation_waits_for_the_scaffold_commit_before_renaming() {
        let shell = pinned_shell().await;
        let mut record = shell.service.record(&shell.app_id).await.expect("record");
        record.name = "打飞机".into();
        assert!(!record.scaffolded);

        let renamed = reconcile_app_init_session_title(
            &shell.lingxi_home,
            shell.root.path(),
            shell.fs.clone(),
            &record,
        )
        .await
        .expect("reconcile");

        assert!(!renamed, "an unscaffolded shell is not renamed");
        assert_eq!(shell.title(), local_apps::PLACEHOLDER_APP_NAME);

        // The same record, one field later: the commit is the only thing that
        // was missing.
        record.scaffolded = true;
        assert!(reconcile_app_init_session_title(
            &shell.lingxi_home,
            shell.root.path(),
            shell.fs.clone(),
            &record,
        )
        .await
        .expect("reconcile"));
        assert_eq!(shell.title(), "打飞机");
    }

    /// The discriminator, stated as a unit. Three writers share the
    /// `custom-title` channel, only one of them marks its records, and a
    /// fourth — the writer's own 32 KiB metadata backstop — re-emits whatever
    /// the title currently is, UNMARKED. So the question is never "is the last
    /// record marked" but "did anyone write text mobile did not".
    #[test]
    fn a_placeholder_is_told_from_a_user_rename_by_text_against_the_anchor() {
        let session = "11111111-2222-3333-4444-555555555555";
        let anchor = format!(
            r#"{{"type":"custom-title","customTitle":"untitled","sessionId":"{session}","mobileEmptySession":1}}"#
        );
        // What `plan_re_append` writes when the backstop fires: the anchor's
        // own text, rebuilt without the marker.
        let backstop_echo = format!(
            r#"{{"type":"custom-title","customTitle":"untitled","sessionId":"{session}"}}"#
        );
        let user_rename = format!(
            r#"{{"type":"custom-title","customTitle":"我的宝贝项目","sessionId":"{session}"}}"#
        );
        let other_session = r#"{"type":"custom-title","customTitle":"elsewhere","sessionId":"99999999-2222-3333-4444-555555555555"}"#;

        assert!(latest_custom_title_is_mobile_placeholder(&anchor, session));
        // An unmarked record echoing the anchor's text is the backstop, not a
        // user. Reading the marker off the last record here is what made
        // `reconcile_app_init_session_title` unreachable in production.
        assert!(latest_custom_title_is_mobile_placeholder(
            &format!("{anchor}\n{backstop_echo}"),
            session
        ));
        // Text mobile never wrote, after the anchor: a user rename, and it
        // stays one however many times the backstop echoes it afterwards.
        assert!(!latest_custom_title_is_mobile_placeholder(
            &format!("{anchor}\n{user_rename}"),
            session
        ));
        assert!(!latest_custom_title_is_mobile_placeholder(
            &format!("{anchor}\n{user_rename}\n{user_rename}"),
            session
        ));
        // An unmarked record with no anchor before it — a `session::branch`
        // fork's title — is superseded by an anchor that follows it.
        assert!(!latest_custom_title_is_mobile_placeholder(
            &user_rename,
            session
        ));
        assert!(latest_custom_title_is_mobile_placeholder(
            &format!("{user_rename}\n{anchor}"),
            session
        ));
        // A record for another session never decides this one.
        assert!(latest_custom_title_is_mobile_placeholder(
            &format!("{anchor}\n{other_session}"),
            session
        ));
        // Nothing this host anchored: leave it alone.
        assert!(!latest_custom_title_is_mobile_placeholder("", session));
        // The effective title is still the LAST record's, marked or not.
        assert_eq!(
            latest_custom_title(&format!("{anchor}\n{user_rename}"), session)
                .expect("a title")
                .0,
            "我的宝贝项目"
        );
    }
}
