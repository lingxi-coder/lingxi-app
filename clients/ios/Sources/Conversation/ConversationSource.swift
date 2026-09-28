// ConversationSource.swift — M10 A2 / P3a.
//
// The seam between the SwiftUI conversation UI (ChatView) and whatever produces
// turns. The app talks ONLY to this protocol; two impls back it:
//
//   - MockConversationSource     — the prior canned behavior (no engine).
//   - EngineConversationSource   — the real in-process engine over UniFFI:
//       builds a `MobileEngineHandle` via `buildIosEngine(...)`, registers a
//       Swift `IosEventListener` whose `onEvent(_:)` maps each inbound
//       `ClientEvent` onto @MainActor-published state, and drives turns via
//       `handle.submit(.sendPrompt(...))`.
//
// This is the in-process analog of the Electron↔bridge real-conversation path:
// one protocol (`client-protocol`), one transport (UniFFI), one renderer
// (SwiftUI). The engine runs ON the device; no network bridge.
//
// SECRETS: the LLM API key is resolved from the iOS Keychain (`Keychain`) with a
// runtime `ANTHROPIC_API_KEY` env override for dev — see
// `EngineConfig.fromEnvironment`. It is NEVER hardcoded, logged, or persisted in
// plaintext here.

import Foundation

// MARK: - ConversationSource seam

