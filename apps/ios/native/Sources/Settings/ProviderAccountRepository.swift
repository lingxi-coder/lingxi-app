import Foundation
import Observation

/// A Host-owned identity. Connections describe endpoints; accounts describe
/// independent credentials and retain their identity when priority changes.
struct ProviderAccount: Codable, Equatable, Identifiable {
    let accountId: String
    let providerId: String
    let displayName: String
    let authKind: String
    let executionBindingId: String
    let enabled: Bool
    let userPriority: UInt32
    let configRevision: UInt64
    let identityGeneration: UInt64
    var id: String { accountId }
}

struct ProviderAccountHealth: Codable, Equatable {
    let accountId: String
    let identityGeneration: UInt64
    let status: String
    let reason: String?
    let cooldownUntilMs: UInt64?
    let healthRevision: UInt64

    var displayStatus: String {
        switch status {
        case "ready": return String(localized: "Ready")
        case "needs_auth": return String(localized: "Sign-in required")
        case "cooling_down": return String(localized: "Cooling down")
        case "quota_exhausted": return String(localized: "Quota exhausted")
        case "storage_unavailable": return String(localized: "Secure storage unavailable")
        case "disabled": return String(localized: "Disabled")
        default: return status
        }
    }

    var displayReason: String? {
        guard let reason, !["credential_missing", "storage_unavailable", "secure_storage_unavailable"].contains(reason) else { return nil }
        return reason.replacingOccurrences(of: "_", with: " ")
    }
}

struct ProviderAccountModelEntitlement: Codable, Equatable, Identifiable {
    let accountId: String
    let identityGeneration: UInt64
    let modelId: String
    let status: String
    let observedAtMs: UInt64
    let retryAtMs: UInt64?
    let reason: String?
    var id: String { "\(accountId)/\(identityGeneration)/\(modelId)" }
    var displayStatus: String {
        switch status {
        case "confirmed": String(localized: "Model access confirmed")
        case "forbidden": String(localized: "Model access forbidden")
        default: String(localized: "Model access unverified")
        }
    }
}

struct ProviderAccountModelAvailability: Codable, Equatable {
    let providerId: String
    let executionBindingId: String
    let modelId: String
    let eligibleAccountIds: [String]
    let confirmedAccountIds: [String]
    let unverifiedAccountIds: [String]
    let forbiddenAccountIds: [String]
}

struct ProviderAccountScopedHealth: Codable, Equatable, Identifiable {
    let accountId: String
    let identityGeneration: UInt64
    let scopeKind: String
    let scopeId: String
    let unavailableUntilMs: UInt64?
    let reason: String
    var id: String { "\(accountId)|\(identityGeneration)|\(scopeKind)|\(scopeId)" }
}

struct ProviderAccountSnapshot: Codable, Equatable {
    let storeScope: String
    let configRevision: UInt64
    let accounts: [ProviderAccount]
    let health: [ProviderAccountHealth]
    var providerPolicies: [String: ProviderAccountPolicy]? = nil
    var modelEntitlements: [ProviderAccountModelEntitlement] = []
    var modelAvailability: [ProviderAccountModelAvailability] = []
    var scopedHealth: [ProviderAccountScopedHealth] = []

    static let empty = Self(storeScope: "", configRevision: 0, accounts: [], health: [])
}

struct ProviderAccountPolicy: Codable, Equatable {
    let allowSubscriptionToMetered: Bool
}

private struct ProviderAccountRoutingObservation: Decodable {
    struct Failure: Decodable { let accountId: String }
    let status: String
    let selectedAccountId: String?
    let identityGeneration: UInt64?
    let failedAccounts: [Failure]
}

struct ProviderAccountCommand: Encodable {
    let action: String
    var operationId: UInt64?
    var expectedRevision: UInt64 = 0
    var providerId: String?
    var accountId: String?
    var displayName: String?
    var executionBindingId: String?
    var authKind: String?
    var accountIds: [String]?
    var enabled: Bool?
    var flowId: String?
    var allowSubscriptionToMetered: Bool?

    func encoded() throws -> String {
        String(decoding: try JSONEncoder().encode(self), as: UTF8.self)
    }
}

struct ProviderAccountDeviceFlow: Identifiable, Equatable {
    let id: String
    let accountId: String
    let userCode: String
    let verificationURL: URL
    let expiresAt: Date
}

