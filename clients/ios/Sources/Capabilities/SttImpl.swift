// SttImpl.swift — iOS native speech-to-text capability (parity with Android
// SystemSpeechRecognizerStt.kt).

import Foundation

#if canImport(Speech) && canImport(AVFoundation)
    import AVFoundation
    import Speech
    #if canImport(UIKit)
        import UIKit
    #endif

    protocol VoiceRecognitionOperation: AnyObject {
        func finishInput()
        func cancel(with error: Error)
    }

    struct SpeechPermissionDeniedError: Error {}

    /// Native STT over `SFSpeechRecognizer` + an `AVAudioEngine` mic tap.
    ///
    /// The AudioService uses this one-shot operation. The composer additionally
    /// uses `finishRecording` on finger-up and `cancelRecognition` on swipe-away or
    /// lifecycle cancellation. Mutable cross-callback state is protected by
    /// `stateLock`; this is the safety invariant behind `@unchecked Sendable`.
    final class SttImpl: @unchecked Sendable {
        private struct Attempt {
            let id: UUID
            var operation: (any VoiceRecognitionOperation)?
            var finishRequested = false
            var cancellation: Error?
        }

        private enum AttachDisposition {
            case proceed
            case finish
            case cancel(Error)
        }

        private let stateLock = NSLock()
        private var attempt: Attempt?
        private let timeoutNanoseconds: UInt64
        private let audioSessionCoordinator: VoiceAudioSessionCoordinator
        private let speechAuthorization: @Sendable () async -> Bool

        init(
            timeoutNanoseconds: UInt64 = 30_000_000_000,
            coordinator: VoiceAudioSessionCoordinator? = nil,
            speechAuthorization: @escaping @Sendable () async -> Bool = {
                let status: SFSpeechRecognizerAuthorizationStatus = await withCheckedContinuation { continuation in
                    SFSpeechRecognizer.requestAuthorization { continuation.resume(returning: $0) }
                }
                return status == .authorized
            }
        ) {
            self.timeoutNanoseconds = timeoutNanoseconds
            audioSessionCoordinator = coordinator ?? .shared
            self.speechAuthorization = speechAuthorization
        }

        var hasActiveNativeOperation: Bool {
            stateLock.lock()
            defer { stateLock.unlock() }
            return attempt?.operation != nil
        }

        /// Flow Mode supplies a trailing-silence interval so one spoken
        /// utterance can finish without an explicit tap.
        func transcribe(
            language: String?,
            automaticEndpointAfterSilence: Duration?,
            routeResolution: AudioRouteResolution? = nil,
            configurationSnapshot: AudioConfigurationSnapshot? = nil,
            maximumPayloadBytes: UInt64? = nil,
            audioSessionPurpose: VoiceAudioSessionCoordinator.Purpose = .recognition,
            routeResolutionChanged: (@MainActor (AudioRouteResolution) -> Void)? = nil
        ) async throws -> String {
            let attemptID = try beginAttempt()
            defer { endAttempt(attemptID) }

            return try await withTaskCancellationHandler {
                try Task.checkCancellation()
                let payloadLimit = maximumPayloadBytes ?? maxAudioPayloadBytes()
                guard payloadLimit > 0, payloadLimit <= UInt64(Int.max) else {
                    throw AudioServiceFailure.invalidRequest
                }
                let snapshot: AudioConfigurationSnapshot
                if let configurationSnapshot {
                    snapshot = configurationSnapshot
                } else {
                    snapshot = await MainActor.run { AudioConfigurationRuntime.snapshot() }
                }
                var selectedRoute: AudioRouteResolution
                if let routeResolution {
                    selectedRoute = routeResolution
                } else {
                    selectedRoute = await MainActor.run {
                        AudioConfigurationRuntime.route(
                            kind: .recognition,
                            snapshot: snapshot,
                            languageOverride: language
                        )
                    }
                }
                let initiallySelectedRoute = selectedRoute
                await MainActor.run { routeResolutionChanged?(initiallySelectedRoute) }
                let resolvedIdentifier = resolveAudioLanguageForNativeDevice(
                    configured: language ?? snapshot.configuration.language,
                    deviceLocale: Locale.autoupdatingCurrent.identifier
                )
                let locale = Locale(identifier: resolvedIdentifier)
                let recognizer = SFSpeechRecognizer(locale: locale)
                func nativeRoute(_ resolution: AudioRouteResolution) throws -> VoiceRecognitionRoute {
                    guard resolution.status == .ready || resolution.status == .permissionRequired,
                          let effective = resolution.effective else { throw AudioServiceFailure.unavailable }
                    switch effective.source {
                    case .system:
                        return .system(languageIdentifier: resolvedIdentifier)
                    case .offline:
                        guard let modelID = effective.modelId,
                              let model = GeneratedVoiceModelCatalog.byID(modelID),
                              model.kind == .stt,
                              let modelDirectory = VoiceModelFiles.modelRoot(for: model)
                        else { throw AudioServiceFailure.modelMissing }
                        return .sherpa(
                            languageIdentifier: resolvedIdentifier,
                            modelID: modelID,
                            modelDirectory: modelDirectory
                        )
                    default:
                        throw AudioServiceFailure.unavailable
                    }
                }
                var route = try nativeRoute(selectedRoute)
                if case .system = route {
                    do {
                        try await requestSpeechAuthorization(for: attemptID)
                    } catch is SpeechPermissionDeniedError {
                        try checkAttempt(attemptID)
                        guard selectedRoute.requested.source == .automatic else {
                            throw AudioServiceFailure.permissionDenied
                        }
                        selectedRoute = await MainActor.run {
                            AudioConfigurationRuntime.route(
                                kind: .recognition,
                                snapshot: snapshot,
                                languageOverride: language
                            )
                        }
                        let fallbackSelectedRoute = selectedRoute
                        await MainActor.run { routeResolutionChanged?(fallbackSelectedRoute) }
                        route = try nativeRoute(selectedRoute)
                    }
                }

                switch route {
                case let .sherpa(_, modelID, modelDirectory):
                    try await requestMicrophoneAuthorization()
                    try checkAttempt(attemptID)
                    let audioLease: VoiceAudioSessionCoordinator.Lease
                    do {
                        audioLease = try await audioSessionCoordinator.acquire(audioSessionPurpose)
                    } catch {
                        throw SpeechRecognitionError.Retriable(message: "audio session: \(error.localizedDescription)")
                    }
                    do {
                        let operation = SherpaRecognitionOperation(
                            modelID: modelID,
                            modelDirectory: modelDirectory,
                            automaticEndpointAfterSilence: automaticEndpointAfterSilence,
                            maximumPayloadBytes: payloadLimit
                        )
                        switch attach(operation, to: attemptID) {
                        case .proceed: break
                        case .finish: operation.finishInput()
                        case let .cancel(error): operation.cancel(with: error)
                        }
                        let transcript = try await operation.run()
                        await audioSessionCoordinator.release(audioLease)
                        return transcript
                    } catch {
                        await audioSessionCoordinator.release(audioLease)
                        throw error
                    }
                case let .unavailable(message):
                        throw SpeechRecognitionError.Other(message: message)
                case .system:
                    break
                }

                try await requestMicrophoneAuthorization()
                try checkAttempt(attemptID)
                guard let recognizer, recognizer.isAvailable else { throw AudioServiceFailure.unavailable }

                let audioLease: VoiceAudioSessionCoordinator.Lease
                do {
                    audioLease = try await audioSessionCoordinator.acquire(audioSessionPurpose)
                } catch {
                    throw SpeechRecognitionError.Retriable(message: "audio session: \(error.localizedDescription)")
                }

                do {
                    try Task.checkCancellation()
                    try checkAttempt(attemptID)

                    let request = SFSpeechAudioBufferRecognitionRequest()
                    request.shouldReportPartialResults = automaticEndpointAfterSilence != nil
                    request.requiresOnDeviceRecognition = false

                    let operation = SpeechRecognitionOperation(
                        request: request,
                        timeoutNanoseconds: timeoutNanoseconds,
                        automaticEndpointAfterSilence: automaticEndpointAfterSilence,
                        maximumPayloadBytes: payloadLimit
                    )
                    switch attach(operation, to: attemptID) {
                    case .proceed:
                        break
                    case .finish:
                        operation.finishInput()
                    case let .cancel(error):
                        operation.cancel(with: error)
                    }

                    let transcript = try await operation.run(with: recognizer)
                    await audioSessionCoordinator.release(audioLease)
                    return transcript
                } catch {
                    await audioSessionCoordinator.release(audioLease)
                    throw error
                }
            } onCancel: {
                self.cancelAttempt(attemptID, with: CancellationError())
            }
        }

        /// End microphone input but allow Speech to emit the final transcript.
        /// Safe before the recognizer has finished authorization or audio setup.
        func finishRecording() {
            stateLock.lock()
            guard var current = attempt else {
                stateLock.unlock()
                return
            }
            current.finishRequested = true
            attempt = current
            let operation = current.operation
            stateLock.unlock()
            operation?.finishInput()
        }

        /// Cancel the active attempt and unblock its awaiting continuation.
        func cancelRecognition() {
            stateLock.lock()
            let id = attempt?.id
            stateLock.unlock()
            guard let id else { return }
            cancelAttempt(id, with: CancellationError())
        }

        private func beginAttempt() throws -> UUID {
            stateLock.lock()
            defer { stateLock.unlock() }
            if let current = attempt,
               current.operation == nil,
               current.cancellation != nil {
                // System permission callbacks cannot be cancelled. A caller may
                // cancel while the OS sheet is open, so let a fresh request take
                // over admission. The old callback stays bound to its attempt ID.
                attempt = nil
            }
            guard attempt == nil else {
                throw SpeechRecognitionError.Retriable(message: "speech recognition is already active")
            }
            let id = UUID()
            attempt = Attempt(id: id)
            return id
        }

        private func endAttempt(_ id: UUID) {
            stateLock.lock()
            defer { stateLock.unlock() }
            guard attempt?.id == id else { return }
            attempt = nil
        }

        private func checkAttempt(_ id: UUID) throws {
            stateLock.lock()
            defer { stateLock.unlock() }
            guard let current = attempt, current.id == id else {
                throw CancellationError()
            }
            if let cancellation = current.cancellation { throw cancellation }
        }

        private func attach(
            _ operation: any VoiceRecognitionOperation,
            to id: UUID
        ) -> AttachDisposition {
            stateLock.lock()
            defer { stateLock.unlock() }
            guard var current = attempt, current.id == id else {
                return .cancel(CancellationError())
            }
            current.operation = operation
            attempt = current
            if let cancellation = current.cancellation { return .cancel(cancellation) }
            return current.finishRequested ? .finish : .proceed
        }

        private func cancelAttempt(_ id: UUID, with error: Error) {
            stateLock.lock()
            guard var current = attempt, current.id == id else {
                stateLock.unlock()
                return
            }
            if current.cancellation == nil { current.cancellation = error }
            attempt = current
            let operation = current.operation
            stateLock.unlock()
            operation?.cancel(with: error)
        }

        private func requestSpeechAuthorization(for attemptID: UUID) async throws {
            let authorized = await speechAuthorization()
            try Task.checkCancellation()
            try checkAttempt(attemptID)
            guard authorized else { throw SpeechPermissionDeniedError() }
        }

        private func requestMicrophoneAuthorization() async throws {
            let micGranted = await AVAudioApplication.requestRecordPermission()
            try Task.checkCancellation()
            guard micGranted else { throw SpeechRecognitionError.PermissionDenied }
        }
    }

    /// Owns exactly one audio tap, Speech request and continuation. Every terminal
    /// path runs `endInputLocked`, cancels the recognition task and removes its
    /// interruption/background observers before resuming the caller.
    private final class SpeechRecognitionOperation: VoiceRecognitionOperation, @unchecked Sendable {
        private let lock = NSRecursiveLock()
        private let request: SFSpeechAudioBufferRecognitionRequest
        private let audioEngine = AVAudioEngine()
        private let clock = ContinuousClock()
        private let timeoutNanoseconds: UInt64
        private let automaticEndpointAfterSilence: Duration?
        private let maximumPayloadBytes: Int

        private var continuation: CheckedContinuation<String, Error>?
        private var recognitionTask: SFSpeechRecognitionTask?
        private var timeoutTask: Task<Void, Never>?
        private var endpointTask: Task<Void, Never>?
        private var terminalResult: Result<String, Error>?
        private var latestPartialTranscript = ""
        private var ambientRMS: Float = 0.005
        private var lastVoiceActivity: ContinuousClock.Instant?
        private var inputStarted = false
        private var inputEnded = false
        private var finishRequested = false
        private var capturedPayloadBytes = 0
        private var observers: [NSObjectProtocol] = []

        init(
            request: SFSpeechAudioBufferRecognitionRequest,
            timeoutNanoseconds: UInt64,
            automaticEndpointAfterSilence: Duration?,
            maximumPayloadBytes: UInt64
        ) {
            self.request = request
            self.timeoutNanoseconds = timeoutNanoseconds
            self.automaticEndpointAfterSilence = automaticEndpointAfterSilence
            self.maximumPayloadBytes = Int(maximumPayloadBytes)
            installLifecycleObservers()
        }

        deinit {
            lock.lock()
            let timeoutTask = timeoutTask
            let endpointTask = endpointTask
            let recognitionTask = recognitionTask
            self.timeoutTask = nil
            self.endpointTask = nil
            self.recognitionTask = nil
            endInputLocked()
            removeLifecycleObserversLocked()
            lock.unlock()
            timeoutTask?.cancel()
            endpointTask?.cancel()
            recognitionTask?.cancel()
        }

        func run(with recognizer: SFSpeechRecognizer) async throws -> String {
            try startAudio()

            return try await withCheckedThrowingContinuation { continuation in
                lock.lock()
                if let terminalResult {
                    lock.unlock()
                    continuation.resume(with: terminalResult)
                    return
                }
                self.continuation = continuation
                lock.unlock()
                startRecognizer(recognizer)
            }
        }

        func finishInput() {
            lock.lock()
            finishRequested = true
            let endpointTask = endpointTask
            self.endpointTask = nil
            if inputStarted { endInputLocked() }
            lock.unlock()
            endpointTask?.cancel()
        }

        func cancel(with error: Error) {
            complete(.failure(error))
        }

        private func startAudio() throws {
            lock.lock()
            defer { lock.unlock() }
            if let terminalResult {
                _ = try terminalResult.get()
                return
            }

            let inputNode = audioEngine.inputNode
            let format = inputNode.outputFormat(forBus: 0)
            inputNode.installTap(onBus: 0, bufferSize: 1024, format: format) { [weak self] buffer, _ in
                self?.consume(buffer)
            }
            inputStarted = true
            audioEngine.prepare()
            do {
                try audioEngine.start()
            } catch {
                endInputLocked()
                throw SpeechRecognitionError.Retriable(message: "mic start: \(error.localizedDescription)")
            }
            if finishRequested { endInputLocked() }
        }

        private func consume(_ buffer: AVAudioPCMBuffer) {
            let channels = max(1, Int(buffer.format.channelCount))
            let frames = Int(buffer.frameLength)
            guard frames > 0, frames <= Int.max / channels / MemoryLayout<Int16>.size else { return }
            let inputBytes = frames * channels * MemoryLayout<Int16>.size

            lock.lock()
            guard terminalResult == nil, !inputEnded else {
                lock.unlock()
                return
            }
            guard inputBytes <= maximumPayloadBytes - capturedPayloadBytes else {
                lock.unlock()
                complete(.failure(AudioServiceFailure.mediaTooLarge))
                return
            }
            capturedPayloadBytes += inputBytes
            request.append(buffer)
            lock.unlock()
            observeAudioActivity(in: buffer)
        }

        private func startRecognizer(_ recognizer: SFSpeechRecognizer) {
            lock.lock()
            guard terminalResult == nil else {
                lock.unlock()
                return
            }
            lock.unlock()

            let task = recognizer.recognitionTask(with: request) { [weak self] result, error in
                guard let self else { return }
                if let result {
                    let transcript = result.bestTranscription.formattedString
                    if result.isFinal {
                        complete(.success(transcript))
                        return
                    }
                    observePartialTranscript(transcript)
                }
                if let error {
                    if let transcript = partialTranscriptAfterFinishing() {
                        complete(.success(transcript))
                        return
                    }
                    let nsError = error as NSError
                    if nsError.domain == "kAFAssistantErrorDomain", nsError.code == 1110 {
                        complete(.failure(SpeechRecognitionError.NoSpeech))
                    } else {
                        complete(.failure(SpeechRecognitionError.Retriable(message: error.localizedDescription)))
                    }
                }
            }

            lock.lock()
            if terminalResult != nil {
                lock.unlock()
                task.cancel()
                return
            }
            recognitionTask = task
            let timeoutNanoseconds = timeoutNanoseconds
            timeoutTask = Task { [weak self] in
                do {
                    try await Task.sleep(nanoseconds: timeoutNanoseconds)
                } catch {
                    return
                }
                guard !Task.isCancelled else { return }
                self?.complete(.failure(SpeechRecognitionError.Retriable(message: "speech recognition timed out")))
            }
            lock.unlock()
        }

        private func complete(_ result: Result<String, Error>) {
            lock.lock()
            guard terminalResult == nil else {
                lock.unlock()
                return
            }
            terminalResult = result
            let continuation = continuation
            self.continuation = nil
            let recognitionTask = recognitionTask
            self.recognitionTask = nil
            let timeoutTask = timeoutTask
            self.timeoutTask = nil
            let endpointTask = endpointTask
            self.endpointTask = nil
            endInputLocked()
            removeLifecycleObserversLocked()
            lock.unlock()

            timeoutTask?.cancel()
            endpointTask?.cancel()
            recognitionTask?.cancel()
            continuation?.resume(with: result)
        }

        /// Arm endpointing only after Speech has recognized real text. Audio
        /// activity then keeps advancing the deadline even when the partial
        /// transcript itself has not changed, avoiding mid-sentence cutoffs.
        private func observePartialTranscript(_ transcript: String) {
            let trimmed = transcript.trimmingCharacters(in: .whitespacesAndNewlines)
            guard !trimmed.isEmpty, let silence = automaticEndpointAfterSilence else { return }

            lock.lock()
            guard terminalResult == nil,
                  !inputEnded,
                  trimmed != latestPartialTranscript
            else {
                lock.unlock()
                return
            }
            latestPartialTranscript = trimmed
            lastVoiceActivity = clock.now
            if endpointTask == nil {
                endpointTask = Task { [weak self] in
                    await self?.finishAfterTrailingSilence(silence)
                }
            }
            lock.unlock()
        }

        private func observeAudioActivity(in buffer: AVAudioPCMBuffer) {
            let rms = Self.rms(of: buffer)
            guard rms > 0 else { return }

            lock.lock()
            guard terminalResult == nil, !inputEnded else {
                lock.unlock()
                return
            }
            let threshold = max(0.015, ambientRMS * 2.2)
            if rms >= threshold {
                lastVoiceActivity = clock.now
            } else {
                ambientRMS = (ambientRMS * 0.95) + (rms * 0.05)
            }
            lock.unlock()
        }

        private func finishAfterTrailingSilence(_ silence: Duration) async {
            while !Task.isCancelled {
                guard let elapsed = trailingSilenceElapsed() else { return }

                if elapsed >= silence {
                    finishInput()
                    return
                }
                do {
                    try await Task.sleep(for: silence - elapsed)
                } catch {
                    return
                }
            }
        }

        private func trailingSilenceElapsed() -> Duration? {
            lock.lock()
            defer { lock.unlock() }
            guard terminalResult == nil,
                  !inputEnded,
                  let lastVoiceActivity
            else { return nil }
            return lastVoiceActivity.duration(to: clock.now)
        }

        private static func rms(of buffer: AVAudioPCMBuffer) -> Float {
            let frameCount = Int(buffer.frameLength)
            guard frameCount > 0 else { return 0 }

            if let samples = buffer.floatChannelData?[0] {
                var sum: Float = 0
                for index in 0..<frameCount {
                    let sample = samples[index]
                    sum += sample * sample
                }
                return sqrt(sum / Float(frameCount))
            }
            if let samples = buffer.int16ChannelData?[0] {
                var sum: Float = 0
                for index in 0..<frameCount {
                    let sample = Float(samples[index]) / Float(Int16.max)
                    sum += sample * sample
                }
                return sqrt(sum / Float(frameCount))
            }
            return 0
        }

        private func partialTranscriptAfterFinishing() -> String? {
            lock.lock()
            defer { lock.unlock() }
            guard finishRequested || inputEnded else { return nil }
            let trimmed = latestPartialTranscript.trimmingCharacters(in: .whitespacesAndNewlines)
            return trimmed.isEmpty ? nil : trimmed
        }

        private func endInputLocked() {
            guard !inputEnded else { return }
            inputEnded = true
            request.endAudio()
            if inputStarted {
                audioEngine.stop()
                audioEngine.inputNode.removeTap(onBus: 0)
                inputStarted = false
            }
        }

        private func installLifecycleObservers() {
            let center = NotificationCenter.default
            observers.append(center.addObserver(
                forName: AVAudioSession.interruptionNotification,
                object: AVAudioSession.sharedInstance(),
                queue: nil
            ) { [weak self] notification in
                let raw = notification.userInfo?[AVAudioSessionInterruptionTypeKey] as? UInt
                guard raw == AVAudioSession.InterruptionType.began.rawValue else { return }
                    self?.complete(.failure(SpeechRecognitionError.Retriable(message: "audio session interrupted")))
            })
            #if canImport(UIKit)
                observers.append(center.addObserver(
                    forName: UIApplication.didEnterBackgroundNotification,
                    object: nil,
                    queue: nil
                ) { [weak self] _ in
                    self?.complete(.failure(SpeechRecognitionError.Retriable(message: "speech recognition stopped in background")))
                })
            #endif
        }

        private func removeLifecycleObserversLocked() {
            guard !observers.isEmpty else { return }
            observers.forEach(NotificationCenter.default.removeObserver)
            observers.removeAll()
        }
    }
#endif
