import AVFoundation
import XCTest
@testable import LingxiCode

@MainActor
final class VoiceCapabilityTests: XCTestCase {
    private var defaults: UserDefaults!
    private var suiteName: String!

    override func setUp() {
        super.setUp()
        suiteName = "VoiceCapabilityTests.\(UUID().uuidString)"
        defaults = UserDefaults(suiteName: suiteName)
        defaults.removePersistentDomain(forName: suiteName)
    }

    override func tearDown() {
        defaults.removePersistentDomain(forName: suiteName)
        defaults = nil
        suiteName = nil
        super.tearDown()
    }

    func testPreferencesPersistWithoutProviderSecrets() {
        let model = VoiceCapabilityModel(defaults: defaults)

        model.setLanguage(VoiceCapabilityModel.automaticLanguageIdentifier)
        model.setMode(.automatic)
        model.setSpeed(1.4)
        model.setAutoPlay(true)

        let restored = VoiceCapabilityModel(defaults: defaults)
        XCTAssertEqual(restored.language, VoiceCapabilityModel.automaticLanguageIdentifier)
        XCTAssertEqual(restored.mode, .automatic)
        XCTAssertEqual(restored.speed, 1.4, accuracy: 0.001)
        XCTAssertTrue(restored.autoPlay)
        XCTAssertNil(defaults.string(forKey: "apiKey"))
    }

    func testFreshPreferencesDefaultToAutomatic() {
        let snapshot = VoicePreferencesSnapshot.load(defaults: defaults)

        XCTAssertEqual(snapshot.recognitionMode, .automatic)
        XCTAssertEqual(snapshot.language, VoicePreferencesSnapshot.automaticLanguage)
        XCTAssertEqual(snapshot.voiceSelection, VoicePreferencesSnapshot.defaultVoiceSelection)
    }

    func testSharedPreferencesMigrateLegacyKeysIntoVersionedContract() {
        defaults.set("on-device", forKey: "voiceRecognitionMode")
        defaults.set("en-US", forKey: "voiceLanguage")
        defaults.set("com.apple.ttsbundle.Samantha-compact", forKey: "systemVoiceIdentifier")
        defaults.set(1.4, forKey: "voiceSpeed")
        defaults.set(true, forKey: "voiceAutoPlay")

        let snapshot = VoicePreferencesSnapshot.load(defaults: defaults)

        XCTAssertEqual(snapshot.schemaVersion, 2)
        XCTAssertEqual(snapshot.recognitionMode, .onDevice)
        XCTAssertEqual(snapshot.language, "en-US")
        XCTAssertEqual(snapshot.voiceSelection, "system:com.apple.ttsbundle.Samantha-compact")
        XCTAssertEqual(snapshot.rate, 1.4, accuracy: 0.001)
        XCTAssertTrue(snapshot.autoPlayReplies)
        XCTAssertEqual(defaults.integer(forKey: "voice.schemaVersion"), 2)
    }

    func testLocalOnlyNeverFallsBackToSystemRecognizer() {
        let preferences = VoicePreferencesSnapshot(
            schemaVersion: 2,
            recognitionMode: .onDevice,
            language: "zh-CN",
            voiceSelection: VoicePreferencesSnapshot.defaultVoiceSelection,
            rate: 1,
            autoPlayReplies: false
        )

        let unavailable = VoiceRuntimeResolver.recognitionRoute(
            preferences: preferences,
            systemRecognizerAvailable: true,
            modelRoot: { _ in nil }
        )
        guard case .unavailable = unavailable else {
            return XCTFail("local-only must block when the Sherpa model is missing")
        }

        let ready = VoiceRuntimeResolver.recognitionRoute(
            preferences: preferences,
            systemRecognizerAvailable: true,
            modelRoot: { _ in URL(fileURLWithPath: "/verified-model") }
        )
        guard case let .sherpa(_, modelID, _) = ready else {
            return XCTFail("local-only must select Sherpa even when the system recognizer is available")
        }
        XCTAssertEqual(modelID, "sherpa.zipformer-zh-14m-mobile")
    }

