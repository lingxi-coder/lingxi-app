import Foundation
import Observation

/// Settings file destinations deliberately exclude MCP's independent scopes.
enum DesktopSettingsLayer: String, CaseIterable, Identifiable {
    case user, project, local, managed
    var id: String { rawValue }
    var title: String { String(localized: String.LocalizationValue(stringLiteral: "settings_parity_\(rawValue)_layer")) }
    var destination: SettingsDestinationDto? {
        switch self { case .user: .user; case .project: .project; case .local: .local; case .managed: nil }
    }
}

@MainActor @Observable
final class DesktopSettingsRepository {
    static let shared = DesktopSettingsRepository()
    private(set) var connected = false
    private(set) var loaded = false
    private(set) var saving = false
    private(set) var revision = 0
    private(set) var pendingSettingsKeys: Set<String> = []
    private(set) var restartRequested = false
    var needsReconnect: Bool { restartRequested || !pendingSettingsKeys.isEmpty }
    private(set) var sourceGeneration = 0
    private(set) var providerModels: [ProviderModelCatalogEntryDto] = []
    private(set) var effective: [String: Any] = [:]
    private(set) var active: [String: Any] = [:]
    private(set) var layers: [String: [String: Any]] = [:]
    private(set) var provenance: [String: String] = [:]
    private(set) var locked: Set<String> = []
    private(set) var mergedKeys: Set<String> = []
    private(set) var filesJSON = "[]"
    private(set) var auth: AuthStateDto?
    private(set) var doctor: DoctorReportDto?
    private(set) var credentialStates: [String: Bool] = [:]
    private(set) var credentialStorageEncrypted: Bool?
    private(set) var catalogs: [String: String] = [:]
    private(set) var documents: [String: String] = [:]
    var errorMessage: String?
    private(set) var statusMessage: String?
    @ObservationIgnored private var submitter: ((ClientCommand) async throws -> Void)?
    @ObservationIgnored private var pending: (key: String, json: String, layer: DesktopSettingsLayer)?
    @ObservationIgnored private var operations: Set<UInt64> = []
    @ObservationIgnored private var readRequests: Set<String> = []
    @ObservationIgnored private var credentialRequests: [UInt64: (ids: [String], expected: Bool?)] = [:]
    @ObservationIgnored private var nextOperation: UInt64 = 8_400_000
    @ObservationIgnored private var confirmationTimeout: Task<Void, Never>?

    func configure(submitter: ((ClientCommand) async throws -> Void)?) {
        confirmationTimeout?.cancel()
        sourceGeneration += 1
        providerModels = []
        self.submitter = submitter
        connected = submitter != nil
        loaded = false
        pendingSettingsKeys = []
        restartRequested = false
        layers = [:]
        effective = [:]
        active = [:]
        locked = []
        provenance = [:]
        mergedKeys = []
        filesJSON = "[]"
        statusMessage = nil
        errorMessage = nil
        catalogs = [:]
        documents = [:]
        credentialStates = [:]
        credentialStorageEncrypted = nil
        credentialRequests = [:]
        auth = nil
        doctor = nil
        if saving { errorMessage = "The engine connection changed before the save was confirmed." }
        saving = false
        pending = nil
        operations = []
        readRequests = []
        if connected { Task { await refresh() } }
    }

    func refresh() async {
        guard connected else { return }
        readRequests.insert("settings")
        await send(.refreshListings(which: [.settings, .auth, .doctor, .models]))
    }

    func ownValue(key: String, layer: DesktopSettingsLayer) -> Any? { layers[layer.rawValue]?[key] }
    func provenanceLabel(for key: String) -> String {
        mergedKeys.contains(key) ? String(localized: "settings_parity_merged_layers") : (provenance[key] ?? String(localized: "settings_parity_default"))
    }
    func canEdit(key: String, layer: DesktopSettingsLayer) -> Bool {
        connected && loaded && layer != .managed && !locked.contains(key) && layers[layer.rawValue] != nil && !saving && layerParsed(layer)
    }
    func layerIssue(_ layer: DesktopSettingsLayer) -> String? {
        if layer == .managed { return String(localized: "settings_parity_managed_readonly") }
        guard loaded else { return nil }
        if !layerParsed(layer) { return "This settings file contains invalid JSON. Repair it before editing this layer." }
        if layers[layer.rawValue] == nil { return String(localized: "settings_parity_layer_unavailable") }
        return nil
    }

    private func layerParsed(_ layer: DesktopSettingsLayer) -> Bool {
        guard let files = try? JSONSerialization.jsonObject(with: Data(filesJSON.utf8)) as? [[String: Any]],
              let file = files.first(where: { $0["layer"] as? String == layer.rawValue }) else { return true }
        return file["parsed"] as? Bool != false
    }

