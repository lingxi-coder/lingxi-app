import Foundation
import XCTest

@testable import LingxiCode

@MainActor
final class ProviderRepositoryTests: XCTestCase {
    private var tempDirectory: URL!
    private var persistenceURL: URL!

    override func setUpWithError() throws {
        tempDirectory = FileManager.default.temporaryDirectory
            .appendingPathComponent("provider-repo-tests-\(UUID().uuidString)", isDirectory: true)
        try FileManager.default.createDirectory(at: tempDirectory, withIntermediateDirectories: true)
        persistenceURL = tempDirectory.appendingPathComponent("provider-settings.json")
    }

    override func tearDownWithError() throws {
        if let tempDirectory {
            try? FileManager.default.removeItem(at: tempDirectory)
        }
    }

    func testLaunchSnapshotMapsProfilesToJSONAndDefaultModel() async throws {
        let repository = ProviderRepository(persistenceURL: persistenceURL)
        let anthropic = repository.addProfile(presetID: "anthropic")
        let openAI = repository.addProfile(presetID: "openai")

        repository.updateProfile(anthropic) {
            $0.modelID = "claude-opus-4"
            $0.baseURL = "https://api.anthropic.com"
            $0.enabled = false
        }
        repository.updateProfile(openAI) {
            $0.modelID = "gpt-4o"
            $0.baseURL = "https://api.openai.com/v1"
        }
        repository.setDefaultProfile(openAI)

        let snapshot = repository.makeLaunchSnapshot()

        XCTAssertEqual(snapshot.defaultModelID, "openai/gpt-4o")
        XCTAssertEqual(snapshot.enabledProfileIDs, ["openai"])
        XCTAssertTrue(snapshot.providerProfilesJSON.contains("\"openai\""))
        XCTAssertTrue(snapshot.providerProfilesJSON.contains("\"type\":\"openai-responses\""))
        XCTAssertTrue(snapshot.providerProfilesJSON.contains("\"apiKeyEnv\":\"OPENAI_API_KEY\""))
        XCTAssertTrue(snapshot.routingJSON.contains("\"mobileEnabledProfiles\":[\"openai\"]"))
        XCTAssertTrue(snapshot.routingJSON.contains("\"retry\":{\"backoffMs\":500,\"maxAttempts\":10}"))
        XCTAssertTrue(snapshot.providerProfilesJSON.contains("\"models\":[{\"id\":\"gpt-4o\"},{\"id\":\"gpt-4o-mini\"},{\"id\":\"o1-preview\"}]"))
    }

    func testApplyValidationRequiresCredentialForEnabledProfile() async throws {
        let repository = ProviderRepository(persistenceURL: persistenceURL)
        let openAI = repository.addProfile(presetID: "openai")

        await repository.applyChanges(openAI)

        let state = try XCTUnwrap(repository.state(for: openAI))
        XCTAssertEqual(state.connectionState, .failed)
        XCTAssertEqual(state.validationMessage, ProviderProfileValidationError.missingCredential.errorDescription)
    }

    func testPersistenceExcludesPendingSecrets() throws {
        let repository = ProviderRepository(persistenceURL: persistenceURL)
        let openAI = repository.addProfile(presetID: "openai")
        repository.updateProfile(openAI) {
            $0.baseURL = "https://proxy.example.com/v1"
            $0.modelID = "gpt-4o-mini"
        }
        repository.setRetryMaxAttempts(5)
        repository.setRetryBackoffMs(900)
        repository.stageSecret("sk-secret-should-not-persist", for: openAI)

        let data = try Data(contentsOf: persistenceURL)
        let string = try XCTUnwrap(String(data: data, encoding: .utf8))

        XCTAssertTrue(string.contains("https://proxy.example.com/v1"))
        XCTAssertTrue(string.contains("gpt-4o-mini"))
        XCTAssertTrue(string.contains("\"retryMaxAttempts\":5"))
        XCTAssertTrue(string.contains("\"retryBackoffMs\":900"))
        XCTAssertFalse(string.contains("sk-secret-should-not-persist"))
    }

