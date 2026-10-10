import XCTest
@testable import LingxiCode

@MainActor
final class IOSAudioCallbackCloudTests: XCTestCase {
    func testCallbackSynthesisUsesProviderAndRetainsActualUsageContext() async throws {
        let host = CallbackCloudHost()
        let fixture = try makeFixture(host: host)
        let operation = request(fixture.service, .synthesize(text: "hello", language: nil, rate: nil, voice: nil))
        let result = await fixture.service.execute(operation)
        guard case let .synthesized(pcm, sampleRate) = result else { return XCTFail("provider synthesis must return PCM") }
        XCTAssertEqual(pcm, Data([0, 0, 1, 0]))
        XCTAssertEqual(sampleRate, 24_000)
        XCTAssertEqual(host.synthesisTexts, ["hello"])
        XCTAssertEqual(host.lastSynthesisRequest?["operationId"] as? String, operation.identity.id)
        XCTAssertEqual((host.lastSynthesisRequest?["maxPayloadBytes"] as? NSNumber)?.uint64Value, 1_024)
        XCTAssertEqual((host.lastSynthesisRequest?["timeoutMs"] as? NSNumber)?.uint64Value, 2_000)
        XCTAssertEqual(fixture.provider.usageRecords.last?.modelID, "actual-tts-model")
        XCTAssertEqual(fixture.provider.usageRecords.last?.accountScope, "account-a")
        XCTAssertFalse(fixture.provider.usageRecords.last?.usageJSON.contains("secret transcript") ?? true)
    }

    func testCallbackSpeechWaitsForOwnedDevicePlayback() async throws {
        let host = CallbackCloudHost()
        let fixture = try makeFixture(host: host)
        let result = await fixture.service.execute(request(fixture.service, .speak(text: "hello", language: nil, rate: nil, voice: nil)))
        guard case .playbackCompleted(durationMs: 77) = result else { return XCTFail("provider speech must complete after device playback") }
        XCTAssertEqual(fixture.playback.playedBytes, [Data([0, 0, 1, 0])])
        XCTAssertEqual(host.synthesisTexts, ["hello"])
        let status = try fixture.service.status(owner: .session(sessionID: "session-a"), handle: nil)
        XCTAssertFalse(status.playing)
    }

    func testCallbackLiveRecognitionCapturesThenTranscribes() async throws {
        let host = CallbackCloudHost()
        let fixture = try makeFixture(host: host)
        let result = await fixture.service.execute(request(fixture.service, .listen(language: "en-US")))
        guard case .transcript(text: "recognized", language: "en-US", confidence: nil) = result else { return XCTFail("provider listen must use capture and SDK transcription") }
        XCTAssertEqual(fixture.recorder.startCount, 1)
        XCTAssertEqual(host.transcribedAudio, [Data([1, 2, 3])])
    }

    func testCallbackFileRecognitionUsesProvidedMediaWithoutMicrophone() async throws {
        let host = CallbackCloudHost()
        let fixture = try makeFixture(host: host)
        let result = await fixture.service.execute(request(fixture.service, .transcribe(audio: Data([9, 8]), mimeType: "audio/wav", language: "en-US")))
        guard case .transcript = result else { return XCTFail("provider file transcription must dispatch") }
        XCTAssertEqual(fixture.recorder.startCount, 0)
        XCTAssertEqual(host.transcribedAudio, [Data([9, 8])])
    }

    func testCallbackCannotBorrowAnotherSessionsProfile() async throws {
        let host = CallbackCloudHost()
        let fixture = try makeFixture(host: host, followsSession: true)
        let result = await fixture.service.execute(request(fixture.service, .listen(language: nil), owner: .session(sessionID: "session-b")))
        guard case .failed = result else { return XCTFail("a different session requires its own trusted profile") }
        XCTAssertEqual(fixture.recorder.startCount, 0)
        XCTAssertTrue(host.transcribedAudio.isEmpty)
    }

