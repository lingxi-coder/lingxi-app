import Foundation
import Observation
import SwiftUI

typealias ProviderCommandSubmitter = (ClientCommand) async throws -> Void

typealias ProviderConnectionTester = (ProviderLaunchProfile, String?) async throws -> ProviderConnectionTestResult
typealias ProviderOAuthLoginHandler = (String) async throws -> ProviderOAuthState
typealias ProviderOAuthStateLoader = (String) async throws -> ProviderOAuthState
typealias ProviderOAuthLogoutHandler = (String) async throws -> Void
typealias ProviderOAuthConnectionTester = (String, ProviderLaunchProfile) async throws -> ProviderConnectionTestResult
typealias ProviderCatalogLoader = () async throws -> [ProviderCatalogEntry]

typealias ProviderApplyReconnectHandler = (ProviderLaunchSnapshot) async throws -> Void

enum ProviderConnectionTestResult: Equatable {
    case success(message: String? = nil, usedStoredCredential: Bool = true)
    case failure(message: String)
}

struct ProviderOAuthState: Equatable {
    let provider: String
    let signedIn: Bool
    let accountLabel: String?
    let accountID: String?
    let organizationID: String?
    let fedramp: Bool
}

enum ProviderProfileValidationError: LocalizedError, Equatable {
    case missingName
    case invalidBaseURL
    case missingModel
    case missingCredential

    var errorDescription: String? {
        switch self {
        case .missingName:
            return String(localized: "settings_provider_error_missing_name")
        case .invalidBaseURL:
            return String(localized: "settings_provider_error_invalid_url")
        case .missingModel:
            return String(localized: "settings_provider_error_missing_model")
        case .missingCredential:
            return String(localized: "settings_provider_error_missing_credential")
        }
    }
}

private enum ProviderRepositoryOperationError: LocalizedError {
    case failed(String)

    var errorDescription: String? {
        switch self {
        case .failed(let message): return message
        }
    }
}

enum ProviderCredentialState: Equatable {
    case unknown
    case configured
    case missing
}

enum ProviderConnectionState: Equatable {
    case idle
    case testing
    case connected
    case failed
}

struct ProviderLaunchProfile: Equatable {
    /// Stable identifier used by the settings UI and persisted profile row.
    let settingsID: String
    /// Provider profile / secure-credential identifier understood by the engine.
    let id: String
    let presetID: String
    let providerType: String
    let displayName: String
    let baseURL: String
    let modelID: String
    let enabled: Bool
    let isDefault: Bool
    let apiKeyEnv: String

    var qualifiedModelID: String {
        "\(id)/\(modelID)"
    }
}

struct ProviderLaunchSnapshot: Equatable {
    let profiles: [ProviderLaunchProfile]
    let providerProfilesJSON: String
    let routingJSON: String
    let defaultModelID: String?
    let enabledProfileIDs: [String]
    let visionDelegationEnabled: Bool
}

struct ProviderRoutingSettings: Codable, Equatable {
    static let defaultRetryMaxAttempts = 10
    static let defaultRetryBackoffMs = 500
    static let minRetryMaxAttempts = 0
    static let maxRetryMaxAttempts = 10
    static let minRetryBackoffMs = 1
    static let maxRetryBackoffMs = 60_000

    var retryMaxAttempts: Int
    var retryBackoffMs: Int
    var fallbackProfileIDs: [String]

    init(
        retryMaxAttempts: Int = ProviderRoutingSettings.defaultRetryMaxAttempts,
        retryBackoffMs: Int = ProviderRoutingSettings.defaultRetryBackoffMs,
        fallbackProfileIDs: [String] = []
    ) {
        self.retryMaxAttempts = retryMaxAttempts
        self.retryBackoffMs = retryBackoffMs
        self.fallbackProfileIDs = fallbackProfileIDs
    }
}

struct ProviderFallbackCandidate: Identifiable, Equatable {
    let profileID: String
    let name: String
    let modelID: String
    let selected: Bool
    let order: Int?

    var id: String { profileID }
}

struct ProviderStoredProfile: Codable, Equatable, Identifiable {
    var id: String
    var presetID: String
    var name: String
    var baseURL: String
    var modelID: String
    var enabled: Bool
    var isDefault: Bool

    init(
        id: String,
        presetID: String,
        name: String,
        baseURL: String,
        modelID: String,
        enabled: Bool,
        isDefault: Bool
    ) {
        self.id = id
        self.presetID = presetID
        self.name = name
        self.baseURL = baseURL
        self.modelID = modelID
        self.enabled = enabled
        self.isDefault = isDefault
    }
}

/// A non-persisted edit transaction.  Provider settings are deliberately
/// edited outside `profiles` so cancelling a sheet cannot leak half-written
/// values into the launch snapshot or the settings summary.
struct ProviderEditorDraft: Identifiable, Equatable {
    let id: String
    let isNew: Bool
    var profile: ProviderStoredProfile
    var credentialState: ProviderCredentialState
    var hasLegacyAnthropicCredential: Bool
    var oauthState: ProviderOAuthState?
    var pendingSecret: String = ""
    var clearCredentialOnApply = false
    var connectionState: ProviderConnectionState = .idle
    var detailMessage: String?
    var validationMessage: String?
    var operationInFlight = false

    var hasPendingSecret: Bool {
        !pendingSecret.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
    }

    var hasStoredAPIKey: Bool {
        !clearCredentialOnApply &&
            (credentialState == .configured || hasLegacyAnthropicCredential)
    }

    var hasStoredCredential: Bool {
        !clearCredentialOnApply && (hasStoredAPIKey || oauthState?.signedIn == true)
    }

    var effectiveHasCredential: Bool {
        hasPendingSecret || hasStoredCredential
    }
}

struct ProviderRuntimeSnapshot: Equatable {
    var models: [String] = []
    var activeModelID: String?
    var activeProfileID: String?
    var lastError: String?
}

struct ProviderCatalogEntry: Identifiable, Equatable {
    let id: String
    let displayName: String
    let baseURL: String
    let protocolName: String
    let authName: String
    let credentialEnv: String?
    let models: [String]
    let modelDetails: [String: ModelRuntimeDetails]

    var supportsOAuth: Bool {
        authName == "ChatGptOAuth" || authName == "OAuthBearer"
    }
}

struct ProviderSettingsSummary: Equatable {
    let defaultProfile: ProviderStoredProfile?
    let enabledCount: Int
    let totalCount: Int
    let runtime: ProviderRuntimeSnapshot
    let defaultRuntimeProfileID: String?

    var defaultModelID: String? {
        defaultProfile?.modelID
    }

    var defaultQualifiedModelID: String? {
        guard let defaultProfile,
              !defaultProfile.modelID.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
        else { return nil }
        return "\(defaultRuntimeProfileID ?? defaultProfile.id)/\(defaultProfile.modelID)"
    }

    /// Runtime model events use a qualified `profile/model` identifier while
    /// persisted settings store the two components separately. Compare both
    /// forms so a healthy runtime is not reported as a configuration mismatch.
    var runtimeMatchesDefault: Bool {
        guard let active = runtime.activeModelID,
              let defaultProfile,
              !defaultProfile.modelID.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
        else { return true }

        let parts = active.split(separator: "/", maxSplits: 1).map(String.init)
        let activeProfileID = runtime.activeProfileID ?? (parts.count == 2 ? parts[0] : nil)
        let activeModelID = parts.last ?? active
        let expectedProfileID = defaultRuntimeProfileID ?? defaultProfile.id
        return activeModelID == defaultProfile.modelID
            && (activeProfileID == nil || activeProfileID == expectedProfileID)
    }

    var isConfigured: Bool {
        defaultProfile != nil && enabledCount > 0
    }
}

struct ProviderProfileState: Identifiable, Equatable {
    private static let storedCredentialMask = "••••••••••••"

    var profile: ProviderStoredProfile
    var credentialState: ProviderCredentialState = .unknown
    var connectionState: ProviderConnectionState = .idle
    var detailMessage: String? = nil
    var pendingSecret: String = ""
    var clearCredentialOnApply = false
    var validationMessage: String? = nil
    var operationInFlight = false
    var hasLegacyAnthropicCredential = false
    var oauthState: ProviderOAuthState? = nil

    var id: String { profile.id }

    var hasPendingSecret: Bool {
        !pendingSecret.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
    }

    var hasStoredCredential: Bool {
        if clearCredentialOnApply {
            return false
        }
        return hasStoredAPIKey || oauthState?.signedIn == true
    }

    var hasStoredAPIKey: Bool {
        !clearCredentialOnApply && (credentialState == .configured || hasLegacyAnthropicCredential)
    }

    var effectiveHasCredential: Bool {
        hasPendingSecret || hasStoredCredential
    }

    /// The secure store never returns credential material to the settings UI.
    /// Render a fixed sentinel only when a stored credential is confirmed and
    /// there is no replacement draft that the user can reveal.
    var credentialFieldMask: String? {
        hasStoredAPIKey && !hasPendingSecret ? Self.storedCredentialMask : nil
    }

    var canRevealCredential: Bool {
        hasPendingSecret
    }

    var statusLabel: String {
        switch connectionState {
        case .testing:
            return String(localized: "settings_provider_status_testing")
        case .connected:
            return String(localized: "settings_provider_status_connected")
        case .failed:
            return String(localized: "settings_provider_status_failed")
        case .idle:
            if hasPendingSecret {
                return String(localized: "settings_provider_status_pending_apply")
            }
            if hasStoredCredential {
                return String(localized: "settings_provider_status_configured")
            }
            return String(localized: "settings_provider_unconfigured")
        }
    }

    var legacyStatus: ConnStatus {
        switch connectionState {
        case .testing: return .testing
        case .connected: return .connected
        case .failed: return .error
        case .idle:
            return hasStoredCredential ? .connected : .idle
        }
    }

    var maskedCredentialSummary: String {
        if clearCredentialOnApply {
            return String(localized: "settings_provider_credential_will_clear")
        }
        if hasPendingSecret {
            return String(localized: "settings_provider_credential_pending")
        }
        if hasLegacyAnthropicCredential {
            return String(localized: "settings_provider_credential_legacy_migrated")
        }
        if oauthState?.signedIn == true, hasStoredAPIKey {
            return String(localized: "settings_provider_credential_api_key_oauth")
        }
        if oauthState?.signedIn == true {
            return String(localized: "settings_provider_status_oauth")
        }
        if hasStoredCredential {
            return String(localized: "settings_provider_key_stored_securely")
        }
        return String(localized: "settings_provider_unconfigured")
    }
}

private struct ProviderPersistenceEnvelope: Codable, Equatable {
    var version: Int
    var profiles: [ProviderStoredProfile]
    var routing: ProviderRoutingSettings?
    var visionDelegationEnabled: Bool?
}

private enum PendingCredentialOperation: Equatable {
    case list(targets: [ProviderCredentialTarget])
    case set(target: ProviderCredentialTarget)
    case delete(target: ProviderCredentialTarget)

    var providerIDs: [String] {
        switch self {
        case .list(let targets): return targets.map(\.settingsID)
        case .set(let target), .delete(let target): return [target.settingsID]
        }
    }
}