/// What ChatView drives. A source owns the turn lifecycle and pushes results
/// into the shared `ConversationModel` (on the main actor).
@MainActor
protocol ConversationSource: AnyObject {
    /// The state ChatView observes.
    var model: ConversationModel { get }
    /// Capability profile for this source instance.
    var sessionMode: SessionMode { get }
    /// Begin a fresh, empty conversation (the "edit / new chat" affordance).
    func startNewConversation()
    /// Submit a user prompt. Appends the user message, flips `streaming`, and
    /// produces the assistant reply (canned for the mock, streamed for the engine).
    /// MUST be a no-op while a turn is in flight (`model.streaming`) so rapid taps
    /// can't start an overlapping turn (PR-4 item 1).
    @discardableResult
    func send(_ text: String) -> ConversationTurnToken?
    /// Submit a prompt and the ordered image attachments reviewed in the composer.
    /// The default keeps mock and non-engine sources source-compatible.
    @discardableResult
    func send(_ text: String, images: [ImageRefDto]) -> ConversationTurnToken?
    /// Cancel the in-flight turn (PR-4 item 2): the engine submits `.cancel(...)`
    /// and keeps ownership until the engine confirms safe completion. A no-op
    /// when nothing is streaming.
    func cancel()
    /// Cancel the in-flight turn (or an inactive durable recovery) and return
    /// only after the engine has released its single-turn slot.
    /// Engine/configuration swaps use this barrier so an old Block-behavior
    /// tool cannot overlap a replacement engine.
    func cancelAndWait() async throws
    /// Dismiss the persistent error banner (PR-4 item 4).
    func dismissError()
    /// Switch the active model (SHIP-BLOCKER #2). The engine source submits
    /// `ClientCommand.setModel(id)` with a REAL model id and reflects the
    /// confirming `ModelChanged`; the mock just swaps the chip. `id` is a real
    /// engine model id when `model.availableModels` is populated.
    func setModel(_ id: String)
    /// Change the permission mode through the same engine coordinator used by
    /// Settings. The string is the wire mode (default/acceptEdits/plan/auto/
    /// dontAsk/bypassPermissions).
    func setPermissionMode(_ mode: String)
    /// Apply `bypassPermissions` only after the controls sheet has shown and the
    /// user has accepted its explicit risk warning.
    func confirmAndSetBypassPermissions(suppressWarning: Bool)
    /// Change provider-aware reasoning selection. `automatic` deliberately
    /// means no user override; other values are validated by the engine.
    func setReasoningSelection(_ selection: String)
    /// Change speed through the engine and wait for its authoritative event.
    func setFastMode(_ enabled: Bool)
    /// Switch the active conversation to `session` (the iOS analog of Android's
    /// `ChatViewModel.openSession`). MUST cancel any in-flight turn and reset the
    /// streaming bookkeeping FIRST so a turn that completes after the switch can't
    /// land its deltas / notice in the newly-shown session. A no-op when the id is
    /// already active.
    func openSession(_ session: SessionRef)
    /// Continue an existing session in another capability profile.
    func forkSession(_ sessionID: String, targetMode: SessionMode) async throws
    /// Lifecycle: the app moved to the background (`scenePhase == .background`).
    /// This hook may persist state, but MUST NOT cancel an active turn or change
    /// its streaming bookkeeping. UIKit owns suspension; backgrounding is not a
    /// user Stop action.
    func handleBackground()
    /// Record a background lease expiration as recoverable, never cancelled.
    /// Returns only after the engine acknowledges `PausedRecoverable`; callers
    /// must not announce the pause or unlock the composer before then.
    func markActiveTurnPausedRecoverable(_ token: ConversationTurnToken?) async throws
    /// Lifecycle: the app returned to the foreground (`scenePhase == .active`). A
    /// hook to restore/refresh state; the default is a no-op.
    func handleForeground()
    /// Optionally build the engine eagerly so the real model catalog
    /// (`ModelList`) populates before the first send (SHIP-BLOCKER #2). A no-op on
    /// the mock; idempotent on the engine.
    func warmUp()
    /// Only the root-selected source may serve global settings pages.
    func setSettingsActive(_ active: Bool)
    /// Build the backing engine and surface construction failures to callers.
    /// Project switching uses this before committing the new workspace so a
    /// failed engine rebuild can roll back atomically.
    func prepare() async throws
    /// Ask the engine for its real resumable-session catalog (submit
    /// `ClientCommand.listSessions`). The reply (`SessionList`) lands out-of-band
    /// on the listener and populates `model.engineSessions`. A no-op on the mock
    /// (which keeps the canned drawer lists). Called when the drawer opens so the
    /// history is fresh.
    func listSessions()
    /// Resume a prior session by its engine UUID (submit
    /// `ClientCommand.resumeSession`). The engine confirms with `SessionResumed`;
    /// the source resets the transcript so the resumed session's turns land
    /// clean. The UI also selects the row locally so the choice reflects
    /// immediately even if engine-side resume is still a follow-up. A no-op on the
    /// mock.
    /// `emptySessionTitle` is supplied only when the persisted project index
    /// proves this is a zero-message legacy session. The engine can then restore
    /// the empty anchor without assigning a different UUID.
    func resumeSession(_ uuid: String, emptySessionTitle: String?)
    /// Ask the engine for its real MCP server listing (submit
    /// `RefreshListings(.mcp)`). The reply (`McpServers`) lands out-of-band and
    /// populates `model.mcpServers`. A no-op on the mock (keeps the canned list).
    func refreshMcpServers()
    /// Refresh the current session's agent roster.  The command is out of band
    /// from the active turn and is safe to call whenever the agent picker opens.
    func listSessionAgents()
    /// Resume a paused local workflow directly from its task row.
    func resumeWorkflow(_ taskID: String)
    /// Select one agent for the message list.  Child agents are read-only on
    /// iOS; selecting the main agent restores the composer.
    func selectAgent(_ id: String)
    /// Load a child agent's durable transcript into the source cache.
    func loadSessionAgentTranscript(_ id: String)
    #if canImport(harness_runtimeFFI)
        /// Resolve a parked permission request (SHIP-BLOCKER #3): submit
        /// `ApprovePermission{requestId, response}` and pop the head of the queue.
        func approvePermission(_ requestId: UInt64, _ response: PermissionResponseDto)
        /// Resolve a parked permission request by denying it: submit
        /// `DenyPermission{requestId}` and pop the head of the queue.
        func denyPermission(_ requestId: UInt64)
        /// Submit a non-turn command such as secure Provider credential CRUD.
        func submitEngineCommand(_ command: ClientCommand) async throws
        /// Exercise the engine's token-free Provider model-list probe.
        func testProviderConnection(
            profile: ProviderLaunchProfile,
            credentialOverride: String?
        ) async throws -> ProviderConnectionTestResult
        /// Read the engine's credential-free built-in Provider catalog.
        func providerCatalog() async throws -> [ProviderCatalogEntry]
        /// Run the native browser OAuth flow. The engine owns PKCE/state and
        /// only receives the callback URL from this coordinator.
        func loginOAuth(provider: String) async throws -> ProviderOAuthState
        func logoutOAuth(provider: String) async throws
        func authState(provider: String) async throws -> ProviderOAuthState
        func testOAuthConnection(
            provider: String,
            profile: ProviderLaunchProfile
        ) async throws -> ProviderConnectionTestResult
        /// Route an app-level `lingxi://oauth/callback` into the pending web
        /// authentication session when iOS delivers it through `onOpenURL`.
        func handleOAuthCallback(_ url: URL)
        /// Observe out-of-band engine events without duplicating the listener.
        func setExternalEventHandler(_ handler: ((ClientEvent) -> Void)?)
    #endif
}