    func testLegacyPersistenceWithoutRoutingLoadsDefaults() throws {
        let legacyJSON = """
        {"version":1,"profiles":[{"id":"openai","presetID":"openai","name":"OpenAI","baseURL":"https://api.openai.com/v1","modelID":"gpt-4o","enabled":true,"isDefault":true}]}
        """
        try legacyJSON.data(using: .utf8)?.write(to: persistenceURL)

        let repository = ProviderRepository(persistenceURL: persistenceURL)

        XCTAssertEqual(repository.routingSettings, ProviderRoutingSettings(retryMaxAttempts: 10, retryBackoffMs: 500, fallbackProfileIDs: []))
        XCTAssertEqual(repository.state(for: "openai")?.profile.modelID, "gpt-4o")
    }

    func testDeepSeekPresetMatchesAndroidOfficialModels() throws {
        let preset = try XCTUnwrap(Presets.llm.first(where: { $0.id == "deepseek" }))

        XCTAssertEqual(preset.defaultUrl, "https://api.deepseek.com")
        XCTAssertEqual(preset.models, ["deepseek-v4-flash", "deepseek-v4-pro"])

        let repository = ProviderRepository(persistenceURL: persistenceURL)
        let deepSeek = repository.addProfile(presetID: "deepseek")
        let profile = try XCTUnwrap(repository.state(for: deepSeek)?.profile)
        XCTAssertEqual(profile.baseURL, "https://api.deepseek.com")
        XCTAssertEqual(profile.modelID, "deepseek-v4-flash")
    }

    func testLegacyDeepSeekProfileMigratesToCurrentEndpointAndModel() throws {
        let legacyJSON = """
        {"version":2,"profiles":[{"id":"deepseek","presetID":"deepseek","name":"DeepSeek","baseURL":"https://api.deepseek.com/v1/","modelID":"deepseek-reasoner","enabled":true,"isDefault":true}],"routing":{"retryMaxAttempts":10,"retryBackoffMs":500,"fallbackProfileIDs":[]}}
        """
        try XCTUnwrap(legacyJSON.data(using: .utf8)).write(to: persistenceURL)

        let repository = ProviderRepository(persistenceURL: persistenceURL)
        let profile = try XCTUnwrap(repository.state(for: "deepseek")?.profile)

        XCTAssertEqual(profile.baseURL, "https://api.deepseek.com")
        XCTAssertEqual(profile.modelID, "deepseek-v4-flash")

        let persisted = try String(contentsOf: persistenceURL, encoding: .utf8)
        XCTAssertFalse(persisted.contains("api.deepseek.com/v1"))
        XCTAssertFalse(persisted.contains("deepseek-reasoner"))
    }

    func testRoutingJsonIncludesRetryAndOrderedFallbackForDefaultModel() throws {
        let repository = ProviderRepository(persistenceURL: persistenceURL)
        let openAI = repository.addProfile(presetID: "openai")
        let kimi = repository.addProfile(presetID: "kimi")
        let deepseek = repository.addProfile(presetID: "deepseek")

        repository.setDefaultProfile(openAI)
        repository.updateProfile(kimi) {
            $0.modelID = "kimi-k3"
            $0.enabled = true
        }
        repository.updateProfile(deepseek) {
            $0.modelID = "deepseek-v4-flash"
            $0.enabled = true
        }
        repository.setRetryMaxAttempts(4)
        repository.setRetryBackoffMs(1200)
        repository.toggleFallbackProfile(kimi)
        repository.toggleFallbackProfile(deepseek)
        repository.moveFallbackProfile(deepseek, by: -1)

        let snapshot = repository.makeLaunchSnapshot()
        let routing = try XCTUnwrap(jsonObject(from: snapshot.routingJSON))
        let retry = try XCTUnwrap(routing["retry"] as? [String: Any])
        let fallback = try XCTUnwrap(routing["fallback"] as? [String: Any])
        let targets = try XCTUnwrap(fallback["openai/gpt-4o"] as? [String])

        XCTAssertEqual(retry["maxAttempts"] as? Int, 4)
        XCTAssertEqual(retry["backoffMs"] as? Int, 1200)
        XCTAssertEqual(targets, ["deepseek/deepseek-v4-flash", "kimi/kimi-k3"])
    }