private struct ProviderAccountDeviceGrant: Decodable {
    let mode: String
    let flowId: String
    let accountId: String
    let userCode: String
    let verificationURL: String
    let intervalSeconds: UInt64
    let expiresInSeconds: UInt64
    enum CodingKeys: String, CodingKey {
        case mode
        case flowId = "flow_id", accountId = "account_id", userCode = "user_code"
        case verificationURL = "verification_url", intervalSeconds = "interval_seconds", expiresInSeconds = "expires_in_seconds"
    }
}

private struct ProviderAccountDevicePoll: Decodable {
    let status: String
    let retryAfterSeconds: UInt64
    let snapshot: ProviderAccountSnapshot?
    enum CodingKeys: String, CodingKey { case status, snapshot; case retryAfterSeconds = "retry_after_seconds" }
}

enum ProviderAccountError: LocalizedError {
    case unavailable
    case operationInProgress
    case invalidResponse
    case authorizationExpired

    var errorDescription: String? {
        switch self {
        case .unavailable: return String(localized: "Account management is unavailable for this Host.")
        case .operationInProgress: return String(localized: "An account operation is already in progress.")
        case .authorizationExpired: return String(localized: "Authorization code expired. Reconnect to get a new code.")
        case .invalidResponse: return String(localized: "The Host returned an invalid account response.")
        }
    }
}

@MainActor
@Observable
final class ProviderAccountRepository {
    typealias SnapshotLoader = () async throws -> String
    typealias CommandHandler = (String, String?) async throws -> String
    static let shared = ProviderAccountRepository()

    private(set) var snapshot = ProviderAccountSnapshot.empty
    private(set) var busy = false
    private(set) var snapshotReadFailed = false
    private(set) var errorMessage: String?
    private(set) var oauthSupported = true
    private var loader: SnapshotLoader?
    private var commandHandler: CommandHandler?
    private(set) var deviceFlow: ProviderAccountDeviceFlow?
    private var devicePollTask: Task<Void, Never>?
    private var cancelDeviceGrant: (() -> Void)?
    private let devicePollDelay: (UInt64) async throws -> Void
    private let recoveryDelay: (UInt64) async throws -> Void
    private let nowMilliseconds: () -> UInt64
    private var recoveryTask: Task<Void, Never>?
    private var recoveryRevision: UInt64 = 0
    private var foreground = true

    init(devicePollDelay: @escaping (UInt64) async throws -> Void = { try await Task.sleep(for: .seconds($0)) },
         nowMilliseconds: @escaping () -> UInt64 = { UInt64(max(0, Date().timeIntervalSince1970 * 1000)) },
         recoveryDelay: @escaping (UInt64) async throws -> Void = { try await Task.sleep(for: .milliseconds($0)) }) {
        self.devicePollDelay = devicePollDelay
        self.nowMilliseconds = nowMilliseconds
        self.recoveryDelay = recoveryDelay
    }

    private(set) var generation: UInt64 = 0
    private var readRevision: UInt64 = 0
    private var lastSelections: [String: (accountID: String, identityGeneration: UInt64)] = [:]

    var available: Bool { loader != nil && commandHandler != nil }

    func configure(load: SnapshotLoader?, command: CommandHandler?, oauthSupported: Bool = true) {
        cancelDeviceOAuth()
        cancelRecoveryRefresh()
        generation &+= 1
        readRevision &+= 1
        loader = load
        commandHandler = command
        self.oauthSupported = oauthSupported
        snapshot = .empty
        snapshotReadFailed = false
        busy = false
        errorMessage = nil
        lastSelections = [:]
    }

    func setForeground(_ active: Bool) {
        guard foreground != active else { return }
        foreground = active
        cancelRecoveryRefresh()
        if active { Task { await refresh() } }
    }

    private func cancelRecoveryRefresh() {
        recoveryTask?.cancel()
        recoveryTask = nil
        recoveryRevision &+= 1
    }

