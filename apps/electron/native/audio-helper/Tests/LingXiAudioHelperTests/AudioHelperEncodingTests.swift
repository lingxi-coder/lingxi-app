import Foundation
import AVFoundation
import Testing
@testable import LingXiAudioHelper

private final class AudioHelperOutputProbe: @unchecked Sendable {
    private let lock = NSLock()
    private var storage: [Data] = []
    func append(_ data: Data) {
        lock.lock(); defer { lock.unlock() }
        storage.append(data)
    }
    var values: [Data] {
        lock.lock(); defer { lock.unlock() }
        return storage
    }
}

private final class ModelRetryURLProtocol: URLProtocol, @unchecked Sendable {
    static let requests = AudioHelperOutputProbe()
    override class func canInit(with request: URLRequest) -> Bool { true }
    override class func canonicalRequest(for request: URLRequest) -> URLRequest { request }
    override func startLoading() {
        let range = request.value(forHTTPHeaderField: "Range")
        Self.requests.append(Data((range ?? "full").utf8))
        let response = HTTPURLResponse(url: request.url!, statusCode: range == nil ? 200 : 416,
                                       httpVersion: "HTTP/1.1", headerFields: ["ETag": "retry-test"])!
        client?.urlProtocol(self, didReceive: response, cacheStoragePolicy: .notAllowed)
        if range == nil { client?.urlProtocol(self, didLoad: Data("invalid archive".utf8)) }
        client?.urlProtocolDidFinishLoading(self)
    }
    override func stopLoading() {}
}

private final class AudioOperationCallCounter: @unchecked Sendable {
    private let lock = NSLock()
    private var storage = 0
    func increment() {
        lock.lock(); defer { lock.unlock() }
        storage += 1
    }
    var value: Int {
        lock.lock(); defer { lock.unlock() }
        return storage
    }
}

private actor ModelInstallTestBarrier {
    private var calls = 0
    private var releasePermits = 0
    private var releaseWaiters: [CheckedContinuation<Void, Never>] = []
    private var callWaiters: [(Int, CheckedContinuation<Void, Never>)] = []

    func pause() async {
        calls += 1
        let ready = callWaiters.filter { calls >= $0.0 }
        callWaiters.removeAll { calls >= $0.0 }
        ready.forEach { $0.1.resume() }
        if releasePermits > 0 {
            releasePermits -= 1
            return
        }
        await withCheckedContinuation { releaseWaiters.append($0) }
    }

    func waitForCallCount(_ expected: Int) async {
        if calls >= expected { return }
        await withCheckedContinuation { callWaiters.append((expected, $0)) }
    }

    func releaseOne() {
        if !releaseWaiters.isEmpty {
            releaseWaiters.removeFirst().resume()
        } else {
            releasePermits += 1
        }
    }

    func releaseAll() {
        releasePermits = Int.max
        let waiters = releaseWaiters
        releaseWaiters.removeAll()
        waiters.forEach { $0.resume() }
    }

    func callCount() -> Int { calls }
}

private func emptyHelperSnapshot() -> HelperSnapshot {
    HelperSnapshot(
        helper: .init(state: "running", message: nil),
        permissions: .init(microphone: "unavailable", speech: "unavailable"),
        owner: nil,
        activity: "idle",
        localeTag: "en-US",
        recognizerAvailable: false,
        recognition: nil,
        playback: nil,
        voices: [],
        models: [],
        capabilities: nil,
        configurationRevision: nil,
        currentOperation: nil
    )
}

struct AudioHelperEncodingTests {
    @Test
    func realtimeCaptureChunkCarriesAnExactOwnerAndOperationWithoutASnapshot() throws {
        let identity = HelperAudioOperationIdentity(id: UUID().uuidString.lowercased(), generation: 7, serviceEpoch: 9)
        let event = HelperEvent(type: "capture_chunk", snapshot: nil, owner: .init(kind: "session", id: "current-session"), progress: nil, model: nil, state: nil, error: nil, message: nil,
                                pcmBase64: Data([0, 0]).base64EncodedString(), sampleRateHz: 24_000, identity: identity, sequence: 3)
        let data = try JSONEncoder().encode(event)
        let value = try #require(JSONSerialization.jsonObject(with: data) as? [String: Any])
        #expect(value["snapshot"] == nil)
        #expect(value["pcmBase64"] as? String == "AAA=")
        #expect(value["sampleRateHz"] as? Int == 24_000)
        #expect((value["identity"] as? [String: Any])?["service_epoch"] as? Int == 9)
        #expect((value["owner"] as? [String: Any])?["id"] as? String == "current-session")
    }

    @Test
    func pcmPlaybackRejectsAnIncompleteSampleBeforeAcquiringDevicePlayback() async throws {
        let output = AudioHelperOutputProbe()
        let state = HelperStateStore(storageRoot: FileManager.default.temporaryDirectory.appending(path: "lingxi-play-invalid-\(UUID().uuidString)"), writer: LineWriter(emit: { output.append($0) }))
        let epoch = try #require(await state.snapshot(message: nil).capabilities?.serviceEpoch)
        await handleHelperInputEnvelope([
            "id": "invalid-play", "kind": "engine_request", "configuration": [:], "configurationRevision": 0,
            "request": ["identity": ["id": UUID().uuidString.lowercased(), "generation": 1, "service_epoch": epoch],
                        "owner": ["type": "session", "session_id": "play-owner"], "max_payload_bytes": 1024,
                        "operation": ["type": "play", "pcm_base64": "AAAA", "sample_rate_hz": 24_000]]
        ], state: state)
        let response = try #require(output.values.compactMap { try? JSONSerialization.jsonObject(with: $0) as? [String: Any] }.first { $0["id"] as? String == "invalid-play" })
        let result = try #require((response["result"] as? [String: Any])?["result"] as? [String: Any])
        #expect(result["type"] as? String == "failed")
        #expect((result["error"] as? [String: Any])?["kind"] as? String == "invalid_request")
        #expect(await state.snapshot(message: nil).activePlaybackCount == 0)
    }

