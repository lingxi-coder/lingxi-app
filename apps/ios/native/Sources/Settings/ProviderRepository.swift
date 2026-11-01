import Foundation
#if canImport(harness_runtimeFFI)
import harness_runtimeFFI
#endif
import Observation
import SwiftUI

typealias ProviderCommandSubmitter = (ClientCommand) async throws -> Void

typealias ProviderCatalogLoader = () async throws -> [ProviderCatalogEntry]

typealias ProviderApplyReconnectHandler = (ProviderLaunchSnapshot) async throws -> Void

private enum ProviderCatalogLoadState {
    case idle
    case loaded
    case failed
}


enum ProviderProfileValidationError: LocalizedError, Equatable {
    case missingName
    case invalidBaseURL
    case missingModel
    case connectionMissingID
    case connectionDuplicateID(String)
    case connectionInvalidBaseURL(String)

    var errorDescription: String? {
        switch self {
        case .missingName:
            return String(localized: "settings_provider_error_missing_name")
        case .invalidBaseURL:
            return String(localized: "settings_provider_error_invalid_url")
        case .missingModel:
            return String(localized: "settings_provider_error_missing_model")
        case .connectionMissingID:
            return String(localized: "settings_provider_error_connection_missing_id")
        case .connectionDuplicateID(let id):
            return String(localized: "settings_provider_error_connection_duplicate_id \(id)")
        case .connectionInvalidBaseURL(let id):
            return String(localized: "settings_provider_error_connection_invalid_url \(id)")
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


enum ProviderConnectionState: Equatable {
    case idle
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
    /// Empty for a provider reachable one way. When set, `id` names the GROUP
    /// and the engine desugars one profile per connection beneath it.
    let connections: [ProviderStoredConnection]

    /// Still the group ref when connections exist: `resolve_in` matches a
    /// profile name OR a group, so this keeps naming one stable thing as
    /// connections are added and removed.
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

/// One way to reach a provider: its own endpoint, optionally serving fewer
/// models than the provider as a whole.
///
/// Connections contain transport configuration only. Independent credentials
/// and their priority are Host-owned ProviderAccounts bound to the profile.
struct ProviderStoredConnection: Codable, Equatable, Identifiable {
    /// Stable within the provider; becomes the `group:connection` profile name
    /// the engine routes on, so it is normalized before it is emitted.
    var id: String
    var baseURL: String
    /// `nil` inherits the provider's model list. A connection that serves only
    /// some of them (a regional endpoint) names the subset here.
    var modelIDs: [String]?

    init(id: String, baseURL: String, modelIDs: [String]? = nil) {
        self.id = id
        self.baseURL = baseURL
        self.modelIDs = modelIDs
    }

    private enum CodingKeys: String, CodingKey {
        case id
        case baseURL
        case modelIDs = "modelIds"
    }
}

extension ProviderStoredConnection {
    /// `:` would produce `group:a:b`, which the engine's connection-profile
    /// parser splits at the wrong colon; `/` would break the `profile/model`
    /// qualified reference. Both are rejected rather than rewritten, so the
    /// id the user typed is the id they see.
    static func isValidID(_ id: String) -> Bool {
        let trimmed = id.trimmingCharacters(in: .whitespacesAndNewlines)
        return !trimmed.isEmpty && !trimmed.contains(":") && !trimmed.contains("/")
    }

    var trimmedID: String { id.trimmingCharacters(in: .whitespacesAndNewlines) }
    var trimmedBaseURL: String { baseURL.trimmingCharacters(in: .whitespacesAndNewlines) }
}

struct ProviderStoredProfile: Codable, Equatable, Identifiable {
    var id: String
    var presetID: String
    var name: String
    var baseURL: String
    var modelID: String
    var enabled: Bool
    var isDefault: Bool
    var showInModelPicker: Bool
    var visibleModelIDs: [String]?
    /// Empty when the provider is reachable exactly one way, which is the shape
    /// the default transport shape. The editor keeps it either
    /// empty or >= 2 entries; a single connection would rename the engine
    /// profile to `group:id` while describing the same one endpoint.
    var connections: [ProviderStoredConnection]

    private enum CodingKeys: String, CodingKey {
        case id
        case presetID
        case name
        case baseURL
        case modelID
        case enabled
        case isDefault
        case showInModelPicker
        case visibleModelIDs = "visibleModelIds"
        case connections
    }

    init(
        id: String,
        presetID: String,
        name: String,
        baseURL: String,
        modelID: String,
        enabled: Bool,
        isDefault: Bool,
        showInModelPicker: Bool = true,
        visibleModelIDs: [String]? = nil,
        connections: [ProviderStoredConnection] = []
    ) {
        self.id = id
        self.presetID = presetID
        self.name = name
        self.baseURL = baseURL
        self.modelID = modelID
        self.enabled = enabled
        self.isDefault = isDefault
        self.showInModelPicker = showInModelPicker
        self.visibleModelIDs = visibleModelIDs
        self.connections = connections
    }

}

extension ProviderStoredProfile {
    /// Add a connection.
    ///
    /// The first add creates two connections from the current endpoint: the
    /// endpoint already configured becomes `default` so nothing the user set is
    /// lost, and the new one starts empty. Going straight from none to one
    /// would rename the engine profile to `group:default` while still
    /// describing a single endpoint.
    mutating func addConnection() {
        if connections.isEmpty {
            connections = [
                ProviderStoredConnection(id: "default", baseURL: baseURL),
                ProviderStoredConnection(id: "", baseURL: ""),
            ]
        } else {
            connections.append(ProviderStoredConnection(id: "", baseURL: ""))
        }
    }

    /// Remove a connection, collapsing back to a flat provider when one is
    /// left — a one-entry list would keep claiming several ways to reach this
    /// provider, and would still cost the `group:id` rename. The survivor's
    /// endpoint is lifted to provider level so the reachable URL is unchanged.
    mutating func removeConnection(at index: Int) {
        guard connections.indices.contains(index) else { return }
        connections.remove(at: index)
        guard connections.count <= 1 else { return }
        if let survivor = connections.first, !survivor.trimmedBaseURL.isEmpty {
            baseURL = survivor.baseURL
        }
        connections = []
    }

    /// Every rule the engine would otherwise discover as a parse error or, for
    /// a duplicate id, as a silently dropped connection.
    func validateConnections() throws {
        guard !connections.isEmpty else { return }
        var seen = Set<String>()
        for connection in connections {
            let id = connection.trimmedID
            guard ProviderStoredConnection.isValidID(id) else {
                throw ProviderProfileValidationError.connectionMissingID
            }
            guard seen.insert(id).inserted else {
                throw ProviderProfileValidationError.connectionDuplicateID(id)
            }
            // Inheriting the provider URL here would point a connection the
            // user just added at the endpoint they are trying to add one
            // BESIDE, so an endpoint is required rather than inherited.
            guard let url = URL(string: connection.trimmedBaseURL),
                  let scheme = url.scheme?.lowercased(),
                  ["http", "https"].contains(scheme),
                  url.host != nil
            else {
                throw ProviderProfileValidationError.connectionInvalidBaseURL(id)
            }
        }
    }
}

/// A non-persisted edit transaction.  Provider settings are deliberately
/// edited outside `profiles` so cancelling a sheet cannot leak half-written
/// values into the launch snapshot or the settings summary.
struct ProviderEditorDraft: Identifiable, Equatable {
    let id: String
    let isNew: Bool
    var profile: ProviderStoredProfile
    var connectionState: ProviderConnectionState = .idle
    var detailMessage: String?
    var validationMessage: String?
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
        authName == "chat_gpt_o_auth" || authName == "o_auth_bearer"
    }
}

enum ProviderEditorModelCatalog {
    case loading
    case unavailable
    case ready(models: [String], detailsByModelID: [String: ModelRuntimeDetails])
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
    var profile: ProviderStoredProfile
    var connectionState: ProviderConnectionState = .idle
    var detailMessage: String?
    var validationMessage: String?
    var id: String { profile.id }
}

private struct ProviderPersistenceEnvelope: Codable, Equatable {
    var version: Int
    var profiles: [ProviderStoredProfile]
    var routing: ProviderRoutingSettings
    var visionDelegationEnabled: Bool
}


private struct ProviderPresetMetadata {
    let providerType: String
    let envVar: String
}

private enum ProviderRepositoryDefaults {
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
        "deepseek": "deepseek-flash",
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
    private let accountRepository: ProviderAccountRepository
    private var commandSubmitter: ProviderCommandSubmitter?
    private var catalogLoader: ProviderCatalogLoader?
    private var catalogGeneration: UInt64 = 0
    private var applyReconnectHandler: ProviderApplyReconnectHandler?
    private var lastAppliedRoutingSettings: ProviderRoutingSettings

    private(set) var profiles: [ProviderProfileState]
    private(set) var routingSettings: ProviderRoutingSettings
    private(set) var visionDelegationEnabled: Bool
    private(set) var lastRepositoryError: String? = nil
    private(set) var routingMessage: String? = nil
    private(set) var routingDirty = false
    private(set) var syncRevision = 0
    private(set) var runtimeSnapshot = ProviderRuntimeSnapshot()
    private(set) var catalogEntries: [ProviderCatalogEntry] = []
    private var catalogLoadState: ProviderCatalogLoadState = .idle

    init(persistenceURL: URL? = nil, fileManager: FileManager = .default, accountRepository: ProviderAccountRepository? = nil) {
        let resolved = persistenceURL ?? Self.defaultPersistenceURL()
        let envelope = Self.loadEnvelope(from: resolved, fileManager: fileManager)
        let routing = envelope?.routing ?? ProviderRoutingSettings()
        self.persistenceURL = resolved
        self.fileManager = fileManager
        self.accountRepository = accountRepository ?? .shared
        self.routingSettings = routing
        self.lastAppliedRoutingSettings = routing
        self.visionDelegationEnabled = envelope?.visionDelegationEnabled ?? true
        self.profiles = (envelope?.profiles ?? []).map { ProviderProfileState(profile: $0) }
        normalizeDefaults()
        sanitizeRoutingSettings(persist: false)
    }

    func configure(
        submitCommand: ProviderCommandSubmitter?,
        applyReconnect: ProviderApplyReconnectHandler? = nil,
        providerCatalog: ProviderCatalogLoader? = nil,
        resetCatalog: Bool = true
    ) {
        commandSubmitter = submitCommand
        applyReconnectHandler = applyReconnect
        catalogLoader = providerCatalog
        if resetCatalog {
            catalogGeneration &+= 1
            catalogEntries = []
            catalogLoadState = .idle
        }
        runtimeSnapshot = ProviderRuntimeSnapshot()
        bumpSyncRevision()
    }

    func acceptCatalog(_ entries: [ProviderCatalogEntry]) {
        catalogEntries = entries
        catalogLoadState = .loaded
        bumpSyncRevision()
        accountRepository.refreshAfterCatalogPublication()
    }

    func refreshOfficialCatalog() async {
        try? await commandSubmitter?(.refreshProviderCatalog(trigger: "manual"))
        _ = await refreshCatalog()
    }

    @discardableResult
    func refreshCatalog() async -> Bool {
        guard let catalogLoader else { return false }
        catalogGeneration &+= 1
        let requestGeneration = catalogGeneration
        catalogLoadState = .idle
        do {
            let entries = try await catalogLoader()
            guard requestGeneration == catalogGeneration else { return false }
            lastRepositoryError = nil
            acceptCatalog(entries)
            return true
        } catch {
            guard requestGeneration == catalogGeneration else { return false }
            catalogLoadState = .failed
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
        // remain available and saved profiles are still rendered.
        if catalogLoader != nil, entries.isEmpty {
            return Presets.llm.filter { $0.id == "custom" }
        }
        guard !entries.isEmpty else { return Presets.llm }
        return entries.filter { $0.id != "custom" }.map { preset(for: $0.id) }
            + Presets.llm.filter { $0.id == "custom" }
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
        return ProviderEditorDraft(id: id, isNew: false, profile: state.profile,
            connectionState: state.connectionState, detailMessage: state.detailMessage,
            validationMessage: state.validationMessage)
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
            )
        )
    }

    /// Commit provider configuration independently of account authentication.
    @discardableResult
    func applyDraft(_ draft: ProviderEditorDraft) async -> Bool {
        let previousProfiles = profiles
        let previousRouting = routingSettings
        let previousSnapshot = makeLaunchSnapshot()
        do {
            _ = try validateAndBuildLaunchProfile(for: state(from: draft))
            if draft.isNew { profiles.append(state(from: draft)) }
            else if let index = indexOfProfile(id: draft.id) { profiles[index] = state(from: draft) }
            else { throw ProviderRepositoryOperationError.failed(String(localized: "settings_provider_edit_unavailable")) }
            normalizeDefaults()
            sanitizeRoutingSettings(persist: false)
            guard persistProfiles() else {
                profiles = previousProfiles
                routingSettings = previousRouting
                return false
            }
            if try await reconnectIfLaunchConfigurationChanged(from: previousSnapshot) {
                lastAppliedRoutingSettings = routingSettings
                routingDirty = false
            }
            lastRepositoryError = nil
            bumpSyncRevision()
            return true
        } catch {
            if let index = indexOfProfile(id: draft.id) {
                profiles[index].connectionState = .failed
                if let validation = error as? ProviderProfileValidationError {
                    profiles[index].validationMessage = validation.errorDescription
                } else { profiles[index].detailMessage = error.localizedDescription }
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
        case .providerAccountRouting(let eventJson):
            accountRepository.recordRouting(eventJson)
            return
        case .turnEnded(_, _, _):
            Task { await accountRepository.refresh() }
            return
        case .modelList(let models, let current, _, _):
            updateRuntimeSnapshot(
                models: models,
                activeModelID: current,
                activeProfileID: profileID(fromQualifiedModel: current)
            )
            return
        case .modelChanged(let model, _):
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
                runtimeSnapshot.lastError = message.displayText
                bumpSyncRevision()
            }
            return
        default:
            break
        }
    }

    func settingsProviders() -> [GenericProvider] {
        profiles.map { state in
            return GenericProvider(
                id: state.id,
                preset: state.profile.presetID,
                name: state.profile.name,
                url: state.profile.baseURL,
                key: "",
                model: state.profile.modelID,
                status: hasConfiguredAccount(for: state.profile) ? .connected : .idle,
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

    func visibleModelReferences(_ references: [String]) -> [String] {
        references.filter { reference in
            let canonicalModel = catalogEntries.lazy.compactMap { $0.modelDetails[reference]?.modelId }.first
            return visibleConversationModelReference(reference)
                && accountRepository.modelIsAvailable(reference: reference, canonicalModelId: canonicalModel)
        }
    }

    func visibleModelIDs(
        for profile: ProviderStoredProfile,
        from candidates: [String]
    ) -> [String] {
        let models = Array(NSOrderedSet(array: candidates)).compactMap { $0 as? String }
            .filter { !$0.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty }
        guard let allowlist = profile.visibleModelIDs else { return models }
        let allowed = Set(allowlist)
        return models.filter { allowed.contains($0) }
    }

    func visibleModelCount(
        for profile: ProviderStoredProfile,
        from candidates: [String]
    ) -> Int {
        visibleModelIDs(for: profile, from: candidates).count
    }

    func editorModelCatalog(
        for profile: ProviderStoredProfile,
        runtimeModelReferences: [String]
    ) -> ProviderEditorModelCatalog {
        if profile.presetID == "custom" {
            let runtimeModels = runtimeModelReferences.compactMap { ref -> String? in
                guard let slash = ref.firstIndex(of: "/") else { return nil }
                let providerID = String(ref[..<slash])
                guard providerID == profile.id || providerID == profile.presetID else { return nil }
                return String(ref[ref.index(after: slash)...])
            }
            let currentModel = profile.modelID.trimmingCharacters(in: .whitespacesAndNewlines)
            let models = Array(
                NSOrderedSet(array: runtimeModels + (currentModel.isEmpty ? [] : [currentModel]))
            ).compactMap { $0 as? String }
            return .ready(models: models, detailsByModelID: [:])
        }
        if let entry = catalogEntries.first(where: { $0.id == profile.presetID }) {
            return .ready(
                models: entry.models,
                detailsByModelID: Dictionary(
                    uniqueKeysWithValues: entry.modelDetails.values.map { ($0.modelId, $0) }
                )
            )
        }
        if catalogLoader != nil {
            switch catalogLoadState {
            case .idle:
                return .loading
            case .loaded, .failed:
                return .unavailable
            }
        }
        return .ready(
            models: preset(for: profile.presetID).models,
            detailsByModelID: [:]
        )
    }

    private func visibleConversationModelReference(_ reference: String) -> Bool {
        let parts = reference.split(separator: "/", maxSplits: 1).map(String.init)
        guard parts.count == 2 else { return true }
        if catalogLoader != nil, catalogLoadState == .loaded {
            guard authoritativeCatalogReferences().contains(reference) else { return false }
        }
        guard let state = profiles.first(where: { engineProfileID(for: $0.profile) == parts[0] }) else {
            return true
        }
        guard state.profile.showInModelPicker else { return false }
        guard let allowlist = state.profile.visibleModelIDs else { return true }
        return allowlist.contains(parts[1])
    }

    private func authoritativeCatalogReferences() -> Set<String> {
        Set(catalogEntries.flatMap { entry in
            let providerID = entry.id == "google" ? "gemini" : entry.id
            return Array(entry.modelDetails.keys) + entry.models.map { "\(providerID)/\($0)" }
        })
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

    /// Anthropic consumer OAuth sign-in remains withheld under its provider policy.
    static let hiddenOAuthLoginProviders: Set<String> = ["anthropic"]

    /// Whether the UI may offer an OAuth sign-in for this preset.

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
        profiles.append(ProviderProfileState(profile: profile))
        normalizeDefaults()
        persistProfiles()
        return newID
    }

    func updateProfile(_ id: String, mutate: (inout ProviderStoredProfile) -> Void) {
        guard let index = indexOfProfile(id: id) else { return }
        mutate(&profiles[index].profile)
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

    func reloadAccountBindings() async throws {
        guard let applyReconnectHandler else { throw ProviderAccountError.unavailable }
        try await applyReconnectHandler(makeLaunchSnapshot())
    }

    func refreshAccounts() async {
        await accountRepository.refresh()
        bumpSyncRevision()
    }

    func applyChanges(_ id: String) async {
        guard let draft = makeDraft(for: id) else { return }
        _ = await applyDraft(draft)
    }

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
        guard let index = indexOfProfile(id: id) else { return false }
        let previous = profiles
        let previousRouting = routingSettings
        let snapshot = makeLaunchSnapshot()
        profiles.remove(at: index)
        normalizeDefaults()
        sanitizeRoutingSettings(persist: false)
        guard persistProfiles() else {
            profiles = previous
            routingSettings = previousRouting
            return false
        }
        do { _ = try await reconnectIfLaunchConfigurationChanged(from: snapshot) }
        catch { lastRepositoryError = error.localizedDescription; return false }
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
            apiKeyEnv: metadata.envVar,
            connections: profile.connections
        )
    }

    private func state(from draft: ProviderEditorDraft) -> ProviderProfileState {
        ProviderProfileState(profile: draft.profile, connectionState: draft.connectionState,
            detailMessage: draft.detailMessage, validationMessage: draft.validationMessage)
    }

    /// Rebuild the engine only when the launch configuration (or an unapplied
    /// routing edit) differs from `previous`. Credential changes alone are live.
    private func reconnectIfLaunchConfigurationChanged(from previous: ProviderLaunchSnapshot) async throws -> Bool {
        let next = makeLaunchSnapshot()
        guard let applyReconnectHandler, next != previous || routingDirty else { return false }
        try await applyReconnectHandler(next)
        return true
    }


    private func profileID(fromQualifiedModel reference: String) -> String? {
        guard let separator = reference.firstIndex(of: "/") else { return nil }
        let profile = String(reference[..<separator]).trimmingCharacters(in: .whitespacesAndNewlines)
        return profile.isEmpty ? nil : profile
    }

    private func validateAndBuildLaunchProfile(
        for state: ProviderProfileState
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
        try profile.validateConnections()
        return buildLaunchProfile(from: profile)
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
        var settings: [String: Any] = [
            "type": profile.providerType,
            "baseUrl": profile.baseURL,
            "apiKeyEnv": profile.apiKeyEnv,
            "models": modelObjects,
        ]
        // A single connection would rename the engine profile to `group:id`
        // while describing the same one endpoint, so only a real branch is
        // emitted. Each entry restates its own `baseUrl`; everything else —
        // the wire protocol and environment metadata are inherited from the
        // keys above. Each account binds an exact expanded connection profile.
        if profile.connections.count > 1 {
            settings["connections"] = profile.connections.map { connection -> [String: Any] in
                var entry: [String: Any] = [
                    "id": connection.id,
                    "baseUrl": connection.baseURL,
                ]
                if let restricted = connection.modelIDs, !restricted.isEmpty {
                    entry["models"] = restricted.map { ["id": $0] }
                }
                return entry
            }
        }
        return settings
    }

    /// Android and Rust both treat catalog-default endpoints as built-ins. Do
    /// not emit a second user profile with the same engine name: llm-client
    /// rejects duplicate profile names before a connection probe can run.
    private func usesBuiltInProfile(_ profile: ProviderLaunchProfile) -> Bool {
        // Connections make this entry describe something the built-in catalog
        // does not: several endpoints under one group. Treating it as a
        // built-in would drop it from the emitted providers map, and the
        // connections would vanish without an error on any surface.
        guard profile.connections.isEmpty else { return false }
        return usesBuiltInProfile(presetID: profile.presetID, baseURL: profile.baseURL)
    }

    func engineProfileID(for profile: ProviderStoredProfile) -> String {
        if usesBuiltInProfile(presetID: profile.presetID, baseURL: profile.baseURL) {
            return profile.presetID == "google" ? "gemini" : profile.presetID
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
        let reservedIDs = Set(catalogEntries.map { $0.id == "google" ? "gemini" : $0.id })
        return reservedIDs.contains(fallbackID) ? "\(fallbackID)-user" : fallbackID
    }

    func logicalProviderID(for profile: ProviderStoredProfile) -> String {
        let binding = engineProfileID(for: profile)
        return binding == "openai-chatgpt" ? "openai" : binding
    }

    func requiresSavedAccountBinding(for profile: ProviderStoredProfile) -> Bool {
        let knownPreset = catalogEntries.contains { $0.id == profile.presetID }
            || ProviderRepositoryDefaults.builtInProfileIDsByPreset[profile.presetID] != nil
        return !knownPreset || !isOfficialEndpoint(for: profile) || !profile.connections.isEmpty
    }

    func accountOAuthAvailable(for profile: ProviderStoredProfile) -> Bool {
        let binding = engineProfileID(for: profile)
        return ["openai", "openai-chatgpt"].contains(binding)
            && !Self.hiddenOAuthLoginProviders.contains(profile.presetID)
    }

    /// Match the exact connection profiles emitted by providerSettingsJSONValue.
    func executionBindingIDs(for profile: ProviderStoredProfile) -> [String] {
        let binding = engineProfileID(for: profile)
        return profile.connections.count > 1
            ? profile.connections.map { "\(binding):\($0.trimmedID)" }
            : [binding]
    }

    func hasConfiguredAccount(for profile: ProviderStoredProfile) -> Bool {
        accountRepository.snapshot.accounts.contains { account in
            guard executionBindingIDs(for: profile).contains(account.executionBindingId), account.enabled else { return false }
            let health = accountRepository.health(for: account)
            return ["ready", "cooling_down", "quota_exhausted"].contains(health?.status ?? "")
                && health?.reason != "credential_missing"
        }
    }

    private func usesBuiltInProfile(presetID: String, baseURL: String) -> Bool {
        guard let catalog = catalogEntries.first(where: { $0.id == presetID }) else { return false }
        let defaultURL = catalog.baseURL
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
            case "anthropic_messages": providerType = "anthropic"
            case "gemini_generate_content": providerType = "gemini"
            case "open_ai_responses": providerType = "openai-responses"
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
                    version: 3,
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
              let envelope = try? JSONDecoder().decode(ProviderPersistenceEnvelope.self, from: data),
              envelope.version == 3
        else {
            return nil
        }
        return envelope
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