    func testRoutingFiltersInvalidFallbackSelections() throws {
        let repository = ProviderRepository(persistenceURL: persistenceURL)
        let openAI = repository.addProfile(presetID: "openai")
        let kimi = repository.addProfile(presetID: "kimi")
        let deepseek = repository.addProfile(presetID: "deepseek")

        repository.setDefaultProfile(openAI)
        repository.updateProfile(kimi) { $0.modelID = "kimi-k3" }
        repository.updateProfile(deepseek) {
            $0.modelID = ""
            $0.enabled = false
        }

        repository.toggleFallbackProfile(kimi)
        repository.toggleFallbackProfile(openAI)
        repository.toggleFallbackProfile(deepseek)

        let candidates = repository.fallbackCandidates()
        XCTAssertEqual(repository.routingSettings.fallbackProfileIDs, ["kimi"])
        XCTAssertEqual(candidates.map(\.profileID), ["kimi"])
        let snapshot = repository.makeLaunchSnapshot()
        let routing = try XCTUnwrap(jsonObject(from: snapshot.routingJSON))
        let fallback = try XCTUnwrap(routing["fallback"] as? [String: Any])
        let targets = try XCTUnwrap(fallback["openai/gpt-4o"] as? [String])
        XCTAssertEqual(targets, ["kimi/kimi-k3"])
        XCTAssertFalse(targets.contains(where: { $0.contains("openai/") || $0.contains("deepseek/") }))
    }

    func testApplyRoutingChangesInvokesReconnectWithValidatedRoutingSnapshot() async throws {
        let repository = ProviderRepository(persistenceURL: persistenceURL)
        let openAI = repository.addProfile(presetID: "openai")
        let kimi = repository.addProfile(presetID: "kimi")
        repository.setDefaultProfile(openAI)
        repository.updateProfile(kimi) { $0.modelID = "kimi-k3" }
        repository.toggleFallbackProfile(kimi)

        var appliedSnapshots: [ProviderLaunchSnapshot] = []
        repository.configure(
            submitCommand: nil,
            applyReconnect: { snapshot in
                appliedSnapshots.append(snapshot)
            }
        )

        await repository.applyRoutingChanges(
            retryMaxAttemptsText: "6",
            retryBackoffMsText: "1500"
        )

        XCTAssertEqual(appliedSnapshots.count, 1)
        let routing = try XCTUnwrap(jsonObject(from: appliedSnapshots[0].routingJSON))
        let retry = try XCTUnwrap(routing["retry"] as? [String: Any])
        XCTAssertEqual(retry["maxAttempts"] as? Int, 6)
        XCTAssertEqual(retry["backoffMs"] as? Int, 1500)
        XCTAssertFalse(repository.routingDirty)
        XCTAssertEqual(repository.routingMessage, "已应用路由并请求重连。")
    }

    func testListStatusCorrelationPreservesUnavailableConfiguredState() async throws {
        let recorder = CommandRecorder()
        let repository = ProviderRepository(persistenceURL: persistenceURL)
        let openAI = repository.addProfile(presetID: "openai")
        let kimi = repository.addProfile(presetID: "kimi")
        repository.configure(submitCommand: { command in
            try await recorder.submit(command: command)
        })

        repository.stageSecret("sk-openai", for: openAI)
        let apply = Task { await repository.applyChanges(openAI) }
        await waitForCommandCount(1, recorder: recorder)

        guard case let .setProviderCredential(operationId: setOperationID, providerId, _) = try XCTUnwrap(recorder.commands.last) else {
            return XCTFail("expected set provider credential")
        }
        XCTAssertEqual(providerId, openAI)
        repository.handle(event: .providerCredentialStatus(
            operationId: setOperationID,
            configuredProviderIds: [openAI],
            unavailableProviderIds: [],
            storageEncrypted: true,
            error: nil
        ))
        await apply.value

        await repository.refreshCredentialStatus()
        guard case let .listProviderCredentials(operationId: listOperationID, providerIds) = try XCTUnwrap(recorder.commands.last) else {
            return XCTFail("expected list provider credentials")
        }
        XCTAssertEqual(Set(providerIds), Set([openAI, kimi]))

        repository.handle(event: .providerCredentialStatus(
            operationId: listOperationID,
            configuredProviderIds: [],
            unavailableProviderIds: [openAI],
            storageEncrypted: true,
            error: "provider credential storage is unavailable"
        ))

        let openAIState = try XCTUnwrap(repository.state(for: openAI))
        let kimiState = try XCTUnwrap(repository.state(for: kimi))
        XCTAssertEqual(openAIState.credentialState, .configured)
        XCTAssertEqual(kimiState.credentialState, .missing)
    }

