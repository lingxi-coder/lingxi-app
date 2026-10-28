//! Platform-specific UniFFI names backed by the canonical mobile Remote implementation.
use super::host::AndroidNativeMediaHost;
use client::protocol::controls::ReasoningSelectionDto;
use remote_control_client::signaling::DevicePlatform;
use std::{collections::HashMap, sync::Arc};
use AndroidNativeMediaHost as NativeMediaHost;

use AndroidRemoteKeyStore as PlatformRemoteKeyStore;
use AndroidRemoteListener as PlatformRemoteListener;
use AndroidRemoteBotsListener as PlatformRemoteBotsListener;
use AndroidRemoteMediaConfigProvider as PlatformRemoteMediaConfigProvider;
use AndroidRemoteTokenProvider as PlatformRemoteTokenProvider;
const REMOTE_DEVICE_PLATFORM: DevicePlatform = DevicePlatform::Android;

#[path = "../../../../packages/mobile-ffi/src/remote.rs"]
mod common;
pub use common::*;

/// Supplies Firebase ID tokens (the Kotlin app asks `FirebaseUser.getIdToken`).
/// Called for every database request, so implementations should cache;
/// `force_refresh` is set on the single retry after a 401.
#[uniffi::export(callback_interface)]
#[async_trait::async_trait]
pub trait AndroidRemoteTokenProvider: Send + Sync {
    /// A valid ID token, minted afresh when `force_refresh` is set.
    async fn id_token(&self, force_refresh: bool) -> Result<String, RemoteFfiError>;
}

/// Resolves ICE/TURN for each verified host connection and incoming media
/// session. The app may use its Firebase-backed product settings here.
#[uniffi::export(callback_interface)]
#[async_trait::async_trait]
pub trait AndroidRemoteMediaConfigProvider: Send + Sync {
    async fn resolve(
        &self,
        request: RemoteMediaConfigRequestFfi,
    ) -> Result<RemoteMediaConfigFfi, RemoteFfiError>;
}

/// Persistent device state, implemented by the Kotlin app over the Android
/// Keystore and DataStore. The device identity crosses as its PKCS#8 encoding
/// (48 bytes: the 16-byte Ed25519 header and the seed), which is what the other
/// clients store. Treat it as a secret.
#[uniffi::export(callback_interface)]
#[async_trait::async_trait]
pub trait AndroidRemoteKeyStore: Send + Sync {
    /// The stored device key, if one was created before.
    async fn load_identity(&self) -> Result<Option<Vec<u8>>, RemoteFfiError>;
    /// Keep the device key.
    async fn store_identity(&self, pkcs8: Vec<u8>) -> Result<(), RemoteFfiError>;
    /// Throw the device key away (a missing key is not an error).
    async fn delete_identity(&self) -> Result<(), RemoteFfiError>;
    /// The pinned key of a computer, if this device has paired with it.
    async fn load_pinned_host(
        &self,
        host_id: String,
    ) -> Result<Option<RemotePinnedHostFfi>, RemoteFfiError>;
    /// Remember a computer's key once the user approved this device on it.
    async fn store_pinned_host(&self, host: RemotePinnedHostFfi) -> Result<(), RemoteFfiError>;
    /// Forget a computer (a missing entry is not an error).
    async fn delete_pinned_host(&self, host_id: String) -> Result<(), RemoteFfiError>;
}

/// Hears about the remote client. Implementations enqueue onto the UI's state and
/// return promptly; calls arrive one at a time, in the order things happened.
#[uniffi::export(callback_interface)]
#[async_trait::async_trait]
pub trait AndroidRemoteListener: Send + Sync {
    /// The connection to the current computer changed state.
    async fn on_connection_state(&self, state: RemoteConnectionStateFfi);
    /// The list of the user's computers changed (only while watching hosts).
    async fn on_hosts(&self, hosts: Vec<RemoteHostFfi>);
    /// Watching the computers ended because the database refused or cancelled it.
    async fn on_hosts_failed(&self, message: String);
    /// The current computer's list of shared sessions changed.
    async fn on_sessions(&self, host_id: String, sessions: Vec<RemoteSessionSummaryFfi>);
    /// An attached session changed.
    async fn on_session_update(&self, update: RemoteSessionUpdateFfi);
}