private struct ProviderCredentialTarget: Equatable {
    let settingsID: String
    let credentialID: String
}

private struct ProviderPresetMetadata {
    let providerType: String
    let envVar: String
}

private enum ProviderRepositoryDefaults {
    static let anthropicLegacyProfileID = "anthropic"
    static let routingMobileEnabledProfilesKey = "mobileEnabledProfiles"
    static let builtInProfileIDsByPreset: [String: String] = [
        "anthropic": "anthropic",
        "openai": "openai",
        "openai-chatgpt": "openai-chatgpt",
        "deepseek": "deepseek",
        "kimi": "kimi",
        "kimi-code": "kimi-code",
        "glm-coding": "glm-coding",
        "zai": "zai",
        "github-copilot": "github-copilot",
        "openrouter": "openrouter",
        "gemini": "gemini",
    ]
    /// Stable conversational defaults. Engine catalogs are sorted by model id
    /// and may put image/audio or otherwise non-chat models first.
    static let preferredDefaultModelByPreset: [String: String] = [
        "anthropic": "claude-sonnet-5",
        "openai": "gpt-5.6-sol",
        "openai-chatgpt": "gpt-5.6-sol",
        "deepseek": "deepseek-v4-flash",
        "kimi": "kimi-k3",
        "kimi-code": "k3",
        "glm-coding": "glm-5.3",
        "zai": "glm-5.3",
        "openrouter": "openrouter/auto",
        "gemini": "gemini-3.7-flash",
    ]
}

@MainActor
@Observable
final class ProviderRepository {
    static let shared = ProviderRepository()

    private let persistenceURL: URL
    private let fileManager: FileManager
    private let credentialOperationTimeout: Duration
    private var pendingOperations: [UInt64: PendingCredentialOperation] = [:]
    private var pendingCompletions: [UInt64: CheckedContinuation<Void, Error>] = [:]
    private var pendingTimeouts: [UInt64: Task<Void, Never>] = [:]
    /// Draft transactions use the credential protocol without publishing a
    /// candidate profile to the live list until the command succeeds.
    private var suppressedCredentialStateUpdates: Set<UInt64> = []
    private var nextOperationID: UInt64 = 1
    private var commandSubmitter: ProviderCommandSubmitter?
    private var connectionTester: ProviderConnectionTester?
    private var oauthLoginHandler: ProviderOAuthLoginHandler?
    private var oauthStateLoader: ProviderOAuthStateLoader?
    private var oauthLogoutHandler: ProviderOAuthLogoutHandler?
    private var oauthConnectionTester: ProviderOAuthConnectionTester?
    private var catalogLoader: ProviderCatalogLoader?
    private var catalogGeneration: UInt64 = 0
    private var applyReconnectHandler: ProviderApplyReconnectHandler?
    private var lastAppliedRoutingSettings: ProviderRoutingSettings

    private(set) var profiles: [ProviderProfileState]
    private(set) var routingSettings: ProviderRoutingSettings
    private(set) var visionDelegationEnabled: Bool
    private(set) var storageEncrypted = true
    private(set) var lastRepositoryError: String? = nil
    private(set) var routingMessage: String? = nil
    private(set) var routingDirty = false
    private(set) var syncRevision = 0
    private(set) var runtimeSnapshot = ProviderRuntimeSnapshot()
    private(set) var catalogEntries: [ProviderCatalogEntry] = []

    init(
        persistenceURL: URL? = nil,
        fileManager: FileManager = .default,
        credentialOperationTimeout: Duration = .seconds(15)
    ) {
        let shouldMigrateLegacyCredential = persistenceURL == nil
        let resolvedPersistenceURL = persistenceURL ?? Self.defaultPersistenceURL()
        let loadedEnvelope = Self.loadEnvelope(from: resolvedPersistenceURL, fileManager: fileManager)
        let loadedProfiles = loadedEnvelope?.profiles ?? []
        let migratedProfiles = loadedProfiles.map(Self.migrateLegacyDeepSeekProfile)
        let normalizedOAuthProfiles = migratedProfiles.map(Self.normalizedOAuthProfile)
        let didMigrateDeepSeek = loadedProfiles != migratedProfiles
        let didNormalizeOAuthProfiles = migratedProfiles != normalizedOAuthProfiles
        let initialRoutingSettings = loadedEnvelope?.routing ?? ProviderRoutingSettings()
        let initialVisionDelegationEnabled = loadedEnvelope?.visionDelegationEnabled ?? true
        self.persistenceURL = resolvedPersistenceURL
        self.fileManager = fileManager
        self.credentialOperationTimeout = credentialOperationTimeout
        self.routingSettings = initialRoutingSettings
        self.lastAppliedRoutingSettings = initialRoutingSettings
        self.visionDelegationEnabled = initialVisionDelegationEnabled
        self.profiles = normalizedOAuthProfiles.map {
            ProviderProfileState(
                profile: $0,
                hasLegacyAnthropicCredential: false
            )
        }
        if shouldMigrateLegacyCredential,
           self.profiles.isEmpty,
           let legacyProfile = Self.legacyAnthropicProfile() {
            self.profiles = [legacyProfile]
        }
        normalizeDefaults()
        sanitizeRoutingSettings(persist: false)
        if didMigrateDeepSeek || didNormalizeOAuthProfiles {
            persistProfiles()
        }
    }

    func configure(
        submitCommand: ProviderCommandSubmitter?,
        testConnection: ProviderConnectionTester? = nil,
        applyReconnect: ProviderApplyReconnectHandler? = nil,
        oauthLogin: ProviderOAuthLoginHandler? = nil,
        oauthState: ProviderOAuthStateLoader? = nil,
        oauthLogout: ProviderOAuthLogoutHandler? = nil,
        testOAuthConnection: ProviderOAuthConnectionTester? = nil,
        providerCatalog: ProviderCatalogLoader? = nil,
        resetCatalog: Bool = true
    ) {
        if cancelPendingListOperations() {
            bumpSyncRevision()
        }
        if resetCatalog {
            catalogGeneration &+= 1
        }
        commandSubmitter = submitCommand
        connectionTester = testConnection
        applyReconnectHandler = applyReconnect
        oauthLoginHandler = oauthLogin
        oauthStateLoader = oauthState
        oauthLogoutHandler = oauthLogout
        oauthConnectionTester = testOAuthConnection
        catalogLoader = providerCatalog
        // A new engine/source may expose a different catalog. Do not keep the
        // previous runtime's entries visible while the new source is loading.
        if resetCatalog {
            catalogEntries = []
        }
        runtimeSnapshot = ProviderRuntimeSnapshot()
        bumpSyncRevision()
    }

    @discardableResult
    func refreshCatalog() async -> Bool {
        guard let catalogLoader else { return false }
        catalogGeneration &+= 1
        let requestGeneration = catalogGeneration
        catalogEntries = []
        do {
            let entries = try await catalogLoader()
            guard requestGeneration == catalogGeneration else { return false }
            catalogEntries = entries
            lastRepositoryError = nil
            bumpSyncRevision()
            return true
        } catch {
            guard requestGeneration == catalogGeneration else { return false }
            lastRepositoryError = error.localizedDescription
            bumpSyncRevision()
            return false
        }
    }

    var catalogPresets: [ProviderPreset] {
        let entries = catalogEntries
        // Once the engine catalog loader is configured, an empty result means
        // the engine is unavailable (or has no built-ins). Do not resurrect a
        // second, potentially stale iOS catalog in that state; custom URLs
        // remain available and saved/legacy profiles are still rendered.
        if catalogLoader != nil, entries.isEmpty {
            return Presets.llm.filter { $0.id == "custom" }
        }
        guard !entries.isEmpty else { return Presets.llm }
        let fallback = Dictionary(uniqueKeysWithValues: Presets.llm.map { ($0.id, $0) })
        return entries.map { entry in
            let base = fallback[entry.id] ?? ProviderPreset(
                id: entry.id,
                name: entry.displayName,
                sub: entry.protocolName,
                color: Accents.color(for: entry.id),
                defaultUrl: entry.baseURL,
                keyPrefix: "",
                models: entry.models
            )
            return ProviderPreset(
                id: entry.id,
                name: entry.displayName,
                sub: base.sub,
                color: base.color,
                defaultUrl: entry.baseURL,
                keyPrefix: base.keyPrefix,
                models: entry.models,
                needsCx: base.needsCx
            )
        } + (fallback["custom"].map { [$0] } ?? [])
    }

    /// The single projection consumed by settings UI.  It intentionally joins
    /// persisted configuration with the engine's live status without mutating
    /// either source of truth.
    var settingsSummary: ProviderSettingsSummary {
        let defaultProfile = profiles.first(where: { $0.profile.isDefault && $0.profile.enabled })?.profile
            ?? profiles.first(where: { $0.profile.isDefault })?.profile
            ?? profiles.first?.profile
        return ProviderSettingsSummary(
            defaultProfile: defaultProfile,
            enabledCount: profiles.filter { $0.profile.enabled }.count,
            totalCount: profiles.count,
            runtime: runtimeSnapshot,
            defaultRuntimeProfileID: defaultProfile.map { engineProfileID(for: $0) }
        )
    }

    func updateRuntimeSnapshot(models: [String], activeModelID: String?, activeProfileID: String? = nil, error: String? = nil) {
        runtimeSnapshot = ProviderRuntimeSnapshot(
            models: models,
            activeModelID: activeModelID,
            activeProfileID: activeProfileID,
            lastError: error
        )
        bumpSyncRevision()
    }

    func makeDraft(for id: String) -> ProviderEditorDraft? {
        guard let state = state(for: id) else { return nil }
        return ProviderEditorDraft(
            id: state.id,
            isNew: false,
            profile: state.profile,
            credentialState: state.credentialState,
            hasLegacyAnthropicCredential: state.hasLegacyAnthropicCredential,
            oauthState: state.oauthState,
            pendingSecret: state.pendingSecret,
            clearCredentialOnApply: state.clearCredentialOnApply,
            connectionState: state.connectionState,
            detailMessage: state.detailMessage,
            validationMessage: state.validationMessage,
            operationInFlight: state.operationInFlight
        )
    }

    func makeNewDraft(presetID: String) -> ProviderEditorDraft {
        let preset = preset(for: presetID)
        let id = nextProfileID(for: presetID)
        let suffix = id.replacingOccurrences(of: "\(presetID)-", with: "")
        let name = id == presetID ? preset.name : "\(preset.name) \(suffix)"
        let defaultModel = ProviderRepositoryDefaults.preferredDefaultModelByPreset[presetID]
            .flatMap { preset.models.contains($0) ? $0 : nil }
            ?? preset.models.first
            ?? ""
        return ProviderEditorDraft(
            id: id,
            isNew: true,
            profile: ProviderStoredProfile(
                id: id,
                presetID: presetID,
                name: name,
                baseURL: preset.defaultUrl,
                modelID: defaultModel,
                enabled: true,
                isDefault: profiles.isEmpty
            ),
            credentialState: .unknown,
            hasLegacyAnthropicCredential: false,
            oauthState: nil
        )
    }