    func testCredentialListTimeoutClearsOnlyRecordedProfilesAndIgnoresLateEvent() async throws {
        let recorder = CommandRecorder()
        let repository = ProviderRepository(
            persistenceURL: persistenceURL,
            credentialOperationTimeout: .milliseconds(20)
        )
        let openAI = repository.addProfile(presetID: "openai")
        repository.configure(submitCommand: { command in
            try await recorder.submit(command: command)
        })

        await repository.refreshCredentialStatus()
        guard case let .listProviderCredentials(operationId: operationID, _) = try XCTUnwrap(recorder.commands.last) else {
            return XCTFail("expected list provider credentials")
        }
        XCTAssertTrue(try XCTUnwrap(repository.state(for: openAI)).operationInFlight)

        try await Task.sleep(for: .milliseconds(60))

        let timedOutState = try XCTUnwrap(repository.state(for: openAI))
        XCTAssertFalse(timedOutState.operationInFlight)
        XCTAssertEqual(repository.lastRepositoryError, "安全存储操作超时")
        let credentialBeforeLateEvent = timedOutState.credentialState

        repository.handle(event: .providerCredentialStatus(
            operationId: operationID,
            configuredProviderIds: [openAI],
            unavailableProviderIds: [],
            storageEncrypted: true,
            error: nil
        ))

        XCTAssertEqual(repository.state(for: openAI)?.credentialState, credentialBeforeLateEvent,
                       "a timed-out list response must be ignored")
    }

    func testCredentialListRefreshReplacesPreviousOperationAndIgnoresItsLateResponse() async throws {
        let recorder = CommandRecorder()
        let repository = ProviderRepository(persistenceURL: persistenceURL)
        let openAI = repository.addProfile(presetID: "openai")
        repository.configure(submitCommand: { command in
            try await recorder.submit(command: command)
        })

        await repository.refreshCredentialStatus()
        guard case let .listProviderCredentials(operationId: oldOperationID, _) = try XCTUnwrap(recorder.commands.last) else {
            return XCTFail("expected first list provider credentials")
        }
        await repository.refreshCredentialStatus()
        guard case let .listProviderCredentials(operationId: currentOperationID, _) = try XCTUnwrap(recorder.commands.last) else {
            return XCTFail("expected replacement list provider credentials")
        }
        XCTAssertNotEqual(oldOperationID, currentOperationID)

        repository.handle(event: .providerCredentialStatus(
            operationId: oldOperationID,
            configuredProviderIds: [openAI],
            unavailableProviderIds: [],
            storageEncrypted: true,
            error: nil
        ))
        var state = try XCTUnwrap(repository.state(for: openAI))
        XCTAssertEqual(state.credentialState, .unknown)
        XCTAssertTrue(state.operationInFlight,
                      "the replacement operation still owns the in-flight state")

        repository.handle(event: .providerCredentialStatus(
            operationId: currentOperationID,
            configuredProviderIds: [openAI],
            unavailableProviderIds: [],
            storageEncrypted: true,
            error: nil
        ))
        state = try XCTUnwrap(repository.state(for: openAI))
        XCTAssertEqual(state.credentialState, .configured)
        XCTAssertFalse(state.operationInFlight)
    }