    /// Refresh Host metadata at its next known recovery deadline. This never
    /// probes a provider or decides whether an account/model is eligible.
    private func scheduleRecoveryRefresh() {
        cancelRecoveryRefresh()
        guard foreground, loader != nil else { return }
        let now = nowMilliseconds()
        let identities = Dictionary(uniqueKeysWithValues: snapshot.accounts.filter(\.enabled).map { ($0.accountId, $0.identityGeneration) })
        let deadlines = snapshot.health.filter { identities[$0.accountId] == $0.identityGeneration }.compactMap(\.cooldownUntilMs)
            + snapshot.modelEntitlements.filter { identities[$0.accountId] == $0.identityGeneration }.compactMap(\.retryAtMs)
            + snapshot.scopedHealth.filter { identities[$0.accountId] == $0.identityGeneration }.compactMap(\.unavailableUntilMs)
        let milliseconds: UInt64
        if let deadline = deadlines.filter({ $0 > now }).min() {
            // Bound waits across remote clock skew without probing a provider.
            milliseconds = min(deadline - now, 30_000)
        } else if !identities.isEmpty,
                  snapshot.modelAvailability.allSatisfy({ $0.eligibleAccountIds.isEmpty })
                    || (deadlines.contains(where: { $0 <= now }) && snapshot.modelAvailability.contains(where: { $0.eligibleAccountIds.isEmpty })) {
            // A snapshot may arrive after its TTL, or a remote clock may differ.
            // Bound metadata retries while blocked; never spin on an expired timestamp.
            milliseconds = 30_000
        } else { return }
        let binding = generation
        let revision = recoveryRevision
        let delay = recoveryDelay
        recoveryTask = Task { [weak self] in
            do { try await delay(milliseconds) } catch { return }
            guard !Task.isCancelled, let self, self.foreground,
                  self.generation == binding, self.recoveryRevision == revision else { return }
            await self.refresh()
        }
    }

    func accounts(providerId: String) -> [ProviderAccount] {
        snapshot.accounts.filter { $0.providerId == providerId }.sorted {
            $0.userPriority == $1.userPriority ? $0.accountId < $1.accountId : $0.userPriority < $1.userPriority
        }
    }

    func configurationDescription(executionBindingId: String) -> String {
        let providerId = snapshot.modelAvailability.first { $0.executionBindingId == executionBindingId }?.providerId
            ?? snapshot.accounts.first { $0.executionBindingId == executionBindingId }?.providerId
            ?? executionBindingId
        return configurationDescription(providerId: providerId)
    }

    func configurationDescription(providerId: String) -> String {
        if snapshotReadFailed { return String(localized: "Unable to read account status") }
        if snapshot.storeScope.isEmpty { return String(localized: "Loading accounts…") }
        let logicalID = providerId == "openai-chatgpt" ? "openai" : providerId
        let rows = accounts(providerId: logicalID)
        if rows.isEmpty { return String(localized: "settings_provider_unconfigured") }
        let enabled = rows.filter(\.enabled)
        let statuses = enabled.compactMap { health(for: $0)?.status }
        let label: String
        if enabled.isEmpty { label = String(localized: "Disabled") }
        else if statuses.contains("ready") { label = String(localized: "settings_provider_status_configured") }
        else if statuses.contains("storage_unavailable") { label = String(localized: "Secure storage unavailable") }
        else if statuses.contains("cooling_down") || statuses.contains("quota_exhausted") { label = String(localized: "Temporarily unavailable") }
        else if statuses.contains("needs_auth") { label = String(localized: "Sign-in required") }
        else { label = String(localized: "Status unverified") }
        return "\(label) · \(enabled.count)/\(rows.count)"
    }

    func health(for account: ProviderAccount) -> ProviderAccountHealth? {
        snapshot.health.filter {
            $0.accountId == account.accountId && $0.identityGeneration == account.identityGeneration
        }.max { $0.healthRevision < $1.healthRevision }
    }

    func wasLastUsed(_ account: ProviderAccount) -> Bool {
        guard let selection = lastSelections[account.providerId] else { return false }
        return selection.accountID == account.accountId && selection.identityGeneration == account.identityGeneration
    }

    /// Execution Host evidence only. UI never predicts or selects an account.
    func recordRouting(_ json: String) {
        guard let event = try? JSONDecoder().decode(ProviderAccountRoutingObservation.self, from: Data(json.utf8)) else { return }
        if let id = event.selectedAccountId, let identity = event.identityGeneration,
           let account = snapshot.accounts.first(where: { $0.accountId == id && $0.identityGeneration == identity }) {
            lastSelections[account.providerId] = (id, identity)
        }
        if event.status == "all_unavailable" || !event.failedAccounts.isEmpty {
            Task { await refresh() }
        }
    }

    func refreshAfterCatalogPublication() {
        let binding = generation
        Task { [weak self] in
            guard let self, self.foreground, self.generation == binding else { return }
            await self.refresh()
        }
    }