    @Test
    func foregroundPermissionArgumentsAreStrictlyBounded() throws {
        #expect(try permissionRequestFromArguments(["LingXiAudioHelper", "--jsonl"]) == nil)
        #expect(try permissionRequestFromArguments([
            "LingXiAudioHelper",
            "--request-permissions",
            "microphone,speech",
        ]) == Set(["microphone", "speech"]))
        #expect(throws: HelperError.self) {
            try permissionRequestFromArguments([
                "LingXiAudioHelper",
                "--request-permissions",
                "camera",
            ])
        }
    }

    @Test
    func archivePathRejectsTraversal() {
        #expect(validateArchivePath("model/tokens.txt"))
        #expect(!validateArchivePath("../escape"))
        #expect(!validateArchivePath("model/../escape"))
        #expect(!validateArchivePath("/absolute/path"))
    }

    @Test
    func recordingPayloadHonorsWaveAndM4aFormats() throws {
        let samples = [Float](repeating: 0, count: 1_600)
        let wave = try recordingPayload(samples: samples, sampleRate: 16_000, format: "wav", maximumPayloadBytes: 64_000)
        let waveData = try #require(Data(base64Encoded: wave.audioBase64))
        #expect(wave.mimeType == "audio/wav")
        #expect(String(data: waveData.prefix(4), encoding: .ascii) == "RIFF")
        #expect(waveData.withUnsafeBytes { $0.load(fromByteOffset: 24, as: UInt32.self) }.littleEndian == 16_000)

        let m4a = try recordingPayload(samples: samples, sampleRate: 16_000, format: "m4a", maximumPayloadBytes: 64_000)
        let m4aData = try #require(Data(base64Encoded: m4a.audioBase64))
        #expect(m4a.mimeType == "audio/mp4")
        #expect(String(data: m4aData.subdata(in: 4..<8), encoding: .ascii) == "ftyp")
        let url = FileManager.default.temporaryDirectory.appendingPathComponent("audio-roundtrip-\(UUID().uuidString).m4a")
        defer { try? FileManager.default.removeItem(at: url) }
        try m4aData.write(to: url)
        let decoded = try AVAudioFile(forReading: url)
        #expect(decoded.processingFormat.sampleRate == 16_000)
        #expect(decoded.processingFormat.channelCount == 1)
        let decodedBuffer = try #require(AVAudioPCMBuffer(pcmFormat: decoded.processingFormat, frameCapacity: 4_096))
        try decoded.read(into: decodedBuffer)
        #expect(decodedBuffer.frameLength >= samples.count)
        #expect(throws: HelperError.self) {
            try recordingPayload(samples: samples, sampleRate: 16_000, format: "m4a", maximumPayloadBytes: m4aData.count - 1)
        }
    }

    @Test
    func stoppedRecordingLeaseIsReleasedEvenWhenPayloadFinalizationFails() {
        var leaseActive = true
        #expect(throws: HelperError.self) {
            try finalizeStoppedRecording(
                ownsCurrentLease: true,
                releaseLease: { leaseActive = false }
            ) {
                throw HelperError.mediaTooLarge("the encoded recording exceeded its payload limit")
            }
        }
        #expect(!leaseActive)

        var newerLeaseActive = true
        let value = finalizeStoppedRecording(
            ownsCurrentLease: false,
            releaseLease: { newerLeaseActive = false }
        ) { 7 }
        #expect(value == 7)
        #expect(newerLeaseActive)
    }

    @Test
    func stoppingRecordingCancellationTargetsItsOriginLease() {
        let recordingOwner = HelperAudioOwner(
            type: "session",
            sessionID: "recording-session",
            instanceID: nil
        )
        let otherOwner = HelperAudioOwner(
            type: "session",
            sessionID: "other-session",
            instanceID: nil
        )
        let origin = HelperAudioOperationIdentity(id: "recording-origin", generation: 1, serviceEpoch: 7)
        let stop = HelperAudioOperationIdentity(id: "recording-stop", generation: 2, serviceEpoch: 7)

        #expect(recordingOriginToRollback(
            for: stop,
            operation: "stop_recording",
            owner: recordingOwner,
            recordingOrigin: origin,
            recordingOwner: recordingOwner
        ) == origin)
        #expect(recordingOriginToRollback(
            for: stop,
            operation: "listen",
            owner: recordingOwner,
            recordingOrigin: origin,
            recordingOwner: recordingOwner
        ) == nil)
        #expect(recordingOriginToRollback(
            for: stop,
            operation: "stop_recording",
            owner: otherOwner,
            recordingOrigin: origin,
            recordingOwner: recordingOwner
        ) == nil)
        #expect(recordingOriginToRollback(
            for: origin,
            operation: "start_recording",
            owner: recordingOwner,
            recordingOrigin: origin,
            recordingOwner: recordingOwner
        ) == origin)
    }

    @Test
    func recordingStartSurfacesAnInitialCaptureFailureInsteadOfReturningAHandle() async {
        let initialFailure = HelperError.mediaTooLarge("the first capture buffer exceeds the configured payload limit")
        let gate = await MainActor.run { CaptureAdmissionGate() }
        let pendingAdmission = Task { @MainActor in
            await gate.waitForFirstBuffer(timeoutNanoseconds: 500_000_000)
        }
        try? await Task.sleep(nanoseconds: 10_000_000)
        await MainActor.run { gate.reportFirstBuffer(failure: initialFailure) }
        let capturedFailure = await pendingAdmission.value
        #expect(capturedFailure?.code == "media-too-large")

        do {
            try validateRecordingStart(recordingFailure: capturedFailure, leaseIsActive: false)
            #expect(Bool(false), "an oversized first capture buffer was admitted as a recording")
        } catch let error as HelperError {
            #expect(error.code == "media-too-large")
        } catch {
            #expect(Bool(false), "unexpected error type: \(error)")
        }

        do {
            try validateRecordingStart(recordingFailure: nil, leaseIsActive: false)
            #expect(Bool(false), "a recording handle was admitted after its capture lease ended")
        } catch let error as HelperError {
            #expect(error.code == "cancelled")
        } catch {
            #expect(Bool(false), "unexpected error type: \(error)")
        }
    }

    @Test
    func rejectedSecondRecordingStartPreservesTheExistingHandle() throws {
        var handle: String? = "recording-original"
        let originalOwner = HelperAudioOwner(
            type: "session",
            sessionID: "session-original",
            instanceID: nil
        )
        var owner: HelperAudioOwner? = originalOwner
        let originalOrigin = HelperAudioOperationIdentity(id: "origin-original", generation: 1, serviceEpoch: 7)
        var origin: HelperAudioOperationIdentity? = originalOrigin
        do {
            try admitRecordingStart(
                existingHandle: &handle,
                existingOwner: &owner,
                existingOrigin: &origin,
                handle: "recording-next",
                owner: HelperAudioOwner(
                    type: "session",
                    sessionID: "session-next",
                                    instanceID: nil
                ),
                origin: HelperAudioOperationIdentity(id: "origin-next", generation: 2, serviceEpoch: 7),
                physicalAudioBusy: true
            )
            #expect(Bool(false), "a second recording start was admitted while one was active")
        } catch let error as HelperError {
            #expect(error.code == "busy")
        } catch {
            #expect(Bool(false), "unexpected error type: \(error)")
        }
        #expect(handle == "recording-original")
        #expect(owner == originalOwner)
        #expect(origin == originalOrigin)
        try validateRecordingHandle(
            requestedHandle: "recording-original",
            activeHandle: handle,
            activeOwner: owner,
            requestedOwner: originalOwner
        )
        #expect(handle == "recording-original")
    }

    @Test
    func defaultAndAutoUtteranceOverridesClearOnlyTheSavedVoice() {
        let fixedOfflineVoice = AudioVoiceSelection(source: .offline, id: "voice-fixed", modelId: "model-fixed")
        let stored = AudioSpeechPreference(source: .offline, offlineModelId: "model-fixed", voice: fixedOfflineVoice)
        for override in ["default", "auto"] {
            let preference = speechPreferenceForSingleUtterance(stored, voiceOverride: override)
            #expect(preference.source == .offline)
            #expect(preference.offlineModelId == "model-fixed")
            #expect(preference.voice == nil)

            let route = resolveAudioRoute(AudioRouteRequest(
                kind: .speech,
                preference: preference,
                language: "en-US",
                systemStatus: .available,
                offlineModels: [
                    AudioOfflineModelAvailability(
                        id: "model-fixed",
                        kind: .speech,
                        languages: ["en"],
                        installed: true,
                        voiceIds: ["voice-default"]
                    ),
                ]
            ))
            #expect(route.status == .ready)
            #expect(route.effective?.source == .offline)
            #expect(route.effective?.modelId == "model-fixed")
            #expect(route.effective?.voiceId == nil)
        }
    }

    @Test
    func manualSpeechFinishWaitsForFinalResultButHasABound() async {
        let gate = await MainActor.run { SpeechFinalResultGate() }
        let firstWaiter = Task { @MainActor in
            await gate.waitForFinalResult(timeoutNanoseconds: 500_000_000)
        }
        let secondWaiter = Task { @MainActor in
            await gate.waitForFinalResult(timeoutNanoseconds: 500_000_000)
        }
        try? await Task.sleep(nanoseconds: 10_000_000)
        await MainActor.run { gate.signalFinalResult() }
        #expect(await firstWaiter.value)
        #expect(await secondWaiter.value)

        let timeoutGate = await MainActor.run { SpeechFinalResultGate() }
        let gotFinal = await timeoutGate.waitForFinalResult(timeoutNanoseconds: 5_000_000)
        #expect(!gotFinal)
    }

    @Test
    func shellDrainsLargeStdoutAndStderrBeforeWaitingForExit() throws {
        let output = try shell(
            "/usr/bin/perl",
            ["-e", "for (1..2048) { print 'o' x 1024; print STDERR 'e' x 1024 }"]
        )
        #expect(output.utf8.count == 2 * 1024 * 1024)
        #expect(output.first == "o")
    }

    @Test
    func recordingAndRenderCollectorsStopBeforeExceedingRawPayloadBound() throws {
        #expect(throws: HelperError.self) {
            try recordingPayload(
                samples: [Float](repeating: 0, count: 17),
                sampleRate: 16_000,
                format: "wav",
                maximumPayloadBytes: 64
            )
        }

        let collector = PCM16SampleCollector(maximumBytes: 8)
        let samples: [Float] = [0.25, -0.25, 0.5, -0.5, 0.75]
        let written = samples.withUnsafeBufferPointer {
            collector.append(samples: $0.baseAddress, count: $0.count)
        }
        #expect(written == 0)
        let (pcm, error) = collector.result()
        #expect(pcm.isEmpty)
        #expect(error?.message.contains("payload limit") == true)
    }

    @Test
    func systemRenderCollectorRejectsOversizedActualPcmBuffer() async {
        let format = AVAudioFormat(
            commonFormat: .pcmFormatFloat32,
            sampleRate: 16_000,
            channels: 2,
            interleaved: false
        )!
        let buffer = AVAudioPCMBuffer(pcmFormat: format, frameCapacity: 2)!
        buffer.frameLength = 2
        buffer.floatChannelData![0][0] = 0.25
        buffer.floatChannelData![0][1] = 0.5
        buffer.floatChannelData![1][0] = -0.25
        buffer.floatChannelData![1][1] = -0.5
        let collector = SystemPCMCollector(maximumBytes: 2)
        let result = Task { try await collector.value() }
        await Task.yield()
        collector.append(buffer)
        do {
            _ = try await result.value
            #expect(Bool(false), "oversized system PCM unexpectedly succeeded")
        } catch let error as HelperError {
            #expect(error.message.contains("payload limit"))
        } catch {
            #expect(Bool(false), "unexpected error type: \(error)")
        }
    }

    @Test
    func cancelledCaptureAdmissionDoesNotStartTheMicrophone() async {
        let cancellation = AudioCancellationFlag()
        cancellation.cancel()
        let session = await MainActor.run {
            ListeningSession(
                owner: HelperOwner(kind: "session", id: "capture-admission-test"),
                languageIdentifier: "en-US",
                route: .captureOnly,
                outputSampleRate: 16_000,
                maximumSampleCount: 128,
                cancellation: cancellation,
                onPartial: { _ in },
                onLevel: { _ in },
                onSilence: {},
                onLimit: {}
            )
        }
        do {
            try await MainActor.run { try session.start() }
            #expect(Bool(false), "a cancelled capture was admitted")
        } catch let error as HelperError {
            #expect(error.message.contains("cancelled"))
        } catch {
            #expect(Bool(false), "unexpected error type: \(error)")
        }
    }

    @Test
    func listeningMonitorStopsAfterCancellationDuringItsSleep() async {
        let tickCount = AudioOperationCallCounter()
        let sleepCount = AudioOperationCallCounter()
        let (ticks, tickContinuation) = AsyncStream<Void>.makeStream()
        let (sleeping, sleepContinuation) = AsyncStream<Void>.makeStream()
        let monitor = Task { @MainActor in
            await runListeningMonitor(
                intervalNanoseconds: 60_000_000_000,
                sleep: { nanoseconds in
                    sleepCount.increment()
                    if sleepCount.value > 1 {
                        sleepContinuation.yield(())
                        try await Task.sleep(nanoseconds: nanoseconds)
                    }
                },
                tick: {
                    tickCount.increment()
                    tickContinuation.yield(())
                    return true
                }
            )
        }
        var tickIterator = ticks.makeAsyncIterator()
        var sleepIterator = sleeping.makeAsyncIterator()
        #expect(await tickIterator.next() != nil)
        #expect(await sleepIterator.next() != nil)
        monitor.cancel()
        await monitor.value
        tickContinuation.finish()
        sleepContinuation.finish()
        #expect(tickCount.value == 1)
    }

    @Test
    func listeningMonitorBoundsSessionsThatNeverDetectSpeech() {
        #expect(listeningMonitorOutcome(
            hasHeardSpeech: false,
            elapsedSinceStart: 14.9,
            elapsedSinceSpeech: 14.9,
            noSpeechTimeout: 15
        ) == .continueListening)
        #expect(listeningMonitorOutcome(
            hasHeardSpeech: false,
            elapsedSinceStart: 15,
            elapsedSinceSpeech: 15,
            noSpeechTimeout: 15
        ) == .noSpeech)
        #expect(listeningMonitorOutcome(
            hasHeardSpeech: true,
            elapsedSinceStart: 3,
            elapsedSinceSpeech: 1.19,
            silenceTimeout: 1.2,
            noSpeechTimeout: 15
        ) == .continueListening)
        #expect(listeningMonitorOutcome(
            hasHeardSpeech: true,
            elapsedSinceStart: 4,
            elapsedSinceSpeech: 1.2,
            silenceTimeout: 1.2,
            noSpeechTimeout: 15
        ) == .finalizeSpeech)
    }

    @Test
    func finishListeningCommandLatchesTheExactIdentityBeforeAdmission() async throws {
        let output = AudioHelperOutputProbe()
        let writer = LineWriter(emit: { output.append($0) })
        let root = FileManager.default.temporaryDirectory.appending(path: "lingxi-finish-listen-\(UUID().uuidString)")
        let state = HelperStateStore(storageRoot: root, writer: writer)
        let epoch = try #require((await state.snapshot(message: nil)).capabilities?.serviceEpoch)
        let identity: [String: Any] = [
            "id": UUID().uuidString.lowercased(),
            "generation": 1,
            "service_epoch": epoch,
        ]
        await state.handleCommand(id: "finish-before-admission", input: [
            "type": "finish_listening",
            "identity": identity,
            "owner": ["kind": "ui", "id": "orb-instance"],
        ])
        let envelope = try #require(output.values.compactMap {
            try? JSONSerialization.jsonObject(with: $0) as? [String: Any]
        }.first(where: { $0["id"] as? String == "finish-before-admission" }))
        let response = try #require(envelope["result"] as? [String: Any])
        #expect(response["type"] as? String == "listening_finished")
        #expect(response["transcript"] == nil)
        #expect(await state.pendingListenFinishCount() == 1)

        await state.cancelOperation(identity)
        #expect(await state.pendingListenFinishCount() == 0)
    }

    @Test
    func modelRetryRestartsUnsatisfiableRangeAndDiscardsBadChecksum() async throws {
        let model = try #require(GeneratedVoiceModelCatalog.all.first)
        let root = FileManager.default.temporaryDirectory.appending(path: "lingxi-model-retry-\(UUID().uuidString)")
        defer { try? FileManager.default.removeItem(at: root) }
        let configuration = URLSessionConfiguration.ephemeral
        configuration.protocolClasses = [ModelRetryURLProtocol.self]
        let session = URLSession(configuration: configuration)
        defer { session.invalidateAndCancel() }
        let store = ModelStore(root: root, writer: LineWriter(emit: { _ in }), downloadSession: session) { emptyHelperSnapshot() }
        let archive = root.appending(path: ".download-\(model.id).part")
        let metadata = root.appending(path: ".download-\(model.id).json")
        try Data("retained".utf8).write(to: archive)
        try Data("{\"etag\":\"old\"}".utf8).write(to: metadata)
        for attempt in 0..<2 {
            try await store.install(modelID: model.id)
            var failed = false
            for _ in 0..<200 {
                if case .failed = await store.snapshots().first(where: { $0.modelId == model.id })?.state {
                    failed = true
                    break
                }
                try await Task.sleep(nanoseconds: 10_000_000)
            }
            #expect(failed, "attempt \(attempt) must settle without a retry loop")
            await store.cancel(modelID: model.id)
            #expect(!FileManager.default.fileExists(atPath: archive.path))
            #expect(!FileManager.default.fileExists(atPath: metadata.path))
        }
        #expect(ModelRetryURLProtocol.requests.values.map { String(decoding: $0, as: UTF8.self) } == ["bytes=8-", "full", "full"])
    }

    @Test
    func modelInstallReservesPartFileBeforePublishingAndQuiescesOnCancel() async throws {
        let model = try #require(GeneratedVoiceModelCatalog.all.first)
        let root = FileManager.default.temporaryDirectory.appending(path: "lingxi-model-cancel-\(UUID().uuidString)")
        let snapshotBarrier = ModelInstallTestBarrier()
        let downloadBarrier = ModelInstallTestBarrier()
        let store = ModelStore(
            root: root,
            writer: LineWriter(emit: { _ in }),
            downloadOverride: { _, archive in
                await downloadBarrier.pause()
                try Data("partial archive".utf8).write(to: archive)
            }
        ) {
            await snapshotBarrier.pause()
            return emptyHelperSnapshot()
        }

        let firstInstall = Task { try await store.install(modelID: model.id) }
        await snapshotBarrier.waitForCallCount(1)
        let secondInstall = Task { try await store.install(modelID: model.id) }
        try await Task.sleep(nanoseconds: 20_000_000)
        let publishedDuringAdmission = await snapshotBarrier.callCount()
        await snapshotBarrier.releaseAll()
        try await firstInstall.value
        try await secondInstall.value
        await downloadBarrier.waitForCallCount(1)
        #expect(publishedDuringAdmission == 1)
        #expect(await downloadBarrier.callCount() == 1)

        let cancelReturned = AudioOperationCallCounter()
        let cancellation = Task {
            await store.cancel(modelID: model.id)
            cancelReturned.increment()
        }
        try await Task.sleep(nanoseconds: 20_000_000)
        #expect(cancelReturned.value == 0)
        await downloadBarrier.releaseOne()
        await cancellation.value
        #expect(cancelReturned.value == 1)
        #expect(!FileManager.default.fileExists(atPath: root.appending(path: model.id).path))
        let cancelledState = await store.snapshots().first(where: { $0.modelId == model.id })?.state
        #expect(cancelledState == .notInstalled)

        try await store.install(modelID: model.id)
        try await Task.sleep(nanoseconds: 20_000_000)
        #expect(await downloadBarrier.callCount() == 2)
        let retryCancellation = Task { await store.cancel(modelID: model.id) }
        try await Task.sleep(nanoseconds: 20_000_000)
        await downloadBarrier.releaseOne()
        await retryCancellation.value
    }

    @Test
    func modelRemovalWaitsForCancelledInstallerBeforeDeletingPartialFiles() async throws {
        let model = try #require(GeneratedVoiceModelCatalog.all.first)
        let root = FileManager.default.temporaryDirectory.appending(path: "lingxi-model-remove-\(UUID().uuidString)")
        let downloadBarrier = ModelInstallTestBarrier()
        let store = ModelStore(
            root: root,
            writer: LineWriter(emit: { _ in }),
            downloadOverride: { _, archive in
                await downloadBarrier.pause()
                try Data("late partial archive".utf8).write(to: archive)
            },
            snapshotProvider: { emptyHelperSnapshot() }
        )

        try await store.install(modelID: model.id)
        await downloadBarrier.waitForCallCount(1)
        let removeReturned = AudioOperationCallCounter()
        let removal = Task {
            try await store.remove(modelID: model.id)
            removeReturned.increment()
        }
        try await Task.sleep(nanoseconds: 20_000_000)
        #expect(removeReturned.value == 0)
        await downloadBarrier.releaseOne()
        try await removal.value
        #expect(removeReturned.value == 1)
        #expect(!FileManager.default.fileExists(atPath: root.appending(path: model.id).path))
        #expect(!FileManager.default.fileExists(atPath: root.appending(path: ".download-\(model.id).part").path))
        let removedState = await store.snapshots().first(where: { $0.modelId == model.id })?.state
        #expect(removedState == .notInstalled)

        try await store.install(modelID: model.id)
        try await Task.sleep(nanoseconds: 20_000_000)
        #expect(await downloadBarrier.callCount() == 2)
        let reinstallCancellation = Task { await store.cancel(modelID: model.id) }
        try await Task.sleep(nanoseconds: 20_000_000)
        await downloadBarrier.releaseOne()
        await reinstallCancellation.value
    }

    @Test
    func helperJsonlDispatchCancelsLongAudioRequestExactlyOnce() async throws {
        let output = AudioHelperOutputProbe()
        let writer = LineWriter(emit: { output.append($0) })
        let (started, startedContinuation) = AsyncStream<Void>.makeStream()
        let executor: HelperAudioOperationExecutor = { _, cancellation in
            startedContinuation.yield(())
            try await Task.sleep(nanoseconds: 60_000_000_000)
            try cancellation.check()
            return .status(recording: false, playing: false)
        }
        let root = FileManager.default.temporaryDirectory.appending(path: "lingxi-audio-dispatch-\(UUID().uuidString)")
        let state = HelperStateStore(storageRoot: root, writer: writer, operationExecutor: executor)
        let serviceSnapshot = await state.snapshot(message: nil)
        let serviceEpoch = try #require(serviceSnapshot.capabilities?.serviceEpoch)
        let identity: [String: Any] = [
            "id": UUID().uuidString.lowercased(),
            "generation": 4,
            "service_epoch": serviceEpoch,
        ]
        let owner: [String: Any] = ["type": "session", "session_id": "session-1"]
        let configuration: [String: Any] = [
            "schemaVersion": 4,
            "recognition": ["source": "automatic"],
            "speech": ["source": "automatic"],
            "language": "auto",
            "rate": 1.0,
            "autoPlayReplies": false,
        ]
        let request: [String: Any] = [
            "id": "engine-request-1",
            "kind": "engine_request",
            "configuration": configuration,
            "configurationRevision": 37,
            "request": [
                "identity": identity,
                "owner": owner,
                "max_payload_bytes": 65_536,
                "timeout_budget_ms": 120_000,
                "operation": ["type": "status"],
            ],
        ]
        let cancel: [String: Any] = [
            "id": "cancel-request-1",
            "kind": "command",
            "command": ["type": "cancel_operation", "identity": identity],
        ]
        let (lines, continuation) = AsyncStream<String>.makeStream()
        let dispatcher = Task {
            await readInputLines(
                lines,
                handle: { await handleHelperInputEnvelope($0, state: state) },
                cancel: { await cancelHelperInputEnvelope($0, state: state) },
                reject: { await state.rejectOverloaded($0) }
            )
        }
        var startedIterator = started.makeAsyncIterator()
        continuation.yield(String(data: try JSONSerialization.data(withJSONObject: request), encoding: .utf8)!)
        #expect(await startedIterator.next() != nil)
        continuation.yield(String(data: try JSONSerialization.data(withJSONObject: cancel), encoding: .utf8)!)
        continuation.finish()
        await dispatcher.value

        var requestResponses: [[String: Any]] = []
        for _ in 0..<100 {
            requestResponses = output.values.compactMap { data in
                guard let envelope = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
                      envelope["id"] as? String == "engine-request-1" else { return nil }
                return envelope
            }
            if !requestResponses.isEmpty { break }
            try await Task.sleep(nanoseconds: 10_000_000)
        }
        #expect(requestResponses.count == 1)
        #expect(requestResponses.first?["id"] as? String == "engine-request-1")
        let engineResult = try #require(requestResponses.first?["result"] as? [String: Any])
        #expect(engineResult["type"] as? String == "engine_result")
        let terminal = try #require(engineResult["result"] as? [String: Any])
        #expect(terminal["type"] as? String == "failed")
        let failure = try #require(terminal["error"] as? [String: Any])
        #expect(failure["kind"] as? String == "cancelled")
        let snapshot = try #require(engineResult["snapshot"] as? [String: Any])
        #expect(snapshot["configurationRevision"] as? Int == 37)
        #expect(snapshot["activeOperationCount"] as? Int == 0)
        #expect(snapshot["pendingOperationCount"] as? Int == 0)
        #expect(snapshot["activeRecordingCount"] as? Int == 0)
        #expect(snapshot["activePlaybackCount"] as? Int == 0)
        #expect(snapshot["currentOperation"] == nil)
        #expect(snapshot["owner"] == nil)
        #expect(snapshot["activity"] as? String == "idle")
        let traces = try #require(snapshot["audioOperations"] as? [[String: Any]])
        #expect(traces.count == 1)
        #expect(traces[0]["configurationRevision"] as? Int == 37)
    }

    @Test
    func helperAllowsFreshUuidAfterReconnectResetsProducerGeneration() async throws {
        let output = AudioHelperOutputProbe()
        let calls = AudioOperationCallCounter()
        let writer = LineWriter(emit: { output.append($0) })
        let executor: HelperAudioOperationExecutor = { _, cancellation in
            try cancellation.check()
            calls.increment()
            return .status(recording: false, playing: false)
        }
        let root = FileManager.default.temporaryDirectory.appending(path: "lingxi-audio-reconnect-\(UUID().uuidString)")
        let state = HelperStateStore(storageRoot: root, writer: writer, operationExecutor: executor)
        let epoch = try #require((await state.snapshot(message: nil)).capabilities?.serviceEpoch)
        let configuration: [String: Any] = [
            "schemaVersion": 4,
            "recognition": ["source": "automatic"],
            "speech": ["source": "automatic"],
            "language": "auto",
            "rate": 1.0,
            "autoPlayReplies": false,
        ]
        let owner: [String: Any] = ["type": "session", "session_id": "stable-session"]
        let (lines, continuation) = AsyncStream<String>.makeStream()
        let dispatcher = Task {
            await readInputLines(
                lines,
                handle: { await handleHelperInputEnvelope($0, state: state) },
                cancel: { await cancelHelperInputEnvelope($0, state: state) },
                reject: { await state.rejectOverloaded($0) }
            )
        }
        for (requestID, generation) in [("reconnect-old", 12), ("reconnect-new", 1)] {
            let request: [String: Any] = [
                "id": requestID,
                "kind": "engine_request",
                "configuration": configuration,
                "configurationRevision": 8,
                "request": [
                    "identity": ["id": UUID().uuidString.lowercased(), "generation": generation, "service_epoch": epoch],
                    "owner": owner,
                    "max_payload_bytes": 65_536,
                    "operation": ["type": "status"],
                ],
            ]
            continuation.yield(String(data: try JSONSerialization.data(withJSONObject: request), encoding: .utf8)!)
        }
        var responses: [String: [String: Any]] = [:]
        for _ in 0..<100 {
            for data in output.values {
                guard let envelope = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
                      let id = envelope["id"] as? String,
                      id == "reconnect-old" || id == "reconnect-new" else { continue }
                responses[id] = envelope
            }
            if responses.count == 2 { break }
            try await Task.sleep(nanoseconds: 10_000_000)
        }
        continuation.finish()
        await dispatcher.value
        #expect(calls.value == 2)
        #expect(responses.count == 2)
        for id in ["reconnect-old", "reconnect-new"] {
            let envelope = try #require(responses[id])
            let result = try #require(envelope["result"] as? [String: Any])
            #expect(result["type"] as? String == "engine_result")
            let operationResult = try #require(result["result"] as? [String: Any])
            #expect(operationResult["type"] as? String == "status")
        }
    }

    @Test
    func zeroTimeoutIsTerminalBeforeNativeOperationDispatch() async throws {
        let output = AudioHelperOutputProbe()
        let calls = AudioOperationCallCounter()
        let writer = LineWriter(emit: { output.append($0) })
        let executor: HelperAudioOperationExecutor = { _, _ in
            calls.increment()
            return .status(recording: false, playing: false)
        }
        let root = FileManager.default.temporaryDirectory.appending(path: "lingxi-audio-zero-timeout-\(UUID().uuidString)")
        let state = HelperStateStore(storageRoot: root, writer: writer, operationExecutor: executor)
        let epoch = try #require((await state.snapshot(message: nil)).capabilities?.serviceEpoch)
        let request: [String: Any] = [
            "id": "zero-timeout",
            "kind": "engine_request",
            "configuration": [
                "schemaVersion": 4,
                "recognition": ["source": "automatic"],
                "speech": ["source": "automatic"],
                "language": "auto",
                "rate": 1.0,
                "autoPlayReplies": false,
            ],
            "configurationRevision": 19,
            "request": [
                "identity": ["id": UUID().uuidString.lowercased(), "generation": 1, "service_epoch": epoch],
                "owner": ["type": "session", "session_id": "expired-session"],
                "timeout_budget_ms": 0,
                "max_payload_bytes": 65_536,
                "operation": ["type": "listen"],
            ],
        ]
        let (lines, continuation) = AsyncStream<String>.makeStream()
        let dispatcher = Task {
            await readInputLines(
                lines,
                handle: { await handleHelperInputEnvelope($0, state: state) },
                cancel: { await cancelHelperInputEnvelope($0, state: state) },
                reject: { await state.rejectOverloaded($0) }
            )
        }
        continuation.yield(String(data: try JSONSerialization.data(withJSONObject: request), encoding: .utf8)!)
        var response: [String: Any]?
        for _ in 0..<100 {
            response = output.values.compactMap { data in
                guard let envelope = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
                      envelope["id"] as? String == "zero-timeout" else { return nil }
                return envelope
            }.first
            if response != nil { break }
            try await Task.sleep(nanoseconds: 10_000_000)
        }
        continuation.finish()
        await dispatcher.value
        #expect(calls.value == 0)
        let engineResult = try #require(response?["result"] as? [String: Any])
        let terminal = try #require(engineResult["result"] as? [String: Any])
        #expect(terminal["type"] as? String == "failed")
        let failure = try #require(terminal["error"] as? [String: Any])
        #expect(failure["kind"] as? String == "timeout")
        let snapshot = try #require(engineResult["snapshot"] as? [String: Any])
        #expect(snapshot["configurationRevision"] as? Int == 19)
        #expect(snapshot["pendingOperationCount"] as? Int == 0)
    }

    @Test
    func helperEofCancelsAdmittedAudioOperationAndDrainsItsScope() async throws {
        let output = AudioHelperOutputProbe()
        let (started, startedContinuation) = AsyncStream<Void>.makeStream()
        let executor: HelperAudioOperationExecutor = { _, cancellation in
            startedContinuation.yield(())
            try await Task.sleep(nanoseconds: 60_000_000_000)
            try cancellation.check()
            return .status(recording: false, playing: false)
        }
        let state = HelperStateStore(
            storageRoot: FileManager.default.temporaryDirectory.appending(path: "lingxi-audio-eof-\(UUID().uuidString)"),
            writer: LineWriter(emit: { output.append($0) }),
            operationExecutor: executor
        )
        let epoch = try #require((await state.snapshot(message: nil)).capabilities?.serviceEpoch)
        let request: [String: Any] = [
            "id": "eof-audio-request",
            "kind": "engine_request",
            "configuration": [
                "schemaVersion": 4,
                "recognition": ["source": "automatic"],
                "speech": ["source": "automatic"],
                "language": "auto",
                "rate": 1.0,
                "autoPlayReplies": false,
            ],
            "configurationRevision": 22,
            "request": [
                "identity": ["id": UUID().uuidString.lowercased(), "generation": 3, "service_epoch": epoch],
                "owner": ["type": "session", "session_id": "eof-session"],
                "timeout_budget_ms": 120_000,
                "max_payload_bytes": 65_536,
                "operation": ["type": "status"],
            ],
        ]
        let (lines, continuation) = AsyncStream<String>.makeStream()
        let dispatcher = Task { await runHelperInputLoop(lines, state: state) }
        var startedIterator = started.makeAsyncIterator()
        continuation.yield(String(data: try JSONSerialization.data(withJSONObject: request), encoding: .utf8)!)
        #expect(await startedIterator.next() != nil)
        continuation.finish()
        await dispatcher.value

        let response = try #require(output.values.compactMap { data -> [String: Any]? in
            guard let envelope = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
                  envelope["id"] as? String == "eof-audio-request" else { return nil }
            return envelope
        }.first)
        let engineResult = try #require(response["result"] as? [String: Any])
        let terminal = try #require(engineResult["result"] as? [String: Any])
        #expect(terminal["type"] as? String == "failed")
        let failure = try #require(terminal["error"] as? [String: Any])
        #expect(failure["kind"] as? String == "cancelled")
        let snapshot = await state.snapshot(message: nil)
        #expect(snapshot.activeOperationCount == 0)
        #expect(snapshot.pendingOperationCount == 0)
        #expect(snapshot.activeRecordingCount == 0)
        #expect(snapshot.activePlaybackCount == 0)
    }

    @Test
    func concurrentEndOwnerRequestsDoNotCancelEachOther() async throws {
        let output = AudioHelperOutputProbe()
        let (started, startedContinuation) = AsyncStream<Void>.makeStream()
        let executor: HelperAudioOperationExecutor = { operation, cancellation in
            guard operation == "status" else { return nil }
            startedContinuation.yield(())
            try await Task.sleep(nanoseconds: 60_000_000_000)
            try cancellation.check()
            return .status(recording: false, playing: false)
        }
        let state = HelperStateStore(
            storageRoot: FileManager.default.temporaryDirectory.appending(path: "lingxi-audio-end-owner-\(UUID().uuidString)"),
            writer: LineWriter(emit: { output.append($0) }),
            operationExecutor: executor
        )
        let epoch = try #require((await state.snapshot(message: nil)).capabilities?.serviceEpoch)
        let owner: [String: Any] = ["type": "session", "session_id": "end-owner-session"]
        let configuration: [String: Any] = [
            "schemaVersion": 4,
            "recognition": ["source": "automatic"],
            "speech": ["source": "automatic"],
            "language": "auto",
            "rate": 1.0,
            "autoPlayReplies": false,
        ]
        let (lines, continuation) = AsyncStream<String>.makeStream()
        let dispatcher = Task { await runHelperInputLoop(lines, state: state) }
        var startedIterator = started.makeAsyncIterator()
        let longOperation: [String: Any] = [
            "id": "owner-status",
            "kind": "engine_request",
            "configuration": configuration,
            "configurationRevision": 3,
            "request": [
                "identity": ["id": UUID().uuidString.lowercased(), "generation": 1, "service_epoch": epoch],
                "owner": owner,
                "timeout_budget_ms": 120_000,
                "max_payload_bytes": 65_536,
                "operation": ["type": "status"],
            ],
        ]
        continuation.yield(String(data: try JSONSerialization.data(withJSONObject: longOperation), encoding: .utf8)!)
        #expect(await startedIterator.next() != nil)
        for (id, generation) in [("end-owner-a", 2), ("end-owner-b", 3)] {
            let endOwner: [String: Any] = [
                "id": id,
                "kind": "engine_request",
                "configuration": configuration,
                "configurationRevision": 0,
                "request": [
                    "identity": ["id": UUID().uuidString.lowercased(), "generation": generation, "service_epoch": epoch],
                    "owner": owner,
                    "timeout_budget_ms": 120_000,
                    "max_payload_bytes": 65_536,
                    "operation": ["type": "end_owner"],
                ],
            ]
            continuation.yield(String(data: try JSONSerialization.data(withJSONObject: endOwner), encoding: .utf8)!)
        }

        var responses: [String: [String: Any]] = [:]
        for _ in 0..<250 {
            for data in output.values {
                guard let envelope = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
                      let id = envelope["id"] as? String,
                      ["owner-status", "end-owner-a", "end-owner-b"].contains(id) else { continue }
                responses[id] = envelope
            }
            if responses.count == 3 { break }
            try await Task.sleep(nanoseconds: 10_000_000)
        }
        continuation.finish()
        await dispatcher.value
        #expect(responses.count == 3)
        for id in ["end-owner-a", "end-owner-b"] {
            let envelope = try #require(responses[id])
            let response = try #require(envelope["result"] as? [String: Any])
            let result = try #require(response["result"] as? [String: Any])
            #expect(result["type"] as? String == "owner_ended")
        }
        let snapshot = await state.snapshot(message: nil)
        #expect(snapshot.activeOperationCount == 0)
        #expect(snapshot.pendingOperationCount == 0)
    }

    @Test
    func missingExplicitOfflineVoiceDoesNotFallBackToSystem() {
        let root = FileManager.default.temporaryDirectory
            .appending(path: "lingxi-missing-voice-\(UUID().uuidString)")
        #expect(throws: HelperError.self) {
            try resolvePlaybackVoice(
                "sherpa:missing:model-voice",
                language: "en-US",
                root: root
            )
        }
    }

    @Test
    func helperSnapshotOmitsInternalPaths() throws {
        let snapshot = HelperSnapshot(
            helper: .init(state: "running", message: nil),
            permissions: .init(microphone: "granted", speech: "authorized"),
            owner: nil,
            activity: "idle",
            localeTag: "en-US",
            recognizerAvailable: true,
            recognition: nil,
            playback: nil,
            voices: [
                .init(
                    id: "system:default",
                    label: "System Default",
                    languageTag: "en-US",
                    source: "system",
                    familyId: "system",
                    isDefault: true,
                    networkRequired: false
                ),
            ],
            models: [
                .init(modelId: "sherpa.moonshine-tiny-en", state: .ready),
            ],
            capabilities: nil,
            configurationRevision: nil,
            currentOperation: nil
        )

        let encoded = try JSONEncoder().encode(snapshot)
        let object = try #require(JSONSerialization.jsonObject(with: encoded) as? [String: Any])
        #expect(object["storageRoot"] == nil)
        let helper = try #require(object["helper"] as? [String: Any])
        #expect(helper["path"] == nil)
    }

    @Test
    func modelStateEncodesSharedKeys() throws {
        let snapshot = HelperModelSnapshot(
            modelId: "sherpa.kitten-nano-en",
            state: .failed("checksum mismatch")
        )
        let encoded = try JSONEncoder().encode(snapshot)
        let object = try #require(JSONSerialization.jsonObject(with: encoded) as? [String: Any])
        #expect(object["modelId"] as? String == "sherpa.kitten-nano-en")
        let state = try #require(object["state"] as? [String: Any])
        #expect(state["type"] as? String == "failed")
        #expect(state["message"] as? String == "checksum mismatch")
    }

    @Test
    func inputLevelEventIsLightweightAndOwnerScoped() throws {
        let event = HelperEvent(
            type: "input_level",
            snapshot: nil,
            owner: .init(kind: "dictation", id: "dictation-1"),
            progress: nil,
            model: nil,
            state: nil,
            error: nil,
            message: nil,
            level: 0.42
        )
        let encoded = try JSONEncoder().encode(event)
        let object = try #require(JSONSerialization.jsonObject(with: encoded) as? [String: Any])
        #expect(object["type"] as? String == "input_level")
        #expect(object["snapshot"] == nil)
        #expect(object["level"] as? Double == 0.42)
        let owner = try #require(object["owner"] as? [String: Any])
        #expect(owner["kind"] as? String == "dictation")
        #expect(owner["id"] as? String == "dictation-1")
    }

    @Test
    func jsonlReaderContinuesToDispatchTargetedCancelDuringLongAudioOperation() async {
        let (lines, lineContinuation) = AsyncStream<String>.makeStream()
        let (signals, signalContinuation) = AsyncStream<String>.makeStream()
        let dispatcher = Task {
            await readInputLines(lines, handle: { envelope in
                if envelope["kind"] as? String == "engine_request" {
                    signalContinuation.yield("started")
                    do {
                        try await Task.sleep(nanoseconds: 60_000_000_000)
                        signalContinuation.yield("completed")
                    } catch {
                        signalContinuation.yield("cancelled")
                    }
                } else {
                    signalContinuation.yield("cancel_acknowledged")
                }
            }, cancel: { _ in })
        }
        var signalIterator = signals.makeAsyncIterator()
        let identity: [String: Any] = ["id": "long-listen", "generation": 4, "service_epoch": 9]
        let request: [String: Any] = [
            "id": "helper-request-1",
            "kind": "engine_request",
            "request": ["identity": identity],
        ]
        let cancel: [String: Any] = [
            "id": "helper-cancel-1",
            "kind": "command",
            "command": ["type": "cancel_operation", "identity": identity],
        ]
        lineContinuation.yield(String(data: try! JSONSerialization.data(withJSONObject: request), encoding: .utf8)!)

        let started = await signalIterator.next()
        #expect(started == "started")
        lineContinuation.yield(String(data: try! JSONSerialization.data(withJSONObject: cancel), encoding: .utf8)!)
        lineContinuation.finish()
        await dispatcher.value
        var received: [String] = []
        while !(received.contains("cancel_acknowledged") && received.contains("cancelled")) {
            guard let signal = await signalIterator.next() else { break }
            received.append(signal)
        }
        #expect(received.contains("cancel_acknowledged"))
        #expect(received.contains("cancelled"))
        signalContinuation.finish()
    }
}