    func testSetDeleteCorrelationAndErrorHandling() async throws {
        let recorder = CommandRecorder()
        let repository = ProviderRepository(persistenceURL: persistenceURL)
        let openAI = repository.addProfile(presetID: "openai")
        repository.configure(submitCommand: { command in
            try await recorder.submit(command: command)
        })

        repository.stageSecret("sk-openai", for: openAI)
        let failedApply = Task { await repository.applyChanges(openAI) }
        await waitForCommandCount(1, recorder: recorder)

        guard case let .setProviderCredential(operationId: setFailureID, _, _) = try XCTUnwrap(recorder.commands.last) else {
            return XCTFail("expected set provider credential")
        }
        repository.handle(event: .providerCredentialStatus(
            operationId: setFailureID,
            configuredProviderIds: [],
            unavailableProviderIds: [openAI],
            storageEncrypted: true,
            error: "failed to store provider credential: locked"
        ))
        await failedApply.value

        var state = try XCTUnwrap(repository.state(for: openAI))
        XCTAssertEqual(state.connectionState, .failed)
        XCTAssertEqual(state.detailMessage, "failed to store provider credential: locked")
        XCTAssertEqual(state.pendingSecret, "sk-openai")

        let successfulApply = Task { await repository.applyChanges(openAI) }
        await waitForCommandCount(2, recorder: recorder)
        guard case let .setProviderCredential(operationId: setSuccessID, _, _) = try XCTUnwrap(recorder.commands.last) else {
            return XCTFail("expected second set provider credential")
        }
        repository.handle(event: .providerCredentialStatus(
            operationId: setSuccessID,
            configuredProviderIds: [openAI],
            unavailableProviderIds: [],
            storageEncrypted: true,
            error: nil
        ))
        await successfulApply.value

        state = try XCTUnwrap(repository.state(for: openAI))
        XCTAssertEqual(state.credentialState, .configured)
        XCTAssertEqual(state.pendingSecret, "")

        repository.clearCredentialRequest(for: openAI)
        let deleteApply = Task { await repository.applyChanges(openAI) }
        await waitForCommandCount(3, recorder: recorder)
        guard case let .deleteProviderCredential(operationId: deleteID, providerId: deleteProviderID) = try XCTUnwrap(recorder.commands.last) else {
            return XCTFail("expected delete provider credential")
        }
        XCTAssertEqual(deleteProviderID, openAI)
        repository.handle(event: .providerCredentialStatus(
            operationId: deleteID,
            configuredProviderIds: [],
            unavailableProviderIds: [],
            storageEncrypted: true,
            error: nil
        ))
        await deleteApply.value

        state = try XCTUnwrap(repository.state(for: openAI))
        XCTAssertEqual(state.credentialState, .missing)
        XCTAssertFalse(state.clearCredentialOnApply)
    }

    func testCredentialMutationRequiresAuthoritativeConfirmation() async throws {
        let recorder = CommandRecorder()
        let repository = ProviderRepository(persistenceURL: persistenceURL)
        let openAI = repository.addProfile(presetID: "openai")
        repository.configure(submitCommand: { command in
            try await recorder.submit(command: command)
        })

        repository.stageSecret("sk-openai", for: openAI)
        let setTask = Task { await repository.applyChanges(openAI) }
        await waitForCommandCount(1, recorder: recorder)
        guard case let .setProviderCredential(operationId: setID, _, _) = try XCTUnwrap(recorder.commands.last) else {
            return XCTFail("expected set provider credential")
        }
        repository.handle(event: .providerCredentialStatus(
            operationId: setID,
            configuredProviderIds: [],
            unavailableProviderIds: [],
            storageEncrypted: true,
            error: nil
        ))
        await setTask.value

        var state = try XCTUnwrap(repository.state(for: openAI))
        XCTAssertEqual(state.credentialState, .unknown)
        XCTAssertEqual(state.pendingSecret, "sk-openai")
        XCTAssertEqual(state.connectionState, .failed)
        XCTAssertEqual(state.detailMessage, "安全存储未确认密钥已保存。")

        let removeTask = Task { await repository.removeProfile(openAI) }
        await waitForCommandCount(2, recorder: recorder)
        guard case let .deleteProviderCredential(operationId: deleteID, providerId: providerID) = try XCTUnwrap(recorder.commands.last) else {
            return XCTFail("expected delete provider credential")
        }
        XCTAssertEqual(providerID, openAI)
        repository.handle(event: .providerCredentialStatus(
            operationId: deleteID,
            configuredProviderIds: [openAI],
            unavailableProviderIds: [],
            storageEncrypted: true,
            error: nil
        ))
        await removeTask.value

        state = try XCTUnwrap(repository.state(for: openAI))
        XCTAssertEqual(state.connectionState, .failed)
        XCTAssertEqual(state.detailMessage, "安全存储仍报告该密钥存在。")
        XCTAssertNotNil(repository.state(for: openAI), "a profile must survive a failed secure deletion")
    }

