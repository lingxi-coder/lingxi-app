// SttImpl.swift — iOS native speech-to-text capability (parity with Android
// SystemSpeechRecognizerStt.kt).
//
// Conforms to the generated `IosStt` UniFFI callback interface. The engine
// (tool-speech) calls `transcribe(language:)`; we request speech-recognition +
// microphone authorization, open a live mic tap into an `SFSpeechRecognizer`,
// listen for a single utterance, and return the final transcript. Errors map
// onto the generated `SpeechFfiError` so the Rust bridge can fan them onto
// `traits::SttError`.

import Foundation

#if canImport(Speech) && canImport(AVFoundation)
    import AVFoundation
    import Speech

    /// Native STT over `SFSpeechRecognizer` + an `AVAudioEngine` mic tap.
    /// One-shot: returns the final transcript of a single utterance, then tears
    /// down the audio graph. Mirrors the Android system-recognizer behavior.
    final class SttImpl: IosStt, @unchecked Sendable {
        private let audioEngine = AVAudioEngine()

        func transcribe(language: String?) async throws -> String {
            try await requestAuthorization()

            let locale = language.map(Locale.init(identifier:)) ?? Locale.current
            guard let recognizer = SFSpeechRecognizer(locale: locale), recognizer.isAvailable else {
                throw SpeechFfiError.Unavailable
            }

            let session = AVAudioSession.sharedInstance()
            do {
                try session.setCategory(.record, mode: .measurement, options: .duckOthers)
                try session.setActive(true, options: .notifyOthersOnDeactivation)
            } catch {
                throw SpeechFfiError.Retriable(message: "audio session: \(error.localizedDescription)")
            }

            let request = SFSpeechAudioBufferRecognitionRequest()
            request.shouldReportPartialResults = false

            let inputNode = audioEngine.inputNode
            let format = inputNode.outputFormat(forBus: 0)
            inputNode.installTap(onBus: 0, bufferSize: 1024, format: format) { buffer, _ in
                request.append(buffer)
            }

            audioEngine.prepare()
            do {
                try audioEngine.start()
            } catch {
                inputNode.removeTap(onBus: 0)
                throw SpeechFfiError.Retriable(message: "mic start: \(error.localizedDescription)")
            }

            defer {
                audioEngine.stop()
                inputNode.removeTap(onBus: 0)
                try? session.setActive(false, options: .notifyOthersOnDeactivation)
            }

            // Drive the recognizer to a single final result.
            return try await withCheckedThrowingContinuation { (cont: CheckedContinuation<String, Error>) in
                let box = ResultBox()
                let task = recognizer.recognitionTask(with: request) { result, error in
                    if let result, result.isFinal {
                        guard box.finish() else { return }
                        cont.resume(returning: result.bestTranscription.formattedString)
                        return
                    }
                    if let error {
                        guard box.finish() else { return }
                        let ns = error as NSError
                        // SFSpeech "no speech detected" → kAFAssistantErrorDomain 1110.
                        if ns.domain == "kAFAssistantErrorDomain", ns.code == 1110 {
                            cont.resume(throwing: SpeechFfiError.NoSpeech)
                        } else {
                            cont.resume(throwing: SpeechFfiError.Retriable(message: error.localizedDescription))
                        }
                    }
                }
                box.task = task
            }
        }

        /// Request both speech-recognition and microphone authorization, throwing
        /// `PermissionDenied` if either is refused.
        private func requestAuthorization() async throws {
            let speechStatus: SFSpeechRecognizerAuthorizationStatus = await withCheckedContinuation { cont in
                SFSpeechRecognizer.requestAuthorization { cont.resume(returning: $0) }
            }
            guard speechStatus == .authorized else { throw SpeechFfiError.PermissionDenied }

            let micGranted: Bool = await withCheckedContinuation { cont in
                AVAudioSession.sharedInstance().requestRecordPermission { cont.resume(returning: $0) }
            }
            guard micGranted else { throw SpeechFfiError.PermissionDenied }
        }
    }

    /// One-shot guard so the recognition callback resumes its continuation at most
    /// once (the callback can fire multiple times — partials, final, error).
    private final class ResultBox: @unchecked Sendable {
        private let lock = NSLock()
        private var done = false
        var task: SFSpeechRecognitionTask?

        func finish() -> Bool {
            lock.lock(); defer { lock.unlock() }
            if done { return false }
            done = true
            task?.cancel()
            return true
        }
    }
#endif