/// Hears about the computer's Bots and the chats of the Bots the app shows; set
/// with `set_bots_listener`. A callback interface of its own, so that an app
/// written before Bots keeps implementing [`AndroidRemoteListener`] as it is.
/// Implementations enqueue onto the UI's state and return promptly; calls arrive
/// one at a time, in the order things happened, in step with those of
/// [`AndroidRemoteListener`].
#[uniffi::export(callback_interface)]
#[async_trait::async_trait]
pub trait AndroidRemoteBotsListener: Send + Sync {
    /// The current computer's Bots changed, or it turned out not to know Bots.
    async fn on_bots(&self, host_id: String, state: RemoteBotsStateFfi);
    /// The chats of a Bot that `activate_bot_threads` keeps current changed.
    async fn on_bot_threads(
        &self,
        host_id: String,
        bot_id: String,
        state: RemoteBotThreadsStateFfi,
    );
}

/// The Remote Control viewer: one instance per signed-in user, connected to one
/// computer at a time.
#[derive(uniffi::Object)]
pub struct AndroidRemoteControlClient {
    inner: common::RemoteControlCore,
}

/// Build the client. `database_url` is the Realtime Database instance
/// (`https://lingxi-5e4b2-default-rtdb.firebaseio.com`), `uid` the Firebase uid
/// the token belongs to; `device_name` is shown next to the pairing code on the desktop.
#[uniffi::export(async_runtime = "tokio")]
pub async fn build_android_remote_control_client(
    database_url: String,
    uid: String,
    device_name: String,
    app_version: String,
    tokens: Box<dyn AndroidRemoteTokenProvider>,
    keys: Box<dyn AndroidRemoteKeyStore>,
    listener: Box<dyn AndroidRemoteListener>,
    media_host: Arc<AndroidNativeMediaHost>,
    media_config_provider: Box<dyn AndroidRemoteMediaConfigProvider>,
) -> Result<Arc<AndroidRemoteControlClient>, RemoteFfiError> {
    let inner = common::build_remote_control_core(
        database_url,
        uid,
        device_name,
        app_version,
        tokens,
        keys,
        listener,
        media_host,
        media_config_provider,
    )
    .await?;
    Ok(Arc::new(AndroidRemoteControlClient { inner }))
}

#[uniffi::export(async_runtime = "tokio")]
impl AndroidRemoteControlClient {
    /// The push registration of this install, sent with the next handshake so the
    /// desktop can address and exclude it.
    pub fn set_push_installation_id(&self, id: Option<String>) {
        self.inner.set_push_installation_id(id)
    }

    /// Read the user's computers once, with their online state.
    pub async fn list_hosts(&self) -> Result<Vec<RemoteHostFfi>, RemoteFfiError> {
        self.inner.list_hosts().await
    }

    /// Keep the list of computers up to date through `on_hosts` (and re-check
    /// online state on a timer). Replaces an earlier watch.
    pub async fn watch_hosts(&self) {
        self.inner.watch_hosts().await
    }

    /// Stop `watch_hosts`.
    pub async fn stop_watching_hosts(&self) {
        self.inner.stop_watching_hosts().await
    }

    /// Dial a computer, replacing the current connection. States arrive through
    /// `on_connection_state`; the first time, `Pairing` carries the code to compare
    /// with the desktop's dialog. Dialling the computer that is already connected
    /// only nudges it.
    pub async fn connect(&self, host_id: String) -> Result<(), RemoteFfiError> {
        self.inner.connect(host_id).await
    }

    /// End the connection: sessions are detached, the link is closed.
    pub async fn disconnect(&self) {
        self.inner.disconnect().await
    }

    /// The user asks to try again after a failure.
    pub async fn retry(&self) {
        self.inner.retry().await
    }

    /// The app returned to the foreground or the network came back: skip the rest
    /// of a backoff, and revive a connection that gave up on its own.
    pub async fn nudge(&self) {
        self.inner.nudge().await
    }

    /// The current state of the connection, if a computer is being dialled.
    pub fn connection_state(&self) -> Option<RemoteConnectionStateFfi> {
        self.inner.connection_state()
    }

    /// List Local Apps shared by the connected computer.
    pub async fn list_apps(&self) -> Result<Vec<RemoteLocalAppFfi>, RemoteFfiError> {
        self.inner.list_apps().await
    }