@Test func automaticSpeechFallbackRespectsPerRequestVoice() {
    let preference = AudioSpeechPreference(source: .automatic, offlineModelId: nil, voice: nil)
    for explicitVoice in [false, true] {
        let route = resolveAudioRoute(AudioRouteRequest(
            kind: .speech, preference: preference, language: "en-US",
            systemStatus: .available, offlineModels: [], systemVoiceIds: ["test-voice"],
            voiceOverride: explicitVoice ? AudioVoiceSelection(source: .system, id: "test-voice", modelId: nil) : nil
        ))
        #expect(route.status == .ready)
        #expect(allowsAutomaticSpeechFallback(route) == !explicitVoice)
    }
}

@Test func cancellingOneModelPreservesAnotherActiveInstall() async throws {
    let first = try #require(GeneratedVoiceModelCatalog.all.first)
    let second = try #require(GeneratedVoiceModelCatalog.all.last)
    #expect(first.id != second.id)
    let root = FileManager.default.temporaryDirectory.appending(path: "lingxi-model-parallel-\(UUID().uuidString)")
    defer { try? FileManager.default.removeItem(at: root) }
    let firstBarrier = ModelInstallTestBarrier()
    let secondBarrier = ModelInstallTestBarrier()
    let store = ModelStore(root: root, writer: LineWriter(emit: { _ in }), downloadOverride: { model, _ in
        if model.id == first.id { await firstBarrier.pause() }
        else { await secondBarrier.pause() }
        try Task.checkCancellation()
    }) { emptyHelperSnapshot() }
    try await store.install(modelID: first.id)
    try await store.install(modelID: second.id)
    await firstBarrier.waitForCallCount(1)
    await secondBarrier.waitForCallCount(1)
    let before = await store.snapshots().first { $0.modelId == second.id }?.state
    let cancelFirst = Task { await store.cancel(modelID: first.id) }
    await firstBarrier.releaseOne()
    await cancelFirst.value
    #expect(await store.snapshots().first { $0.modelId == second.id }?.state == before)
    try await store.remove(modelID: first.id)
    #expect(await store.snapshots().first { $0.modelId == second.id }?.state == before)
    let cancelSecond = Task { await store.cancel(modelID: second.id) }
    await secondBarrier.releaseOne()
    await cancelSecond.value
    #expect(await store.snapshots().first { $0.modelId == second.id }?.state == .notInstalled)
}