    func testAutomaticPrefersSystemThenUsesVerifiedSherpaFallback() {
        let preferences = VoicePreferencesSnapshot(
            schemaVersion: 2,
            recognitionMode: .automatic,
            language: "en-US",
            voiceSelection: VoicePreferencesSnapshot.defaultVoiceSelection,
            rate: 1,
            autoPlayReplies: false
        )

        let system = VoiceRuntimeResolver.recognitionRoute(
            preferences: preferences,
            systemRecognizerAvailable: true,
            modelRoot: { _ in URL(fileURLWithPath: "/verified-model") }
        )
        guard case .system = system else { return XCTFail("automatic must prefer the system recognizer") }

        let fallback = VoiceRuntimeResolver.recognitionRoute(
            preferences: preferences,
            systemRecognizerAvailable: false,
            modelRoot: { _ in URL(fileURLWithPath: "/verified-model") }
        )
        guard case let .sherpa(_, modelID, _) = fallback else {
            return XCTFail("automatic must use a verified Sherpa fallback")
        }
        XCTAssertEqual(modelID, "sherpa.moonshine-tiny-en")
    }

    func testDeniedSpeechAuthorizationMakesSystemRecognizerUnavailableForRouting() {
        XCTAssertFalse(VoiceRuntimeResolver.systemRecognitionAvailable(
            serviceAvailable: true,
            authorization: .denied
        ))
        XCTAssertFalse(VoiceRuntimeResolver.systemRecognitionAvailable(
            serviceAvailable: true,
            authorization: .restricted
        ))
        XCTAssertTrue(VoiceRuntimeResolver.systemRecognitionAvailable(
            serviceAvailable: true,
            authorization: .notDetermined
        ))
    }

    func testFirstRunConfigurationIsExplicitlyUnconfirmed() {
        let model = VoiceCapabilityModel(defaults: defaults)

        XCTAssertTrue(model.speechConfigurationConfirmed)
        XCTAssertTrue(model.ttsConfigurationConfirmed)
    }

    func testReadinessDifferentiatesPermissionsAndUnavailableCapabilities() {
        let readiness = VoiceCapabilityModel.buildConfigurationReadiness(
            speechConfigured: true,
            ttsConfigured: true,
            speechAuthorization: .denied,
            microphonePermissionStatus: .undetermined,
            recognizerAvailable: false,
            hasConfiguredVoice: false,
            systemVoicesAvailable: true
        )

        XCTAssertFalse(readiness.speechReady)
        XCTAssertFalse(readiness.ttsReady)
        XCTAssertTrue(readiness.issues.contains {
            $0.component == .speech && $0.kind == .permissionDenied
        })
        XCTAssertTrue(readiness.issues.contains {
            $0.component == .microphone && $0.kind == .permissionUndetermined
        })
        XCTAssertTrue(readiness.issues.contains {
            $0.component == .speech && $0.kind == .unavailable
        })
        XCTAssertTrue(readiness.issues.contains {
            $0.component == .tts && $0.kind == .unavailable
        })
    }

    func testReadinessAllowsDictationAndFlowWhenConfigurationAndRuntimeAreReady() {
        let readiness = VoiceCapabilityModel.buildConfigurationReadiness(
            speechConfigured: true,
            ttsConfigured: true,
            speechAuthorization: .authorized,
            microphonePermissionStatus: .granted,
            recognizerAvailable: true,
            hasConfiguredVoice: true,
            systemVoicesAvailable: true
        )

        XCTAssertTrue(readiness.isReadyForDictation)
        XCTAssertTrue(readiness.isReadyForFlow)
        XCTAssertNil(readiness.message)
        XCTAssertTrue(readiness.issues.isEmpty)
    }

