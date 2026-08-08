import Foundation
import Observation
import SwiftUI

typealias ProviderCommandSubmitter = (ClientCommand) async throws -> Void

typealias ProviderConnectionTester = (ProviderLaunchProfile, String?) async throws -> ProviderConnectionTestResult

typealias ProviderApplyReconnectHandler = (ProviderLaunchSnapshot) async throws -> Void

enum ProviderConnectionTestResult: Equatable {
    case success(message: String? = nil)
    case failure(message: String)
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

struct ProviderProfileState: Identifiable, Equatable {
    var profile: ProviderStoredProfile
    var credentialState: ProviderCredentialState = .unknown
    var connectionState: ProviderConnectionState = .idle
    var detailMessage: String? = nil
    var pendingSecret: String = ""
    var clearCredentialOnApply = false
    var validationMessage: String? = nil
    var operationInFlight = false
    var hasLegacyAnthropicCredential = false

    var id: String { profile.id }

    var hasPendingSecret: Bool {
        !pendingSecret.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
    }

    var hasStoredCredential: Bool {
        if clearCredentialOnApply {
            return false
        }
        return credentialState == .configured || hasLegacyAnthropicCredential
    }

    var effectiveHasCredential: Bool {
        hasPendingSecret || hasStoredCredential
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
        "deepseek": "deepseek",
        "kimi": "kimi",
        "kimi-code": "kimi-code",
        "openrouter": "openrouter",
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
    private var nextOperationID: UInt64 = 1
    private var commandSubmitter: ProviderCommandSubmitter?
    private var connectionTester: ProviderConnectionTester?
    private var applyReconnectHandler: ProviderApplyReconnectHandler?
    private var lastAppliedRoutingSettings: ProviderRoutingSettings

    private(set) var profiles: [ProviderProfileState]
    private(set) var routingSettings: ProviderRoutingSettings
    private(set) var storageEncrypted = true
    private(set) var lastRepositoryError: String? = nil
    private(set) var routingMessage: String? = nil
    private(set) var routingDirty = false
    private(set) var syncRevision = 0

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
        let didMigrateDeepSeek = loadedProfiles != migratedProfiles
        let initialRoutingSettings = loadedEnvelope?.routing ?? ProviderRoutingSettings()
        self.persistenceURL = resolvedPersistenceURL
        self.fileManager = fileManager
        self.credentialOperationTimeout = credentialOperationTimeout
        self.routingSettings = initialRoutingSettings
        self.lastAppliedRoutingSettings = initialRoutingSettings
        self.profiles = migratedProfiles.map {
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
        if didMigrateDeepSeek {
            persistProfiles()
        }
    }

    func configure(
        submitCommand: ProviderCommandSubmitter?,
        testConnection: ProviderConnectionTester? = nil,
        applyReconnect: ProviderApplyReconnectHandler? = nil
    ) {
        if cancelPendingListOperations() {
            bumpSyncRevision()
        }
        commandSubmitter = submitCommand
        connectionTester = testConnection
        applyReconnectHandler = applyReconnect
    }

    func handle(event: ClientEvent) {
        guard case let .providerCredentialStatus(
            operationId,
            configuredProviderIds,
            unavailableProviderIds,
            storageEncrypted,
            error
        ) = event else {
            return
        }
        guard let operation = pendingOperations.removeValue(forKey: operationId) else {
            return
        }
        pendingTimeouts.removeValue(forKey: operationId)?.cancel()
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
        Presets.llm.first(where: { $0.id == presetID }) ?? Presets.llm[Presets.llm.count - 1]
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

    func refreshCredentialStatus() async {
        let targets = profiles.map { state in
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
            try await commandSubmitter(.listProviderCredentials(operationId: operationID, providerIds: credentialIDs))
        } catch {
            failCredentialOperation(operationID, error: error)
            return
        }
        bumpSyncRevision()
    }

    func testConnection(_ id: String) async {
        guard let index = indexOfProfile(id: id) else { return }
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
            case .success(let message):
                profiles[currentIndex].connectionState = .connected
                profiles[currentIndex].detailMessage = message ?? String(localized: "settings_provider_test_success")
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
                    profiles[updatedIndex].profile.enabled = false
                    profiles[updatedIndex].profile.isDefault = false
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
        do {
            let validated = try validatedRoutingSettings(
                retryMaxAttemptsText: retryMaxAttemptsText,
                retryBackoffMsText: retryBackoffMsText
            )
            routingSettings.retryMaxAttempts = validated.retryMaxAttempts
            routingSettings.retryBackoffMs = validated.retryBackoffMs
            sanitizeRoutingSettings()
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
            routingMessage = error.localizedDescription
            lastRepositoryError = error.localizedDescription
            bumpSyncRevision()
            return false
        }
    }

    func removeProfile(_ id: String) async {
        guard let index = indexOfProfile(id: id) else { return }
        let state = profiles[index]
        let shouldDeleteCredential = commandSubmitter != nil || state.hasStoredCredential || state.clearCredentialOnApply
        if shouldDeleteCredential {
            do {
                try await deleteCredentialIfPossible(
                    settingsID: state.id,
                    credentialID: engineProfileID(for: state.profile)
                )
            } catch {
                guard let currentIndex = indexOfProfile(id: id) else { return }
                profiles[currentIndex].connectionState = .failed
                profiles[currentIndex].detailMessage = error.localizedDescription
                lastRepositoryError = error.localizedDescription
                bumpSyncRevision()
                return
            }
        }
        if state.id == ProviderRepositoryDefaults.anthropicLegacyProfileID {
            Keychain.clear(.apiBase)
            Keychain.clear(.model)
        }
        guard let currentIndex = indexOfProfile(id: id) else { return }
        profiles.remove(at: currentIndex)
        normalizeDefaults()
        sanitizeRoutingSettings(persist: false)
        persistProfiles()
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
            enabledProfileIDs: enabledIDs
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
        credentialID: String
    ) async throws {
        if let commandSubmitter {
            let operationID = takeOperationID()
            pendingOperations[operationID] = .set(
                target: .init(settingsID: settingsID, credentialID: credentialID)
            )
            if let index = indexOfProfile(id: settingsID) {
                profiles[index].operationInFlight = true
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
            if let index = indexOfProfile(id: settingsID) {
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
        credentialID: String
    ) async throws {
        if let commandSubmitter {
            let operationID = takeOperationID()
            pendingOperations[operationID] = .delete(
                target: .init(settingsID: settingsID, credentialID: credentialID)
            )
            if let index = indexOfProfile(id: settingsID) {
                profiles[index].operationInFlight = true
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
            if let index = indexOfProfile(id: settingsID) {
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
        switch presetID {
        case "anthropic":
            return .init(providerType: "anthropic", envVar: "ANTHROPIC_API_KEY")
        case "google":
            return .init(providerType: "gemini", envVar: "GEMINI_API_KEY")
        case "openai":
            return .init(providerType: "openai-responses", envVar: "OPENAI_API_KEY")
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

    private func persistProfiles() {
        do {
            let directory = persistenceURL.deletingLastPathComponent()
            try fileManager.createDirectory(at: directory, withIntermediateDirectories: true, attributes: nil)
            let encoder = JSONEncoder()
            encoder.outputFormatting = [.withoutEscapingSlashes]
            let data = try encoder.encode(
                ProviderPersistenceEnvelope(version: 2, profiles: profiles.map(\.profile), routing: routingSettings)
            )
            try data.write(to: persistenceURL, options: [.atomic])
            lastRepositoryError = nil
        } catch {
            lastRepositoryError = error.localizedDescription
        }
        bumpSyncRevision()
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