    /// Obtain a short-lived browser bootstrap from the connected computer.
    pub async fn open_app(&self, app_id: String) -> Result<RemoteLocalAppOpenFfi, RemoteFfiError> {
        self.inner.open_app(app_id).await
    }

    /// The current computer's shared sessions; `None` until the first list arrives.
    pub fn sessions(&self) -> Option<Vec<RemoteSessionSummaryFfi>> {
        self.inner.sessions()
    }

    /// Forget a computer: the pin goes, the desktop still lists this device until the
    /// user removes it there. Disconnects first when it is the current one.
    pub async fn forget_host(&self, host_id: String) {
        self.inner.forget_host(host_id).await
    }

    /// After a `Revoked` failure: make a new device key and pair again.
    pub async fn repair_host(&self, host_id: String) -> Result<(), RemoteFfiError> {
        self.inner.repair_host(host_id).await
    }

    /// Follow a session: attach now if connected, and again after every reconnect
    /// (resuming from the last position held). Changes arrive through
    /// `on_session_update`, starting with the state held so far.
    pub async fn attach_session(&self, session_id: String) -> Result<(), RemoteFfiError> {
        self.inner.attach_session(session_id).await
    }

    /// Stop following a session; the desktop is told. Its position is kept for a later attach.
    pub async fn detach_session(&self, session_id: String) -> Result<(), RemoteFfiError> {
        self.inner.detach_session(session_id).await
    }

    /// Tell the desktop whether the screen shows the session (it uses this for notifications).
    pub async fn set_session_foreground(
        &self,
        session_id: String,
        foreground: bool,
    ) -> Result<(), RemoteFfiError> {
        self.inner
            .set_session_foreground(session_id, foreground)
            .await
    }

    /// Fetch the page of messages before the ones held; they arrive as
    /// `older_messages` of a session update.
    pub async fn load_older_messages(&self, session_id: String) -> Result<(), RemoteFfiError> {
        self.inner.load_older_messages(session_id).await
    }

    /// The bytes of an image a record refers to.
    pub async fn fetch_image(
        &self,
        session_id: String,
        image_ref: String,
    ) -> Result<RemoteImageFfi, RemoteFfiError> {
        self.inner.fetch_image(session_id, image_ref).await
    }

    /// Send a prompt. It shows up in `outgoing` until its own record comes back;
    /// a failure shows there too (see `retry_prompt`). While a turn runs only text is sent.
    pub async fn send_prompt(
        &self,
        session_id: String,
        text: String,
        images: Vec<RemotePromptImageFfi>,
    ) -> Result<(), RemoteFfiError> {
        self.inner.send_prompt(session_id, text, images).await
    }

    /// Hash an original file without loading it into memory.
    pub async fn prepare_file(
        &self,
        session_id: String,
        path: String,
        name: String,
    ) -> Result<RemoteFileFfi, RemoteFfiError> {
        self.inner.prepare_file(session_id, path, name).await
    }

    /// Stream original bytes on the independent file DataChannel.
    pub async fn upload_file(
        &self,
        file: RemoteFileFfi,
        path: String,
    ) -> Result<RemoteFileFfi, RemoteFfiError> {
        self.inner.upload_file(file, path).await
    }

    /// Published files this account/device may access in the attached session.
    pub async fn list_files(
        &self,
        session_id: String,
    ) -> Result<Vec<RemoteFileFfi>, RemoteFfiError> {
        self.inner.list_files(session_id).await
    }

    /// Save a verified receipt to an app-selected destination; interrupted files resume.
    pub async fn download_file(
        &self,
        file: RemoteFileFfi,
        path: String,
    ) -> Result<RemoteFileFfi, RemoteFfiError> {
        self.inner.download_file(file, path).await
    }

    /// Bytes durably acknowledged by the receiving device.
    pub fn file_progress(&self, transfer_id: String) -> u64 {
        self.inner.file_progress(transfer_id)
    }

    /// Cancel an active operation. Network errors retain resumable checkpoints.
    pub fn cancel_file(&self, transfer_id: String) {
        self.inner.cancel_file(transfer_id)
    }