    func testSavingConfigurationPreservesRequestedSystemDefault() throws {
        let model = VoiceCapabilityModel(defaults: defaults)
        guard let fallbackVoice = model.selectedVoice else {
            throw XCTSkip("This test host has no installed system speech voices")
        }

        let readiness = model.saveConfiguration()

        XCTAssertTrue(model.speechConfigurationConfirmed)
        XCTAssertTrue(model.ttsConfigurationConfirmed)
        XCTAssertTrue(readiness.speechConfigured)
        XCTAssertTrue(readiness.ttsConfigured)
        XCTAssertEqual(
            defaults.string(forKey: VoicePreferencesSnapshot.Keys.voiceSelection),
            VoicePreferencesSnapshot.defaultVoiceSelection
        )
    }

    func testSaveStatusKeepsBlockingReadinessIssueVisible() {
        let readiness = VoiceConfigurationReadiness(
            speechConfigured: true,
            ttsConfigured: true,
            speechReady: false,
            ttsReady: true,
            issues: [
                .init(
                    component: .speech,
                    kind: .permissionDenied,
                    message: "语音识别权限已关闭，请前往系统设置开启"
                ),
            ]
        )

        let message = VoiceCapabilityModel.configurationSaveMessage(for: readiness)

        XCTAssertTrue(message.contains("已保存"))
        XCTAssertTrue(message.contains("语音识别权限已关闭"))
    }

    func testStoppingPreviewCancelsPlaybackAndClearsPreviewState() async {
        let player = CapabilityBlockingSpeechPlayer()
        let playback = VoicePreviewPlayback(player: player)
        let model = VoiceCapabilityModel(defaults: defaults, previewPlayback: playback)
        guard model.selectedVoice != nil else {
            return XCTFail("Test host must expose at least one system voice")
        }

        let previewTask = Task { @MainActor in await model.preview() }
        for _ in 0..<6 { await Task.yield() }
        XCTAssertTrue(model.isPreviewing)
        XCTAssertEqual(player.requests.count, 1)

        await model.stopPreview()
        await previewTask.value

        XCTAssertFalse(model.isPreviewing)
        XCTAssertGreaterThanOrEqual(player.stopCalls, 1)
    }

    func testChangingChoicesPersistsImmediately() throws {
        let model = VoiceCapabilityModel(defaults: defaults)
        guard model.selectedVoice != nil else {
            throw XCTSkip("This test host has no installed system speech voices")
        }

        model.setMode(.onDevice)
        model.setMode(.automatic)
        XCTAssertEqual(defaults.string(forKey: "voiceRecognitionMode"), VoiceRecognitionMode.automatic.rawValue)

        let alternativeVoice = model.voices.first { $0.id != model.voiceIdentifier }
        if let alternativeVoice {
            model.setVoice(alternativeVoice.id)
            XCTAssertEqual(defaults.string(forKey: "systemVoiceIdentifier"), alternativeVoice.id)
        }
    }

    func testReloadFromDefaultsReadsExternalConfigurationChanges() throws {
        let writer = VoiceCapabilityModel(defaults: defaults)
        guard writer.selectedVoice != nil else {
            throw XCTSkip("This test host has no installed system speech voices")
        }
        let reader = VoiceCapabilityModel(defaults: defaults)

        writer.setLanguage("en-US")
        writer.setMode(.automatic)
        writer.saveConfiguration()
        reader.reloadFromDefaults()

        XCTAssertEqual(reader.language, "en-US")
        XCTAssertEqual(reader.mode, .automatic)
        XCTAssertTrue(reader.speechConfigurationConfirmed)
        XCTAssertTrue(reader.ttsConfigurationConfirmed)
        XCTAssertFalse(reader.voiceIdentifier.isEmpty)
    }

    func testFirstRunVoiceLanguageFollowsSystemUntilUserOverridesIt() {
        let capability = VoiceCapabilityModel(defaults: defaults)

        XCTAssertEqual(capability.language, VoiceCapabilityModel.automaticLanguageIdentifier)
        XCTAssertNil(defaults.string(forKey: "voiceLanguage"))
    }