/// Default `warmUp` for sources with nothing to pre-build (the mock). The engine
/// source overrides it to eagerly build the handle + list models. Lifecycle
/// hooks are no-ops by default; the engine source may override foregrounding to
/// refresh state, but backgrounding never implies cancellation.
extension ConversationSource {
    var sessionMode: SessionMode { .code }

    @discardableResult
    func send(_ text: String, images: [ImageRefDto]) -> ConversationTurnToken? {
        send(text)
    }

    func warmUp() {}
    func setSettingsActive(_ active: Bool) {}
    func prepare() async throws {}
    func cancelAndWait() async throws { cancel() }
    func handleBackground() {}
    func markActiveTurnPausedRecoverable(_ token: ConversationTurnToken?) async throws {}
    func handleForeground() {}
    func forkSession(_ sessionID: String, targetMode: SessionMode) async throws {}
    /// Default session ops for sources with no engine catalog (the mock): no-ops,
    /// so the mock keeps its canned drawer lists and ignores resume requests.
    func listSessions() {}
    func resumeSession(_ uuid: String) {
        resumeSession(uuid, emptySessionTitle: nil)
    }
    func resumeSession(_ uuid: String, emptySessionTitle: String?) {}
    func refreshMcpServers() {}
    func listSessionAgents() {}
    func resumeWorkflow(_ taskID: String) {}
    func selectAgent(_ id: String) {
        guard model.agentSummaries.contains(where: { $0.id == id }) else { return }
        model.markAgentTranscriptLoading(id)
    }
    func loadSessionAgentTranscript(_ id: String) {}
}

#if canImport(harness_runtimeFFI)
    /// Default permission handling for sources that never park a turn on a
    /// permission gate (the mock). The engine source overrides both.
    extension ConversationSource {
        func approvePermission(_ requestId: UInt64, _ response: PermissionResponseDto) {}
        func denyPermission(_ requestId: UInt64) {}
        func submitEngineCommand(_ command: ClientCommand) async throws {}
        func testProviderConnection(
            profile: ProviderLaunchProfile,
            credentialOverride: String?
        ) async throws -> ProviderConnectionTestResult {
            .failure(message: String(localized: "chat_provider_engine_unavailable"))
        }
        func providerCatalog() async throws -> [ProviderCatalogEntry] { [] }
        func loginOAuth(provider: String) async throws -> ProviderOAuthState {
            throw NSError(domain: "ConversationSource", code: 1, userInfo: [NSLocalizedDescriptionKey: "OAuth requires the engine"])
        }
        func logoutOAuth(provider: String) async throws {}
        func authState(provider: String) async throws -> ProviderOAuthState {
            ProviderOAuthState(provider: provider, signedIn: false, accountLabel: nil, accountID: nil, organizationID: nil, fedramp: false)
        }
        func testOAuthConnection(
            provider: String,
            profile: ProviderLaunchProfile
        ) async throws -> ProviderConnectionTestResult {
            .failure(message: String(localized: "chat_provider_engine_unavailable"))
        }
        func handleOAuthCallback(_ url: URL) {}
        func setExternalEventHandler(_ handler: ((ClientEvent) -> Void)?) {}
    }
#endif