@Test func recordingStatusReleasesTerminatedHandleAndAllowsRestart() throws {
    var handle: String? = "terminated"
    var hasOrigin = true
    let recording = recordingStatus(
        ownsRecording: true,
        recordingFailure: .mediaTooLarge("payload limit reached"),
        leaseIsActive: false,
        releaseTerminatedRecording: { handle = nil; hasOrigin = false }
    )
    #expect(!recording)
    #expect(handle == nil)
    try validateRecordingStartAvailability(existingHandle: handle, hasRecordingOrigin: hasOrigin, physicalAudioBusy: false)
}

@Test func recordingStatusPreservesActiveAndOtherOwnerLeases() {
    var releases = 0
    #expect(recordingStatus(ownsRecording: true, recordingFailure: nil, leaseIsActive: true,
                            releaseTerminatedRecording: { releases += 1 }))
    #expect(!recordingStatus(ownsRecording: false, recordingFailure: .mediaTooLarge("limit"), leaseIsActive: false,
                             releaseTerminatedRecording: { releases += 1 }))
    #expect(!recordingStatus(ownsRecording: true, recordingFailure: nil, leaseIsActive: false,
                             releaseTerminatedRecording: { releases += 1 }))
    #expect(releases == 0)
}


@Test func systemVoiceNamesDoNotResolveAsStableIdentifiers() {
    let route = resolveAudioRoute(AudioRouteRequest(kind: .speech,
        preference: AudioSpeechPreference(source: .system, voice: .init(source: .system, id: "Samantha")),
        language: "en-US", systemStatus: .available, offlineModels: [], systemVoiceIds: ["com.apple.voice.compact.en-US.Samantha"]))
    #expect(route.status == .unavailable)
    #expect(route.reason == "systemVoiceUnknown")
}