    @discardableResult
    func refresh() async -> Bool {
        guard let loader else { return false }
        let binding = generation
        readRevision &+= 1
        let read = readRevision
        do {
            let result = try Self.decode(try await loader())
            guard binding == generation, read == readRevision, !Task.isCancelled else { return false }
            accept(result)
            snapshotReadFailed = false
            errorMessage = nil
            return true
        } catch {
            guard binding == generation, read == readRevision, !Task.isCancelled else { return false }
            snapshotReadFailed = true
            errorMessage = error.localizedDescription
            return false
        }
    }

    /// Secrets cross a separate FFI argument and never enter account JSON or
    /// native preferences. The Rust authority owns Keychain slots and fencing.
    @discardableResult
    func perform(_ command: ProviderAccountCommand, credential: String? = nil) async -> Bool {
        if command.action == "begin_oauth" {
            guard oauthSupported else { errorMessage = ProviderAccountError.unavailable.localizedDescription; return false }
            guard deviceFlow == nil else { errorMessage = ProviderAccountError.operationInProgress.localizedDescription; return false }
        }
        guard !busy else { errorMessage = ProviderAccountError.operationInProgress.localizedDescription; return false }
        guard let commandHandler else { errorMessage = ProviderAccountError.unavailable.localizedDescription; return false }
        let binding = generation
        readRevision &+= 1
        busy = true
        errorMessage = nil
        defer { if binding == generation { busy = false } }
        var request = command
        request.expectedRevision = snapshot.configRevision
        // Remote Desktop parses JSON in JavaScript before dispatching to Rust.
        request.operationId = UInt64.random(in: 1...9_007_199_254_740_991)
        do {
            let response = try await commandHandler(request.encoded(), credential)
            guard binding == generation, !Task.isCancelled else {
                if request.action == "begin_oauth", let grant = try? JSONDecoder().decode(ProviderAccountDeviceGrant.self, from: Data(response.utf8)) {
                    let cancel = ProviderAccountCommand(action: "cancel_oauth", expectedRevision: request.expectedRevision,
                        accountId: grant.accountId, flowId: grant.flowId)
                    _ = try? await commandHandler(cancel.encoded(), nil)
                }
                return false
            }
            if request.action == "begin_oauth" {
                try startDeviceOAuth(response, request: request, handler: commandHandler, binding: binding)
                return true
            }
            let result = try Self.decode(response)
            readRevision &+= 1
            accept(result)
            // Metadata mutation responses may omit derived presence/health.
            // Re-read the Host rather than inferring authentication from add.
            await refresh()
            guard binding == generation else { return false }
            return true
        } catch {
            guard binding == generation else { return false }
            // A revision conflict must refresh the authoritative set before
            // another edit. Never retry a mutation against a new revision.
            await refresh()
            guard binding == generation else { return false }
            errorMessage = error.localizedDescription
            return false
        }
    }

    func cancelDeviceOAuth() {
        devicePollTask?.cancel()
        devicePollTask = nil
        let cancel = cancelDeviceGrant
        cancelDeviceGrant = nil
        deviceFlow = nil
        cancel?()
    }