    static func json(_ value: Any) -> String {
        guard let data = try? JSONSerialization.data(withJSONObject: value, options: [.prettyPrinted, .sortedKeys, .fragmentsAllowed]),
              let string = String(data: data, encoding: .utf8) else { return "null" }
        return string
    }
    static func object(_ json: String) -> [String: Any]? {
        (try? JSONSerialization.jsonObject(with: Data(json.utf8))) as? [String: Any]
    }

    func save(key: String, json: String, layer: DesktopSettingsLayer, command: ClientCommand? = nil) async {
        guard canEdit(key: key, layer: layer), let destination = layer.destination, let submitter else {
            errorMessage = "Connect the engine and load a writable layer. Managed values are read-only."
            return
        }
        let generation = sourceGeneration
        do {
            let value = try JSONSerialization.jsonObject(with: Data(json.utf8), options: [.fragmentsAllowed])
            if key == "providers", let providers = value as? [String: [String: Any]] {
                let credentialKeys: Set<String> = ["apiKey", "api_key", "apiToken", "accessToken", "secretValue"]
                if providers.values.contains(where: { provider in
                    provider.contains { credentialKeys.contains($0.key) && !($0.value is NSNull) }
                }) {
                    throw NSError(domain: "DesktopSettings", code: 1, userInfo: [NSLocalizedDescriptionKey:
                        String(localized: "settings_parity_provider_validation_secret")])
                }
            }
            let patch = Self.json([key: value])
            pending = (key, Self.json(value), layer)
            saving = true
            startConfirmationTimeout()
            errorMessage = nil
            statusMessage = "Waiting for engine confirmation…"
            try await submitter(command ?? .updateSettings(destination: destination, patchJson: patch))
            guard generation == sourceGeneration else { return }
            try await submitter(.refreshListings(which: [.settings]))
        } catch { if generation == sourceGeneration { fail(error.localizedDescription) } }
    }

    func send(_ command: ClientCommand) async {
        guard let submitter else { errorMessage = "Connect to an engine to load or change these settings."; return }
        let generation = sourceGeneration
        do { try await submitter(command) } catch { if generation == sourceGeneration { fail(error.localizedDescription) } }
    }

    func admin(domain: String, action: String, target: String? = nil, scope: String? = nil,
               revision: String? = nil, payload: String? = nil, mutation: Bool = false) async {
        guard connected else { return }
        guard !saving else { errorMessage = "Wait for the current engine operation to finish."; return }
        if scope == "managed" && mutation { errorMessage = "Managed configuration is read-only."; return }
        nextOperation += 1
        let operation = nextOperation
        if !["get_catalog", "get_snapshot", "get_document"].contains(action) { operations.insert(operation) } else { readRequests.insert(domain) }
        if mutation { saving = true; startConfirmationTimeout(); statusMessage = "Waiting for engine confirmation…" }
        errorMessage = nil
        let command: ClientCommand
        switch domain {
        case "skill": command = .skillAdmin(command: .init(action: action, operationId: operation, target: target, scope: scope, revision: revision, payloadJson: payload))
        case "mcp": command = .mcpAdmin(command: .init(action: action, operationId: operation, target: target, scope: scope, revision: revision, payloadJson: payload))
        case "plugin": command = .pluginAdmin(command: .init(action: action, operationId: operation, target: target, scope: scope, revision: revision, payloadJson: payload))
        case "hook": command = .hookAdmin(command: .init(action: action, operationId: operation, target: target, scope: scope, revision: revision, payloadJson: payload))
        default: fail("Unknown configuration domain."); return
        }
        await send(command)
    }

    /// Used by multi-step import so a command submission never masquerades
    /// as a confirmed write. Existing command handlers own correlation/timeouts.
    func waitForConfirmation(generation: Int) async throws {
        while saving && generation == sourceGeneration {
            try Task.checkCancellation()
            try await Task.sleep(for: .milliseconds(40))
        }
        try Task.checkCancellation()
        guard generation == sourceGeneration else { throw ProviderBulkImport.ImportError("The engine connection changed before confirmation.") }
        if let errorMessage { throw ProviderBulkImport.ImportError(errorMessage) }
    }

    func refreshCredentials(providerIDs: [String] = []) async {
        guard connected else { return }
        let ids = Array(Set(providerIDs + providerModels.map(\.providerId) + Array((effective["providers"] as? [String: Any] ?? [:]).keys)))
            .map { $0 == "builtin" ? "anthropic" : $0 }.sorted()
        guard !ids.isEmpty else { return }
        nextOperation += 1
        let operation = nextOperation
        credentialRequests[operation] = (ids, nil)
        await send(.listProviderCredentials(operationId: operation, providerIds: ids, previewProviderIds: []))
    }

