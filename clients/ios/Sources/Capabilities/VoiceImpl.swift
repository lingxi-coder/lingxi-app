// VoiceImpl.swift — iOS native mic-recorder capability (parity with Android
// RecorderController.kt).
//
// Conforms to the generated `IosVoice` UniFFI callback interface. The engine
// (tool-voice) drives start/stop/is_recording; we capture the raw mic with
// `AVAudioRecorder` to an AAC/m4a file in the app's temp dir, then return the
// encoded bytes + MIME type as a `VoiceRecordingFfi` on stop. Errors map onto
// the generated `VoiceFfiError`. This is the RAW recorder, distinct from
// `SttImpl` (the system recognizer).

import Foundation

#if canImport(AVFoundation)
    import AVFoundation

    /// Native mic recorder over `AVAudioRecorder` producing AAC/m4a.
    final class VoiceImpl: NSObject, IosVoice, @unchecked Sendable {
        private let lock = NSLock()
        private var recorder: AVAudioRecorder?
        private var fileURL: URL?

        func startRecording(sampleRateHz: UInt32, format: String) async throws {
            try await requestMicAuthorization()

            let session = AVAudioSession.sharedInstance()
            do {
                try session.setCategory(.playAndRecord, mode: .default)
                try session.setActive(true)
            } catch {
                throw VoiceFfiError.Other(message: "audio session: \(error.localizedDescription)")
            }

            let url = FileManager.default.temporaryDirectory
                .appendingPathComponent("lingxi-voice-\(UUID().uuidString).m4a")
            let settings: [String: Any] = [
                AVFormatIDKey: kAudioFormatMPEG4AAC,
                AVSampleRateKey: Double(sampleRateHz),
                AVNumberOfChannelsKey: 1,
                AVEncoderAudioQualityKey: AVAudioQuality.high.rawValue,
            ]
            do {
                let rec = try AVAudioRecorder(url: url, settings: settings)
                guard rec.record() else {
                    throw VoiceFfiError.Other(message: "recorder failed to start")
                }
                lock.lock()
                recorder = rec
                fileURL = url
                lock.unlock()
            } catch let e as VoiceFfiError {
                throw e
            } catch {
                throw VoiceFfiError.Other(message: error.localizedDescription)
            }
        }

        func stopRecording() async throws -> VoiceRecordingFfi {
            lock.lock()
            let rec = recorder
            let url = fileURL
            recorder = nil
            fileURL = nil
            lock.unlock()

            guard let rec, let url else { throw VoiceFfiError.NotRecording }
            rec.stop()
            try? AVAudioSession.sharedInstance().setActive(false)

            do {
                let data = try Data(contentsOf: url)
                try? FileManager.default.removeItem(at: url)
                return VoiceRecordingFfi(audioBytes: data, mimeType: "audio/m4a")
            } catch {
                throw VoiceFfiError.Other(message: "read recording: \(error.localizedDescription)")
            }
        }

        func isRecording() async -> Bool {
            lock.lock(); defer { lock.unlock() }
            return recorder?.isRecording ?? false
        }

        private func requestMicAuthorization() async throws {
            let granted: Bool = await withCheckedContinuation { cont in
                AVAudioSession.sharedInstance().requestRecordPermission { cont.resume(returning: $0) }
            }
            guard granted else { throw VoiceFfiError.PermissionDenied }
        }
    }
#endif