    func testClearingDefaultCredentialKeepsAsyncResultOnOriginalProfileAfterReorder() async throws {
        let recorder = CommandRecorder()
        let repository = ProviderRepository(persistenceURL: persistenceURL)
        let openAI = repository.addProfile(presetID: "openai")
        let kimi = repository.addProfile(presetID: "kimi")
        repository.configure(
            submitCommand: { command in
                try await recorder.submit(command: command)
            },
            applyReconnect: { _ in }
        )

        repository.stageSecret("sk-openai", for: openAI)
        let initialApply = Task { await repository.applyChanges(openAI) }
        await waitForCommandCount(1, recorder: recorder)
        guard case let .setProviderCredential(operationId: setID, _, _) = try XCTUnwrap(recorder.commands.last) else {
            return XCTFail("expected set provider credential")
        }
        repository.handle(event: .providerCredentialStatus(
            operationId: setID,
            configuredProviderIds: [openAI],
            unavailableProviderIds: [],
            storageEncrypted: true,
            error: nil
        ))
        await initialApply.value

        repository.clearCredentialRequest(for: openAI)
        let clearApply = Task { await repository.applyChanges(openAI) }
        await waitForCommandCount(2, recorder: recorder)
        guard case let .deleteProviderCredential(operationId: deleteID, providerId: providerID) = try XCTUnwrap(recorder.commands.last) else {
            return XCTFail("expected delete provider credential")
        }
        XCTAssertEqual(providerID, openAI)
        repository.handle(event: .providerCredentialStatus(
            operationId: deleteID,
            configuredProviderIds: [],
            unavailableProviderIds: [],
            storageEncrypted: true,
            error: nil
        ))
        await clearApply.value

        let openAIState = try XCTUnwrap(repository.state(for: openAI))
        let kimiState = try XCTUnwrap(repository.state(for: kimi))
        XCTAssertFalse(openAIState.profile.enabled)
        XCTAssertFalse(openAIState.profile.isDefault)
        XCTAssertEqual(openAIState.detailMessage, "已应用配置并请求重连。")
        XCTAssertEqual(openAIState.connectionState, .idle)
        XCTAssertTrue(kimiState.profile.isDefault)
        XCTAssertNil(kimiState.detailMessage, "async completion must not leak onto the newly sorted default profile")
    }

    func testConnectionResultUsesProfileIDAfterProfilesReorder() async throws {
        let gate = ProviderConnectionTestGate()
        let started = expectation(description: "connection test started")
        let repository = ProviderRepository(persistenceURL: persistenceURL)
        let openAI = repository.addProfile(presetID: "openai")
        let kimi = repository.addProfile(presetID: "kimi")
        repository.stageSecret("sk-openai", for: openAI)
        repository.configure(
            submitCommand: nil,
            testConnection: { _, _ in
                started.fulfill()
                return await gate.waitForResult()
            }
        )

        let testTask = Task { await repository.testConnection(openAI) }
        await fulfillment(of: [started])

        repository.setDefaultProfile(kimi)
        repository.updateProfile(kimi) { $0.name = "Kimi Default" }
        XCTAssertEqual(repository.profiles.first?.id, kimi, "test precondition: profile order must change while the request is suspended")

        await gate.succeed()
        await testTask.value

        let openAIState = try XCTUnwrap(repository.state(for: openAI))
        let kimiState = try XCTUnwrap(repository.state(for: kimi))
        XCTAssertEqual(openAIState.connectionState, .connected)
        XCTAssertEqual(openAIState.detailMessage, "连接测试成功。")
        XCTAssertEqual(kimiState.connectionState, .idle)
        XCTAssertNil(kimiState.detailMessage)
    }

    private func waitForCommandCount(_ count: Int, recorder: CommandRecorder) async {
        for _ in 0..<100 where recorder.commands.count < count {
            await Task.yield()
        }
        XCTAssertGreaterThanOrEqual(recorder.commands.count, count)
    }

    private func jsonObject(from string: String) throws -> [String: Any]? {
        let data = try XCTUnwrap(string.data(using: .utf8))
        return try JSONSerialization.jsonObject(with: data) as? [String: Any]
    }
}

@MainActor
private final class CommandRecorder {
    private(set) var commands: [ClientCommand] = []

    func submit(command: ClientCommand) async throws {
        commands.append(command)
    }
}

private actor ProviderConnectionTestGate {
    private var continuation: CheckedContinuation<ProviderConnectionTestResult, Never>?
    private var pendingResult: ProviderConnectionTestResult?

    func waitForResult() async -> ProviderConnectionTestResult {
        if let pendingResult {
            self.pendingResult = nil
            return pendingResult
        }
        return await withCheckedContinuation { continuation in
            self.continuation = continuation
        }
    }

    func succeed() {
        if let continuation {
            self.continuation = nil
            continuation.resume(returning: .success())
        } else {
            pendingResult = .success()
        }
    }
}
