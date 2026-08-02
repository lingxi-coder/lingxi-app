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

    func testFirstRunVoiceLanguageFollowsSystemUntilUserOverridesIt() {
        let capability = VoiceCapabilityModel(defaults: defaults)
        let appState = AppState(defaults: defaults)

        XCTAssertEqual(capability.language, VoiceCapabilityModel.automaticLanguageIdentifier)
        XCTAssertEqual(appState.voiceLanguage, VoiceCapabilityModel.automaticLanguageIdentifier)
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

    init(transcript: String, ignoresCancellation: Bool = false) {
        self.transcript = transcript
        self.ignoresCancellation = ignoresCancellation
    }

    func transcribe(language _: String?) async throws -> String {
        transcribeCalls += 1
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