    private func startDeviceOAuth(_ response: String, request: ProviderAccountCommand,
                                  handler: @escaping CommandHandler, binding: UInt64) throws {
        let grant = try JSONDecoder().decode(ProviderAccountDeviceGrant.self, from: Data(response.utf8))
        guard grant.mode == "device_code", grant.accountId == request.accountId,
              !grant.flowId.isEmpty, !grant.userCode.isEmpty,
              let url = URL(string: grant.verificationURL), url.scheme == "https", url.host != nil,
              (1...60).contains(grant.intervalSeconds), (1...3600).contains(grant.expiresInSeconds) else {
            throw ProviderAccountError.invalidResponse
        }
        let flow = ProviderAccountDeviceFlow(id: grant.flowId, accountId: grant.accountId,
            userCode: grant.userCode, verificationURL: url,
            expiresAt: Date().addingTimeInterval(Double(grant.expiresInSeconds)))
        deviceFlow = flow
        cancelDeviceGrant = {
            Task {
                let cancel = ProviderAccountCommand(action: "cancel_oauth", expectedRevision: request.expectedRevision,
                    accountId: flow.accountId, flowId: flow.id)
                _ = try? await handler(cancel.encoded(), nil)
            }
        }
        devicePollTask = Task { @MainActor [weak self] in
            guard let self else { return }
            var delay = grant.intervalSeconds
            do {
                while !Task.isCancelled {
                    try await self.devicePollDelay(delay)
                    try Task.checkCancellation()
                    guard self.generation == binding, self.deviceFlow?.id == flow.id else { return }
                    guard Date() < flow.expiresAt else { throw ProviderAccountError.authorizationExpired }
                    let command = ProviderAccountCommand(action: "poll_oauth", expectedRevision: request.expectedRevision,
                        accountId: flow.accountId, flowId: flow.id)
                    let raw = try await handler(command.encoded(), nil)
                    try Task.checkCancellation()
                    guard self.generation == binding, self.deviceFlow?.id == flow.id else { return }
                    let result = try JSONDecoder().decode(ProviderAccountDevicePoll.self, from: Data(raw.utf8))
                    if result.status == "completed", let snapshot = result.snapshot {
                        self.readRevision &+= 1
                        self.accept(snapshot)
                        self.deviceFlow = nil
                        self.cancelDeviceGrant = nil
                        self.devicePollTask = nil
                        await self.refresh()
                        return
                    }
                    guard result.status == "pending", (1...60).contains(result.retryAfterSeconds) else {
                        throw ProviderAccountError.invalidResponse
                    }
                    delay = result.retryAfterSeconds
                }
            } catch is CancellationError {
            } catch {
                guard self.generation == binding, self.deviceFlow?.id == flow.id else { return }
                self.cancelDeviceOAuth()
                self.errorMessage = error.localizedDescription
            }
        }
    }

    func scopedHealth(for account: ProviderAccount) -> [ProviderAccountScopedHealth] {
        snapshot.scopedHealth.filter {
            $0.accountId == account.accountId && $0.identityGeneration == account.identityGeneration
        }.sorted { $0.id < $1.id }
    }

    func modelEntitlements(for account: ProviderAccount) -> [ProviderAccountModelEntitlement] {
        snapshot.modelEntitlements.filter {
            $0.accountId == account.accountId && $0.identityGeneration == account.identityGeneration
        }.sorted { $0.modelId.localizedStandardCompare($1.modelId) == .orderedAscending }
    }

    func modelAvailability(bindingId: String, modelId: String) -> ProviderAccountModelAvailability? {
        snapshot.modelAvailability.first { $0.executionBindingId == bindingId && $0.modelId == modelId }
    }

    func modelIsAvailable(reference: String, canonicalModelId: String? = nil) -> Bool {
        guard !snapshot.storeScope.isEmpty else { return true }
        let parts = reference.split(separator: "/", maxSplits: 1).map(String.init)
        guard parts.count == 2 else { return false }
        return modelAvailability(bindingId: parts[0], modelId: canonicalModelId ?? parts[1])?.eligibleAccountIds.isEmpty == false
    }

    func modelAccessDescription(bindingId: String, modelId: String) -> String {
        guard let row = modelAvailability(bindingId: bindingId, modelId: modelId) else {
            return String(localized: "Status unverified")
        }
        if row.eligibleAccountIds.isEmpty && row.confirmedAccountIds.isEmpty
            && row.unverifiedAccountIds.isEmpty && row.forbiddenAccountIds.isEmpty {
            return String(localized: "No eligible account currently")
        }
        let groups = [(row.confirmedAccountIds, String(localized: "Model access confirmed")),
            (row.unverifiedAccountIds, String(localized: "Model access unverified")),
            (row.forbiddenAccountIds, String(localized: "Model access forbidden"))]
        let labels = groups.flatMap { ids, status in
            ids.compactMap { id -> String? in
                guard let account = snapshot.accounts.first(where: { $0.accountId == id }) else { return nil }
                return "\(account.displayName): \(status)"
            }
        }
        return labels.isEmpty ? String(localized: "Model access unverified") : labels.joined(separator: " · ")
    }

    private func accept(_ result: ProviderAccountSnapshot) {
        guard snapshot.storeScope != result.storeScope || result.configRevision >= snapshot.configRevision else { return }
        snapshot = result
        scheduleRecoveryRefresh()
    }

    private static func decode(_ json: String) throws -> ProviderAccountSnapshot {
        try JSONDecoder().decode(ProviderAccountSnapshot.self, from: Data(json.utf8))
    }
}