    func testAutomaticLanguageResolvesCurrentLocaleIdentifier() {
        let resolved = VoiceCapabilityModel.resolvedRecognitionLocaleIdentifier(
            configuredLanguage: VoiceCapabilityModel.automaticLanguageIdentifier,
            currentLocale: Locale(identifier: "en_US")
        )

        XCTAssertEqual(resolved, "en-US")
    }

    func testEffectiveRecognitionStatusExplainsOnlineFallbackWhenOnDeviceUnavailable() {
        let status = VoiceCapabilityModel.buildEffectiveRecognitionStatus(
            mode: .onDevice,
            requestedLanguage: "ja-JP",
            currentLocale: Locale(identifier: "zh_CN"),
            recognizerAvailable: true,
            onDeviceAvailable: false,
            speechAuthorization: .authorized,
            microphoneGranted: true
        )

        XCTAssertEqual(status.modeLabel, "系统在线回退")
        XCTAssertEqual(status.fallbackReason, "当前语言不支持设备端识别")
        XCTAssertTrue(status.detail.contains("回退"))
    }

    func testUnavailableRecognizerDoesNotClaimOnlineFallback() {
        let status = VoiceCapabilityModel.buildEffectiveRecognitionStatus(
            mode: .onDevice,
            requestedLanguage: "ja-JP",
            currentLocale: Locale(identifier: "zh_CN"),
            recognizerAvailable: false,
            onDeviceAvailable: false,
            speechAuthorization: .authorized,
            microphoneGranted: true
        )

        XCTAssertEqual(status.modeLabel, "暂不可用")
        XCTAssertEqual(status.fallbackReason, "当前系统语言的识别器暂不可用")
        XCTAssertFalse(status.detail.contains("回退"))
    }

    func testPermissionDiagnosticsDifferentiateUndeterminedAndDenied() {
        XCTAssertEqual(
            VoiceCapabilityModel.speechPermissionDiagnostic(.notDetermined),
            VoicePermissionDiagnostic(label: "待授权", detail: "首次使用时会请求语音识别权限")
        )
        XCTAssertEqual(
            VoiceCapabilityModel.microphonePermissionDiagnostic(.denied),
            VoicePermissionDiagnostic(label: "已拒绝", detail: "请在系统设置中开启麦克风权限")
        )
    }

    func testSpeedIsClampedToSupportedRange() {
        let model = VoiceCapabilityModel(defaults: defaults)

        model.setSpeed(0.1)
        XCTAssertEqual(model.speed, 0.5)

        model.setSpeed(5)
        XCTAssertEqual(model.speed, 2)
    }

    func testUtteranceRateUsesPersistedMultiplier() {
        XCTAssertEqual(
            VoiceCapabilityModel.utteranceRate(from: 1),
            AVSpeechUtteranceDefaultSpeechRate,
            accuracy: 0.0001
        )
        XCTAssertEqual(
            VoiceCapabilityModel.utteranceRate(from: 0.1),
            AVSpeechUtteranceDefaultSpeechRate * 0.5,
            accuracy: 0.0001
        )
    }

    func testHoldCaptureStartsImmediatelyAndReleaseFinishesSameSession() async {
        let session = StubVoiceTranscriptionSession(transcript: "hello")
        let capture = VoiceCapture(makeSession: { session })
        let completed = expectation(description: "transcript delivered")
        var result: VoiceCaptureResult?

        capture.start { value in
            result = value
            completed.fulfill()
        }
        XCTAssertEqual(capture.phase, .listening)

        await Task.yield()
        XCTAssertEqual(session.transcribeCalls, 1)
        XCTAssertNil(session.automaticEndpointAfterSilence)

        capture.finish()
        XCTAssertEqual(capture.phase, .finishing)
        XCTAssertEqual(session.finishCalls, 1)

        await fulfillment(of: [completed], timeout: 1)
        XCTAssertEqual(result, .transcript("hello"))
        XCTAssertEqual(capture.phase, .idle)
        XCTAssertEqual(session.cancelCalls, 0)
    }

