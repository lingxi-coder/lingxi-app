// SttImpl.swift — iOS native speech-to-text capability (parity with Android
// SystemSpeechRecognizerStt.kt).

import Foundation

#if canImport(Speech) && canImport(AVFoundation)
    import AVFoundation
    import Speech
    #if canImport(UIKit)
        import UIKit
    #endif

    /// Native STT over `SFSpeechRecognizer` + an `AVAudioEngine` mic tap.
    ///
    /// UniFFI calls `transcribe` as a one-shot operation. The composer additionally
    /// uses `finishRecording` on finger-up and `cancelRecognition` on swipe-away or
    /// lifecycle cancellation. Mutable cross-callback state is protected by
    /// `stateLock`; this is the safety invariant behind `@unchecked Sendable`.
    final class SttImpl: IosStt, @unchecked Sendable {
        private struct Attempt {
            let id: UUID
            var operation: SpeechRecognitionOperation?
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

        init(timeoutNanoseconds: UInt64 = 30_000_000_000) {
            self.timeoutNanoseconds = timeoutNanoseconds
        }

        func transcribe(language: String?) async throws -> String {
            let attemptID = try beginAttempt()
            defer { endAttempt(attemptID) }

            return try await withTaskCancellationHandler {
                try Task.checkCancellation()
                try await requestAuthorization()
                try checkAttempt(attemptID)

                let configuredLanguage = UserDefaults.standard.string(forKey: "voiceLanguage")
                let resolvedIdentifier = VoiceCapabilityModel.resolvedRecognitionLocaleIdentifier(
                    configuredLanguage: language ?? configuredLanguage,
                    currentLocale: .autoupdatingCurrent
                )
                let locale = Locale(identifier: resolvedIdentifier)
                guard let recognizer = SFSpeechRecognizer(locale: locale), recognizer.isAvailable else {
                    throw SpeechFfiError.Unavailable
                }

                do {
                    try await VoiceAudioSessionCoordinator.shared.activate(.recognition)
                } catch {
                    throw SpeechFfiError.Retriable(message: "audio session: \(error.localizedDescription)")
                }

                do {
                    try Task.checkCancellation()
                    try checkAttempt(attemptID)

                    let request = SFSpeechAudioBufferRecognitionRequest()
                    request.shouldReportPartialResults = false
                    let preferOnDevice = UserDefaults.standard.string(forKey: "voiceRecognitionMode")
                        == VoiceRecognitionMode.onDevice.rawValue
                    request.requiresOnDeviceRecognition = preferOnDevice && recognizer.supportsOnDeviceRecognition

                    let operation = SpeechRecognitionOperation(
                        request: request,
                        timeoutNanoseconds: timeoutNanoseconds
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
                    await VoiceAudioSessionCoordinator.shared.deactivate(.recognition)
                    return transcript
                } catch {
                    await VoiceAudioSessionCoordinator.shared.deactivate(.recognition)
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
            guard attempt == nil else {
                throw SpeechFfiError.Retriable(message: "speech recognition is already active")
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
            _ operation: SpeechRecognitionOperation,
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

        /// Request both speech-recognition and microphone authorization.
        private func requestAuthorization() async throws {
            let speechStatus: SFSpeechRecognizerAuthorizationStatus = await withCheckedContinuation { continuation in
                SFSpeechRecognizer.requestAuthorization { continuation.resume(returning: $0) }
            }
            try Task.checkCancellation()
            guard speechStatus == .authorized else { throw SpeechFfiError.PermissionDenied }

            let micGranted = await AVAudioApplication.requestRecordPermission()
            try Task.checkCancellation()
            guard micGranted else { throw SpeechFfiError.PermissionDenied }
        }
    }

    /// Owns exactly one audio tap, Speech request and continuation. Every terminal
    /// path runs `endInputLocked`, cancels the recognition task and removes its
    /// interruption/background observers before resuming the caller.
    private final class SpeechRecognitionOperation: @unchecked Sendable {
        private let lock = NSRecursiveLock()
        private let request: SFSpeechAudioBufferRecognitionRequest
        private let audioEngine = AVAudioEngine()
        private let timeoutNanoseconds: UInt64

        private var continuation: CheckedContinuation<String, Error>?
        private var recognitionTask: SFSpeechRecognitionTask?
        private var timeoutTask: Task<Void, Never>?
        private var terminalResult: Result<String, Error>?
        private var inputStarted = false
        private var inputEnded = false
        private var finishRequested = false
        private var observers: [NSObjectProtocol] = []

        init(request: SFSpeechAudioBufferRecognitionRequest, timeoutNanoseconds: UInt64) {
            self.request = request
            self.timeoutNanoseconds = timeoutNanoseconds
            installLifecycleObservers()
        }

        deinit {
            lock.lock()
            endInputLocked()
            removeLifecycleObserversLocked()
            lock.unlock()
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
            if inputStarted { endInputLocked() }
            lock.unlock()
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
            inputNode.installTap(onBus: 0, bufferSize: 1024, format: format) { [request] buffer, _ in
                request.append(buffer)
            }
            inputStarted = true
            audioEngine.prepare()
            do {
                try audioEngine.start()
            } catch {
                endInputLocked()
                throw SpeechFfiError.Retriable(message: "mic start: \(error.localizedDescription)")
            }
            if finishRequested { endInputLocked() }
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
                if let result, result.isFinal {
                    complete(.success(result.bestTranscription.formattedString))
                    return
                }
                if let error {
                    let nsError = error as NSError
                    if nsError.domain == "kAFAssistantErrorDomain", nsError.code == 1110 {
                        complete(.failure(SpeechFfiError.NoSpeech))
                    } else {
                        complete(.failure(SpeechFfiError.Retriable(message: error.localizedDescription)))
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
                self?.complete(.failure(SpeechFfiError.Retriable(message: "speech recognition timed out")))
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
            endInputLocked()
            removeLifecycleObserversLocked()
            lock.unlock()

            timeoutTask?.cancel()
            recognitionTask?.cancel()
            continuation?.resume(with: result)
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
                self?.complete(.failure(SpeechFfiError.Retriable(message: "audio session interrupted")))
            })
            #if canImport(UIKit)
                observers.append(center.addObserver(
                    forName: UIApplication.didEnterBackgroundNotification,
                    object: nil,
                    queue: nil
                ) { [weak self] _ in
                    self?.complete(.failure(SpeechFfiError.Retriable(message: "speech recognition stopped in background")))
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