    /// Test a draft without publishing it to the repository.
    func testConnection(for draft: ProviderEditorDraft) async -> ProviderEditorDraft {
        var updated = draft
        guard !draft.operationInFlight,
              !(state(for: draft.id)?.operationInFlight ?? false)
        else { return draft }
        updated.connectionState = .testing
        updated.operationInFlight = true
        updated.detailMessage = nil
        updated.validationMessage = nil
        do {
            if updated.clearCredentialOnApply && !updated.hasPendingSecret {
                throw ProviderRepositoryOperationError.failed(
                    String(localized: "settings_provider_credential_pending_clear")
                )
            }
            let state = state(from: updated)
            let launchProfile = try validateAndBuildLaunchProfile(for: state)
            let result: ProviderConnectionTestResult
            if let provider = oauthProvider(for: updated.profile.presetID),
               (updated.profile.presetID == "openai-chatgpt"
                || (updated.oauthState?.signedIn == true
                    && !updated.hasStoredAPIKey
                    && !updated.hasPendingSecret)) {
                guard let oauthConnectionTester else {
                    throw ProviderRepositoryOperationError.failed(
                        String(localized: "settings_provider_oauth_unavailable")
                    )
                }
                result = try await oauthConnectionTester(provider, launchProfile)
            } else {
                guard let connectionTester else {
                    throw ProviderRepositoryOperationError.failed(
                        String(localized: "settings_provider_test_callback_unavailable")
                    )
                }
                result = try await connectionTester(
                    launchProfile,
                    updated.hasPendingSecret ? updated.pendingSecret.trimmingCharacters(in: .whitespacesAndNewlines) : nil
                )
            }
            updated.operationInFlight = false
            switch result {
            case .success(let message, _):
                updated.connectionState = .connected
                updated.detailMessage = message ?? String(localized: "settings_provider_test_success")
            case .failure(let message):
                updated.connectionState = .failed
                updated.detailMessage = message
            }
        } catch let error as ProviderProfileValidationError {
            updated.operationInFlight = false
            updated.connectionState = .failed
            updated.validationMessage = error.errorDescription
        } catch {
            updated.operationInFlight = false
            updated.connectionState = .failed
            updated.detailMessage = error.localizedDescription
        }
        return updated
    }

    func loginOAuth(for draft: ProviderEditorDraft) async -> ProviderEditorDraft {
        var updated = draft
        guard !draft.operationInFlight,
              !(state(for: draft.id)?.operationInFlight ?? false)
        else { return draft }
        guard oauthLoginAvailable(for: draft.profile.presetID),
              let provider = oauthProvider(for: draft.profile.presetID),
              let oauthLoginHandler
        else {
            updated.connectionState = .failed
            updated.detailMessage = String(localized: "settings_provider_oauth_unavailable")
            return updated
        }
        updated.operationInFlight = true
        if let index = indexOfProfile(id: draft.id) {
            profiles[index].operationInFlight = true
            profiles[index].connectionState = .testing
            bumpSyncRevision()
        }
        do {
            updated.oauthState = try await oauthLoginHandler(provider)
            updated.operationInFlight = false
            updated.connectionState = .idle
            updated.detailMessage = nil
            syncDraftOAuthState(updated)
        } catch {
            updated.operationInFlight = false
            updated.connectionState = .failed
            updated.detailMessage = error.localizedDescription
            if let index = indexOfProfile(id: draft.id) {
                profiles[index].operationInFlight = false
                profiles[index].connectionState = .failed
                profiles[index].detailMessage = error.localizedDescription
                bumpSyncRevision()
            }
        }
        return updated
    }

    func logoutOAuth(for draft: ProviderEditorDraft) async -> ProviderEditorDraft {
        var updated = draft
        guard !draft.operationInFlight,
              !(state(for: draft.id)?.operationInFlight ?? false)
        else { return draft }
        guard let provider = oauthProvider(for: draft.profile.presetID), let oauthLogoutHandler else {
            updated.connectionState = .failed
            updated.detailMessage = String(localized: "settings_provider_oauth_unavailable")
            return updated
        }
        updated.operationInFlight = true
        if let index = indexOfProfile(id: draft.id) {
            profiles[index].operationInFlight = true
            profiles[index].connectionState = .testing
            bumpSyncRevision()
        }
        do {
            try await oauthLogoutHandler(provider)
            updated.oauthState = nil
            updated.operationInFlight = false
            updated.connectionState = .idle
            syncDraftOAuthState(updated)
        } catch {
            updated.operationInFlight = false
            updated.connectionState = .failed
            updated.detailMessage = error.localizedDescription
            if let index = indexOfProfile(id: draft.id) {
                profiles[index].operationInFlight = false
                profiles[index].connectionState = .failed
                profiles[index].detailMessage = error.localizedDescription
                bumpSyncRevision()
            }
        }
        return updated
    }

    /// Commits one draft as a profile transaction followed by a secure-store
    /// transaction. A draft is never persisted until validation succeeds;
    /// credential material still travels exclusively through the secure-store
    /// command path.
    @discardableResult
    func applyDraft(_ draft: ProviderEditorDraft) async -> Bool {
        guard !draft.operationInFlight,
              !(state(for: draft.id)?.operationInFlight ?? false)
        else {
            lastRepositoryError = String(localized: "settings_provider_operation_in_progress")
            bumpSyncRevision()
            return false
        }
        var normalizedDraft = draft
        // Entering a replacement secret always wins over a previously queued
        // delete. This also protects non-UI callers that construct drafts
        // directly instead of using the editor's toggle binding.
        if normalizedDraft.hasPendingSecret {
            normalizedDraft.clearCredentialOnApply = false
        }
        if normalizedDraft.clearCredentialOnApply {
            let keepsOAuth = normalizedDraft.oauthState?.signedIn == true
            normalizedDraft.profile.enabled = keepsOAuth
            normalizedDraft.profile.isDefault = keepsOAuth
        }
        let previousState = state(for: normalizedDraft.id)
        var persistedProfile: ProviderStoredProfile?
        var didPersistProfiles = false
        var didStartCredentialOperation = false
        var credentialCommitted = false
        do {
            if normalizedDraft.isNew {
                profiles.append(state(from: normalizedDraft))
            } else if let index = indexOfProfile(id: normalizedDraft.id) {
                profiles[index] = state(from: normalizedDraft)
            } else {
                throw ProviderRepositoryOperationError.failed(String(localized: "settings_provider_edit_unavailable"))
            }
            normalizeDefaults()

            guard let currentState = state(for: normalizedDraft.id) else {
                throw ProviderRepositoryOperationError.failed(String(localized: "settings_provider_edit_unavailable"))
            }
            persistedProfile = currentState.profile
            let launchProfile = try validateAndBuildLaunchProfile(
                for: currentState,
                allowDisabledWithoutCredential: true
            )

            // Commit the non-secret profile first. If secure storage rejects
            // the operation, the target row can be restored without touching
            // any other profile that may have changed during the await.
            guard persistProfiles() else {
                throw ProviderRepositoryOperationError.failed(
                    lastRepositoryError ?? String(localized: "settings_provider_save_failed")
                )
            }
            didPersistProfiles = true

            if normalizedDraft.clearCredentialOnApply {
                didStartCredentialOperation = true
                try await deleteCredentialIfPossible(
                    settingsID: launchProfile.settingsID,
                    credentialID: launchProfile.id,
                    publishState: false
                )
                credentialCommitted = true
                if let index = indexOfProfile(id: normalizedDraft.id) {
                    profiles[index].credentialState = .missing
                    profiles[index].hasLegacyAnthropicCredential = false
                    profiles[index].pendingSecret = ""
                    profiles[index].clearCredentialOnApply = false
                }
            } else if let secret = effectiveSecret(for: currentState) {
                didStartCredentialOperation = true
                try await storeCredentialIfPossible(
                    secret,
                    settingsID: launchProfile.settingsID,
                    credentialID: launchProfile.id,
                    publishState: false
                )
                credentialCommitted = true
                if let index = indexOfProfile(id: normalizedDraft.id) {
                    profiles[index].credentialState = .configured
                    profiles[index].pendingSecret = ""
                    profiles[index].clearCredentialOnApply = false
                    if profiles[index].profile.presetID == ProviderRepositoryDefaults.anthropicLegacyProfileID {
                        profiles[index].hasLegacyAnthropicCredential = true
                    }
                }
            }

            // The profile and secure credential now agree. Keep the legacy
            // compatibility mirror aligned before reconnecting the engine.
            mirrorLegacyAnthropicSettingsIfNeeded(
                for: launchProfile,
                state: state(from: normalizedDraft)
            )
            if let applyReconnectHandler {
                try await applyReconnectHandler(makeLaunchSnapshot())
                if let index = indexOfProfile(id: normalizedDraft.id) {
                    profiles[index].detailMessage = String(localized: "settings_provider_applied_reconnect")
                }
                lastAppliedRoutingSettings = routingSettings
                routingDirty = false
            } else if let index = indexOfProfile(id: normalizedDraft.id) {
                profiles[index].detailMessage = String(localized: "settings_provider_saved")
            }
            bumpSyncRevision()
            return true
        } catch let error as ProviderProfileValidationError {
            if (!didPersistProfiles || (didStartCredentialOperation && !credentialCommitted)), let persistedProfile {
                _ = rollbackDraft(
                    id: normalizedDraft.id,
                    previousState: previousState,
                    expectedProfile: persistedProfile
                )
            }
            lastRepositoryError = error.errorDescription
            bumpSyncRevision()
            return false
        } catch {
            if (!didPersistProfiles || (didStartCredentialOperation && !credentialCommitted)), let persistedProfile {
                _ = rollbackDraft(
                    id: normalizedDraft.id,
                    previousState: previousState,
                    expectedProfile: persistedProfile
                )
            } else if didPersistProfiles, let index = indexOfProfile(id: normalizedDraft.id) {
                profiles[index].connectionState = .failed
                profiles[index].detailMessage = error.localizedDescription
            }
            lastRepositoryError = error.localizedDescription
            bumpSyncRevision()
            return false
        }
    }

    func discardDraft(_ draft: ProviderEditorDraft) {
        // Drafts are value types, so discarding is intentionally a no-op. This
        // method documents the lifecycle and gives callers one stable hook.
    }

