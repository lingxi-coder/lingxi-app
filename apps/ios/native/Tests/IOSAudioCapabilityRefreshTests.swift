import XCTest
@testable import LingxiCode

@MainActor
final class IOSAudioCapabilityRefreshTests: XCTestCase {
    func testOlderSameContextResponseCannotOverwriteNewConfiguration() async throws {
        let host = DelayedAudioCapabilityHost()
        let service = IOSAudioProviderService()
        service.installHostBuilder { _, _ in host }
        let old = Task { await service.refresh(snapshot: snapshot(model: "old", revision: 1)) }
        await host.waitForOldRequest()
        await service.refresh(snapshot: snapshot(model: "new", revision: 2))
        host.completeOld(.success(host.response(model: "old")))
        await old.value
        XCTAssertEqual(service.capabilities[.recognition]?.route.defaultModelId, "new")
        XCTAssertEqual(service.capabilities[.speech]?.route.defaultModelId, "new")
        XCTAssertNil(service.lastError)
    }

    func testOlderFailureCannotEraseNewCapabilitiesOrErrorState() async throws {
        let host = DelayedAudioCapabilityHost()
        let service = IOSAudioProviderService()
        service.installHostBuilder { _, _ in host }
        let old = Task { await service.refresh(snapshot: snapshot(model: "old", revision: 1)) }
        await host.waitForOldRequest()
        await service.refresh(snapshot: snapshot(model: "new", revision: 2))
        host.completeOld(.failure(AudioServiceFailure.nativeFailure("stale failure")))
        await old.value
        XCTAssertEqual(service.capabilities[.recognition]?.route.defaultModelId, "new")
        XCTAssertNil(service.lastError)
    }

    func testPinnedProviderRequestPreservesCallerBoundsAndIdentity() async throws {
        let host = DelayedAudioCapabilityHost()
        let service = IOSAudioProviderService()
        service.installHostBuilder { _, _ in host }
        let operation = try await service.pin(kind: .speech, snapshot: snapshot(model: "new", revision: 1),
            operationID: "caller-operation", maximumBytes: 1_024, timeoutBudgetMs: 2_000)
        let request = try JSONSerialization.jsonObject(with: Data(operation.requestJSON.utf8)) as! [String: Any]
        XCTAssertEqual(request["operationId"] as? String, "caller-operation")
        XCTAssertEqual((request["maxPayloadBytes"] as? NSNumber)?.uint64Value, 1_024)
        XCTAssertEqual((request["timeoutMs"] as? NSNumber)?.uint64Value, 2_000)
        XCTAssertEqual((request["cloud"] as? [String: Any])?["modelId"] as? String, "new")
    }

    func testFollowSessionCannotUseRepositoryHostWithoutAttachedEngine() async throws {
        let service = IOSAudioProviderService()
        let host = DelayedAudioCapabilityHost()
        var repositoryHostCalls = 0
        service.installHostBuilder { _, _ in repositoryHostCalls += 1; return host }
        service.setSessionContext(sessionID: "session-a", profileID: "openai:work", accountScope: "account-a")
        let configuration = AudioConfigurationV4(recognition: AudioRecognitionPreference(source: .provider), speech: AudioSpeechPreference(source: .automatic))
        do {
            _ = try await service.pin(kind: .recognition, snapshot: AudioConfigurationSnapshot(configuration: configuration, revision: 1), owner: .session(sessionID: "session-a"))
            XCTFail("follow-session must use the exact attached engine services")
        } catch {}
        XCTAssertEqual(repositoryHostCalls, 0)
    }

    func testLateRealtimeUsageIsAccountScopedAndTranscriptFree() {
        let service = IOSAudioProviderService()
        let usage = #"{"type":"usage","turnId":"turn-a","inputTokens":7,"outputTokens":9,"native":{"total_tokens":16,"text":"private words"},"usageContext":{"operationId":"operation-a","profileId":"openai:work","accountScope":"account-a","modelId":null}}"#
        service.recordRealtimeUsage(usage)
        XCTAssertEqual(service.usageRecords.count, 1)
        XCTAssertEqual(service.usageRecords[0].accountScope, "account-a")
        XCTAssertEqual(service.usageRecords[0].turnID, "turn-a")
        XCTAssertNil(service.usageRecords[0].modelID)
        XCTAssertFalse(service.usageRecords[0].usageJSON.contains("private words"))
        XCTAssertTrue(service.usageRecords[0].usageJSON.contains("total_tokens"))
    }

    private func snapshot(model: String, revision: UInt64) -> AudioConfigurationSnapshot {
        let cloud = AudioCloudBinding(binding: "explicit_profile", profileId: "openai:work", modelId: model)
        return AudioConfigurationSnapshot(configuration: AudioConfigurationV4(
            recognition: AudioRecognitionPreference(source: .provider, cloud: cloud),
            speech: AudioSpeechPreference(source: .provider, cloud: cloud),
            conversation: AudioConversationPreference(cloud: cloud)
        ), revision: revision)
    }
}

@MainActor
private final class DelayedAudioCapabilityHost: IOSAudioProviderHostDriving {
    private var oldRequest: CheckedContinuation<String, Error>?
    private var admission: CheckedContinuation<Void, Never>?

    func capabilities(requestJson: String) async throws -> String {
        let request = try JSONSerialization.jsonObject(with: Data(requestJson.utf8)) as! [String: Any]
        let cloud = request["cloud"] as? [String: Any] ?? [:]
        let model = cloud["modelId"] as? String ?? "new"
        if model == "old", request["kind"] as? String == "recognition" {
            return try await withCheckedThrowingContinuation { continuation in
                oldRequest = continuation
                admission?.resume(); admission = nil
            }
        }
        return response(model: model)
    }

    func response(model: String) -> String {
        #"{"profileId":"openai:work","providerId":"openai","supported":true,"readiness":"ready","modelId":""# + model + #"","models":[{"id":"old","voices":[]},{"id":"new","voices":[]}]}"#
    }

    func waitForOldRequest() async {
        guard oldRequest == nil else { return }
        await withCheckedContinuation { admission = $0 }
    }

    func completeOld(_ result: Result<String, Error>) {
        oldRequest?.resume(with: result); oldRequest = nil
    }

    func transcribe(requestJson: String, audio: Data, mimeType: String) async throws -> String { "{}" }
    func synthesize(requestJson: String, text: String) async throws -> String { "{}" }
    func cancel(operationId: String) async throws {}
}