    func saveCredential(providerID: String, secret: String?) async {
        guard connected, !saving, !providerID.isEmpty else { return }
        nextOperation += 1
        let operation = nextOperation
        credentialRequests[operation] = ([providerID], secret != nil)
        saving = true
        errorMessage = nil
        statusMessage = "Waiting for secure storage confirmation…"
        startConfirmationTimeout()
        if let secret {
            await send(.setProviderCredential(operationId: operation, providerId: providerID, credential: .init(value: secret)))
        } else {
            await send(.deleteProviderCredential(operationId: operation, providerId: providerID))
        }
    }

    func consume(_ event: ClientEvent) {
        switch event {
        case let .settingsSnapshot(effectiveJson, provenanceJson, filesJson, activeJson, locked, layersJson, mergedKeys):
            guard let effective = Self.object(effectiveJson) else { fail("The engine returned invalid settings."); return }
            readRequests.remove("settings")
            self.effective = effective
            active = activeJson.flatMap(Self.object) ?? [:]
            provenance = Self.object(provenanceJson) as? [String: String] ?? [:]
            layers = layersJson.flatMap(Self.object) as? [String: [String: Any]] ?? [:]
            self.locked = Set(locked ?? [])
            self.mergedKeys = Set(mergedKeys ?? [])
            if activeJson.flatMap(Self.object) != nil {
                let candidateKeys = Set(effective.keys).union(active.keys)
                pendingSettingsKeys = Set(candidateKeys.filter { key in
                    !self.locked.contains(key) && provenance[key] != "managed"
                        && Self.json(effective[key] ?? NSNull()) != Self.json(active[key] ?? NSNull())
                })
            }
            filesJSON = filesJson ?? "[]"
            loaded = true
            revision += 1
            if let pending {
                let actual = ownValue(key: pending.key, layer: pending.layer) ?? NSNull()
                if Self.json(actual) == pending.json {
                    confirmationTimeout?.cancel()
                    saving = false
                    self.pending = nil
                    restartRequested = true
                    statusMessage = "Saved and confirmed by the engine. Apply saved changes to refresh this connection."
                } else {
                    fail("The engine snapshot does not match the requested change. Review the layer and retry.")
                }
            }
        case let .providerModelCatalog(providers):
            providerModels = providers
            Task { await refreshCredentials() }
        case let .providerCredentialStatus(operationId, configured, unavailable, encrypted, _, error):
            guard let request = credentialRequests.removeValue(forKey: operationId) else { return }
            credentialStorageEncrypted = encrypted
            for id in request.ids { credentialStates[id] = unavailable.contains(id) ? nil : configured.contains(id) }
            if let expected = request.expected {
                confirmationTimeout?.cancel()
                saving = false
                if let error { fail(error) }
                else if request.ids.contains(where: { unavailable.contains($0) || configured.contains($0) != expected }) {
                    fail("Secure storage did not confirm the requested credential change.")
                } else { restartRequested = true; statusMessage = "Credential change confirmed by secure storage." }
            } else if let error { errorMessage = error }
        case let .authState(state): auth = state
        case let .doctorReport(report): doctor = report
        case let .skillCatalog(json): readRequests.remove("skill"); catalogs["skill"] = json
        case let .pluginCatalog(json): readRequests.remove("plugin"); catalogs["plugin"] = json
        case let .mcpConfigurationSnapshot(json): readRequests.remove("mcp"); catalogs["mcp"] = json
        case let .skillDocument(json): readRequests.remove("skill"); documents["skill"] = json
        case let .configurationOperation(domain, operationId, status, effect, message, detailsJson):
            let name = String(describing: domain)
            if operationId == 0, name == "hook", let detailsJson {
                readRequests.remove("hook")
                documents["hook"] = detailsJson
                return
            }
            guard operations.contains(operationId) else { return }
            if let detailsJson { documents[name] = detailsJson }
            if status == .succeeded || status == .failed {
                operations.remove(operationId)
                confirmationTimeout?.cancel()
                saving = false
                if status == .failed { errorMessage = message ?? "Configuration operation failed." }
                else {
                    if effect == .restartRequired { restartRequested = true }
                    statusMessage = message ?? (effect == .restartRequired ? String(localized: "settings_parity_saved_pending") : nil)
                }
            }
        case let .error(_, message):
            if saving { fail(message) }
            else if connected && !readRequests.isEmpty { errorMessage = message; readRequests = [] }
        default: break
        }
    }

    private func startConfirmationTimeout() {
        confirmationTimeout?.cancel()
        let generation = sourceGeneration
        confirmationTimeout = Task { [weak self] in
            try? await Task.sleep(for: .seconds(20))
            guard !Task.isCancelled, let self, self.sourceGeneration == generation, self.saving else { return }
            self.fail("The engine has not confirmed this operation. Refresh before retrying; the change may already have been written.")
        }
    }

    private func fail(_ message: String) {
        confirmationTimeout?.cancel()
        errorMessage = message
        statusMessage = nil
        saving = false
        pending = nil
        operations = []
        credentialRequests = credentialRequests.filter { $0.value.expected == nil }
    }
}