    private func makeFixture(host: CallbackCloudHost, followsSession: Bool = false) throws -> (service: IOSAudioService, provider: IOSAudioProviderService, recorder: CallbackRecordingDriver, playback: CallbackPlaybackDriver) {
        let defaults = UserDefaults(suiteName: "IOSAudioCallbackCloudTests.\(UUID().uuidString)")!
        let store = AudioConfigurationStore(defaults: defaults)
        var configuration = store.configuration
        let cloud = AudioCloudBinding(binding: followsSession ? "follow_session" : "explicit_profile", profileId: followsSession ? nil : "openai:work")
        configuration.recognition = AudioRecognitionPreference(source: .provider, cloud: cloud)
        configuration.speech = AudioSpeechPreference(source: .provider, cloud: cloud)
        _ = try store.save(configuration, expectedRevision: store.revision)
        let provider = IOSAudioProviderService()
        provider.installHostBuilder { _, _ in host }
        provider.setSessionContext(sessionID: "session-a", profileID: "openai:work", accountScope: "account-a")
        let recorder = CallbackRecordingDriver()
        let playback = CallbackPlaybackDriver()
        let service = IOSAudioService(configurationStore: store, recorder: recorder, pcmPlayback: playback, providerService: provider, serviceEpoch: 72)
        return (service, provider, recorder, playback)
    }

    private func request(_ service: IOSAudioService, _ operation: IOSAudioOperation, owner: IOSAudioOwner = .session(sessionID: "session-a")) -> IOSAudioOperationRequest {
        IOSAudioOperationRequest(identity: .init(id: UUID().uuidString.lowercased(), generation: 1, serviceEpoch: service.serviceEpoch), owner: owner, initiator: nil, timeoutBudgetMs: 2_000, maxPayloadBytes: 1_024, operation: operation)
    }
}

@MainActor
private final class CallbackCloudHost: IOSAudioProviderHostDriving {
    var synthesisTexts: [String] = []
    var lastSynthesisRequest: [String: Any]?
    var transcribedAudio: [Data] = []
    func capabilities(requestJson: String) async throws -> String {
        let request = try JSONSerialization.jsonObject(with: Data(requestJson.utf8)) as! [String: Any]
        let kind = request["kind"] as? String ?? ""
        let model = kind == "speech" ? "tts-model" : "asr-model"
        return #"{"profileId":"openai:work","providerId":"openai","supported":true,"readiness":"ready","modelId":""# + model + #"","models":[{"id":""# + model + #"","voices":[]}]}"#
    }
    func transcribe(requestJson: String, audio: Data, mimeType: String) async throws -> String {
        transcribedAudio.append(audio)
        return #"{"text":"recognized","usage":{"audio_seconds":1}}"#
    }
    func synthesize(requestJson: String, text: String) async throws -> String {
        synthesisTexts.append(text)
        lastSynthesisRequest = try JSONSerialization.jsonObject(with: Data(requestJson.utf8)) as? [String: Any]
        return #"{"pcmBase64":"AAABAA==","sampleRateHz":24000,"usage":{"characters":5,"native":{"text":"secret transcript","tokens":2}},"usageContext":{"profileId":"openai:work","accountScope":"account-a","modelId":"actual-tts-model"}}"#
    }
    func cancel(operationId: String) async throws {}
}

private final class CallbackRecordingDriver: AudioRecordingDriving, @unchecked Sendable {
    private let lock = NSLock()
    private var starts = 0
    var startCount: Int { lock.withLock { starts } }
    func startRecordingOwned(operationID: String, ownerID: String, sampleRateHz: UInt32, format: String, maximumBytes: UInt64) async throws -> String { lock.withLock { starts += 1 }; return "finished" }
    func stopRecordingOwned(handle: String, ownerID: String, maximumBytes: UInt64) async throws -> IOSAudioRecording { IOSAudioRecording(audioBytes: Data([1, 2, 3]), mimeType: "audio/m4a") }
    func isRecordingOwned(handle: String?, ownerID: String) -> Bool { false }
    func cancel(startOperationID: String) async {}
    func end(ownerID: String) async {}
    func stopAll() async {}
}

@MainActor
private final class CallbackPlaybackDriver: IOSPcmPlaybackDriving {
    var playedBytes: [Data] = []
    var isPlaying = false
    var positionMs: UInt64 { 0 }
    func play(pcm: Data, sampleRateHz: UInt32, maximumBytes: UInt64) async throws -> UInt64 { playedBytes.append(pcm); return 77 }
    func stop() { isPlaying = false }
    func pause() {}
    func resume() {}
}