    /// Attach only published receipts; desktop validates ownership and session.
    pub async fn send_prompt_with_files(
        &self,
        session_id: String,
        text: String,
        images: Vec<RemotePromptImageFfi>,
        file_refs: Vec<String>,
    ) -> Result<(), RemoteFfiError> {
        self.inner
            .send_prompt_with_files(session_id, text, images, file_refs)
            .await
    }

    /// Send a failed prompt again with the same request id.
    pub async fn retry_prompt(
        &self,
        session_id: String,
        prompt_id: String,
    ) -> Result<(), RemoteFfiError> {
        self.inner.retry_prompt(session_id, prompt_id).await
    }

    /// Drop a prompt from the outgoing list.
    pub async fn dismiss_prompt(
        &self,
        session_id: String,
        prompt_id: String,
    ) -> Result<(), RemoteFfiError> {
        self.inner.dismiss_prompt(session_id, prompt_id).await
    }

    /// Cancel the running turn.
    pub async fn cancel_turn(&self, session_id: String) -> Result<(), RemoteFfiError> {
        self.inner.cancel_turn(session_id).await
    }

    /// Allow once or deny a permission request.
    pub async fn respond_permission(
        &self,
        session_id: String,
        request_id: u64,
        allow: bool,
    ) -> Result<(), RemoteFfiError> {
        self.inner
            .respond_permission(session_id, request_id, allow)
            .await
    }

    /// Answer a question: question text to the chosen label(s) (several joined by ", ").
    pub async fn answer_question(
        &self,
        session_id: String,
        request_id: u64,
        answers: HashMap<String, String>,
    ) -> Result<(), RemoteFfiError> {
        self.inner
            .answer_question(session_id, request_id, answers)
            .await
    }

    /// Dismiss a question.
    pub async fn cancel_question(
        &self,
        session_id: String,
        request_id: u64,
    ) -> Result<(), RemoteFfiError> {
        self.inner.cancel_question(session_id, request_id).await
    }

    /// Change the session's model.
    pub async fn set_model(&self, session_id: String, model: String) -> Result<(), RemoteFfiError> {
        self.inner.set_model(session_id, model).await
    }

    /// Change the session's reasoning.
    pub async fn set_reasoning(
        &self,
        session_id: String,
        selection: ReasoningSelectionDto,
    ) -> Result<(), RemoteFfiError> {
        self.inner.set_reasoning(session_id, selection).await
    }

    /// Change the session's permission mode.
    pub async fn set_permission_mode(
        &self,
        session_id: String,
        mode: RemotePermissionModeFfi,
    ) -> Result<(), RemoteFfiError> {
        self.inner.set_permission_mode(session_id, mode).await
    }

    /// The models the desktop offers (asked once per connection).
    pub async fn list_models(&self) -> Result<RemoteModelsFfi, RemoteFfiError> {
        self.inner.list_models().await
    }
    /// Subscribe to the computer's Bots and their chats.
    pub fn set_bots_listener(&self, listener: Option<Box<dyn AndroidRemoteBotsListener>>) {
        self.inner.set_bots_listener(listener);
    }
    /// The current computer's Bot list.
    pub fn bots_state(&self) -> RemoteBotsStateFfi { self.inner.bots_state() }
    /// The cached chats of one Bot.
    pub fn bot_threads_state(&self, bot_id: String) -> RemoteBotThreadsStateFfi { self.inner.bot_threads_state(bot_id) }
    /// Keep a visible Bot's chats current.
    pub async fn activate_bot_threads(&self, bot_id: String) -> Result<(), RemoteFfiError> { self.inner.activate_bot_threads(bot_id).await }
    /// Stop following a Bot's chats.
    pub fn deactivate_bot_threads(&self, bot_id: String) { self.inner.deactivate_bot_threads(bot_id); }
    /// Start a Bot chat and return its session id.
    pub async fn create_bot_thread(&self, bot_id: String) -> Result<String, RemoteFfiError> { self.inner.create_bot_thread(bot_id).await }
    /// The Bot a known chat belongs to.
    pub fn bot_of_thread(&self, session_id: String) -> Option<String> { self.inner.bot_of_thread(session_id) }
    /// The sending Bot of a delegated prompt.
    pub fn prompt_origin_bot_name(&self, session_id: String, turn_id: u64) -> Option<String> { self.inner.prompt_origin_bot_name(session_id, turn_id) }

}