    func handle(event: ClientEvent) {
        switch event {
        case .modelList(let models, let current, _):
            updateRuntimeSnapshot(
                models: models,
                activeModelID: current,
                activeProfileID: profileID(fromQualifiedModel: current)
            )
            return
        case .modelChanged(let model):
            runtimeSnapshot.activeModelID = model
            runtimeSnapshot.activeProfileID = profileID(fromQualifiedModel: model)
            runtimeSnapshot.lastError = nil
            bumpSyncRevision()
            return
        case .error(_, _):
            // Turn errors are rendered by ConversationSource. They are not
            // provider-configuration state and must not leave a stale red
            // banner in the Provider settings screen.
            return
        case .systemNotice(let message, let isError):
            if isError {
                runtimeSnapshot.lastError = message
                bumpSyncRevision()
            }
            return
        default:
            break
        }
        guard case let .providerCredentialStatus(
            operationId,
            configuredProviderIds,
            unavailableProviderIds,
            storageEncrypted,
            _,
            error
        ) = event else {
            return
        }
        guard let operation = pendingOperations.removeValue(forKey: operationId) else {
            return
        }
        pendingTimeouts.removeValue(forKey: operationId)?.cancel()
        let publishCredentialState = suppressedCredentialStateUpdates.remove(operationId) == nil
        self.storageEncrypted = storageEncrypted
        let configured = Set(configuredProviderIds)
        let unavailable = Set(unavailableProviderIds)
        let operationFailure: String? = {
            if let error { return error }
            switch operation {
            case .list:
                return nil
            case .set(let target):
                if unavailable.contains(target.credentialID) { return String(localized: "settings_provider_secure_storage_unavailable") }
                if !configured.contains(target.credentialID) {
                    return String(localized: "settings_provider_secure_storage_not_confirmed")
                }
                return nil
            case .delete(let target):
                if unavailable.contains(target.credentialID) { return String(localized: "settings_provider_secure_storage_unavailable") }
                if configured.contains(target.credentialID) {
                    return String(localized: "settings_provider_secure_storage_still_present")
                }
                return nil
            }
        }()

        switch operation {
        case .list(let targets):
            for target in targets {
                guard let index = indexOfProfile(id: target.settingsID) else { continue }
                if unavailable.contains(target.credentialID) {
                    continue
                }
                profiles[index].credentialState = configured.contains(target.credentialID) ? .configured : .missing
                if configured.contains(target.credentialID), profiles[index].connectionState == .idle {
                    profiles[index].detailMessage = nil
                }
            }
        case .set(let target):
            guard publishCredentialState else { break }
            guard let index = indexOfProfile(id: target.settingsID) else { break }
            if let operationFailure {
                profiles[index].connectionState = .failed
                profiles[index].detailMessage = operationFailure
            } else {
                profiles[index].credentialState = .configured
                profiles[index].pendingSecret = ""
                profiles[index].clearCredentialOnApply = false
                profiles[index].detailMessage = String(localized: "settings_provider_credential_saved")
            }
        case .delete(let target):
            guard publishCredentialState else { break }
            guard let index = indexOfProfile(id: target.settingsID) else { break }
            if let operationFailure {
                profiles[index].connectionState = .failed
                profiles[index].detailMessage = operationFailure
            } else {
                profiles[index].credentialState = .missing
                profiles[index].pendingSecret = ""
                profiles[index].clearCredentialOnApply = false
                profiles[index].hasLegacyAnthropicCredential = false
                profiles[index].detailMessage = String(localized: "settings_provider_credential_removed")
            }
        }

        clearOperationInFlightIfFinished(for: operation)

        lastRepositoryError = operationFailure
        if let continuation = pendingCompletions.removeValue(forKey: operationId) {
            if let message = operationFailure {
                continuation.resume(throwing: ProviderRepositoryOperationError.failed(message))
            } else {
                continuation.resume()
            }
        }
        bumpSyncRevision()
    }

    func legacyProviders() -> [GenericProvider] {
        profiles.map { state in
            return GenericProvider(
                id: state.id,
                preset: state.profile.presetID,
                name: state.profile.name,
                url: state.profile.baseURL,
                key: "",
                model: state.profile.modelID,
                status: state.legacyStatus,
                isDefault: state.profile.isDefault,
                enabled: state.profile.enabled
            )
        }
        .sorted { lhs, rhs in
            if lhs.isDefault != rhs.isDefault {
                return lhs.isDefault && !rhs.isDefault
            }
            return lhs.name.localizedStandardCompare(rhs.name) == .orderedAscending
        }
    }

    func state(for id: String) -> ProviderProfileState? {
        profiles.first(where: { $0.id == id })
    }

    func preset(for presetID: String) -> ProviderPreset {
        if let catalog = catalogEntries.first(where: { $0.id == presetID }) {
            let fallback = Presets.llm.first(where: { $0.id == presetID })
            return ProviderPreset(
                id: catalog.id,
                name: catalog.displayName,
                sub: fallback?.sub ?? catalog.protocolName,
                color: fallback?.color ?? Accents.color(for: catalog.id),
                defaultUrl: catalog.baseURL,
                keyPrefix: fallback?.keyPrefix ?? "",
                models: catalog.models,
                needsCx: fallback?.needsCx ?? false
            )
        }
        return Presets.llm.first(where: { $0.id == presetID }) ?? Presets.llm[Presets.llm.count - 1]
    }

    func isOfficialEndpoint(for profile: ProviderStoredProfile) -> Bool {
        guard profile.presetID != "custom" else { return false }
        let preset = preset(for: profile.presetID)
        return profile.baseURL.trimmingCharacters(in: .whitespacesAndNewlines)
            .caseInsensitiveCompare(preset.defaultUrl) == .orderedSame
    }

    /// OAuth providers whose LOGIN entry point is withheld from the UI.
    ///
    /// Anthropic's "Authentication and credential use" policy reserves
    /// claude.ai OAuth for Claude Code and claude.ai themselves, so LingXi does
    /// not offer it as a sign-in method. Everything behind it is intact — the
    /// engine coordinator, the uniffi surface, PKCE, the token exchange, the
    /// refresh driver — and it is re-enabled by removing the id from this set.
    /// No other change is needed.
    ///
    /// Only LOGIN is withheld. An account that signed in before this gate keeps
    /// its status row and its Logout button, so an existing credential stays
    /// visible and removable rather than becoming an orphan in the keychain
    /// that nothing in the UI can reach.
    static let hiddenOAuthLoginProviders: Set<String> = ["anthropic"]

    /// Whether the UI may offer an OAuth sign-in for this preset.
    func oauthLoginAvailable(for presetID: String) -> Bool {
        guard let provider = oauthProvider(for: presetID) else { return false }
        return !Self.hiddenOAuthLoginProviders.contains(provider)
    }

    func oauthProvider(for presetID: String) -> String? {
        switch presetID {
        case "anthropic": return "anthropic"
        case "openai-chatgpt": return "openai-chatgpt"
        default: return nil
        }
    }

    func fallbackCandidates() -> [ProviderFallbackCandidate] {
        let orderByProfileID = Dictionary(uniqueKeysWithValues: routingSettings.fallbackProfileIDs.enumerated().map { ($1, $0) })
        return eligibleFallbackProfiles().map { profile in
            ProviderFallbackCandidate(
                profileID: profile.id,
                name: profile.name,
                modelID: profile.modelID,
                selected: orderByProfileID[profile.id] != nil,
                order: orderByProfileID[profile.id]
            )
        }
    }

    func setRetryMaxAttempts(_ value: Int) {
        routingSettings.retryMaxAttempts = clampedRetryMaxAttempts(value)
        sanitizeRoutingSettings()
    }

    func setRetryBackoffMs(_ value: Int) {
        routingSettings.retryBackoffMs = clampedRetryBackoffMs(value)
        sanitizeRoutingSettings()
    }

    func setVisionDelegationEnabled(_ enabled: Bool) {
        guard visionDelegationEnabled != enabled else { return }
        visionDelegationEnabled = enabled
        _ = persistProfiles()
        Task {
            try? await applyReconnectHandler?(makeLaunchSnapshot())
        }
    }

    func toggleFallbackProfile(_ profileID: String) {
        if let index = routingSettings.fallbackProfileIDs.firstIndex(of: profileID) {
            routingSettings.fallbackProfileIDs.remove(at: index)
        } else if eligibleFallbackProfiles().contains(where: { $0.id == profileID }) {
            routingSettings.fallbackProfileIDs.append(profileID)
        }
        sanitizeRoutingSettings()
    }

    func moveFallbackProfile(_ profileID: String, by delta: Int) {
        guard let index = routingSettings.fallbackProfileIDs.firstIndex(of: profileID) else { return }
        let targetIndex = index + delta
        guard routingSettings.fallbackProfileIDs.indices.contains(targetIndex) else { return }
        routingSettings.fallbackProfileIDs.swapAt(index, targetIndex)
        sanitizeRoutingSettings()
    }

    func addProfile(presetID: String) -> String {
        let preset = preset(for: presetID)
        let newID = nextProfileID(for: presetID)
        let defaultName: String
        if newID == presetID {
            defaultName = preset.name
        } else {
            let suffix = newID.replacingOccurrences(of: "\(presetID)-", with: "")
            defaultName = "\(preset.name) \(suffix)"
        }
        let profile = ProviderStoredProfile(
            id: newID,
            presetID: presetID,
            name: defaultName,
            baseURL: preset.defaultUrl,
            modelID: preset.models.first ?? "",
            enabled: true,
            isDefault: profiles.isEmpty
        )
        profiles.append(
            ProviderProfileState(
                profile: profile,
                hasLegacyAnthropicCredential: false
            )
        )
        normalizeDefaults()
        persistProfiles()
        return newID
    }

    func updateProfile(_ id: String, mutate: (inout ProviderStoredProfile) -> Void) {
        guard let index = indexOfProfile(id: id) else { return }
        mutate(&profiles[index].profile)
        profiles[index].profile = Self.normalizedOAuthProfile(profiles[index].profile)
        profiles[index].validationMessage = nil
        if profiles[index].profile.isDefault, !profiles[index].profile.enabled {
            profiles[index].profile.isDefault = false
        }
        normalizeDefaults()
        sanitizeRoutingSettings(persist: false)
        persistProfiles()
    }

    func setDefaultProfile(_ id: String) {
        guard profiles.contains(where: { $0.id == id }) else { return }
        for index in profiles.indices {
            profiles[index].profile.isDefault = profiles[index].id == id
            if profiles[index].profile.isDefault {
                profiles[index].profile.enabled = true
            }
        }
        sanitizeRoutingSettings(persist: false)
        persistProfiles()
    }