    func testCancelledHoldStopsSessionAndNeverDeliversTranscript() async {
        let session = StubVoiceTranscriptionSession(transcript: "must not submit")
        let capture = VoiceCapture(makeSession: { session })
        var deliveredResults: [VoiceCaptureResult] = []

        capture.start { deliveredResults.append($0) }
        await Task.yield()
        capture.cancel()
        await Task.yield()

        XCTAssertEqual(capture.phase, .idle)
        XCTAssertEqual(session.cancelCalls, 1)
        XCTAssertTrue(deliveredResults.isEmpty)
    }

    func testStartingNewCaptureCancelsPreviousSessionAndIgnoresItsLateResult() async {
        let first = StubVoiceTranscriptionSession(transcript: "stale", ignoresCancellation: true)
        let second = StubVoiceTranscriptionSession(transcript: "fresh")
        var sessions: [StubVoiceTranscriptionSession] = [first, second]
        let capture = VoiceCapture(makeSession: { sessions.removeFirst() })
        var deliveredResults: [VoiceCaptureResult] = []

        capture.start { deliveredResults.append($0) }
        await Task.yield()
        capture.start { deliveredResults.append($0) }
        await Task.yield()

        XCTAssertEqual(first.cancelCalls, 1)
        first.finishRecording()
        await Task.yield()
        second.finishRecording()
        await Task.yield()

        XCTAssertEqual(deliveredResults, [.transcript("fresh")])
    }

    func testVoiceHoldGesturePolicyCancelsOnlyAfterUpwardThreshold() {
        XCTAssertFalse(VoiceHoldGesturePolicy.shouldCancel(verticalTranslation: -71))
        XCTAssertTrue(VoiceHoldGesturePolicy.shouldCancel(verticalTranslation: -72))
        XCTAssertFalse(VoiceHoldGesturePolicy.shouldCancel(verticalTranslation: 120))
    }
}

@MainActor
private final class StubVoiceTranscriptionSession: VoiceTranscriptionSession {
    private let transcript: String
    private let ignoresCancellation: Bool
    private var continuation: CheckedContinuation<String, Error>?
    private var pendingFinish = false
    private var pendingCancellation = false

    private(set) var transcribeCalls = 0
    private(set) var finishCalls = 0
    private(set) var cancelCalls = 0
    private(set) var automaticEndpointAfterSilence: Duration?

    init(transcript: String, ignoresCancellation: Bool = false) {
        self.transcript = transcript
        self.ignoresCancellation = ignoresCancellation
    }

    func transcribe(
        language _: String?,
        automaticEndpointAfterSilence: Duration?
    ) async throws -> String {
        transcribeCalls += 1
        self.automaticEndpointAfterSilence = automaticEndpointAfterSilence
        return try await withCheckedThrowingContinuation { continuation in
            if pendingCancellation {
                continuation.resume(throwing: CancellationError())
            } else if pendingFinish {
                continuation.resume(returning: transcript)
            } else {
                self.continuation = continuation
            }
        }
    }

    func finishRecording() {
        finishCalls += 1
        pendingFinish = true
        continuation?.resume(returning: transcript)
        continuation = nil
    }

    func cancelRecognition() {
        cancelCalls += 1
        guard !ignoresCancellation else { return }
        pendingCancellation = true
        continuation?.resume(throwing: CancellationError())
        continuation = nil
    }
}

@MainActor
private final class CapabilityBlockingSpeechPlayer: VoiceSpeechPlaying {
    private(set) var requests: [VoiceSpeechRequest] = []
    private(set) var stopCalls = 0
    private var continuation: CheckedContinuation<Void, Never>?

    func speak(_ request: VoiceSpeechRequest) async throws -> VoiceSpeechPlaybackOutcome {
        requests.append(request)
        await withCheckedContinuation { continuation = $0 }
        try Task.checkCancellation()
        return .completed
    }

    func stop() {
        stopCalls += 1
        continuation?.resume()
        continuation = nil
    }
}