    func stageSecret(_ secret: String, for id: String) {
        guard let index = indexOfProfile(id: id) else { return }
        profiles[index].pendingSecret = secret
        if !secret.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
            profiles[index].clearCredentialOnApply = false
        }
        profiles[index].validationMessage = nil
        profiles[index].detailMessage = nil
        bumpSyncRevision()
    }

    func clearCredentialRequest(for id: String) {
        guard let index = indexOfProfile(id: id) else { return }
        profiles[index].pendingSecret = ""
        profiles[index].clearCredentialOnApply = true
        profiles[index].validationMessage = nil
        profiles[index].detailMessage = String(localized: "settings_provider_credential_clear_scheduled")
        bumpSyncRevision()
    }

    func cancelCredentialClear(for id: String) {
        guard let index = indexOfProfile(id: id) else { return }
        profiles[index].clearCredentialOnApply = false
        profiles[index].detailMessage = nil
        bumpSyncRevision()
    }

    func discardCredentialChanges(for id: String) {
        guard let index = indexOfProfile(id: id) else { return }
        profiles[index].pendingSecret = ""
        profiles[index].clearCredentialOnApply = false
        profiles[index].validationMessage = nil
        profiles[index].detailMessage = nil
        profiles[index].connectionState = .idle
        bumpSyncRevision()
    }

    func refreshCredentialStatus() async {
        await refreshOAuthStatus()
        // Anthropic supports both credential kinds. ChatGPT OAuth is a separate
        // provider and intentionally has no API-key slot to query.
        let targets = profiles.filter { $0.profile.presetID != "openai-chatgpt" }.map { state in
            ProviderCredentialTarget(
                settingsID: state.id,
                credentialID: engineProfileID(for: state.profile)
            )
        }
        guard !targets.isEmpty else {
            lastRepositoryError = nil
            return
        }
        guard let commandSubmitter else {
            return
        }
        cancelPendingListOperations()
        let operationID = takeOperationID()
        pendingOperations[operationID] = .list(targets: targets)
        setOperationInFlight(true, for: targets.map(\.settingsID))
        scheduleCredentialOperationTimeout(operationID)
        do {
            let credentialIDs = targets.reduce(into: [String]()) { result, target in
                if !result.contains(target.credentialID) {
                    result.append(target.credentialID)
                }
            }
            // `previewProviderIds` is an opt-in subset of `providerIds` for
            // which the engine returns a MASKED credential preview
            // (`engine-mobile/src/host.rs:9768`). iOS surfaces no preview yet,
            // so it asks for none — the same default Electron's wrapper uses
            // (`clients/electron/src/main/bridge.ts:1165`). Empty here is the
            // behaviour this call had before the parameter existed, not a
            // stub: an id listed for preview and not for status is rejected.
            try await commandSubmitter(
                .listProviderCredentials(
                    operationId: operationID, providerIds: credentialIDs, previewProviderIds: []))
        } catch {
            failCredentialOperation(operationID, error: error)
            return
        }
        bumpSyncRevision()
    }

    private func refreshOAuthStatus() async {
        guard let oauthStateLoader else { return }
        let targets = profiles.compactMap { state -> (String, String)? in
            guard let provider = oauthProvider(for: state.profile.presetID) else { return nil }
            return (state.id, provider)
        }
        for (settingsID, provider) in targets {
            do {
                let state = try await oauthStateLoader(provider)
                guard let index = indexOfProfile(id: settingsID) else { continue }
                profiles[index].oauthState = state
                if state.signedIn {
                    profiles[index].detailMessage = nil
                }
            } catch {
                guard let index = indexOfProfile(id: settingsID) else { continue }
                profiles[index].oauthState = nil
                profiles[index].detailMessage = error.localizedDescription
            }
        }
        bumpSyncRevision()
    }

    func testConnection(_ id: String) async {
        guard let index = indexOfProfile(id: id) else { return }
        if let provider = oauthProvider(for: profiles[index].profile.presetID),
           profiles[index].oauthState?.signedIn == true,
           (profiles[index].profile.presetID == "openai-chatgpt" || !profiles[index].hasStoredAPIKey),
           !profiles[index].hasPendingSecret {
            await testOAuthConnection(id, provider: provider)
            return
        }
        do {
            let launchProfile = try validateAndBuildLaunchProfile(for: profiles[index])
            profiles[index].validationMessage = nil
            profiles[index].connectionState = .testing
            profiles[index].detailMessage = nil
            bumpSyncRevision()
            guard let connectionTester else {
                profiles[index].connectionState = .idle
                profiles[index].detailMessage = String(localized: "settings_provider_test_callback_unavailable")
                bumpSyncRevision()
                return
            }
            let result = try await connectionTester(
                launchProfile,
                profiles[index].hasPendingSecret
                    ? profiles[index].pendingSecret.trimmingCharacters(in: .whitespacesAndNewlines)
                    : nil
            )
            guard let currentIndex = indexOfProfile(id: id) else { return }
            switch result {
            case .success(let message, let usedStoredCredential):
                profiles[currentIndex].connectionState = .connected
                let successMessage = message ?? String(localized: "settings_provider_test_success")
                let unsavedSuffix = usedStoredCredential
                    ? ""
                    : String(localized: "settings_provider_key_unsaved_suffix")
                profiles[currentIndex].detailMessage = successMessage + unsavedSuffix
            case .failure(let message):
                profiles[currentIndex].connectionState = .failed
                profiles[currentIndex].detailMessage = message
            }
        } catch let error as ProviderProfileValidationError {
            if let currentIndex = indexOfProfile(id: id) {
                profiles[currentIndex].connectionState = .failed
                profiles[currentIndex].validationMessage = error.errorDescription
            }
        } catch {
            if let currentIndex = indexOfProfile(id: id) {
                profiles[currentIndex].connectionState = .failed
                profiles[currentIndex].detailMessage = error.localizedDescription
            }
        }
        bumpSyncRevision()
    }

    func loginOAuth(for id: String) async {
        guard let index = indexOfProfile(id: id),
              oauthLoginAvailable(for: profiles[index].profile.presetID),
              let provider = oauthProvider(for: profiles[index].profile.presetID),
              let oauthLoginHandler
        else { return }
        profiles[index].operationInFlight = true
        profiles[index].connectionState = .testing
        profiles[index].detailMessage = nil
        do {
            let state = try await oauthLoginHandler(provider)
            guard let currentIndex = indexOfProfile(id: id) else { return }
            profiles[currentIndex].oauthState = state
            profiles[currentIndex].profile.enabled = true
            profiles[currentIndex].operationInFlight = false
            profiles[currentIndex].connectionState = .idle
            persistProfiles()
            try await applyReconnectHandler?(makeLaunchSnapshot())
            // Every `await` in this @MainActor class is a reentrancy point: a
            // `removeProfile` that lands while the reconnect is in flight shifts
            // — or deletes — the row `currentIndex` was resolved against, so
            // writing through the stale index tags the WRONG profile or traps
            // out of bounds. Re-resolve, exactly as the catch arms below do.
            guard let reconnectedIndex = indexOfProfile(id: id) else {
                bumpSyncRevision()
                return
            }
            profiles[reconnectedIndex].detailMessage = String(localized: "settings_provider_oauth_login_success")
        } catch {
            if let currentIndex = indexOfProfile(id: id) {
                profiles[currentIndex].operationInFlight = false
                profiles[currentIndex].connectionState = .failed
                profiles[currentIndex].detailMessage = error.localizedDescription
            }
            lastRepositoryError = error.localizedDescription
        }
        bumpSyncRevision()
    }

    func logoutOAuth(for id: String) async {
        guard let index = indexOfProfile(id: id),
              let provider = oauthProvider(for: profiles[index].profile.presetID),
              let oauthLogoutHandler
        else { return }
        profiles[index].operationInFlight = true
        do {
            try await oauthLogoutHandler(provider)
            guard let currentIndex = indexOfProfile(id: id) else { return }
            profiles[currentIndex].oauthState = nil
            profiles[currentIndex].operationInFlight = false
            profiles[currentIndex].connectionState = .idle
            persistProfiles()
            try await applyReconnectHandler?(makeLaunchSnapshot())
            // Re-resolve after the await: the row may have been removed or
            // reordered while the reconnect was in flight (see `loginOAuth`).
            guard let reconnectedIndex = indexOfProfile(id: id) else {
                bumpSyncRevision()
                return
            }
            profiles[reconnectedIndex].detailMessage = String(localized: "settings_provider_oauth_logout_success")
        } catch {
            if let currentIndex = indexOfProfile(id: id) {
                profiles[currentIndex].operationInFlight = false
                profiles[currentIndex].connectionState = .failed
                profiles[currentIndex].detailMessage = error.localizedDescription
            }
            lastRepositoryError = error.localizedDescription
        }
        bumpSyncRevision()
    }

    private func testOAuthConnection(_ id: String, provider: String) async {
        guard let index = indexOfProfile(id: id) else { return }
        profiles[index].connectionState = .testing
        profiles[index].detailMessage = nil
        do {
            guard let oauthConnectionTester else {
                throw ProviderRepositoryOperationError.failed(
                    String(localized: "settings_provider_oauth_test_unavailable")
                )
            }
            let profile = try validateAndBuildLaunchProfile(for: profiles[index])
            let result = try await oauthConnectionTester(provider, profile)
            guard let currentIndex = indexOfProfile(id: id) else { return }
            switch result {
            case .success(let message, _):
                profiles[currentIndex].connectionState = .connected
                profiles[currentIndex].detailMessage = message ?? String(localized: "settings_provider_oauth_test_success")
            case .failure(let message):
                profiles[currentIndex].connectionState = .failed
                profiles[currentIndex].detailMessage = message
            }
        } catch {
            if let currentIndex = indexOfProfile(id: id) {
                profiles[currentIndex].connectionState = .failed
                profiles[currentIndex].detailMessage = error.localizedDescription
            }
        }
        bumpSyncRevision()
    }

    func applyChanges(_ id: String) async {
        guard let index = indexOfProfile(id: id) else { return }
        do {
            let isClearingCredential = profiles[index].clearCredentialOnApply
            let launchProfile = try validateAndBuildLaunchProfile(for: profiles[index], allowDisabledWithoutCredential: true)
            profiles[index].validationMessage = nil
            persistProfiles()
            mirrorLegacyAnthropicSettingsIfNeeded(for: launchProfile, state: profiles[index])

            if isClearingCredential {
                try await deleteCredentialIfPossible(
                    settingsID: launchProfile.settingsID,
                    credentialID: launchProfile.id
                )
                if let updatedIndex = indexOfProfile(id: id) {
                    let keepsOAuth = profiles[updatedIndex].oauthState?.signedIn == true
                    profiles[updatedIndex].profile.enabled = keepsOAuth
                    profiles[updatedIndex].profile.isDefault = keepsOAuth
                    normalizeDefaults()
                    persistProfiles()
                }
            } else if let secret = effectiveSecret(for: profiles[index]) {
                try await storeCredentialIfPossible(
                    secret,
                    settingsID: launchProfile.settingsID,
                    credentialID: launchProfile.id
                )
            }

            if let applyReconnectHandler {
                try await applyReconnectHandler(makeLaunchSnapshot())
                if let currentIndex = indexOfProfile(id: id) {
                    profiles[currentIndex].detailMessage = String(localized: "settings_provider_applied_reconnect")
                }
                lastAppliedRoutingSettings = routingSettings
                routingDirty = false
            } else if let currentIndex = indexOfProfile(id: id),
                      profiles[currentIndex].detailMessage == nil {
                profiles[currentIndex].detailMessage = String(localized: "settings_provider_saved")
            }
            if let currentIndex = indexOfProfile(id: id) {
                profiles[currentIndex].connectionState = .idle
            }
        } catch let error as ProviderProfileValidationError {
            if let currentIndex = indexOfProfile(id: id) {
                profiles[currentIndex].connectionState = .failed
                profiles[currentIndex].validationMessage = error.errorDescription
            }
        } catch {
            if let currentIndex = indexOfProfile(id: id) {
                profiles[currentIndex].connectionState = .failed
                profiles[currentIndex].detailMessage = error.localizedDescription
            }
            lastRepositoryError = error.localizedDescription
        }
        bumpSyncRevision()
    }

    @discardableResult
    func applyRoutingChanges(
        retryMaxAttemptsText: String,
        retryBackoffMsText: String
    ) async -> Bool {
        let previousRoutingSettings = routingSettings
        let previousRoutingMessage = routingMessage
        var didPersist = false
        do {
            let validated = try validatedRoutingSettings(
                retryMaxAttemptsText: retryMaxAttemptsText,
                retryBackoffMsText: retryBackoffMsText
            )
            routingSettings.retryMaxAttempts = validated.retryMaxAttempts
            routingSettings.retryBackoffMs = validated.retryBackoffMs
            sanitizeRoutingSettings(persist: false)
            guard persistProfiles() else {
                throw ProviderRepositoryOperationError.failed(
                    lastRepositoryError ?? String(localized: "settings_provider_save_failed")
                )
            }
            didPersist = true
            if let applyReconnectHandler {
                try await applyReconnectHandler(makeLaunchSnapshot())
                lastAppliedRoutingSettings = routingSettings
                routingDirty = false
                routingMessage = String(localized: "settings_provider_routing_applied_reconnect")
            } else {
                routingMessage = String(localized: "settings_provider_routing_saved")
            }
            bumpSyncRevision()
            return true
        } catch {
            if !didPersist {
                routingSettings = previousRoutingSettings
                routingMessage = previousRoutingMessage
                routingDirty = routingSettings != lastAppliedRoutingSettings
            }
            routingMessage = error.localizedDescription
            lastRepositoryError = error.localizedDescription
            bumpSyncRevision()
            return false
        }
    }

    @discardableResult
    func removeProfile(_ id: String) async -> Bool {
        guard let index = indexOfProfile(id: id) else {
            lastRepositoryError = String(localized: "settings_provider_edit_unavailable")
            bumpSyncRevision()
            return false
        }
        let state = profiles[index]
        guard !state.operationInFlight else {
            lastRepositoryError = String(localized: "settings_provider_operation_in_progress")
            bumpSyncRevision()
            return false
        }
        let previousProfiles = profiles
        let previousRoutingSettings = routingSettings
        if state.oauthState?.signedIn == true, let oauthProvider = oauthProvider(for: state.profile.presetID), let oauthLogoutHandler {
            do {
                try await oauthLogoutHandler(oauthProvider)
            } catch {
                // `index` was resolved BEFORE the await. This @MainActor class
                // is reentrant across every suspension, so a concurrent
                // `removeProfile`/reload may have shifted or dropped that row —
                // re-resolve before writing, like the credential-delete arm
                // below already does.
                guard let currentIndex = indexOfProfile(id: id) else {
                    lastRepositoryError = error.localizedDescription
                    bumpSyncRevision()
                    return false
                }
                profiles[currentIndex].connectionState = .failed
                profiles[currentIndex].detailMessage = error.localizedDescription
                lastRepositoryError = error.localizedDescription
                bumpSyncRevision()
                return false
            }
        }
        let shouldDeleteCredential = commandSubmitter != nil || state.hasStoredCredential || state.clearCredentialOnApply
        guard let currentIndex = indexOfProfile(id: id) else {
            lastRepositoryError = String(localized: "settings_provider_edit_unavailable")
            bumpSyncRevision()
            return false
        }
        profiles.remove(at: currentIndex)
        normalizeDefaults()
        sanitizeRoutingSettings(persist: false)
        guard persistProfiles() else {
            profiles = previousProfiles
            routingSettings = previousRoutingSettings
            bumpSyncRevision()
            return false
        }

        if shouldDeleteCredential {
            do {
                try await deleteCredentialIfPossible(
                    settingsID: state.id,
                    credentialID: engineProfileID(for: state.profile)
                )
            } catch {
                // The profile was removed only after its JSON commit. If the
                // secure-store delete is rejected, restore the saved row so a
                // failed delete cannot strand the user without a visible
                // provider or leave its persisted settings inconsistent.
                profiles = previousProfiles
                routingSettings = previousRoutingSettings
                _ = persistProfiles()
                if let restoredIndex = indexOfProfile(id: id) {
                    profiles[restoredIndex].connectionState = .failed
                    profiles[restoredIndex].detailMessage = error.localizedDescription
                }
                lastRepositoryError = error.localizedDescription
                bumpSyncRevision()
                return false
            }
        }
        if state.id == ProviderRepositoryDefaults.anthropicLegacyProfileID {
            Keychain.clear(.apiBase)
            Keychain.clear(.model)
        }
        lastRepositoryError = nil
        bumpSyncRevision()
        return true
    }

    func makeLaunchSnapshot() -> ProviderLaunchSnapshot {
        let launchProfiles = profiles
            .map { state in
                buildLaunchProfile(from: state.profile)
            }
            .sorted { lhs, rhs in
                if lhs.isDefault != rhs.isDefault {
                    return lhs.isDefault && !rhs.isDefault
                }
                return lhs.id < rhs.id
            }
        let enabledIDs = launchProfiles
            .filter(\.enabled)
            .reduce(into: [String]()) { result, profile in
                if !result.contains(profile.id) {
                    result.append(profile.id)
                }
            }
        let providerProfilesObject = launchProfiles.reduce(into: [String: [String: Any]]()) { result, profile in
            guard !usesBuiltInProfile(profile) else { return }
            result[profile.id] = providerSettingsJSONValue(for: profile)
        }
        let providerProfilesJSON = Self.encodeJSONObject(providerProfilesObject) ?? "{}"
        let defaultModelID = launchProfiles.first(where: { $0.isDefault && $0.enabled })?.qualifiedModelID
        let routingJSON = Self.encodeJSONObject(
            buildRoutingJSONObject(
                enabledProfileIDs: enabledIDs,
                defaultModelID: defaultModelID,
                launchProfiles: launchProfiles
            )
        ) ?? "{}"
        return ProviderLaunchSnapshot(
            profiles: launchProfiles,
            providerProfilesJSON: providerProfilesJSON,
            routingJSON: routingJSON,
            defaultModelID: defaultModelID,
            enabledProfileIDs: enabledIDs,
            visionDelegationEnabled: visionDelegationEnabled
        )
    }

    private func buildLaunchProfile(from profile: ProviderStoredProfile) -> ProviderLaunchProfile {
        let metadata = metadata(for: profile.presetID)
        return ProviderLaunchProfile(
            settingsID: profile.id,
            id: engineProfileID(for: profile),
            presetID: profile.presetID,
            providerType: metadata.providerType,
            displayName: profile.name,
            baseURL: profile.baseURL,
            modelID: profile.modelID,
            enabled: profile.enabled,
            isDefault: profile.isDefault,
            apiKeyEnv: metadata.envVar
        )
    }

    private func state(from draft: ProviderEditorDraft) -> ProviderProfileState {
        ProviderProfileState(
            profile: draft.profile,
            credentialState: draft.credentialState,
            connectionState: draft.connectionState,
            detailMessage: draft.detailMessage,
            pendingSecret: draft.pendingSecret,
            clearCredentialOnApply: draft.clearCredentialOnApply,
            validationMessage: draft.validationMessage,
            operationInFlight: draft.operationInFlight,
            hasLegacyAnthropicCredential: draft.hasLegacyAnthropicCredential,
            oauthState: draft.oauthState
        )
    }

    /// Restore only the profile touched by a failed draft operation. The
    /// expected-profile check prevents a late failure from overwriting a
    /// concurrent edit made after the draft was persisted.
    @discardableResult
    private func rollbackDraft(
        id: String,
        previousState: ProviderProfileState?,
        expectedProfile: ProviderStoredProfile
    ) -> Bool {
        guard let index = indexOfProfile(id: id),
              profiles[index].profile == expectedProfile
        else { return false }

        if let previousState {
            profiles[index] = previousState
        } else {
            profiles.remove(at: index)
        }
        normalizeDefaults()
        return persistProfiles()
    }

    private func syncDraftOAuthState(_ draft: ProviderEditorDraft) {
        guard !draft.isNew, let index = indexOfProfile(id: draft.id) else { return }
        profiles[index].oauthState = draft.oauthState
        profiles[index].connectionState = draft.connectionState
        profiles[index].detailMessage = draft.detailMessage
        bumpSyncRevision()
    }

    private func profileID(fromQualifiedModel reference: String) -> String? {
        guard let separator = reference.firstIndex(of: "/") else { return nil }
        let profile = String(reference[..<separator]).trimmingCharacters(in: .whitespacesAndNewlines)
        return profile.isEmpty ? nil : profile
    }

    private func validateAndBuildLaunchProfile(
        for state: ProviderProfileState,
        allowDisabledWithoutCredential: Bool = false
    ) throws -> ProviderLaunchProfile {
        let profile = state.profile
        if profile.name.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
            throw ProviderProfileValidationError.missingName
        }
        guard let url = URL(string: profile.baseURL.trimmingCharacters(in: .whitespacesAndNewlines)),
              let scheme = url.scheme?.lowercased(),
              ["http", "https"].contains(scheme),
              url.host != nil
        else {
            throw ProviderProfileValidationError.invalidBaseURL
        }
        if profile.modelID.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
            throw ProviderProfileValidationError.missingModel
        }
        if !state.clearCredentialOnApply,
           profile.enabled || !allowDisabledWithoutCredential {
            if effectiveSecret(for: state) == nil, !state.hasStoredCredential {
                throw ProviderProfileValidationError.missingCredential
            }
        }
        return buildLaunchProfile(from: profile)
    }

    private func effectiveSecret(for state: ProviderProfileState) -> String? {
        if state.clearCredentialOnApply {
            return nil
        }
        let trimmedPending = state.pendingSecret.trimmingCharacters(in: .whitespacesAndNewlines)
        if !trimmedPending.isEmpty {
            return trimmedPending
        }
        if state.hasLegacyAnthropicCredential,
           state.id == ProviderRepositoryDefaults.anthropicLegacyProfileID {
            return Keychain.get(.apiKey)
        }
        return nil
    }

    private func storeCredentialIfPossible(
        _ secret: String,
        settingsID: String,
        credentialID: String,
        publishState: Bool = true
    ) async throws {
        if let commandSubmitter {
            let operationID = takeOperationID()
            pendingOperations[operationID] = .set(
                target: .init(settingsID: settingsID, credentialID: credentialID)
            )
            if let index = indexOfProfile(id: settingsID) {
                profiles[index].operationInFlight = true
            }
            if !publishState {
                suppressedCredentialStateUpdates.insert(operationID)
            }
            try await awaitCredentialOperation(operationID) {
                try await commandSubmitter(
                    .setProviderCredential(
                        operationId: operationID,
                        providerId: credentialID,
                        credential: ProviderCredentialSecretDto(value: secret)
                    )
                )
            }
            return
        }
        if settingsID == ProviderRepositoryDefaults.anthropicLegacyProfileID {
            guard Keychain.set(.apiKey, secret) else {
                throw NSError(domain: "ProviderRepository", code: 1, userInfo: [
                    NSLocalizedDescriptionKey: String(localized: "settings_provider_legacy_keychain_write_failed"),
                ])
            }
            if publishState, let index = indexOfProfile(id: settingsID) {
                profiles[index].credentialState = .configured
                profiles[index].pendingSecret = ""
                profiles[index].hasLegacyAnthropicCredential = true
                profiles[index].clearCredentialOnApply = false
            }
            return
        }
        throw NSError(domain: "ProviderRepository", code: 2, userInfo: [
            NSLocalizedDescriptionKey: String(localized: "settings_provider_secure_storage_save_unavailable"),
        ])
    }

    private func deleteCredentialIfPossible(
        settingsID: String,
        credentialID: String,
        publishState: Bool = true
    ) async throws {
        if let commandSubmitter {
            let operationID = takeOperationID()
            pendingOperations[operationID] = .delete(
                target: .init(settingsID: settingsID, credentialID: credentialID)
            )
            if let index = indexOfProfile(id: settingsID) {
                profiles[index].operationInFlight = true
            }
            if !publishState {
                suppressedCredentialStateUpdates.insert(operationID)
            }
            try await awaitCredentialOperation(operationID) {
                try await commandSubmitter(.deleteProviderCredential(operationId: operationID, providerId: credentialID))
            }
            return
        }
        if settingsID == ProviderRepositoryDefaults.anthropicLegacyProfileID {
            guard Keychain.clear(.apiKey) else {
                throw NSError(domain: "ProviderRepository", code: 3, userInfo: [
                    NSLocalizedDescriptionKey: String(localized: "settings_provider_legacy_keychain_delete_failed"),
                ])
            }
            if publishState, let index = indexOfProfile(id: settingsID) {
                profiles[index].credentialState = .missing
                profiles[index].hasLegacyAnthropicCredential = false
                profiles[index].clearCredentialOnApply = false
            }
            return
        }
        throw NSError(domain: "ProviderRepository", code: 4, userInfo: [
            NSLocalizedDescriptionKey: String(localized: "settings_provider_secure_storage_delete_unavailable"),
        ])
    }

    private func mirrorLegacyAnthropicSettingsIfNeeded(for profile: ProviderLaunchProfile, state: ProviderProfileState) {
        guard profile.settingsID == ProviderRepositoryDefaults.anthropicLegacyProfileID else { return }
        Keychain.set(.apiBase, profile.baseURL)
        Keychain.set(.model, profile.modelID)
        if state.clearCredentialOnApply {
            Keychain.clear(.apiKey)
        }
    }

    private func providerSettingsJSONValue(for profile: ProviderLaunchProfile) -> [String: Any] {
        let presetModels = preset(for: profile.presetID).models
        let models = ([profile.modelID] + presetModels)
            .map { $0.trimmingCharacters(in: .whitespacesAndNewlines) }
            .filter { !$0.isEmpty }
            .reduce(into: [String]()) { result, next in
                if !result.contains(next) {
                    result.append(next)
                }
            }
        let modelObjects: [[String: Any]] = models.map { modelID in
            ["id": modelID]
        }
        return [
            "type": profile.providerType,
            "baseUrl": profile.baseURL,
            "apiKeyEnv": profile.apiKeyEnv,
            "models": modelObjects,
        ]
    }

    /// Android and Rust both treat catalog-default endpoints as built-ins. Do
    /// not emit a second user profile with the same engine name: llm-client
    /// rejects duplicate profile names before a connection probe can run.
    private func usesBuiltInProfile(_ profile: ProviderLaunchProfile) -> Bool {
        usesBuiltInProfile(presetID: profile.presetID, baseURL: profile.baseURL)
    }

    private func engineProfileID(for profile: ProviderStoredProfile) -> String {
        if usesBuiltInProfile(presetID: profile.presetID, baseURL: profile.baseURL),
           let builtInID = ProviderRepositoryDefaults.builtInProfileIDsByPreset[profile.presetID] {
            return builtInID
        }

        let normalizedID = profile.id
            .lowercased()
            .map { character in
                character.isLetter || character.isNumber || "_-.".contains(character)
                    ? character
                    : "_"
            }
            .reduce(into: "") { $0.append($1) }
            .trimmingCharacters(in: CharacterSet(charactersIn: "_"))
        let boundedID = String(normalizedID.prefix(64))
        let fallbackID = boundedID.isEmpty ? "mobile-provider" : boundedID
        let reservedIDs = Set(ProviderRepositoryDefaults.builtInProfileIDsByPreset.values)
        return reservedIDs.contains(fallbackID) ? "\(fallbackID)-user" : fallbackID
    }

    private func usesBuiltInProfile(presetID: String, baseURL: String) -> Bool {
        guard ProviderRepositoryDefaults.builtInProfileIDsByPreset[presetID] != nil else {
            return false
        }
        let defaultURL = preset(for: presetID).defaultUrl
        let configuredURL = baseURL.trimmingCharacters(in: .whitespacesAndNewlines)
        return configuredURL.isEmpty || normalizedProviderURL(configuredURL) == normalizedProviderURL(defaultURL)
    }

    private func normalizedProviderURL(_ rawValue: String) -> String {
        rawValue.trimmingCharacters(in: .whitespacesAndNewlines.union(CharacterSet(charactersIn: "/")))
    }

    private func buildRoutingJSONObject(
        enabledProfileIDs: [String],
        defaultModelID: String?,
        launchProfiles: [ProviderLaunchProfile]
    ) -> [String: Any] {
        var routing: [String: Any] = [
            ProviderRepositoryDefaults.routingMobileEnabledProfilesKey: enabledProfileIDs,
            "retry": [
                "maxAttempts": routingSettings.retryMaxAttempts,
                "backoffMs": routingSettings.retryBackoffMs,
            ],
        ]
        guard let defaultModelID,
              !defaultModelID.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
        else {
            return routing
        }
        let launchProfilesByID = Dictionary(uniqueKeysWithValues: launchProfiles.map { ($0.settingsID, $0) })
        let fallbackTargets = routingSettings.fallbackProfileIDs.compactMap { profileID -> String? in
            guard let profile = launchProfilesByID[profileID],
                  enabledProfileIDs.contains(profile.id)
            else {
                return nil
            }
            let modelID = profile.modelID.trimmingCharacters(in: .whitespacesAndNewlines)
            return modelID.isEmpty ? nil : "\(profile.id)/\(modelID)"
        }
        if !fallbackTargets.isEmpty {
            routing["fallback"] = [defaultModelID: fallbackTargets]
        }
        return routing
    }

    private func metadata(for presetID: String) -> ProviderPresetMetadata {
        if let catalog = catalogEntries.first(where: { $0.id == presetID }) {
            let providerType: String
            switch catalog.protocolName {
            case "AnthropicMessages": providerType = "anthropic"
            case "GeminiGenerateContent": providerType = "gemini"
            case "OpenAiResponses": providerType = "openai-responses"
            default: providerType = "openai"
            }
            return .init(providerType: providerType, envVar: catalog.credentialEnv ?? "")
        }
        switch presetID {
        case "anthropic":
            return .init(providerType: "anthropic", envVar: "ANTHROPIC_API_KEY")
        case "google":
            return .init(providerType: "gemini", envVar: "GEMINI_API_KEY")
        case "openai":
            return .init(providerType: "openai-responses", envVar: "OPENAI_API_KEY")
        case "openai-chatgpt":
            return .init(providerType: "openai-responses", envVar: "")
        case "deepseek":
            return .init(providerType: "openai", envVar: "DEEPSEEK_API_KEY")
        case "kimi":
            return .init(providerType: "openai", envVar: "MOONSHOT_API_KEY")
        case "kimi-code":
            return .init(providerType: "openai", envVar: "KIMI_API_KEY")
        case "openrouter":
            return .init(providerType: "openai", envVar: "OPENROUTER_API_KEY")
        case "qwen":
            return .init(providerType: "openai", envVar: "DASHSCOPE_API_KEY")
        case "custom":
            return .init(providerType: "openai", envVar: "CUSTOM_API_KEY")
        default:
            return .init(providerType: "openai", envVar: "CUSTOM_API_KEY")
        }
    }

    private static func normalizedOAuthProfile(_ profile: ProviderStoredProfile) -> ProviderStoredProfile {
        guard profile.presetID == "anthropic" || profile.presetID == "openai-chatgpt",
              let defaultURL = Presets.llm.first(where: { $0.id == profile.presetID })?.defaultUrl
        else {
            return profile
        }
        var normalized = profile
        normalized.baseURL = defaultURL
        return normalized
    }

    private func indexOfProfile(id: String) -> Int? {
        profiles.firstIndex(where: { $0.id == id })
    }

    private func normalizeDefaults() {
        if profiles.isEmpty {
            return
        }
        let defaultIndex = profiles.firstIndex(where: { $0.profile.isDefault && $0.profile.enabled })
            ?? profiles.firstIndex(where: { $0.profile.enabled })
        for index in profiles.indices {
            profiles[index].profile.isDefault = index == defaultIndex
        }
        profiles.sort { lhs, rhs in
            if lhs.profile.isDefault != rhs.profile.isDefault {
                return lhs.profile.isDefault && !rhs.profile.isDefault
            }
            return lhs.profile.name.localizedStandardCompare(rhs.profile.name) == .orderedAscending
        }
        bumpSyncRevision()
    }

    private func sanitizeRoutingSettings(persist: Bool = true) {
        routingSettings.retryMaxAttempts = clampedRetryMaxAttempts(routingSettings.retryMaxAttempts)
        routingSettings.retryBackoffMs = clampedRetryBackoffMs(routingSettings.retryBackoffMs)
        let eligibleProfileIDs = Set(eligibleFallbackProfiles().map(\.id))
        var seen = Set<String>()
        routingSettings.fallbackProfileIDs = routingSettings.fallbackProfileIDs.filter { profileID in
            guard eligibleProfileIDs.contains(profileID), !seen.contains(profileID) else {
                return false
            }
            seen.insert(profileID)
            return true
        }
        routingDirty = routingSettings != lastAppliedRoutingSettings
        if persist {
            persistProfiles()
        }
    }

    @discardableResult
    private func persistProfiles() -> Bool {
        do {
            let directory = persistenceURL.deletingLastPathComponent()
            try fileManager.createDirectory(at: directory, withIntermediateDirectories: true, attributes: nil)
            let encoder = JSONEncoder()
            encoder.outputFormatting = [.withoutEscapingSlashes]
            let data = try encoder.encode(
                ProviderPersistenceEnvelope(
                    version: 2,
                    profiles: profiles.map(\.profile),
                    routing: routingSettings,
                    visionDelegationEnabled: visionDelegationEnabled
                )
            )
            try data.write(to: persistenceURL, options: [.atomic])
            lastRepositoryError = nil
            bumpSyncRevision()
            return true
        } catch {
            lastRepositoryError = error.localizedDescription
            bumpSyncRevision()
            return false
        }
    }

    private func nextProfileID(for presetID: String) -> String {
        if !profiles.contains(where: { $0.id == presetID }) {
            return presetID
        }
        var suffix = 2
        while profiles.contains(where: { $0.id == "\(presetID)-\(suffix)" }) {
            suffix += 1
        }
        return "\(presetID)-\(suffix)"
    }

    private func takeOperationID() -> UInt64 {
        defer { nextOperationID &+= 1 }
        return nextOperationID
    }

    private func awaitCredentialOperation(
        _ operationID: UInt64,
        submit: @escaping @MainActor () async throws -> Void
    ) async throws {
        try await withCheckedThrowingContinuation { continuation in
            pendingCompletions[operationID] = continuation
            scheduleCredentialOperationTimeout(operationID)
            Task { [weak self] in
                do {
                    try await submit()
                } catch {
                    self?.failCredentialOperation(operationID, error: error)
                }
            }
        }
    }

    private func expireCredentialOperation(_ operationID: UInt64) {
        failCredentialOperation(
            operationID,
            error: ProviderRepositoryOperationError.failed(String(localized: "settings_provider_secure_storage_timeout"))
        )
    }

    private func failCredentialOperation(_ operationID: UInt64, error: Error) {
        guard pendingOperations[operationID] != nil else { return }
        pendingTimeouts.removeValue(forKey: operationID)?.cancel()
        let operation = pendingOperations.removeValue(forKey: operationID)
        suppressedCredentialStateUpdates.remove(operationID)
        if let operation {
            clearOperationInFlightIfFinished(for: operation)
        }
        pendingCompletions.removeValue(forKey: operationID)?.resume(throwing: error)
        lastRepositoryError = error.localizedDescription
        bumpSyncRevision()
    }

    private func scheduleCredentialOperationTimeout(_ operationID: UInt64) {
        let timeout = credentialOperationTimeout
        pendingTimeouts[operationID]?.cancel()
        pendingTimeouts[operationID] = Task { [weak self] in
            do {
                try await Task.sleep(for: timeout)
            } catch {
                return
            }
            guard !Task.isCancelled else { return }
            self?.expireCredentialOperation(operationID)
        }
    }

    @discardableResult
    private func cancelPendingListOperations() -> Bool {
        let operationIDs = pendingOperations.compactMap { operationID, operation -> UInt64? in
            if case .list = operation { return operationID }
            return nil
        }
        for operationID in operationIDs {
            pendingTimeouts.removeValue(forKey: operationID)?.cancel()
            suppressedCredentialStateUpdates.remove(operationID)
            guard let operation = pendingOperations.removeValue(forKey: operationID) else { continue }
            clearOperationInFlightIfFinished(for: operation)
        }
        return !operationIDs.isEmpty
    }

    private func setOperationInFlight(_ inFlight: Bool, for providerIDs: [String]) {
        for providerID in providerIDs {
            guard let index = indexOfProfile(id: providerID) else { continue }
            profiles[index].operationInFlight = inFlight
        }
    }

    private func clearOperationInFlightIfFinished(for operation: PendingCredentialOperation) {
        for providerID in operation.providerIDs {
            let stillPending = pendingOperations.values.contains { pending in
                pending.providerIDs.contains(providerID)
            }
            if !stillPending, let index = indexOfProfile(id: providerID) {
                profiles[index].operationInFlight = false
            }
        }
    }

    private func bumpSyncRevision() {
        syncRevision &+= 1
    }

    private func eligibleFallbackProfiles() -> [ProviderStoredProfile] {
        let defaultID = profiles.first(where: { $0.profile.isDefault && $0.profile.enabled })?.id
        return profiles
            .map(\.profile)
            .filter { profile in
                profile.enabled &&
                    profile.id != defaultID &&
                    !profile.modelID.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
            }
    }

    private func validatedRoutingSettings(
        retryMaxAttemptsText: String,
        retryBackoffMsText: String
    ) throws -> ProviderRoutingSettings {
        let retryText = retryMaxAttemptsText.trimmingCharacters(in: .whitespacesAndNewlines)
        let backoffText = retryBackoffMsText.trimmingCharacters(in: .whitespacesAndNewlines)
        guard let retryMaxAttempts = Int(retryText),
              ProviderRoutingSettings.minRetryMaxAttempts...ProviderRoutingSettings.maxRetryMaxAttempts ~= retryMaxAttempts
        else {
            throw ProviderRepositoryOperationError.failed(
                String(localized: "settings_provider_retry_range \(ProviderRoutingSettings.minRetryMaxAttempts) \(ProviderRoutingSettings.maxRetryMaxAttempts)")
            )
        }
        guard let retryBackoffMs = Int(backoffText),
              ProviderRoutingSettings.minRetryBackoffMs...ProviderRoutingSettings.maxRetryBackoffMs ~= retryBackoffMs
        else {
            throw ProviderRepositoryOperationError.failed(
                String(localized: "settings_provider_backoff_range \(ProviderRoutingSettings.minRetryBackoffMs) \(ProviderRoutingSettings.maxRetryBackoffMs)")
            )
        }
        return ProviderRoutingSettings(
            retryMaxAttempts: retryMaxAttempts,
            retryBackoffMs: retryBackoffMs,
            fallbackProfileIDs: routingSettings.fallbackProfileIDs
        )
    }

    private func clampedRetryMaxAttempts(_ value: Int) -> Int {
        min(max(value, ProviderRoutingSettings.minRetryMaxAttempts), ProviderRoutingSettings.maxRetryMaxAttempts)
    }

    private func clampedRetryBackoffMs(_ value: Int) -> Int {
        min(max(value, ProviderRoutingSettings.minRetryBackoffMs), ProviderRoutingSettings.maxRetryBackoffMs)
    }

    private static func loadEnvelope(from url: URL, fileManager: FileManager) -> ProviderPersistenceEnvelope? {
        guard fileManager.fileExists(atPath: url.path),
              let data = try? Data(contentsOf: url),
              let envelope = try? JSONDecoder().decode(ProviderPersistenceEnvelope.self, from: data)
        else {
            return nil
        }
        return envelope
    }

    private static func migrateLegacyDeepSeekProfile(
        _ profile: ProviderStoredProfile
    ) -> ProviderStoredProfile {
        guard profile.presetID == "deepseek" else { return profile }

        var migrated = profile
        let trimmedURL = profile.baseURL.trimmingCharacters(in: .whitespacesAndNewlines)
        let normalizedURL = trimmedURL.trimmingCharacters(in: CharacterSet(charactersIn: "/"))
        if normalizedURL == "https://api.deepseek.com/v1" {
            migrated.baseURL = "https://api.deepseek.com"
        }
        if ["deepseek-chat", "deepseek-reasoner"].contains(profile.modelID) {
            migrated.modelID = "deepseek-v4-flash"
        }
        return migrated
    }

    private static func legacyAnthropicProfile() -> ProviderProfileState? {
        legacyAnthropicProfile(
            legacyKey: Keychain.get(.apiKey),
            legacyBase: Keychain.get(.apiBase),
            legacyModel: Keychain.get(.model)
        )
    }

    /// Rebuild the pre-multi-provider Anthropic configuration from the three
    /// legacy Keychain slots, for a user upgrading before any profile has been
    /// persisted.
    ///
    /// Only an api key or a base URL override evidences a CONFIGURED Anthropic
    /// provider. `Keychain.model` alone does NOT: since the multi-provider
    /// picker landed, `applyActiveModel` writes the engine's active model there
    /// on every `ModelList`/`ModelChanged` — so a user who never configured any
    /// provider still has that slot filled. Treating it as evidence fabricated
    /// an enabled, default Anthropic profile out of thin air, and because that
    /// slot now holds a provider-QUALIFIED reference, the fabricated profile
    /// re-qualified it (`anthropic/` + `deepseek/deepseek-v4-flash`) and the
    /// composer's picker rendered "DeepSeek V4 Flash" under the ANTHROPIC
    /// header in Anthropic's colour.
    ///
    /// The stored model is likewise only adopted when it names an Anthropic
    /// model: an unqualified id no OTHER preset claims (a custom proxy model
    /// legitimately routes through this profile), or one qualified with this
    /// profile's own id. Anything else falls back to the preset's first model
    /// instead of being smuggled into the Anthropic profile.
    static func legacyAnthropicProfile(
        legacyKey: String?,
        legacyBase: String?,
        legacyModel: String?
    ) -> ProviderProfileState? {
        guard legacyKey != nil || legacyBase != nil else {
            return nil
        }
        let profileID = ProviderRepositoryDefaults.anthropicLegacyProfileID
        let preset = Presets.llm.first(where: { $0.id == profileID })
        let presetModel = preset?.models.first ?? "claude-sonnet-5"
        let profile = ProviderStoredProfile(
            id: profileID,
            presetID: profileID,
            name: preset?.name ?? "Anthropic",
            baseURL: legacyBase ?? preset?.defaultUrl ?? "https://api.anthropic.com",
            modelID: anthropicModelID(from: legacyModel, profileID: profileID) ?? presetModel,
            enabled: true,
            isDefault: true
        )
        return ProviderProfileState(
            profile: profile,
            credentialState: legacyKey == nil ? .unknown : .configured,
            connectionState: .idle,
            hasLegacyAnthropicCredential: legacyKey != nil
        )
    }

    /// The bare Anthropic model id `reference` names, or `nil` when it names a
    /// model on some other provider. `qualifiedModelID` re-adds the profile
    /// prefix, so storing a qualified reference here would double-qualify it.
    private static func anthropicModelID(from reference: String?, profileID: String) -> String? {
        guard let reference = reference?.trimmingCharacters(in: .whitespacesAndNewlines),
              !reference.isEmpty
        else { return nil }
        guard let slash = reference.firstIndex(of: "/") else {
            // An UNQUALIFIED id is not automatically Anthropic's. `ModelList`'s
            // `current` is emitted bare whenever the session carries no
            // `model_profile`, so `Keychain.model` can hold a bare foreign id
            // like `deepseek-v4-flash`; adopting it here produced exactly the
            // reported symptom one qualifier shorter — `anthropic/` +
            // `deepseek-v4-flash` clears the engine's "bare id" guard and the
            // picker rendered "DeepSeek V4 Flash" under the ANTHROPIC header.
            // An id no other preset claims is still adopted: that is how a
            // custom Anthropic-compatible proxy model survives the migration.
            let claimedElsewhere = Presets.llm.contains {
                $0.id != profileID && $0.models.contains(reference)
            }
            return claimedElsewhere ? nil : reference
        }
        guard reference[..<slash] == profileID else { return nil }
        let bare = String(reference[reference.index(after: slash)...])
        // `anthropic/<provider>/<model>` is a double-qualified reference, never
        // an Anthropic model id.
        return bare.isEmpty || bare.contains("/") ? nil : bare
    }

    private static func defaultPersistenceURL() -> URL {
        let fileManager = FileManager.default
        let appSupport = fileManager.urls(for: .applicationSupportDirectory, in: .userDomainMask).first
        let base = appSupport ?? fileManager.urls(for: .documentDirectory, in: .userDomainMask).first!
        return base
            .appendingPathComponent("LingxiCode", isDirectory: true)
            .appendingPathComponent("provider-settings.json", isDirectory: false)
    }

    private static func encodeJSONObject(_ object: Any) -> String? {
        guard JSONSerialization.isValidJSONObject(object),
              let data = try? JSONSerialization.data(withJSONObject: object, options: [.sortedKeys]),
              let string = String(data: data, encoding: .utf8)
        else {
            return nil
        }
        return string
    }
}
